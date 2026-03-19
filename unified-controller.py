#!/usr/bin/env python3
"""
Unified Controller Daemon for Pi Home
- Switch Pro Controller -> keyboard/mouse via uinput (Wayland + X11)
- B button: back within current app (Escape)
- Y button: kill foreground app, return to flex-launcher (launchpad)
- Right joystick: scroll (mouse wheel up/down/left/right)
- Left joystick: arrow key navigation with repeat
- D-pad: arrow keys
- Sound self-healing: ensures media always plays with sound (unmute + volume via CDP)

Replaces deprecated home-button-daemon.py.
"""

import subprocess
import time
import threading
import json
import urllib.request
import os
import sys
import signal

try:
    import evdev
    from evdev import UInput, ecodes, AbsInfo
except ImportError:
    subprocess.check_call([sys.executable, "-m", "pip", "install", "--break-system-packages", "-q", "evdev"])
    import evdev
    from evdev import UInput, ecodes, AbsInfo

try:
    import websocket
except ImportError:
    subprocess.check_call([sys.executable, "-m", "pip", "install", "--break-system-packages", "-q", "websocket-client"])
    import websocket

# --- Config ---
CONTROLLER_NAME = "Pro Controller"
COOLDOWN_HOME = 2  # seconds between Y presses
CDP_PORT = int(os.environ.get("JMP_CDP_PORT", "9222"))
CDP_HOST = os.environ.get("JMP_CDP_HOST", "localhost")
SOUND_CHECK_INTERVAL = 5  # seconds between sound health checks
DEFAULT_VOLUME = 80  # percent

# Switch Pro Controller button codes (evdev)
BTN_B = 304       # B (bottom/South)
BTN_A = 305       # A (right/East)
BTN_X = 307       # X (top/North)
BTN_Y = 308       # Y (left/West)
BTN_L = 309       # L bumper
BTN_R = 310       # R bumper
BTN_ZL = 311      # ZL trigger
BTN_ZR = 312      # ZR trigger
BTN_MINUS = 313   # - button
BTN_PLUS = 314    # + button
BTN_LSTICK = 315  # L stick click
BTN_RSTICK = 316  # R stick click
BTN_HOME = 317    # Home button
BTN_CAPTURE = 318 # Capture button

# Axes
ABS_HAT0X = 16    # D-pad left/right
ABS_HAT0Y = 17    # D-pad up/down
ABS_X = 0         # Left stick X
ABS_Y = 1         # Left stick Y
ABS_RX = 3        # Right stick X
ABS_RY = 4        # Right stick Y

# Button -> keyboard mapping
BUTTON_MAP = {
    BTN_A: ecodes.KEY_ENTER,        # A = Select/Enter
    BTN_B: ecodes.KEY_ESC,          # B = Back (within app)
    BTN_X: ecodes.KEY_J,            # X = J (confirm in some UIs)
    # BTN_Y handled separately -> go home / kill app
    BTN_L: ecodes.KEY_PAGEUP,       # L bumper = Page Up
    BTN_R: ecodes.KEY_PAGEDOWN,     # R bumper = Page Down
    BTN_ZL: ecodes.KEY_REWIND,      # ZL = Rewind
    BTN_ZR: ecodes.KEY_FASTFORWARD, # ZR = Fast Forward
    BTN_PLUS: ecodes.KEY_SPACE,     # + = Play/Pause
    BTN_MINUS: ecodes.KEY_TAB,      # - = Menu/Tab
    BTN_LSTICK: ecodes.KEY_F,       # L stick click = Fullscreen
    BTN_RSTICK: ecodes.KEY_M,       # R stick click = Mute toggle
}

DPAD_MAP_X = {-1: ecodes.KEY_LEFT, 1: ecodes.KEY_RIGHT}
DPAD_MAP_Y = {-1: ecodes.KEY_UP, 1: ecodes.KEY_DOWN}

# Stick tuning
STICK_DEADZONE = 12000
STICK_REPEAT_DELAY = 0.4   # initial delay before repeat
STICK_REPEAT_RATE = 0.12   # repeat interval

