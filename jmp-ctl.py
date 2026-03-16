#!/usr/bin/env python3
"""
jmp-ctl — AI-first CLI controller for Jellyfin Media Player via Chrome DevTools Protocol (CDP).

Connects to JMP's embedded QtWebEngine browser via WebSocket on port 9222,
allowing headless/programmatic control of the full Jellyfin web UI.

Requirements:
  - JMP running with: QTWEBENGINE_REMOTE_DEBUGGING=9222 jellyfinmediaplayer
  - pip install websocket-client (auto-installed if missing)

Usage:
  jmp-ctl.py status              # Show current page URL and player state
  jmp-ctl.py set-server URL      # Set Jellyfin server (e.g. https://jellyfin.example.com)
  jmp-ctl.py login USER PASS     # Login with username and password
  jmp-ctl.py play                # Resume playback
  jmp-ctl.py pause               # Pause playback
  jmp-ctl.py stop                # Stop playback
  jmp-ctl.py seek SECONDS        # Seek to position in seconds
  jmp-ctl.py volume PERCENT      # Set volume (0-100)
  jmp-ctl.py mute                # Toggle mute
  jmp-ctl.py fullscreen          # Toggle fullscreen
  jmp-ctl.py navigate PATH       # Navigate to Jellyfin web path (e.g. /web/#/home)
  jmp-ctl.py search QUERY        # Search library
  jmp-ctl.py eval JS             # Execute arbitrary JavaScript and print result
  jmp-ctl.py screenshot FILE     # Capture page screenshot as PNG
  jmp-ctl.py dom                 # Dump current page DOM (first 2000 chars)
  jmp-ctl.py items               # List visible media items on current page
"""

import json, urllib.request, sys, time, base64, os

try:
    import websocket
except ImportError:
    import subprocess
    subprocess.check_call([sys.executable, "-m", "pip", "install", "--break-system-packages", "-q", "websocket-client"])
    import websocket

CDP_PORT = int(os.environ.get("JMP_CDP_PORT", "9222"))
CDP_HOST = os.environ.get("JMP_CDP_HOST", "localhost")

class JMPController:
    def __init__(self, host=CDP_HOST, port=CDP_PORT):
        self.host = host
        self.port = port
        self._id = 0
        self.ws = None

    def connect(self):
        url = f"http://{self.host}:{self.port}/json"
        try:
            resp = urllib.request.urlopen(url, timeout=5)
        except Exception as e:
            print(f"Error: Cannot reach CDP at {url}")
            print(f"Is JMP running with QTWEBENGINE_REMOTE_DEBUGGING={self.port}?")
            sys.exit(1)
        pages = json.loads(resp.read())
        targets = [p for p in pages if p.get("type") == "page"]
        if not targets:
            print("Error: No page targets found. Is JMP fully loaded?")
            sys.exit(1)
        ws_url = targets[0]["webSocketDebuggerUrl"]
        self.ws = websocket.create_connection(ws_url, suppress_origin=True, timeout=10)
        return self

    def cdp(self, method, params=None, timeout=15):
        self._id += 1
        msg = {"id": self._id, "method": method}
        if params:
            msg["params"] = params
        self.ws.send(json.dumps(msg))
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.ws.settimeout(max(0.1, deadline - time.time()))
            try:
                r = json.loads(self.ws.recv())
            except websocket.WebSocketTimeoutException:
                break
            if r.get("id") == self._id:
                return r
        return {"error": "timeout"}

    def js(self, expression):
        r = self.cdp("Runtime.evaluate", {
            "expression": expression,
            "returnByValue": True,
            "awaitPromise": True
        })
        result = r.get("result", {}).get("result", {})
        if result.get("type") == "undefined":
            return None
        return result.get("value", result.get("description", str(result)))

    def close(self):
        if self.ws:
            self.ws.close()

def cmd_status(ctl, args):
    url = ctl.js("window.location.href")
    title = ctl.js("document.title")
    state = ctl.js("""
        (function() {
            var v = document.querySelector('video');
            if (!v) return 'no-video';
            return JSON.stringify({
                paused: v.paused,
                currentTime: Math.round(v.currentTime),
                duration: Math.round(v.duration || 0),
                volume: Math.round(v.volume * 100),
                muted: v.muted,
                src: v.src ? v.src.substring(0, 80) : 'none'
            });
        })()
    """)
    print(f"URL: {url}")
    print(f"Title: {title}")
    if state and state != 'no-video':
        try:
            s = json.loads(state)
            status = "paused" if s["paused"] else "playing"
            pos = f"{s['currentTime']//60}:{s['currentTime']%60:02d}"
            dur = f"{s['duration']//60}:{s['duration']%60:02d}"
            print(f"Player: {status} [{pos}/{dur}] vol={s['volume']}% muted={s['muted']}")
        except:
            print(f"Player: {state}")
    else:
        print("Player: no video element")

