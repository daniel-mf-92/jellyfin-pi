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
  Jellyfin server: set via JELLYFIN_URL (e.g. http://localhost:8096)

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

# Load .env from script directory
from pathlib import Path
_env_file = Path(__file__).resolve().parent / ".env"
if _env_file.exists():
    for _line in _env_file.read_text().splitlines():
        _line = _line.strip()
        if _line and not _line.startswith("#") and "=" in _line:
            _k, _, _v = _line.partition("=")
            _k, _v = _k.strip(), _v.strip().strip("\"'")
            if _k and _k not in os.environ:
                os.environ[_k] = _v

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

JELLYFIN_URL = os.environ.get("JELLYFIN_URL", "http://localhost:8096")
JELLYFIN_USER = os.environ.get("JELLYFIN_USER", "")
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
        if status == 0:
            raise ConnectionError("Could not connect to Jellyfin server")
        return []  # HTTP error but server reachable — treat as empty

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

    def report_playback_start(self, item_id, media_source_id, position_ticks=0,
                             play_method="DirectStream"):
        """Report playback started to Jellyfin (so it tracks progress)."""
        url = f"{self.server_url}/Sessions/Playing"
        data = {
            "ItemId": item_id,
            "MediaSourceId": media_source_id,
            "PositionTicks": position_ticks,
            "PlayMethod": play_method,
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

    def build_stream_url(self, item_id, media_source_id, start_time_secs=0,
                         source_bitrate_bps=None):
        """Build stream URL, transcoding to match available bandwidth.

        Uses startTimeTicks for resume positions so Jellyfin starts transcoding
        at the right point. VLC cannot seek in transcoded HTTP streams.

        Returns:
            tuple: (url, play_method, effective_bitrate_bps)
        """
        bw = _get_bandwidth_config()
        # Conservative defaults — the WireGuard pipe is often < 1 Mbps
        video_br = bw.get("video_bitrate", 500000)
        audio_br = bw.get("audio_bitrate", 64000)
        max_w = bw.get("max_width", 854)
        max_h = bw.get("max_height", 480)

        # Floor at 200kbps, round to nearest 50kbps
        video_br = max(200000, (video_br // 50000) * 50000)

        transcode_params = {
            "Static": "false",
            "VideoCodec": "h264",
            "AudioCodec": "aac",
            "MaxVideoBitDepth": "8",
            "VideoBitRate": str(video_br),
            "AudioBitRate": str(audio_br),
            "MaxWidth": str(max_w),
            "MaxHeight": str(max_h),
            "SubtitleStreamIndex": "-1",
            "mediaSourceId": media_source_id,
            "api_key": self.token,
        }
        # Server-side seek for resume: Jellyfin starts transcoding at the right position.
        # Without this, VLC can't seek in a transcoded HTTP stream and crashes after ~8s.
        if start_time_secs > 0:
            transcode_params["startTimeTicks"] = str(int(start_time_secs * 10_000_000))

        params = urllib.parse.urlencode(transcode_params)
        effective_bps = video_br + audio_br
        log.info(
            f"Stream profile: {video_br/1000:.0f}kbps video, "
            f"{audio_br/1000:.0f}kbps audio, {max_w}x{max_h} "
            f"(total={effective_bps/1000:.0f}kbps)"
        )
        return (f"{self.server_url}/Videos/{item_id}/stream.mkv?{params}",
                "Transcode", effective_bps)


# ---------------------------------------------------------------------------
# Bandwidth-aware streaming
# ---------------------------------------------------------------------------

BW_CONFIG_FILE = Path("/tmp/pi-home-wg-bandwidth.json")

def _get_bandwidth_config():
    """Read bandwidth config written by master script every 5 min."""
    try:
        if BW_CONFIG_FILE.exists():
            with open(BW_CONFIG_FILE) as f:
                cfg = json.load(f)
            # Sanity: file must be < 30 min old
            ts = cfg.get("timestamp", "")
            if ts and ts != "now":
                from datetime import datetime, timezone
                try:
                    written = datetime.fromisoformat(ts.replace("Z", "+00:00"))
                    age = (datetime.now(timezone.utc) - written).total_seconds()
                    if age > 1800:
                        log.debug(f"BW config stale ({age:.0f}s old), ignoring")
                        return {}
                except Exception:
                    pass
            return cfg
    except Exception as e:
        log.warning(f"Could not read bandwidth config: {e}")
    return {}


# ---------------------------------------------------------------------------
# Adaptive RAM cache
# ---------------------------------------------------------------------------

# Tunables (override via environment)
CACHE_RAM_RESERVE_GB = float(os.environ.get("VLC_CACHE_RAM_RESERVE_GB", "2"))
CACHE_MAX_SECS = int(os.environ.get("VLC_CACHE_MAX_SECS", "300"))      # 5 min cap
CACHE_MIN_SECS = int(os.environ.get("VLC_CACHE_MIN_SECS", "15"))       # floor
CACHE_MAX_PREFETCH_MB = int(os.environ.get("VLC_CACHE_MAX_PREFETCH_MB", "512"))
CACHE_DEFAULT_BITRATE_BPS = 4_000_000  # 4 Mbps fallback assumption


def _get_available_ram_bytes():
    """Read MemAvailable from /proc/meminfo."""
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemAvailable:"):
                    return int(line.split()[1]) * 1024  # kB -> bytes
    except Exception as e:
        log.warning(f"Could not read /proc/meminfo: {e}")
    return 0


def get_adaptive_cache_params(stream_bitrate_bps=None):
    """
    Calculate VLC cache parameters based on available system RAM and stream
    bitrate.  Returns a dict with keys used by launch_vlc().

    Strategy:
      1. Read available RAM, subtract a safety reserve.
      2. Given the stream bitrate, compute how many seconds of content the
         remaining RAM can hold.
      3. Clamp to [CACHE_MIN_SECS .. CACHE_MAX_SECS].
      4. Size the prefetch buffer as 25 % of usable RAM (capped).
    """
    available = _get_available_ram_bytes()

    if available <= 0:
        # Cannot determine RAM — use conservative 30 s defaults
        log.warning("RAM detection failed, using 30 s static cache")
        return {
            "network_caching_ms": 30_000,
            "file_caching_ms": 30_000,
            "live_caching_ms": 30_000,
            "prefetch_buffer_size": 16 * 1024 * 1024,  # 16 MB
        }

    reserve = int(CACHE_RAM_RESERVE_GB * 1024 ** 3)
    usable = max(available - reserve, 256 * 1024 * 1024)   # at least 256 MB
    usable = min(usable, int(available * 0.75))             # never exceed 75 %

    # Effective bitrate (bits/sec -> bytes/sec)
    bitrate = stream_bitrate_bps if stream_bitrate_bps and stream_bitrate_bps > 0 else CACHE_DEFAULT_BITRATE_BPS
    stream_Bps = bitrate / 8

    # Time-based cache
    max_cache_secs = usable / stream_Bps if stream_Bps > 0 else CACHE_MAX_SECS
    cache_secs = max(CACHE_MIN_SECS, min(max_cache_secs, CACHE_MAX_SECS))
    cache_ms = int(cache_secs * 1000)

    # Prefetch buffer — 25 % of usable, capped
    prefetch = min(usable // 4, CACHE_MAX_PREFETCH_MB * 1024 * 1024)
    prefetch = max(prefetch, 1024 * 1024)  # at least 1 MB

    # network-caching controls how long VLC buffers BEFORE showing the first
    # frame — it must stay LOW (fast startup).  The deep read-ahead lives in
    # prefetch-buffer-size which buffers in the background without blocking.
    STARTUP_CACHE_MS = 15000  # 15 s — WireGuard via Azure has 150ms RTT + jitter
    FILE_CACHE_MS = 300       # local /dev/shm files need almost nothing

    log.info(
        f"Adaptive cache: avail={available / 1024 ** 3:.1f}GB "
        f"usable={usable / 1024 ** 3:.1f}GB "
        f"bitrate={bitrate / 1e6:.1f}Mbps "
        f"prefetch={prefetch / 1024 ** 2:.0f}MB"
    )

    return {
        "network_caching_ms": STARTUP_CACHE_MS,
        "file_caching_ms": FILE_CACHE_MS,
        "live_caching_ms": STARTUP_CACHE_MS,
        "prefetch_buffer_size": int(prefetch),
    }


# ---------------------------------------------------------------------------
# Persistent RAM cache  (/dev/shm — tmpfs, survives process exit)
# ---------------------------------------------------------------------------
#
# Strategy: download every stream to /dev/shm while VLC plays it.  After VLC
# exits the file stays in RAM.  Next play of the same item is instant from
# file://.  When system MemAvailable drops below a floor, evict the OLDEST
# accessed entry first (LRU).  The currently-playing entry is never evicted.
#
# /dev/shm is 8 GB on this Pi 5 (50 % of 16 GB).  We can resize it with
# VLC_CACHE_SHM_SIZE_GB or via `sudo mount -o remount,size=12g /dev/shm`.

CACHE_DIR = Path(os.environ.get("VLC_CACHE_DIR", "/dev/shm/vlc-cache"))
CACHE_PRESSURE_FLOOR_GB = float(os.environ.get("VLC_CACHE_PRESSURE_FLOOR_GB", "2.0"))


class StreamCache:
    """
    RAM-backed LRU media cache with memory-pressure eviction.

    Eviction order (chronological):
      1. Oldest-accessed entry first  (pure LRU)
      2. Currently-playing entry is pinned — never evicted
      3. Eviction only triggers when MemAvailable < CACHE_PRESSURE_FLOOR_GB
    """

    def __init__(self):
        self._lock = threading.Lock()
        self._entries = {}              # item_id -> dict
        self._download_stops = {}       # item_id -> threading.Event
        self._playing_id = None         # pinned item
        CACHE_DIR.mkdir(parents=True, exist_ok=True)
        self._scan_existing()
        threading.Thread(target=self._pressure_loop, daemon=True).start()
        log.info(
            f"StreamCache: dir={CACHE_DIR} "
            f"floor={CACHE_PRESSURE_FLOOR_GB}GB "
            f"existing={len(self._entries)} items"
        )

    # -- bootstrap -----------------------------------------------------------

    def _scan_existing(self):
        """Re-register files that survived a restart (oldest priority)."""
        for f in CACHE_DIR.iterdir():
            if f.is_file():
                item_id = f.stem
                try:
                    st = f.stat()
                except OSError:
                    continue
                self._entries[item_id] = {
                    "path": f,
                    "size": st.st_size,
                    "complete": True,
                    "downloading": False,
                    "last_access": st.st_mtime,
                }

    # -- public API ----------------------------------------------------------

    def get(self, item_id):
        """Return local file path if fully cached, else None."""
        with self._lock:
            e = self._entries.get(item_id)
            if e and e["complete"] and e["path"].exists():
                e["last_access"] = time.time()
                return str(e["path"])
        return None

    def pin(self, item_id):
        """Mark item as currently playing (immune to eviction)."""
        with self._lock:
            self._playing_id = item_id
            e = self._entries.get(item_id)
            if e:
                e["last_access"] = time.time()

    def unpin(self):
        """Clear the currently-playing pin (item stays cached, just evictable)."""
        with self._lock:
            if self._playing_id and self._playing_id in self._entries:
                self._entries[self._playing_id]["last_access"] = time.time()
            self._playing_id = None

    def start_download(self, item_id, url):
        """Begin background download of stream to /dev/shm (idempotent)."""
        with self._lock:
            e = self._entries.get(item_id)
            if e and (e["complete"] or e.get("downloading")):
                return                  # already done or in progress
            stop_evt = threading.Event()
            self._download_stops[item_id] = stop_evt
            self._entries[item_id] = {
                "path": CACHE_DIR / item_id,
                "size": 0,
                "complete": False,
                "downloading": True,
                "last_access": time.time(),
            }
        threading.Thread(
            target=self._download, args=(item_id, url, stop_evt), daemon=True
        ).start()

    def cancel_download(self, item_id):
        evt = self._download_stops.pop(item_id, None)
        if evt:
            evt.set()

    def stats(self):
        with self._lock:
            total = sum(e["size"] for e in self._entries.values())
            n = len(self._entries)
            pinned = self._playing_id or "none"
        return f"{n} items {total / 1024 ** 2:.0f}MB pinned={pinned}"

    # -- download ------------------------------------------------------------

    def _download(self, item_id, url, stop_evt):
        entry = self._entries.get(item_id)
        if not entry:
            return
        path = entry["path"]
        log.info(f"Cache download start: {item_id[:8]}...")
        try:
            req = urllib.request.Request(url)
            with urllib.request.urlopen(req, timeout=600) as resp:
                with open(path, "wb") as f:
                    while not stop_evt.is_set():
                        chunk = resp.read(256 * 1024)   # 256 KB
                        if not chunk:
                            break
                        f.write(chunk)
                        with self._lock:
                            entry["size"] += len(chunk)
                        # Pause under pressure (don't make it worse)
                        if _get_available_ram_bytes() < CACHE_PRESSURE_FLOOR_GB * 1024 ** 3:
                            log.info(f"Cache download paused ({item_id[:8]}): RAM pressure")
                            for _ in range(15):          # wait up to 30 s
                                if stop_evt.wait(2):
                                    break
                                if _get_available_ram_bytes() >= CACHE_PRESSURE_FLOOR_GB * 1024 ** 3:
                                    break
                            else:
                                log.warning(f"Cache download aborted ({item_id[:8]}): sustained pressure")
                                with self._lock:
                                    entry["downloading"] = False
                                return

            with self._lock:
                entry["complete"] = True
                entry["downloading"] = False
            log.info(
                f"Cache complete: {item_id[:8]} "
                f"size={entry['size'] / 1024 ** 2:.0f}MB [{self.stats()}]"
            )
        except Exception as e:
            log.warning(f"Cache download error ({item_id[:8]}): {e}")
            with self._lock:
                entry["downloading"] = False
                # Partial file stays — might be useful for restart later

    # -- pressure monitor (LRU eviction) ------------------------------------

    def _pressure_loop(self):
        """Every 5 s: if MemAvailable < floor, evict oldest non-pinned entry."""
        while not shutdown_event.is_set():
            shutdown_event.wait(5)
            avail = _get_available_ram_bytes()
            floor = CACHE_PRESSURE_FLOOR_GB * 1024 ** 3
            if avail < floor:
                self._evict_lru(avail)

    def _evict_lru(self, avail_bytes):
        with self._lock:
            candidates = [
                (k, v) for k, v in self._entries.items()
                if k != self._playing_id and not v.get("downloading")
            ]
            if not candidates:
                log.warning(
                    f"RAM pressure ({avail_bytes / 1024 ** 3:.1f}GB free) "
                    f"but nothing evictable"
                )
                return
            # Pure LRU: oldest last_access first
            candidates.sort(key=lambda kv: kv[1]["last_access"])
            victim_id, victim = candidates[0]
            try:
                victim["path"].unlink(missing_ok=True)
            except OSError:
                pass
            freed_mb = victim["size"] / 1024 ** 2
            del self._entries[victim_id]
        log.info(
            f"Cache evict (LRU): {victim_id[:8]} "
            f"freed={freed_mb:.0f}MB "
            f"(was {avail_bytes / 1024 ** 3:.1f}GB free) [{self.stats()}]"
        )

    def cleanup(self):
        """Remove everything (called on shutdown)."""
        with self._lock:
            for e in self._entries.values():
                try:
                    e["path"].unlink(missing_ok=True)
                except OSError:
                    pass
            self._entries.clear()
            self._playing_id = None


# Global instance
stream_cache = StreamCache()


# ---------------------------------------------------------------------------
# VLC launcher
# ---------------------------------------------------------------------------

def launch_vlc(url, start_time_secs=0, item_id=None, jellyfin_auth=None,
               stream_bitrate_bps=None):
    """
    Launch VLC fullscreen with the given stream URL.
    Blocks until VLC exits. Handles foreground-app tracking.
    Returns the approximate playback position in seconds when VLC exited.

    stream_bitrate_bps: if known, the total bitrate of the stream in bits/sec.
    Used to size the adaptive RAM cache; falls back to a sensible default.
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

    # -- Persistent RAM cache: serve from /dev/shm if available -------------
    play_url = url
    _deferred_cache_download = None  # (item_id, url) when bg download was skipped
    if item_id:
        cached_path = stream_cache.get(item_id)
        if cached_path:
            play_url = cached_path
            log.info(f"Playing from RAM cache: {cached_path}")
        else:
            # Check if bandwidth is too narrow to share with a background download
            _bw_cfg = _get_bandwidth_config()
            _bw_total = _bw_cfg.get("total_bps", 0)
            if _bw_total and _bw_total < 3_000_000:
                log.info(
                    f"Skipping background cache download: bandwidth too low "
                    f"({_bw_total / 1000:.0f} kbps < 3000 kbps) — "
                    f"would starve VLC playback"
                )
                _deferred_cache_download = (item_id, url)
            else:
                reason = "adequate" if _bw_total else "unknown (no config)"
                log.info(
                    f"Starting background cache download: bandwidth {reason} "
                    f"({_bw_total / 1000:.0f} kbps)"
                )
                stream_cache.start_download(item_id, url)
                _deferred_cache_download = None
        stream_cache.pin(item_id)

    cache = get_adaptive_cache_params(stream_bitrate_bps)

    cmd = [
        VLC_BIN,
        "--fullscreen",
        "--play-and-exit",
        f"--audio-desync={VLC_AUDIO_DESYNC}",
        f"--network-caching={cache['network_caching_ms']}",
        f"--file-caching={cache['file_caching_ms']}",
        f"--live-caching={cache['live_caching_ms']}",
        "--http-reconnect",
        "--http-continuous",
        "--no-video-title-show",
        "--quiet",
        "--input-fast-seek",
        f"--prefetch-buffer-size={cache['prefetch_buffer_size']}",
        "--avcodec-threads=4",
    ]

    if start_time_secs > 0:
        cmd.append(f"--start-time={start_time_secs}")

    cmd.append(play_url)

    env = {**os.environ, **WAYLAND_ENV}

    from_cache = play_url != url
    log.info(
        f"Launching VLC: start={start_time_secs}s "
        f"desync={VLC_AUDIO_DESYNC}ms "
        f"net_cache={cache['network_caching_ms']}ms "
        f"prefetch={cache['prefetch_buffer_size'] / 1024 ** 2:.0f}MB "
        f"source={'RAM cache' if from_cache else 'network'}"
    )
    log.debug(f"VLC command: {' '.join(cmd[:8])}... <url>")

    set_foreground("vlc")

    # -- Instant black splash while VLC loads ------------------------------
    splash_proc = None
    splash_script = Path.home() / "bin" / "black-splash.py"
    if splash_script.exists():
        try:
            splash_proc = subprocess.Popen(
                ["python3", str(splash_script)],
                env={**os.environ, **WAYLAND_ENV},
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            log.debug("Black splash launched")
        except Exception as e:
            log.warning(f"Splash launch failed: {e}")

    def _kill_splash_when_vlc_ready(splash, vlc):
        """Background: poll for VLC window, then kill splash."""
        for _ in range(60):          # up to 6 s
            time.sleep(0.1)
            if vlc.poll() is not None:
                break                # VLC already exited
            try:
                r = subprocess.run(
                    ["wlrctl", "toplevel", "focus", "app_id:vlc"],
                    capture_output=True, timeout=1,
                )
                if r.returncode == 0:
                    break
            except Exception:
                pass
        if splash.poll() is None:
            splash.terminate()
            log.debug("Splash dismissed")

    vlc_start_time = time.time()

    try:
        with vlc_lock:
            current_vlc_proc = subprocess.Popen(
                cmd,
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

        # Kill splash once VLC window appears (non-blocking)
        if splash_proc:
            threading.Thread(
                target=_kill_splash_when_vlc_ready,
                args=(splash_proc, current_vlc_proc),
                daemon=True,
            ).start()

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

        # Kill splash if still alive (safety net)
        if splash_proc and splash_proc.poll() is None:
            splash_proc.terminate()

        # Unpin but keep cached — LRU timestamp refreshed, data stays in RAM
        stream_cache.unpin()

        # Deferred cache download: if we skipped during playback due to low
        # bandwidth, start it now so the file is cached for next time.
        if _deferred_cache_download:
            _post_bw = _get_bandwidth_config()
            _post_total = _post_bw.get("total_bps", 0)
            if not _post_total or _post_total >= 3_000_000:
                _did, _durl = _deferred_cache_download
                log.info(
                    f"Starting deferred cache download for {_did} "
                    f"(post-playback bandwidth: {_post_total / 1000:.0f} kbps)"
                )
                stream_cache.start_download(_did, _durl)
            else:
                log.info(
                    f"Skipping deferred cache download: bandwidth still low "
                    f"({_post_total / 1000:.0f} kbps)"
                )

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
    // --- TV Navigation CSS: Force focus indicators to always be visible ---
    // CSS injection runs EVERY time (idempotent, replaces existing style tag)
    var tvStyle = document.createElement('style');
    tvStyle.id = 'vlc-bridge-tv-css';
    tvStyle.textContent = [
        '/* Force focus visible on ALL interactive elements */',
        '*:focus-visible { outline: 3px solid #00a4dc !important; outline-offset: 2px !important; }',
        'button:focus, a:focus, [tabindex]:not([tabindex="-1"]):focus {',
        '  outline: 3px solid #00a4dc !important; outline-offset: 2px !important;',
        '  box-shadow: 0 0 0 4px rgba(0, 164, 220, 0.3) !important; }',
        '.card:focus, .cardBox:focus, .card:focus-visible, .cardBox:focus-visible {',
        '  outline: 3px solid #00a4dc !important; outline-offset: 2px !important;',
        '  transform: scale(1.05); transition: transform 0.15s ease; }',
        '.card.show-focus { outline: 3px solid #00a4dc !important; }',
        '.itemsContainer-tv *:focus { outline: 3px solid #00a4dc !important; outline-offset: 2px !important; }',
        '.listItem:focus, .listItem:focus-visible {',
        '  outline: 3px solid #00a4dc !important; outline-offset: -1px !important;',
        '  background-color: rgba(0, 164, 220, 0.15) !important; }',
        '/* Override outline:none rules */',
        'button, a, input, select, textarea { outline: revert !important; }',
        '/* Hide cursor in TV mode */',
        '.layout-tv { cursor: none !important; }',
        '.layout-tv * { cursor: none !important; }',
    ].join('\n');
    if (!document.getElementById('vlc-bridge-tv-css')) {
        document.head.appendChild(tvStyle);
        console.log('VLC_BRIDGE:TV_CSS_INJECTED');
    }

    // --- Prevent mouse mode: intercept mousemove to preserve keyboard focus ---
    // Jellyfin switches from keyboard to mouse mode on any mouse event.
    // Block synthetic/virtual mouse events from triggering mode switch.
    var origAddEventListener = EventTarget.prototype.addEventListener;
    var mouseBlocked = false;
    document.addEventListener('mousemove', function(e) {
        // If movement is tiny (stick drift), prevent propagation
        if (Math.abs(e.movementX) < 3 && Math.abs(e.movementY) < 3) {
            e.stopImmediatePropagation();
        }
    }, true);

    // Force keyboard focus mode after any keydown
    document.addEventListener('keydown', function() {
        // Re-add focus class to body if Jellyfin removed it
        document.body.classList.add('keyboard-focus');
        document.body.classList.remove('mouse-focus');
    }, true);

    // Avoid double-injection of playback hooks
    if (window.__vlcBridgeInstalled) return 'css_refreshed';
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

                # Rebuild URL through adaptive build_stream_url
                effective_bps = None
                if self.auth and item_id and media_source_id:
                    try:
                        src_bps = self._get_source_bitrate(item_id)
                        url, _method, effective_bps = self.auth.build_stream_url(
                            item_id, media_source_id,
                            start_time_secs=start_secs,
                            source_bitrate_bps=src_bps,
                        )
                        # Server handled seek — don't double-seek in VLC
                        if "startTimeTicks" in url:
                            start_secs = 0
                    except Exception as e:
                        log.warning(f"CDP URL rebuild failed, using JS URL: {e}")

                # Stop JMP's internal playback via JS
                self._stop_jmp_playback()

                # Launch VLC
                self._launch_vlc_for_item(url, item_id, media_source_id, start_secs,
                                          stream_bitrate_bps=effective_bps)

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
            effective_bps = None
            if item_id and self.auth:
                start_secs = self._get_resume_position(item_id)
                try:
                    src_bps = self._get_source_bitrate(item_id)
                    url, _method, effective_bps = self.auth.build_stream_url(
                        item_id, media_source_id,
                        start_time_secs=start_secs,
                        source_bitrate_bps=src_bps,
                    )
                    if "startTimeTicks" in url:
                        start_secs = 0
                except Exception as e:
                    log.warning(f"CDP simple URL rebuild failed: {e}")

            self._stop_jmp_playback()
            self._launch_vlc_for_item(url, item_id, media_source_id, start_secs,
                                      stream_bitrate_bps=effective_bps)

        elif msg.startswith("VLC_BRIDGE:"):
            log.info(f"Bridge JS: {msg}")

    def _get_source_bitrate(self, item_id):
        """Fetch the source bitrate (bps) for an item via PlaybackInfo API."""
        if not self.auth:
            return None
        try:
            pinfo = self.auth.get_playback_info(item_id)
            if not pinfo:
                return None
            sources = pinfo.get("MediaSources", [])
            if sources:
                bps = int(sources[0].get("Bitrate") or 0)
                return bps if bps > 0 else None
        except (ValueError, TypeError, KeyError) as e:
            log.debug(f"Could not extract source bitrate: {e}")
        return None

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

    def _launch_vlc_for_item(self, url, item_id, media_source_id, start_secs,
                             stream_bitrate_bps=None):
        """Launch VLC for a specific item (runs in a thread to not block CDP)."""
        thread = threading.Thread(
            target=self._vlc_playback_thread,
            args=(url, item_id, media_source_id, start_secs, stream_bitrate_bps),
            daemon=True,
        )
        thread.start()

    def _vlc_playback_thread(self, url, item_id, media_source_id, start_secs,
                              stream_bitrate_bps=None):
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
            stream_bitrate_bps=stream_bitrate_bps,
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
        Uses exponential backoff on connection failures.
        """
        self.running = True
        log.info("API poller running, polling sessions...")
        consecutive_failures = 0

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

                # Success — reset backoff
                if consecutive_failures > 0:
                    log.info(
                        f"API poll recovered after "
                        f"{consecutive_failures} failures"
                    )
                consecutive_failures = 0

            except Exception as e:
                consecutive_failures += 1
                if consecutive_failures == 1:
                    log.info(f"API poll failed, backing off: {e}")
                elif consecutive_failures % 20 == 0:
                    log.info(
                        f"API poll still failing "
                        f"({consecutive_failures} consecutive): {e}"
                    )
                else:
                    log.debug(f"Session poll error: {e}")

            # Exponential backoff on failure, normal interval on success
            if consecutive_failures > 0:
                backoff = min(
                    SESSION_POLL_INTERVAL * (2 ** consecutive_failures), 30
                )
            else:
                backoff = SESSION_POLL_INTERVAL
            shutdown_event.wait(backoff)

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

        # Extract bitrate for adaptive cache sizing
        try:
            stream_bitrate_bps = int(media_source.get("Bitrate") or 0) or None
        except (ValueError, TypeError):
            stream_bitrate_bps = None

        # Build stream URL with server-side seek for transcodes
        stream_url, play_method, effective_bps = self.auth.build_stream_url(
            item_id, media_source_id, start_time_secs=start_secs,
            source_bitrate_bps=stream_bitrate_bps)
        # Server handled the seek — don't double-seek in VLC
        if "startTimeTicks" in stream_url:
            start_secs = 0

        log.info(
            f"Intercepting: {item_name} "
            f"(resume={start_secs:.0f}s, source={media_source_id[:8]}..., "
            f"bitrate={stream_bitrate_bps or 'unknown'}, "
            f"method={play_method}, effective={effective_bps}bps)"
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
            stream_bitrate_bps=effective_bps,
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

    # --- Main loop (CDP with reconnection) ---
    # Whether CDP was available at startup or not, we enter a loop that
    # keeps trying to (re)establish CDP.  The API poller runs throughout.
    while not shutdown_event.is_set():
        if cdp_bridge:
            # CDP is primary — run it in the foreground (blocks until drop)
            try:
                cdp_bridge.run()
            except Exception as e:
                log.error(f"CDP bridge crashed: {e}")
            finally:
                cdp_bridge.close()

            if shutdown_event.is_set():
                break

            log.info("CDP lost, API poller active. Will retry CDP in 15s...")
        else:
            log.info("CDP not available, API poller is primary. Will retry CDP in 15s...")

        # --- CDP retry loop ---
        # Periodically check if CDP becomes available (JMP restart, etc.)
        while not shutdown_event.is_set():
            shutdown_event.wait(15)
            if shutdown_event.is_set():
                break

            if not HAS_WEBSOCKET:
                continue  # no websocket lib, can never use CDP

            status, _ = http_get(f"{CDP_URL}/json", timeout=3)
            if status != 200:
                continue

            log.info("CDP endpoint detected, attempting reconnect...")
            cdp_bridge = CDPBridge(auth)
            if cdp_bridge.connect():
                log.info("CDP reconnected successfully")
                break  # back to outer loop to run CDP
            else:
                log.warning("CDP connect failed, will retry in 15s...")
                cdp_bridge.close()
                cdp_bridge = CDPBridge(auth)  # fresh instance for next try

    # --- Cleanup ---
    api_poller.stop()
    if cdp_bridge:
        cdp_bridge.close()
    log.info("VLC Bridge stopped")


if __name__ == "__main__":
    main()