# Right stick scroll tuning
SCROLL_DEADZONE = 8000
SCROLL_INTERVAL = 0.08     # time between scroll events when stick held
SCROLL_SPEED = 3           # lines per scroll event

APP_PROCESSES = ["jellyfinmediaplayer", "moonlight-qt", "retroarch", "vlc"]


def find_controller():
    for path in evdev.list_devices():
        dev = evdev.InputDevice(path)
        if CONTROLLER_NAME in dev.name:
            return dev
    return None


def app_is_running():
    for app in APP_PROCESSES:
        result = subprocess.run(["pgrep", "-f", app], capture_output=True)
        if result.returncode == 0:
            return True
    return False


def go_home():
    """Kill foreground apps and return to flex-launcher."""
    for proc in APP_PROCESSES:
        subprocess.run(["killall", proc], capture_output=True)
    subprocess.run(["killall", "chromium"], capture_output=True)
    time.sleep(0.5)
    result = subprocess.run(["pgrep", "-x", "flex-launcher"], capture_output=True)
    if result.returncode != 0:
        env = os.environ.copy()
        env["WAYLAND_DISPLAY"] = "wayland-0"
        env["XDG_RUNTIME_DIR"] = "/run/user/1000"
        subprocess.Popen(
            ["flex-launcher", "-c",
             os.path.expanduser("~/.config/flex-launcher/config.ini")],
            env=env,
            stdout=open("/tmp/flex-launcher.log", "w"),
            stderr=subprocess.STDOUT
        )


def create_virtual_keyboard():
    """Create uinput device with keyboard keys + relative axes for scroll."""
    keys = list(set(
        list(BUTTON_MAP.values()) +
        list(DPAD_MAP_X.values()) +
        list(DPAD_MAP_Y.values()) +
        [ecodes.KEY_BACKSPACE]
    ))
    cap = {
        ecodes.EV_KEY: keys,
        ecodes.EV_REL: [ecodes.REL_WHEEL, ecodes.REL_HWHEEL],
    }
    return UInput(cap, name="pi-home-controller")


def press_key(ui, key):
    ui.write(ecodes.EV_KEY, key, 1)
    ui.syn()
    time.sleep(0.05)
    ui.write(ecodes.EV_KEY, key, 0)
    ui.syn()


def scroll(ui, vertical=0, horizontal=0):
    """Emit scroll wheel events. Positive = up/right, negative = down/left."""
    if vertical:
        ui.write(ecodes.EV_REL, ecodes.REL_WHEEL, vertical)
    if horizontal:
        ui.write(ecodes.EV_REL, ecodes.REL_HWHEEL, horizontal)
    if vertical or horizontal:
        ui.syn()


# --- Sound Self-Healing (CDP) ---