def cmd_set_server(ctl, args):
    if not args:
        print("Usage: jmp-ctl.py set-server URL")
        sys.exit(1)
    server_url = args[0]
    # Clear storage and navigate to server selection
    ctl.js("localStorage.clear(); sessionStorage.clear()")
    ctl.cdp("Page.navigate", {"url": "about:blank"})
    time.sleep(1)
    ctl.cdp("Page.navigate", {"url": "file:///usr/local/share/jellyfinmediaplayer/web-client/extension/find-webclient.html"})
    time.sleep(4)
    # Fill server URL
    result = ctl.js(f'''
        var i = document.querySelector("input");
        if (i) {{
            i.value = "{server_url}";
            i.dispatchEvent(new Event("input", {{bubbles: true}}));
            "filled"
        }} else {{
            "no input found: " + document.body.innerHTML.substring(0, 200)
        }}
    ''')
    print(f"Fill: {result}")
    time.sleep(1)
    # Click connect
    result = ctl.js('''
        var b = document.querySelector("button, .raised");
        b ? (b.click(), "clicked " + b.textContent.trim()) : "no button"
    ''')
    print(f"Connect: {result}")
    time.sleep(3)
    print(f"Current URL: {ctl.js('window.location.href')}")

def cmd_login(ctl, args):
    if len(args) < 2:
        print("Usage: jmp-ctl.py login USERNAME PASSWORD")
        sys.exit(1)
    username, password = args[0], args[1]
    # Type username
    result = ctl.js(f'''
        (function() {{
            var inputs = document.querySelectorAll("input");
            var user = null, pass = null;
            for (var i of inputs) {{
                var t = (i.type || "").toLowerCase();
                var n = (i.name || i.id || i.autocomplete || "").toLowerCase();
                if (t === "password" || n.includes("password")) pass = i;
                else if (t === "text" || t === "email" || n.includes("user") || n.includes("name")) user = i;
            }}
            if (!user) return "no username field found (" + inputs.length + " inputs)";
            var nativeSet = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
            nativeSet.call(user, "{username}");
            user.dispatchEvent(new Event("input", {{bubbles: true}}));
            if (pass) {{
                nativeSet.call(pass, "{password}");
                pass.dispatchEvent(new Event("input", {{bubbles: true}}));
            }}
            return "filled user" + (pass ? "+pass" : " (no pass field yet)");
        }})()
    ''')
    print(f"Login fill: {result}")
    time.sleep(1)
    # Click sign in button
    result = ctl.js('''
        (function() {
            var btns = document.querySelectorAll("button, .raised, [type=submit]");
            for (var b of btns) {
                var t = b.textContent.toLowerCase();
                if (t.includes("sign in") || t.includes("login") || t.includes("submit")) {
                    b.click();
                    return "clicked: " + b.textContent.trim();
                }
            }
            return "no sign-in button found (" + btns.length + " buttons)";
        })()
    ''')
    print(f"Submit: {result}")
    time.sleep(3)
    print(f"Current URL: {ctl.js('window.location.href')}")

def cmd_play(ctl, args):
    r = ctl.js("var v=document.querySelector('video'); v ? (v.play(), 'playing') : 'no video'")
    print(r)

def cmd_pause(ctl, args):
    r = ctl.js("var v=document.querySelector('video'); v ? (v.pause(), 'paused') : 'no video'")
    print(r)

def cmd_stop(ctl, args):
    r = ctl.js("typeof window.Emby !== 'undefined' ? (window.Emby.PlaybackManager.stop(), 'stopped') : 'no Emby API'")
    print(r)

def cmd_seek(ctl, args):
    if not args:
        print("Usage: jmp-ctl.py seek SECONDS"); sys.exit(1)
    r = ctl.js(f"var v=document.querySelector('video'); v ? (v.currentTime={args[0]}, 'seeked to {args[0]}s') : 'no video'")
    print(r)

