#!/usr/bin/env python3
"""
vlc-bridge.py -- VLC External Player Bridge for Jellyfin Media Player (JMP)
=============================================================================

ARCHITECTURE
------------
JMP is a Qt app wrapping the Jellyfin web client in QtWebEngine/Chromium.
Its embedded mpv player is unreliable on Pi 5, so this daemon intercepts
playback and hands it off to VLC instead.

Two strategies are used (tried in order):

  STRATEGY 1 — CDP (Chrome DevTools Protocol)
    - Connects to JMP's embedded Chromium via WebSocket on localhost:9222
    - Injects JavaScript that hooks the Jellyfin web client's playback manager
    - When play is triggered, JS extracts the direct stream URL and auth token
    - Sends the URL back to Python via console.log("VLC_PLAY:...")
    - Lower latency (~instant), but depends on CDP being available

  STRATEGY 2 — API Polling (fallback)
    - Authenticates with the Jellyfin server directly
    - Polls /Sessions every 500ms watching for NowPlaying on our device
    - When playback starts, grabs stream URL via /Items/{id}/PlaybackInfo
    - Stops JMP playback via Sessions API, launches VLC
    - Higher latency (~1-2s) but robust and independent of CDP

PLAYBACK FLOW
  1. Detect play request (via CDP hook or API session polling)
  2. Extract: item ID, media source ID, auth token, server URL, resume position
  3. Build direct stream URL: {server}/Videos/{itemId}/stream?static=true&...
  4. Kill JMP's internal playback (stop via API or JS)
  5. Launch VLC fullscreen with --start-time and --audio-desync
  6. Write "vlc" to /tmp/foreground-app (unified-controller media mode)
  7. Wait for VLC to exit
  8. Write "jellyfin" to /tmp/foreground-app, refocus JMP window

ENVIRONMENT
  WAYLAND_DISPLAY=wayland-0
  XDG_RUNTIME_DIR=/run/user/1000
  Jellyfin server: http://localhost:8096

DEPENDENCIES
  Python 3 stdlib + websocket-client (pip install websocket-client)
  Falls back to urllib if websocket-client is unavailable.

SYSTEMD
  Run as vlc-bridge.service (user unit), after JMP and unified-controller.
"""

import json
import logging
import os
import signal
import subprocess
import sys
import threading
import time
import urllib.request
import urllib.error
import urllib.parse

# Optional: websocket-client library
try:
    import websocket as ws_client
    HAS_WEBSOCKET = True
except ImportError:
    HAS_WEBSOCKET = False

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

JELLYFIN_URL = os.environ.get(
    "JELLYFIN_URL", "http://localhost:8096"
)
JELLYFIN_USER = os.environ.get("JELLYFIN_USER", "your-email@example.com")
JELLYFIN_PASS = os.environ.get("JELLYFIN_PASS", "")

CDP_PORT = int(os.environ.get("CDP_PORT", "9222"))
CDP_URL = f"http://localhost:{CDP_PORT}"

VLC_BIN = "/usr/bin/vlc"
VLC_AUDIO_DESYNC = -300  # ms, matches mpv audio-delay=-0.3
FOREGROUND_FILE = "/tmp/foreground-app"
JMP_APP_ID = "com.github.iwalton3.jellyfin-media-player"

# How often to poll Jellyfin sessions (Strategy 2)
SESSION_POLL_INTERVAL = 0.5  # seconds

# Wayland environment for VLC
WAYLAND_ENV = {
    "WAYLAND_DISPLAY": "wayland-0",
    "XDG_RUNTIME_DIR": "/run/user/1000",
    "QT_QPA_PLATFORM": "wayland",
}

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
    stream=sys.stdout,
)
log = logging.getLogger("vlc-bridge")

# ---------------------------------------------------------------------------
# Globals
# ---------------------------------------------------------------------------

shutdown_event = threading.Event()
current_vlc_proc = None  # subprocess.Popen of running VLC
vlc_lock = threading.Lock()


# ---------------------------------------------------------------------------
# Signal handling
# ---------------------------------------------------------------------------

def handle_signal(signum, frame):
    sig_name = signal.Signals(signum).name
    log.info(f"Received {sig_name}, shutting down...")
    shutdown_event.set()
    # Kill VLC if running
    with vlc_lock:
        if current_vlc_proc and current_vlc_proc.poll() is None:
            log.info("Killing VLC process on shutdown")
            current_vlc_proc.terminate()