class SoundHealer(threading.Thread):
    """Periodically checks JMP video element and ensures sound is on."""

    def __init__(self):
        super().__init__(daemon=True)
        self.running = True

    def run(self):
        while self.running:
            try:
                self._check_and_fix_sound()
            except Exception:
                pass
            time.sleep(SOUND_CHECK_INTERVAL)

    def _check_and_fix_sound(self):
        # Check if JMP CDP is available
        try:
            resp = urllib.request.urlopen(
                f"http://{CDP_HOST}:{CDP_PORT}/json", timeout=2)
            pages = json.loads(resp.read())
        except Exception:
            return  # JMP not running or CDP not available

        targets = [p for p in pages if p.get("type") == "page"]
        if not targets:
            return

        ws_url = targets[0].get("webSocketDebuggerUrl")
        if not ws_url:
            return

        try:
            ws = websocket.create_connection(ws_url, suppress_origin=True, timeout=5)
        except Exception:
            return

        try:
            # Check video state and fix if muted or volume 0
            msg = json.dumps({
                "id": 1,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": f"""
                        (function() {{
                            var v = document.querySelector('video');
                            if (!v) return JSON.stringify({{has_video: false}});
                            var wasMuted = v.muted;
                            var wasVol = v.volume;
                            var fixed = false;
                            if (v.muted) {{
                                v.muted = false;
                                fixed = true;
                            }}
                            if (v.volume < 0.1) {{
                                v.volume = {DEFAULT_VOLUME / 100.0};
                                fixed = true;
                            }}
                            return JSON.stringify({{
                                has_video: true,
                                paused: v.paused,
                                wasMuted: wasMuted,
                                wasVol: Math.round(wasVol * 100),
                                nowMuted: v.muted,
                                nowVol: Math.round(v.volume * 100),
                                fixed: fixed
                            }});
                        }})()
                    """,
                    "returnByValue": True,
                    "awaitPromise": False
                }
            })
            ws.send(msg)
            deadline = time.time() + 5
            while time.time() < deadline:
                ws.settimeout(max(0.1, deadline - time.time()))
                try:
                    r = json.loads(ws.recv())
                except Exception:
                    break
                if r.get("id") == 1:
                    result = r.get("result", {}).get("result", {})
                    val = result.get("value")
                    if val and isinstance(val, str):
                        data = json.loads(val)
                        if data.get("fixed"):
                            print(f"[sound-healer] Fixed: muted={data['wasMuted']}->{data['nowMuted']}, "
                                  f"vol={data['wasVol']}%->{data['nowVol']}%", flush=True)
                    break
        finally:
            ws.close()

    def stop(self):
        self.running = False


# --- Right Stick Scroll Thread ---

class ScrollThread(threading.Thread):
    """Emits scroll events while right stick is held."""

    def __init__(self, ui):
        super().__init__(daemon=True)
        self.ui = ui
        self.rx = 0  # raw axis value
        self.ry = 0
        self.running = True

    def run(self):
        while self.running:
            vscroll = 0
            hscroll = 0

            if abs(self.ry) > SCROLL_DEADZONE:
                # Stick up (negative Y) = scroll up (positive wheel)
                vscroll = -SCROLL_SPEED if self.ry > 0 else SCROLL_SPEED

            if abs(self.rx) > SCROLL_DEADZONE:
                hscroll = SCROLL_SPEED if self.rx > 0 else -SCROLL_SPEED

            if vscroll or hscroll:
                scroll(self.ui, vertical=vscroll, horizontal=hscroll)
                time.sleep(SCROLL_INTERVAL)
            else:
                time.sleep(0.05)

    def stop(self):
        self.running = False


# --- Left Stick Arrow Key Repeat Thread ---

class StickArrowThread(threading.Thread):
    """Emits arrow key repeats when left stick is held."""

    def __init__(self, ui):
        super().__init__(daemon=True)
        self.ui = ui
        self.x_dir = 0   # -1, 0, 1
        self.y_dir = 0
        self.running = True
        self._x_start = 0
        self._y_start = 0
        self._x_repeating = False
        self._y_repeating = False

    def run(self):
        while self.running:
            now = time.time()

            # X axis
            if self.x_dir != 0:
                key = ecodes.KEY_LEFT if self.x_dir < 0 else ecodes.KEY_RIGHT
                if not self._x_repeating:
                    if now - self._x_start >= STICK_REPEAT_DELAY:
                        self._x_repeating = True
                        press_key(self.ui, key)
                else:
                    press_key(self.ui, key)
            # Y axis
            if self.y_dir != 0:
                key = ecodes.KEY_UP if self.y_dir < 0 else ecodes.KEY_DOWN
                if not self._y_repeating:
                    if now - self._y_start >= STICK_REPEAT_DELAY:
                        self._y_repeating = True
                        press_key(self.ui, key)
                else:
                    press_key(self.ui, key)

            time.sleep(STICK_REPEAT_RATE if (self._x_repeating or self._y_repeating) else 0.05)

    def set_x(self, direction):
        if direction != self.x_dir:
            self.x_dir = direction
            self._x_start = time.time()
            self._x_repeating = False
            if direction != 0:
                key = ecodes.KEY_LEFT if direction < 0 else ecodes.KEY_RIGHT
                press_key(self.ui, key)

    def set_y(self, direction):
        if direction != self.y_dir:
            self.y_dir = direction
            self._y_start = time.time()
            self._y_repeating = False
            if direction != 0:
                key = ecodes.KEY_UP if direction < 0 else ecodes.KEY_DOWN
                press_key(self.ui, key)

    def stop(self):
        self.running = False


def main():
    print("unified-controller: starting", flush=True)

    ui = create_virtual_keyboard()
    print("Virtual keyboard+scroll created", flush=True)

    # Start sound healer
    healer = SoundHealer()
    healer.start()
    print(f"Sound healer active (check every {SOUND_CHECK_INTERVAL}s, default vol {DEFAULT_VOLUME}%)", flush=True)

    # Start scroll thread
    scroller = ScrollThread(ui)
    scroller.start()

    # Start left stick arrow thread
    stick_arrows = StickArrowThread(ui)
    stick_arrows.start()

    last_home = 0
    dpad_x_key = None
    dpad_y_key = None

    def shutdown(sig, frame):
        print("\nunified-controller: shutting down", flush=True)
        healer.stop()
        scroller.stop()
        stick_arrows.stop()
        ui.close()
        sys.exit(0)

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)

    while True:
        dev = find_controller()
        if not dev:
            print("Waiting for Pro Controller...", flush=True)
            time.sleep(5)
            continue

        print(f"Connected: {dev.path} ({dev.name})", flush=True)
        try:
            for event in dev.read_loop():
                # --- Button events ---
                if event.type == ecodes.EV_KEY:
                    if event.code == BTN_Y:
                        if event.value == 1:  # press
                            now = time.time()
                            if now - last_home > COOLDOWN_HOME and app_is_running():
                                print("Y -> go home (launchpad)", flush=True)
                                go_home()
                                last_home = now
                    elif event.code in BUTTON_MAP:
                        if event.value == 1:  # press
                            ui.write(ecodes.EV_KEY, BUTTON_MAP[event.code], 1)
                            ui.syn()
                        elif event.value == 0:  # release
                            ui.write(ecodes.EV_KEY, BUTTON_MAP[event.code], 0)
                            ui.syn()

                # --- Axis events ---
                elif event.type == ecodes.EV_ABS:
                    # D-pad
                    if event.code == ABS_HAT0X:
                        if dpad_x_key:
                            ui.write(ecodes.EV_KEY, dpad_x_key, 0)
                            ui.syn()
                            dpad_x_key = None
                        if event.value in DPAD_MAP_X:
                            dpad_x_key = DPAD_MAP_X[event.value]
                            ui.write(ecodes.EV_KEY, dpad_x_key, 1)
                            ui.syn()

                    elif event.code == ABS_HAT0Y:
                        if dpad_y_key:
                            ui.write(ecodes.EV_KEY, dpad_y_key, 0)
                            ui.syn()
                            dpad_y_key = None
                        if event.value in DPAD_MAP_Y:
                            dpad_y_key = DPAD_MAP_Y[event.value]
                            ui.write(ecodes.EV_KEY, dpad_y_key, 1)
                            ui.syn()

                    # Left stick -> arrow keys (via thread for repeat)
                    elif event.code == ABS_X:
                        if event.value < -STICK_DEADZONE:
                            stick_arrows.set_x(-1)
                        elif event.value > STICK_DEADZONE:
                            stick_arrows.set_x(1)
                        else:
                            stick_arrows.set_x(0)

                    elif event.code == ABS_Y:
                        if event.value < -STICK_DEADZONE:
                            stick_arrows.set_y(-1)
                        elif event.value > STICK_DEADZONE:
                            stick_arrows.set_y(1)
                        else:
                            stick_arrows.set_y(0)

                    # Right stick -> scroll (via thread for continuous)
                    elif event.code == ABS_RX:
                        scroller.rx = event.value

                    elif event.code == ABS_RY:
                        scroller.ry = event.value

        except OSError:
            print("Controller disconnected, retrying...", flush=True)
            scroller.rx = 0
            scroller.ry = 0
            stick_arrows.set_x(0)
            stick_arrows.set_y(0)
            time.sleep(3)


if __name__ == "__main__":
    main()