def cmd_volume(ctl, args):
    if not args:
        print("Usage: jmp-ctl.py volume PERCENT"); sys.exit(1)
    vol = int(args[0]) / 100.0
    r = ctl.js(f"var v=document.querySelector('video'); v ? (v.volume={vol}, 'volume={args[0]}%') : 'no video'")
    print(r)

def cmd_mute(ctl, args):
    r = ctl.js("var v=document.querySelector('video'); v ? (v.muted=!v.muted, 'muted='+v.muted) : 'no video'")
    print(r)

def cmd_fullscreen(ctl, args):
    r = ctl.js("document.fullscreenElement ? (document.exitFullscreen(), 'exited') : (document.documentElement.requestFullscreen(), 'entered')")
    print(r)

def cmd_navigate(ctl, args):
    if not args:
        print("Usage: jmp-ctl.py navigate PATH"); sys.exit(1)
    path = args[0]
    if not path.startswith("http"):
        base = ctl.js("window.location.origin")
        path = base + path
    ctl.cdp("Page.navigate", {"url": path})
    time.sleep(2)
    print(f"Navigated to: {ctl.js('window.location.href')}")

def cmd_search(ctl, args):
    if not args:
        print("Usage: jmp-ctl.py search QUERY"); sys.exit(1)
    query = " ".join(args)
    base = ctl.js("window.location.origin")
    from urllib.parse import quote
    ctl.cdp("Page.navigate", {"url": f"{base}/web/#/search.html?query={quote(query)}"})
    time.sleep(3)
    items = ctl.js('''
        Array.from(document.querySelectorAll(".card, .listItem, [data-type]")).slice(0, 20).map(function(e) {
            return (e.querySelector(".cardText, .listItemBody") || e).textContent.trim().substring(0, 80);
        }).filter(Boolean).join("\n")
    ''')
    print(f"Search results for '{query}':\n{items or 'No results'}")

def cmd_eval(ctl, args):
    if not args:
        print("Usage: jmp-ctl.py eval 'JavaScript expression'"); sys.exit(1)
    result = ctl.js(" ".join(args))
    print(result)

def cmd_screenshot(ctl, args):
    outfile = args[0] if args else "jmp-screenshot.png"
    r = ctl.cdp("Page.captureScreenshot", {"format": "png"})
    data = r.get("result", {}).get("data")
    if data:
        with open(outfile, "wb") as f:
            f.write(base64.b64decode(data))
        print(f"Screenshot saved: {outfile}")
    else:
        print(f"Error: {r}")

def cmd_dom(ctl, args):
    html = ctl.js("document.documentElement.outerHTML.substring(0, 2000)")
    print(html)

def cmd_items(ctl, args):
    items = ctl.js('''
        (function() {
            var cards = document.querySelectorAll(".card, .listItem, [data-type]");
            var results = [];
            cards.forEach(function(c, i) {
                if (i >= 30) return;
                var name = (c.querySelector(".cardText, .cardPadder + div, .listItemBody") || c).textContent.trim();
                var type = c.getAttribute("data-type") || "item";
                var id = c.getAttribute("data-id") || "";
                if (name) results.push(type + ": " + name.substring(0, 60) + (id ? " [" + id + "]" : ""));
            });
            return results.join("\n") || "No items visible";
        })()
    ''')
    print(items)

COMMANDS = {
    "status": cmd_status,
    "set-server": cmd_set_server,
    "login": cmd_login,
    "play": cmd_play,
    "pause": cmd_pause,
    "stop": cmd_stop,
    "seek": cmd_seek,
    "volume": cmd_volume,
    "mute": cmd_mute,
    "fullscreen": cmd_fullscreen,
    "navigate": cmd_navigate,
    "search": cmd_search,
    "eval": cmd_eval,
    "screenshot": cmd_screenshot,
    "dom": cmd_dom,
    "items": cmd_items,
}

def main():
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help", "help"):
        print(__doc__.strip())
        sys.exit(0)

    cmd = sys.argv[1]
    args = sys.argv[2:]

    if cmd not in COMMANDS:
        print(f"Unknown command: {cmd}")
        print(f"Available: {', '.join(sorted(COMMANDS.keys()))}")
        sys.exit(1)

    ctl = JMPController()
    try:
        ctl.connect()
        COMMANDS[cmd](ctl, args)
    finally:
        ctl.close()

if __name__ == "__main__":
    main()