signal.signal(signal.SIGTERM, handle_signal)
signal.signal(signal.SIGINT, handle_signal)


# ---------------------------------------------------------------------------
# Utility: HTTP helpers (stdlib only)
# ---------------------------------------------------------------------------

def http_get(url, headers=None, timeout=10):
    """GET request using urllib. Returns (status_code, response_body_str)."""
    req = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", errors="replace")
    except Exception as e:
        log.debug(f"HTTP GET {url} failed: {e}")
        return 0, ""


def http_post(url, data=None, headers=None, timeout=10):
    """POST request using urllib. data is a dict (sent as JSON)."""
    hdrs = {"Content-Type": "application/json"}
    if headers:
        hdrs.update(headers)
    body = json.dumps(data).encode("utf-8") if data else b""
    req = urllib.request.Request(url, data=body, headers=hdrs, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", errors="replace")
    except Exception as e:
        log.debug(f"HTTP POST {url} failed: {e}")
        return 0, ""


def http_delete(url, headers=None, timeout=10):
    """DELETE request using urllib."""
    req = urllib.request.Request(url, headers=headers or {}, method="DELETE")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", errors="replace")
    except Exception as e:
        log.debug(f"HTTP DELETE {url} failed: {e}")
        return 0, ""


# ---------------------------------------------------------------------------
# Utility: foreground app tracking
# ---------------------------------------------------------------------------

def set_foreground(app_name):
    """Write the current foreground app to /tmp/foreground-app."""
    try:
        with open(FOREGROUND_FILE, "w") as f:
            f.write(app_name)
        log.info(f"Foreground set to: {app_name}")
    except Exception as e:
        log.warning(f"Failed to write {FOREGROUND_FILE}: {e}")


def focus_jmp():
    """Bring JMP window to foreground via wlrctl."""
    try:
        subprocess.run(
            ["wlrctl", "toplevel", "focus", f"app_id:{JMP_APP_ID}"],
            env={**os.environ, **WAYLAND_ENV},
            timeout=5,
        )
    except Exception as e:
        log.warning(f"Failed to focus JMP: {e}")


# ---------------------------------------------------------------------------
# Jellyfin API authentication
# ---------------------------------------------------------------------------

class JellyfinAuth:
    """Authenticates with the Jellyfin server and stores the token."""

    def __init__(self, server_url, username, password):
        self.server_url = server_url.rstrip("/")
        self.username = username
        self.password = password
        self.token = None
        self.user_id = None
        self.session_id = None
        self.device_name = "VLC-Bridge"
        self.device_id = "vlc-bridge-pi5"
        self.client_name = "VLC Bridge"
        self.client_version = "1.0.0"

    def auth_header(self):
        """Build the X-Emby-Authorization header."""
        parts = [
            f'MediaBrowser Client="{self.client_name}"',
            f'Device="{self.device_name}"',
            f'DeviceId="{self.device_id}"',
            f'Version="{self.client_version}"',
        ]
        if self.token:
            parts.append(f'Token="{self.token}"')
        return ", ".join(parts)

    def headers(self):
        """Return headers dict for authenticated requests."""
        return {"X-Emby-Authorization": self.auth_header()}

    def authenticate(self):
        """Authenticate with username/password and store the access token."""
        url = f"{self.server_url}/Users/AuthenticateByName"
        data = {"Username": self.username, "Pw": self.password}
        status, body = http_post(url, data=data, headers=self.headers())
        if status == 200:
            result = json.loads(body)
            self.token = result.get("AccessToken")
            self.user_id = result.get("User", {}).get("Id")
            self.session_id = result.get("SessionInfo", {}).get("Id")
            log.info(
                f"Authenticated as {self.username} "
                f"(userId={self.user_id}, token={self.token[:8]}...)"
            )
            return True
        else:
            log.error(f"Authentication failed: HTTP {status}")
            return False

    def get_sessions(self):
        """Get active sessions from the server."""
        url = f"{self.server_url}/Sessions"
        status, body = http_get(url, headers=self.headers())
        if status == 200:
            return json.loads(body)
        return []

    def get_playback_info(self, item_id):
        """Get playback info for an item (media sources, stream URLs)."""
        url = (
            f"{self.server_url}/Items/{item_id}/PlaybackInfo"
            f"?UserId={self.user_id}"
        )
        status, body = http_post(url, data={}, headers=self.headers())
        if status == 200:
            return json.loads(body)
        log.error(f"PlaybackInfo failed for {item_id}: HTTP {status}")
        return None

    def get_item(self, item_id):
        """Get item details."""
        url = f"{self.server_url}/Users/{self.user_id}/Items/{item_id}"
        status, body = http_get(url, headers=self.headers())
        if status == 200:
            return json.loads(body)
        return None

    def stop_session(self, session_id):
        """Send stop command to a session."""
        url = f"{self.server_url}/Sessions/{session_id}/Playing/Stop"
        status, _ = http_post(url, headers=self.headers())
        log.info(f"Stop session {session_id}: HTTP {status}")
        return status == 204 or status == 200

    def report_playback_start(self, item_id, media_source_id, position_ticks=0):
        """Report playback started to Jellyfin (so it tracks progress)."""
        url = f"{self.server_url}/Sessions/Playing"
        data = {
            "ItemId": item_id,
            "MediaSourceId": media_source_id,
            "PositionTicks": position_ticks,
            "PlayMethod": "DirectStream",
            "PlaySessionId": f"vlc-{int(time.time())}",
        }
        status, _ = http_post(url, data=data, headers=self.headers())
        log.debug(f"Report playback start: HTTP {status}")

    def report_playback_stop(self, item_id, position_ticks=0):
        """Report playback stopped to Jellyfin."""
        url = f"{self.server_url}/Sessions/Playing/Stopped"
        data = {
            "ItemId": item_id,
            "PositionTicks": position_ticks,
        }
        status, _ = http_post(url, data=data, headers=self.headers())
        log.debug(f"Report playback stop: HTTP {status}")

    def build_stream_url(self, item_id, media_source_id):
        """Build a direct stream URL for VLC."""
        params = urllib.parse.urlencode({
            "static": "true",
            "mediaSourceId": media_source_id,
            "api_key": self.token,
        })
        return f"{self.server_url}/Videos/{item_id}/stream?{params}"


# ---------------------------------------------------------------------------
# VLC launcher
# ---------------------------------------------------------------------------

def launch_vlc(url, start_time_secs=0, item_id=None, jellyfin_auth=None):
    """
    Launch VLC fullscreen with the given stream URL.
    Blocks until VLC exits. Handles foreground-app tracking.
    Returns the approximate playback position in seconds when VLC exited.
    """
    global current_vlc_proc

    # Kill any existing VLC first
    with vlc_lock:
        if current_vlc_proc and current_vlc_proc.poll() is None:
            log.info("Killing existing VLC process")
            current_vlc_proc.terminate()
            try:
                current_vlc_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                current_vlc_proc.kill()

    cmd = [
        VLC_BIN,
        "--fullscreen",
        "--play-and-exit",
        f"--audio-desync={VLC_AUDIO_DESYNC}",
        "--no-video-title-show",
        "--quiet",
    ]

    if start_time_secs > 0:
        cmd.append(f"--start-time={start_time_secs}")

    cmd.append(url)

    env = {**os.environ, **WAYLAND_ENV}

    log.info(
        f"Launching VLC: start_time={start_time_secs}s, "
        f"audio_desync={VLC_AUDIO_DESYNC}ms"
    )
    log.debug(f"VLC command: {' '.join(cmd[:6])}... <url>")

    set_foreground("vlc")
    vlc_start_time = time.time()

    try:
        with vlc_lock:
            current_vlc_proc = subprocess.Popen(
                cmd,
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

        # Wait for VLC to exit
        current_vlc_proc.wait()
        exit_code = current_vlc_proc.returncode
        elapsed = time.time() - vlc_start_time

        log.info(f"VLC exited (code={exit_code}) after {elapsed:.1f}s")

        # Estimate playback position: start_time + elapsed
        approx_position = start_time_secs + elapsed

        return approx_position

    except Exception as e:
        log.error(f"VLC process error: {e}")
        return start_time_secs

    finally:
        with vlc_lock:
            current_vlc_proc = None

        # Return focus to JMP
        set_foreground("jellyfin")
        focus_jmp()


# ---------------------------------------------------------------------------
# Strategy 1: CDP (Chrome DevTools Protocol)
# ---------------------------------------------------------------------------

# JavaScript to inject into JMP's web view. This hooks the Jellyfin playback
# manager to intercept play requests and send stream URLs to us via console.log.
CDP_INJECT_JS = r"""
(function() {
    // Avoid double-injection
    if (window.__vlcBridgeInstalled) return 'already_installed';
    window.__vlcBridgeInstalled = true;

    console.log('VLC_BRIDGE:INJECTED');

    // --- Strategy A: Hook HTMLMediaElement.play ---
    // Intercept any <video> element's play() call and check if the src
    // looks like a Jellyfin stream URL.
    const origPlay = HTMLMediaElement.prototype.play;
    HTMLMediaElement.prototype.play = function() {
        const src = this.src || '';
        const sourceEl = this.querySelector && this.querySelector('source');
        const sourceSrc = sourceEl ? sourceEl.src : '';
        const url = src || sourceSrc;

        // Check if this is a Jellyfin video stream
        if (url && (url.includes('/Videos/') || url.includes('/video'))) {
            console.log('VLC_PLAY:' + url);
            // Stop the internal player
            this.pause();
            try { this.removeAttribute('src'); this.load(); } catch(e) {}
            return Promise.resolve();
        }

        return origPlay.call(this);
    };

    // --- Strategy B: Watch for src changes on video elements ---
    // Use MutationObserver to catch dynamically created video elements
    // and intercept their source assignment.
    const observer = new MutationObserver(function(mutations) {
        for (const m of mutations) {
            for (const node of m.addedNodes) {
                if (!node || !node.tagName) continue;
                const videos = [];
                if (node.tagName === 'VIDEO') videos.push(node);
                if (node.querySelectorAll) {
                    node.querySelectorAll('video').forEach(v => videos.push(v));
                }
                for (const video of videos) {
                    hookVideoElement(video);
                }
            }
        }
    });

    function hookVideoElement(video) {
        if (video.__vlcHooked) return;
        video.__vlcHooked = true;

        // Watch the src property
        const origSrcDesc = Object.getOwnPropertyDescriptor(
            HTMLMediaElement.prototype, 'src'
        );
        if (origSrcDesc && origSrcDesc.set) {
            let _src = video.getAttribute('src') || '';
            Object.defineProperty(video, 'src', {
                get: function() { return _src; },
                set: function(val) {
                    _src = val;
                    if (val && (val.includes('/Videos/') || val.includes('/video'))) {
                        console.log('VLC_PLAY:' + val);
                        // Don't actually set the src — VLC will handle it
                        return;
                    }
                    origSrcDesc.set.call(this, val);
                },
                configurable: true,
            });
        }
    }

    if (document.body) {
        observer.observe(document.body, { childList: true, subtree: true });
    } else {
        document.addEventListener('DOMContentLoaded', function() {
            observer.observe(document.body, { childList: true, subtree: true });
        });
    }

    // --- Strategy C: Intercept fetch/XHR for PlaybackInfo ---
    // When the web client fetches PlaybackInfo, we know a play is about to
    // happen. Extract the item ID and build our own stream URL.
    const origFetch = window.fetch;
    window.fetch = function(input, init) {
        const url = typeof input === 'string' ? input : (input.url || '');
        if (url.includes('/PlaybackInfo')) {
            // Extract item ID from URL: /Items/{id}/PlaybackInfo
            const match = url.match(/\/Items\/([a-f0-9]+)\/PlaybackInfo/i);
            if (match) {
                const itemId = match[1];
                return origFetch.apply(this, arguments).then(function(response) {
                    return response.clone().json().then(function(data) {
                        if (data.MediaSources && data.MediaSources.length > 0) {
                            const ms = data.MediaSources[0];
                            const msId = ms.Id;
                            // Get auth token from ApiClient
                            let token = '';
                            let server = '';
                            try {
                                token = window.ApiClient.accessToken();
                                server = window.ApiClient.serverAddress();
                            } catch(e) {}
                            const streamUrl = server +
                                '/Videos/' + itemId +
                                '/stream?static=true' +
                                '&mediaSourceId=' + msId +
                                '&api_key=' + token;
                            // Get resume position (ticks)
                            let ticks = 0;
                            try {
                                const userData = data.MediaSources[0].UserData ||
                                    window.ApiClient._currentUser?.UserData;
                                // Try getting from the item's user data
                            } catch(e) {}
                            console.log('VLC_PLAY_INFO:' + JSON.stringify({
                                url: streamUrl,
                                itemId: itemId,
                                mediaSourceId: msId,
                                token: token,
                                server: server,
                                ticks: ticks,
                            }));
                        }
                        return response;
                    }).catch(function() {
                        return response;
                    });
                });
            }
        }
        return origFetch.apply(this, arguments);
    };

    // --- Strategy D: Direct ApiClient hook ---
    // Try to hook into the Jellyfin playback manager if available.
    function tryHookPlaybackManager() {
        try {
            const events = window.Events || window.Emby?.Events;
            if (!events) return false;

            // The playback manager fires 'playbackstart'
            document.addEventListener('itemplayback', function(e) {
                console.log('VLC_BRIDGE:itemplayback event fired');
            });

            return true;
        } catch(e) {
            return false;
        }
    }
    tryHookPlaybackManager();

    // --- Periodic check: get resume info for the current item ---
    // This helps us get the resume position when play is triggered.
    window.__vlcGetResumeInfo = function(itemId) {
        try {
            const server = window.ApiClient.serverAddress();
            const token = window.ApiClient.accessToken();
            const userId = window.ApiClient.getCurrentUserId();
            return fetch(server + '/Users/' + userId + '/Items/' + itemId, {
                headers: { 'X-Emby-Authorization': 'MediaBrowser Token="' + token + '"' }
            }).then(r => r.json()).then(item => {
                return {
                    ticks: (item.UserData && item.UserData.PlaybackPositionTicks) || 0,
                    name: item.Name || '',
                };
            });
        } catch(e) {
            return Promise.resolve({ ticks: 0, name: '' });
        }
    };

    return 'installed';
})();
"""


class CDPBridge:
    """
    Strategy 1: Connect to JMP via Chrome DevTools Protocol WebSocket.
    Injects JavaScript to intercept playback and receives stream URLs
    via console.log messages.
    """

    def __init__(self, jellyfin_auth):
        self.auth = jellyfin_auth
        self.ws = None
        self.msg_id = 0
        self.running = False
        self._recv_thread = None

    def _next_id(self):
        self.msg_id += 1
        return self.msg_id

    def connect(self):
        """Connect to CDP WebSocket. Returns True on success."""
        if not HAS_WEBSOCKET:
            log.warning("websocket-client not available, CDP strategy disabled")
            return False

        try:
            # Get the WebSocket URL from CDP
            status, body = http_get(f"{CDP_URL}/json", timeout=5)
            if status != 200:
                log.debug(f"CDP /json returned {status}")
                return False

            pages = json.loads(body)
            if not pages:
                log.debug("No CDP pages found")
                return False

            ws_url = pages[0].get("webSocketDebuggerUrl", "")
            if not ws_url:
                log.debug("No webSocketDebuggerUrl in CDP response")
                return False

            log.info(f"Connecting to CDP: {ws_url}")
            self.ws = ws_client.create_connection(
                ws_url,
                suppress_origin=True,
                timeout=10,
            )

            # Enable Runtime events (so we receive console.log messages)
            self._send({"method": "Runtime.enable"})
            time.sleep(0.2)

            # Inject our JavaScript hooks
            result = self._send_and_recv({
                "method": "Runtime.evaluate",
                "params": {
                    "expression": CDP_INJECT_JS,
                    "returnByValue": True,
                },
            })
            if result:
                val = (
                    result.get("result", {})
                    .get("result", {})
                    .get("value", "")
                )
                log.info(f"JS injection result: {val}")

            return True

        except Exception as e:
            log.warning(f"CDP connect failed: {e}")
            return False

    def _send(self, msg):
        """Send a CDP message (fire-and-forget)."""
        msg["id"] = self._next_id()
        self.ws.send(json.dumps(msg))
        return msg["id"]

    def _send_and_recv(self, msg, timeout=5):
        """Send a CDP message and wait for the response."""
        msg_id = self._send(msg)
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                self.ws.settimeout(timeout)
                raw = self.ws.recv()
                data = json.loads(raw)
                if data.get("id") == msg_id:
                    return data
            except Exception:
                break
        return None

    def run(self):
        """
        Main loop: listen for CDP events and handle VLC_PLAY messages.
        Blocks until shutdown_event is set or connection is lost.
        """
        self.running = True
        log.info("CDP bridge running, listening for playback events...")

        while self.running and not shutdown_event.is_set():
            try:
                self.ws.settimeout(1.0)
                raw = self.ws.recv()
                data = json.loads(raw)

                # Check for console messages
                if data.get("method") == "Runtime.consoleAPICalled":
                    args = data.get("params", {}).get("args", [])
                    for arg in args:
                        val = arg.get("value", "")
                        if isinstance(val, str):
                            self._handle_console_message(val)

            except ws_client.WebSocketTimeoutException:
                # Normal — no messages received within timeout
                continue
            except ws_client.WebSocketConnectionClosedException:
                log.warning("CDP WebSocket connection closed")
                self.running = False
                break
            except Exception as e:
                if not shutdown_event.is_set():
                    log.warning(f"CDP recv error: {e}")
                    self.running = False
                break

        log.info("CDP bridge stopped")

    def _handle_console_message(self, msg):
        """Handle a console.log message from the injected JS."""
        if msg.startswith("VLC_PLAY_INFO:"):
            # Full play info with JSON payload
            try:
                info = json.loads(msg[len("VLC_PLAY_INFO:"):])
                url = info.get("url", "")
                item_id = info.get("itemId", "")
                media_source_id = info.get("mediaSourceId", "")
                ticks = info.get("ticks", 0)

                log.info(f"CDP play request: item={item_id}")

                # Get resume position from API if ticks is 0
                start_secs = ticks / 10_000_000 if ticks else 0
                if start_secs == 0 and item_id and self.auth:
                    start_secs = self._get_resume_position(item_id)

                # Stop JMP's internal playback via JS
                self._stop_jmp_playback()

                # Launch VLC
                self._launch_vlc_for_item(url, item_id, media_source_id, start_secs)

            except json.JSONDecodeError as e:
                log.error(f"Failed to parse VLC_PLAY_INFO: {e}")

        elif msg.startswith("VLC_PLAY:"):
            # Simple URL-only play request
            url = msg[len("VLC_PLAY:"):]
            log.info(f"CDP simple play request: {url[:80]}...")

            # Extract item ID from URL if possible
            item_id = ""
            media_source_id = ""
            import re
            m = re.search(r"/Videos/([a-f0-9]+)/", url, re.IGNORECASE)
            if m:
                item_id = m.group(1)
            m2 = re.search(r"mediaSourceId=([a-f0-9]+)", url, re.IGNORECASE)
            if m2:
                media_source_id = m2.group(1)

            start_secs = 0
            if item_id and self.auth:
                start_secs = self._get_resume_position(item_id)

            self._stop_jmp_playback()
            self._launch_vlc_for_item(url, item_id, media_source_id, start_secs)

        elif msg.startswith("VLC_BRIDGE:"):
            log.info(f"Bridge JS: {msg}")

    def _get_resume_position(self, item_id):
        """Get the resume position for an item in seconds."""
        try:
            item = self.auth.get_item(item_id)
            if item:
                ticks = (
                    item.get("UserData", {})
                    .get("PlaybackPositionTicks", 0)
                )
                secs = ticks / 10_000_000
                if secs > 5:  # Only resume if > 5 seconds in
                    log.info(f"Resume position for {item_id}: {secs:.0f}s")
                    return secs
        except Exception as e:
            log.warning(f"Failed to get resume position: {e}")
        return 0

    def _stop_jmp_playback(self):
        """Stop JMP's internal playback via CDP JavaScript."""
        try:
            self._send({
                "method": "Runtime.evaluate",
                "params": {
                    "expression": """
                        (function() {
                            // Stop all video elements
                            document.querySelectorAll('video').forEach(v => {
                                v.pause();
                                v.removeAttribute('src');
                                v.load();
                            });
                            // Try to stop via Jellyfin playback manager
                            try {
                                if (window.Emby && window.Emby.PlaybackManager) {
                                    window.Emby.PlaybackManager.stop();
                                }
                            } catch(e) {}
                            try {
                                // Alternative: use the global playbackManager
                                const pm = document.querySelector(
                                    '.videoPlayerContainer'
                                );
                                if (pm) pm.innerHTML = '';
                            } catch(e) {}
                            return 'stopped';
                        })();
                    """,
                },
            })
        except Exception as e:
            log.warning(f"Failed to stop JMP playback: {e}")

    def _launch_vlc_for_item(self, url, item_id, media_source_id, start_secs):
        """Launch VLC for a specific item (runs in a thread to not block CDP)."""
        thread = threading.Thread(
            target=self._vlc_playback_thread,
            args=(url, item_id, media_source_id, start_secs),
            daemon=True,
        )
        thread.start()

    def _vlc_playback_thread(self, url, item_id, media_source_id, start_secs):
        """Thread: launch VLC, wait for exit, report position."""
        # Report playback start to Jellyfin
        if self.auth and item_id:
            start_ticks = int(start_secs * 10_000_000)
            self.auth.report_playback_start(
                item_id, media_source_id or item_id, start_ticks
            )

        # Launch VLC (blocks until exit)
        end_position = launch_vlc(
            url, start_time_secs=start_secs,
            item_id=item_id, jellyfin_auth=self.auth,
        )

        # Report playback stopped to Jellyfin with approximate position
        if self.auth and item_id:
            end_ticks = int(end_position * 10_000_000)
            self.auth.report_playback_stop(item_id, end_ticks)

        # Re-inject hooks (JMP page might have changed state)
        try:
            if self.ws and self.running:
                self._send({
                    "method": "Runtime.evaluate",
                    "params": {"expression": CDP_INJECT_JS},
                })
        except Exception:
            pass

    def close(self):
        """Close the CDP WebSocket connection."""
        self.running = False
        if self.ws:
            try:
                self.ws.close()
            except Exception:
                pass


# ---------------------------------------------------------------------------
# Strategy 2: API Polling
# ---------------------------------------------------------------------------

class APIPoller:
    """
    Strategy 2: Poll Jellyfin /Sessions API to detect when JMP starts
    playing, then intercept and hand off to VLC.
    """

    def __init__(self, jellyfin_auth):
        self.auth = jellyfin_auth
        self.running = False
        self._last_playing_item = None
        self._jmp_device_names = {
            "Jellyfin Media Player",
            "jellyfin-media-player",
            "jellyfinmediaplayer",
        }

    def run(self):
        """
        Main loop: poll sessions and intercept playback.
        Blocks until shutdown_event is set.
        """
        self.running = True
        log.info("API poller running, polling sessions...")

        while self.running and not shutdown_event.is_set():
            try:
                sessions = self.auth.get_sessions()
                jmp_session = self._find_jmp_session(sessions)

                if jmp_session:
                    now_playing = jmp_session.get("NowPlayingItem")
                    if now_playing:
                        item_id = now_playing.get("Id", "")
                        item_name = now_playing.get("Name", "unknown")

                        # Only trigger once per item (avoid re-triggering)
                        if item_id != self._last_playing_item:
                            self._last_playing_item = item_id
                            log.info(
                                f"API detected playback: "
                                f"{item_name} ({item_id})"
                            )
                            self._intercept_playback(
                                jmp_session, now_playing
                            )
                    else:
                        # Nothing playing — reset tracker
                        self._last_playing_item = None

            except Exception as e:
                log.debug(f"Session poll error: {e}")

            # Sleep between polls (interruptible)
            shutdown_event.wait(SESSION_POLL_INTERVAL)

        log.info("API poller stopped")

    def _find_jmp_session(self, sessions):
        """Find the JMP session among all active sessions."""
        for session in sessions:
            client = session.get("Client", "")
            device = session.get("DeviceName", "")
            # Match JMP by client name or device name
            if (
                "Jellyfin Media Player" in client
                or device in self._jmp_device_names
            ):
                return session
        return None

    def _intercept_playback(self, session, now_playing):
        """Intercept playback: stop JMP, launch VLC."""
        session_id = session.get("Id", "")
        item_id = now_playing.get("Id", "")
        item_name = now_playing.get("Name", "unknown")
        media_type = now_playing.get("Type", "")

        # Only intercept video playback (not music)
        if media_type not in ("Movie", "Episode", "Video", "MusicVideo", ""):
            log.info(f"Skipping non-video item: {item_name} (type={media_type})")
            return

        # Get resume position from the session's PlayState
        play_state = session.get("PlayState", {})
        position_ticks = play_state.get("PositionTicks", 0)
        start_secs = position_ticks / 10_000_000 if position_ticks else 0

        # Get playback info to find media source
        playback_info = self.auth.get_playback_info(item_id)
        if not playback_info:
            log.error(f"Could not get playback info for {item_id}")
            return

        media_sources = playback_info.get("MediaSources", [])
        if not media_sources:
            log.error(f"No media sources for {item_id}")
            return

        media_source = media_sources[0]
        media_source_id = media_source.get("Id", item_id)

        # Build stream URL
        stream_url = self.auth.build_stream_url(item_id, media_source_id)

        log.info(
            f"Intercepting: {item_name} "
            f"(resume={start_secs:.0f}s, source={media_source_id[:8]}...)"
        )

        # Stop JMP playback
        self.auth.stop_session(session_id)
        time.sleep(0.3)

        # Report playback start
        start_ticks = int(start_secs * 10_000_000)
        self.auth.report_playback_start(
            item_id, media_source_id, start_ticks
        )

        # Launch VLC (blocks until exit)
        end_position = launch_vlc(
            stream_url, start_time_secs=start_secs,
            item_id=item_id, jellyfin_auth=self.auth,
        )

        # Report playback stopped
        end_ticks = int(end_position * 10_000_000)
        self.auth.report_playback_stop(item_id, end_ticks)

        # Reset tracking so the same item can be played again
        self._last_playing_item = None

    def stop(self):
        self.running = False


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def wait_for_cdp(timeout=60):
    """Wait for CDP to become available (JMP needs time to start)."""
    log.info(f"Waiting for CDP on port {CDP_PORT} (max {timeout}s)...")
    deadline = time.time() + timeout
    while time.time() < deadline and not shutdown_event.is_set():
        status, _ = http_get(f"{CDP_URL}/json", timeout=3)
        if status == 200:
            log.info("CDP available")
            return True
        shutdown_event.wait(2)
    log.warning("CDP not available within timeout")
    return False


def main():
    log.info("=" * 60)
    log.info("VLC Bridge starting")
    log.info(f"  Server:  {JELLYFIN_URL}")
    log.info(f"  User:    {JELLYFIN_USER}")
    log.info(f"  CDP:     localhost:{CDP_PORT}")
    log.info(f"  VLC:     {VLC_BIN}")
    log.info(f"  WS lib:  {'websocket-client' if HAS_WEBSOCKET else 'NOT AVAILABLE'}")
    log.info("=" * 60)

    # --- Authenticate with Jellyfin ---
    auth = JellyfinAuth(JELLYFIN_URL, JELLYFIN_USER, JELLYFIN_PASS)
    for attempt in range(1, 11):
        if shutdown_event.is_set():
            return
        if auth.authenticate():
            break
        log.warning(f"Auth attempt {attempt}/10 failed, retrying in 5s...")
        shutdown_event.wait(5)
    else:
        log.error("Failed to authenticate after 10 attempts, exiting")
        return

    # --- Try Strategy 1: CDP ---
    cdp_bridge = None
    api_poller = None

    if HAS_WEBSOCKET:
        cdp_available = wait_for_cdp(timeout=120)
        if cdp_available:
            cdp_bridge = CDPBridge(auth)
            if cdp_bridge.connect():
                log.info("Strategy 1 (CDP) active")
            else:
                log.warning("CDP connect failed, falling back to API polling")
                cdp_bridge = None

    # --- Start API poller as fallback (or primary if CDP unavailable) ---
    # Always start the API poller in a background thread; if CDP is active,
    # it serves as a safety net. If CDP is the sole strategy, the API poller
    # becomes primary.
    api_poller = APIPoller(auth)
    api_thread = threading.Thread(target=api_poller.run, daemon=True, name="api-poller")
    api_thread.start()
    log.info("Strategy 2 (API polling) active as background/fallback")

    # --- Main loop ---
    if cdp_bridge:
        # CDP is primary — run it in the foreground
        try:
            cdp_bridge.run()
        except Exception as e:
            log.error(f"CDP bridge crashed: {e}")
        finally:
            cdp_bridge.close()

        # CDP died — API poller is still running as fallback
        if not shutdown_event.is_set():
            log.info("CDP lost, API poller continues as primary")
            # Just wait for shutdown
            while not shutdown_event.is_set():
                shutdown_event.wait(1)
    else:
        # No CDP — just wait for shutdown while API poller runs
        log.info("Running with API polling only (no CDP)")
        while not shutdown_event.is_set():
            shutdown_event.wait(1)

    # --- Cleanup ---
    api_poller.stop()
    if cdp_bridge:
        cdp_bridge.close()
    log.info("VLC Bridge stopped")


if __name__ == "__main__":
    main()
