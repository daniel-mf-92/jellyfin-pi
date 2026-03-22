#!/usr/bin/env python3
"""
unified-controller.py -- Unified Switch Pro Controller mapper for Pi5-home-A
==============================================================================

ARCHITECTURE
------------
This script is the single controller daemon for the Jellyfin Pi media centre.
It replaces several earlier scripts: gamepad-kbd.py, switch-controller-mapper.py,
media-controller-daemon.sh, home-button-daemon.py, and go-home.sh.

MODES (auto-switch based on foreground app via /tmp/foreground-app):
  LAUNCHER   -- flex-launcher in front: d-pad->arrows, A->Enter, B->Esc
  NAVIGATION -- JMP/Kodi/Chromium/etc: d-pad->arrows, A->mouse click,
                B->Backspace (back), X->Backspace, sticks->mouse/scroll,
                bumpers->seek (accelerating), ZL->fullscreen, ZR->play/pause
  MEDIA      -- overlay mode: d-pad up/down->volume (accel), d-pad left/right
                ->subtitle, A->play/pause, bumpers->seek (accel), B->exit mode

INPUT:  evdev (Nintendo Switch Pro Controller via Bluetooth)
OUTPUT: Two UInput virtual devices (split to avoid libinput misclassification):
        - Switch-Pro-Keyboard  (EV_KEY: arrows, enter, esc, backspace, etc.)
        - Switch-Pro-Mouse     (EV_REL + BTN_LEFT/BTN_RIGHT)

PLAYBACK CONTROL STACK (tried in order):
  1. MPRIS D-Bus  -- works with JMP, Chromium, any MPRIS player
  2. mpv IPC      -- /tmp/mpv-socket (standalone mpv)
  3. Keyboard     -- KEY_PAGEUP/KEY_PAGEDOWN seek, KEY_SPACE play/pause

HOME BUTTON (Y):
  - Outside fullscreen: pkill media apps, return to flex-launcher
  - In fullscreen: hold 3 seconds to confirm (prevents accidental exit)

IDLE: 15-minute auto-disconnect via bluetoothctl (saves battery, wakes on press)

SYSTEMD: Run as unified-controller.service (user or system unit)
"""

import asyncio
import enum
import json as _json
import os
import signal
import socket
import re
import subprocess
import sys
import time
from collections import defaultdict

os.environ.setdefault("XDG_RUNTIME_DIR", "/run/user/1000")
os.environ.setdefault("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus")
os.environ.setdefault("WAYLAND_DISPLAY", "wayland-0")

try:
    import evdev
    from evdev import UInput, ecodes, ff, InputDevice
except ImportError:
    print("ERROR: python3-evdev not installed. Run: sudo apt install python3-evdev", flush=True)
    sys.exit(1)


# ─── Configuration ───────────────────────────────────────────────────────────

CONTROLLER_MAC = "98:41:5C:37:CB:EB"
CONTROLLER_NAMES = ["Pro Controller"]
IDLE_TIMEOUT = 900           # 15 minutes → auto-disconnect
FOREGROUND_POLL_S = 0.5      # how often to check foreground app
RECONNECT_POLL_S = 5.0       # how often to check for reconnected controller

# D-pad repeat with acceleration
DPAD_INITIAL_DELAY = 0.400   # 400ms before first repeat
DPAD_REPEAT_FAST = 0.150     # 150ms repeat rate (initial)
DPAD_ACCEL_THRESHOLD = 2.0   # after 2.0s held, accelerate
DPAD_REPEAT_ACCEL = 0.080    # 80ms repeat rate (accelerated)

# Analog stick thresholds
STICK_DIGITAL_DEAD = 8000    # left stick → digital direction threshold
MOUSE_DEAD = 12000            # left stick → mouse cursor dead zone
SCROLL_DEAD = 10000           # right stick → scroll dead zone
MOUSE_SPEED = 12             # pixels per poll tick at full deflection
SCROLL_SPEED = 3.0           # scroll units per poll tick at full deflection
MOUSE_POLL_S = 0.012         # ~83Hz mouse output

# Stick axis range (evdev reports -32768 to 32767 typically)
STICK_MAX = 32767

# Haptic feedback durations (milliseconds)
HAPTIC_NAV_MS = 20           # weak motor only, navigation tick
HAPTIC_SELECT_MS = 50        # both motors, select/confirm

# Apps that trigger each mode
LAUNCHER_APPS = {"flex-launcher", "flex_launcher"}
NAVIGATION_APPS = {"kodi", "org.videolan.vlc", "vlc", "mpv", "chromium", "chromium-browser", "jellyfin-media-player",
                   "com.github.iwalton3.jellyfin-media-player", "jmp", "firefox", "moonlight-qt", "moonlight", "com.moonlight_stream.moonlight"}

# Apps where mouse events must be suppressed (TV mode keyboard navigation)
JELLYFIN_APPS = {"com.github.iwalton3.jellyfin-media-player", "jellyfin-media-player", "jellyfin", "jmp"}

# Accelerating hold config
ACCEL_INITIAL_DELAY = 0.300     # delay before first repeat when held
ACCEL_REPEAT_INTERVAL = 0.250   # interval between accelerating actions (fast repeat)
SEEK_STEPS = [5, 10, 20, 40, 80, 120]  # seconds — doubles each repeat
VOLUME_STEP_INITIAL = 2         # volume change per tick (out of 100)
VOLUME_STEP_MAX = 10            # max volume change per tick
VOLUME_ACCEL_EVERY = 3          # increase step every N ticks

MPV_SOCKET = "/tmp/mpv-socket"


# ─── Mode Enum ───────────────────────────────────────────────────────────────

class Mode(enum.Enum):
    LAUNCHER = "LAUNCHER"
    NAVIGATION = "NAVIGATION"
    MEDIA = "MEDIA"


# ─── Logging ─────────────────────────────────────────────────────────────────

def log(msg):
    ts = time.strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{ts}] {msg}", flush=True)


# ─── mpv IPC helpers ─────────────────────────────────────────────────────────

def _mpv_command(cmd_list):
    """Send a command to mpv via IPC socket. Returns True on success."""
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(1)
        sock.connect(MPV_SOCKET)
        cmd = _json.dumps({"command": cmd_list}) + "\n"
        sock.sendall(cmd.encode())
        sock.close()
        return True
    except Exception:
        return False


def _mpv_get_property(prop):
    """Get a property from mpv via IPC. Returns value or None."""
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(1)
        sock.connect(MPV_SOCKET)
        cmd = _json.dumps({"command": ["get_property", prop]}) + "\n"
        sock.sendall(cmd.encode())
        data = sock.recv(4096).decode()
        sock.close()
        resp = _json.loads(data.strip().split("\n")[0])
        return resp.get("data")
    except Exception:
        return None


def mpv_is_active():
    """Check if mpv IPC socket is available."""
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(0.5)
        sock.connect(MPV_SOCKET)
        sock.close()
        return True
    except Exception:
        return False


def mpv_is_fullscreen():
    """Check if mpv is currently fullscreen."""
    val = _mpv_get_property("fullscreen")
    return val is True


# --- MPRIS D-Bus helpers (for JMP/Chromium-based players) ---

def _find_mpris_player():
    """Discover the first active MPRIS player on D-Bus (handles instance suffixes)."""
    try:
        env = {
            "XDG_RUNTIME_DIR": "/run/user/1000",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
        }
        r = subprocess.run(
            ["dbus-send", "--session", "--dest=org.freedesktop.DBus", "--print-reply",
             "/org/freedesktop/DBus", "org.freedesktop.DBus.ListNames"],
            capture_output=True, text=True, timeout=2, env=env
        )
        for line in r.stdout.splitlines():
            if "org.mpris.MediaPlayer2." in line:
                match = re.search(r'"(org\.mpris\.MediaPlayer2\.[^"]+)"', line)
                if match:
                    return match.group(1)
    except Exception:
        pass
    return None


def _mpris_command(method, *args):
    """Send an MPRIS command via dbus-send. Returns True on success."""
    dest = _find_mpris_player()
    if not dest:
        return False
    try:
        env = {
            "XDG_RUNTIME_DIR": "/run/user/1000",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
        }
        cmd = ["dbus-send", "--session", "--dest=" + dest, "--print-reply",
               "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player." + method]
        cmd.extend(args)
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=2, env=env)
        return "method return" in r.stdout
    except Exception:
        return False


def _mpris_seek(seconds):
    """Seek via MPRIS. seconds can be positive or negative."""
    usec = int(seconds * 1000000)
    return _mpris_command("Seek", "int64:" + str(usec))


def _mpris_play_pause():
    """Toggle play/pause via MPRIS."""
    return _mpris_command("PlayPause")



# ─── Accelerating Hold Engine ────────────────────────────────────────────────

class AccelHold:
    """Tracks held buttons and fires accelerating actions.

    Usage:
        accel.press("seek_fwd")   — call on button down
        accel.release("seek_fwd") — call on button up
        accel.tick()              — call every loop iteration (~10ms)

    Actions fire immediately on press, then repeat with acceleration.
    """

    def __init__(self, action_callback):
        """action_callback(action_name, tick_count) is called for each fire."""
        self.callback = action_callback
        # action_name → { "press_time": float, "next_fire": float, "ticks": int }
        self.held = {}

    def press(self, action):
        if action not in self.held:
            now = time.monotonic()
            self.held[action] = {
                "press_time": now,
                "next_fire": now + ACCEL_INITIAL_DELAY,
                "ticks": 0,
            }
            # Fire immediately on press
            self.callback(action, 0)

    def release(self, action):
        self.held.pop(action, None)

    def release_all(self):
        self.held.clear()

    def tick(self):
        now = time.monotonic()
        for action, state in list(self.held.items()):
            if now >= state["next_fire"]:
                state["ticks"] += 1
                self.callback(action, state["ticks"])
                state["next_fire"] = now + ACCEL_REPEAT_INTERVAL


# ─── Controller Discovery ───────────────────────────────────────────────────

def find_controller():
    """Find the Pro Controller evdev device (not IMU, not virtual)."""
    for path in evdev.list_devices():
        try:
            dev = InputDevice(path)
            if dev.name in CONTROLLER_NAMES:
                phys = (dev.phys or "").lower()
                if "imu" not in dev.name.lower() and "virtual" not in phys:
                    return dev
        except (OSError, PermissionError):
            continue
    return None


def is_bt_connected():
    """Check if controller is connected via Bluetooth."""
    try:
        r = subprocess.run(
            ["bluetoothctl", "info", CONTROLLER_MAC],
            capture_output=True, text=True, timeout=5
        )
        return "Connected: yes" in r.stdout
    except Exception:
        return False


def bt_disconnect():
    """Disconnect controller via bluetoothctl (saves battery, wakes on button press)."""
    log(f"Idle timeout ({IDLE_TIMEOUT}s), disconnecting {CONTROLLER_MAC}")
    try:
        subprocess.run(
            ["bluetoothctl", "disconnect", CONTROLLER_MAC],
            capture_output=True, text=True, timeout=10
        )
    except Exception as e:
        log(f"Disconnect error: {e}")


# ─── Foreground App Detection ────────────────────────────────────────────────

FOREGROUND_STATE_FILE = "/tmp/foreground-app"

def get_foreground_app():
    """Get the foreground app from state file (written by show-*.sh scripts)."""
    try:
        with open(FOREGROUND_STATE_FILE, "r") as f:
            app = f.read().strip().lower()
        if app:
            return [app]
    except FileNotFoundError:
        pass
    except Exception:
        pass
    # Fallback: assume launcher if no state file
    return ["flex-launcher"]


def detect_mode(visible_apps):
    """Determine mode from the list of visible app IDs."""
    app_set = set(visible_apps)
    has_launcher = bool(app_set & LAUNCHER_APPS)
    has_nav_app = bool(app_set & NAVIGATION_APPS)

    if has_launcher and not has_nav_app:
        return Mode.LAUNCHER
    elif has_nav_app:
        return Mode.NAVIGATION
    else:
        return Mode.NAVIGATION


# ─── Haptic Feedback ─────────────────────────────────────────────────────────

class HapticEngine:
    """Manage FF_RUMBLE effects on the controller."""

    def __init__(self, device):
        self.device = device
        self.nav_effect_id = -1
        self.select_effect_id = -1
        self._setup_effects()

    def _setup_effects(self):
        """Haptics fully disabled — do not upload any effects."""
        log("Haptic effects DISABLED (no vibration)")
        self.nav_effect_id = -1
        self.select_effect_id = -1

    def play_nav(self):
        if self.nav_effect_id >= 0:
            try:
                self.device.write(ecodes.EV_FF, self.nav_effect_id, 1)
            except Exception:
                pass

    def play_select(self):
        if self.select_effect_id >= 0:
            try:
                self.device.write(ecodes.EV_FF, self.select_effect_id, 1)
            except Exception:
                pass

    def cleanup(self):
        for eid in (self.nav_effect_id, self.select_effect_id):
            if eid >= 0:
                try:
                    self.device.erase_effect(eid)
                except Exception:
                    pass


# ─── Virtual Input (UInput) — SPLIT into Keyboard + Mouse ─────────────────

class VirtualInput:
    """Keyboard output via wtype (Wayland virtual keyboard protocol).
    Mouse output via UInput (for cursor movement and scroll).
    """

    MOUSE_KEYS = [
        ecodes.BTN_LEFT, ecodes.BTN_RIGHT,
    ]

    # Map evdev key codes to wtype key names
    _WTYPE_KEYS = {
        ecodes.KEY_UP: "Up", ecodes.KEY_DOWN: "Down",
        ecodes.KEY_LEFT: "Left", ecodes.KEY_RIGHT: "Right",
        ecodes.KEY_ENTER: "Return", ecodes.KEY_ESC: "Escape",
        ecodes.KEY_BACKSPACE: "BackSpace", ecodes.KEY_TAB: "Tab",
        ecodes.KEY_SPACE: "space", ecodes.KEY_F: "f",
        ecodes.KEY_PAGEUP: "Prior", ecodes.KEY_PAGEDOWN: "Next",
        ecodes.KEY_HOME: "Home",
        ecodes.KEY_PLAYPAUSE: "XF86AudioPlay",
        ecodes.KEY_VOLUMEUP: "XF86AudioRaiseVolume",
        ecodes.KEY_VOLUMEDOWN: "XF86AudioLowerVolume",
        ecodes.KEY_NEXTSONG: "XF86AudioNext",
        ecodes.KEY_PREVIOUSSONG: "XF86AudioPrev",
        ecodes.KEY_SUBTITLE: "v",
        ecodes.KEY_V: "v", ecodes.KEY_LEFTSHIFT: "Shift_L",
        ecodes.KEY_F5: "F5", ecodes.KEY_F11: "F11",
    }

    _WTYPE_ENV = {
        "WAYLAND_DISPLAY": "wayland-0",
        "XDG_RUNTIME_DIR": "/run/user/1000",
    }

    def __init__(self):
        mouse_caps = {
            ecodes.EV_KEY: list(self.MOUSE_KEYS),
            ecodes.EV_REL: [
                ecodes.REL_X, ecodes.REL_Y,
                ecodes.REL_WHEEL, ecodes.REL_HWHEEL,
            ],
        }
        self.mouse = UInput(mouse_caps, name="Switch-Pro-Mouse")
        log("Virtual input: wtype (keyboard) + UInput (mouse)")

    def _is_mouse_key(self, key):
        return key in self.MOUSE_KEYS

    def _wtype_key(self, key):
        """Send a key tap via wtype."""
        name = self._WTYPE_KEYS.get(key)
        if name:
            try:
                subprocess.Popen(
                    ["wtype", "-k", name],
                    env={**os.environ, **self._WTYPE_ENV},
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
            except Exception as e:
                log(f"wtype error: {e}")

    def key_press(self, key):
        if self._is_mouse_key(key):
            self.mouse.write(ecodes.EV_KEY, key, 1)
            self.mouse.syn()
        else:
            self._wtype_key(key)

    def key_release(self, key):
        if self._is_mouse_key(key):
            self.mouse.write(ecodes.EV_KEY, key, 0)
            self.mouse.syn()

    def key_tap(self, key):
        if self._is_mouse_key(key):
            self.mouse.write(ecodes.EV_KEY, key, 1)
            self.mouse.syn()
            self.mouse.write(ecodes.EV_KEY, key, 0)
            self.mouse.syn()
        else:
            self._wtype_key(key)

    def key_combo(self, *keys):
        mouse_keys = [k for k in keys if self._is_mouse_key(k)]
        kbd_keys = [k for k in keys if not self._is_mouse_key(k)]
        if kbd_keys:
            names = [self._WTYPE_KEYS.get(k) for k in kbd_keys if k in self._WTYPE_KEYS]
            if len(names) >= 2:
                cmd = ["wtype"]
                for n in names[:-1]:
                    cmd.extend(["-M", n])
                cmd.extend(["-k", names[-1]])
                for n in reversed(names[:-1]):
                    cmd.extend(["-m", n])
                try:
                    subprocess.Popen(cmd, env={**os.environ, **self._WTYPE_ENV},
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                except Exception:
                    pass
            elif names:
                self._wtype_key(kbd_keys[0])
        for k in mouse_keys:
            self.mouse.write(ecodes.EV_KEY, k, 1)
            self.mouse.syn()
            self.mouse.write(ecodes.EV_KEY, k, 0)
            self.mouse.syn()

    def mouse_move(self, dx, dy):
        if dx != 0 or dy != 0:
            self.mouse.write(ecodes.EV_REL, ecodes.REL_X, dx)
            self.mouse.write(ecodes.EV_REL, ecodes.REL_Y, dy)
            self.mouse.syn()

    def scroll(self, vertical=0, horizontal=0):
        needs_syn = False
        if vertical != 0:
            self.mouse.write(ecodes.EV_REL, ecodes.REL_WHEEL, vertical)
            needs_syn = True
        if horizontal != 0:
            self.mouse.write(ecodes.EV_REL, ecodes.REL_HWHEEL, horizontal)
            needs_syn = True
        if needs_syn:
            self.mouse.syn()

    def close(self):
        try:
            self.mouse.close()
        except Exception:
            pass


# ─── D-pad Repeat Engine ────────────────────────────────────────────────────

class DpadRepeat:
    """Handle d-pad key repeat with acceleration."""

    def __init__(self, vinput, haptic):
        self.vinput = vinput
        self.haptic = haptic
        self.held = {}

    def press(self, key):
        if key not in self.held:
            now = time.monotonic()
            self.vinput.key_tap(key)
            self.held[key] = (now, now + DPAD_INITIAL_DELAY)

    def release(self, key):
        if key in self.held:
            del self.held[key]

    def tick(self):
        now = time.monotonic()
        for key, (press_time, next_fire) in list(self.held.items()):
            if now >= next_fire:
                self.vinput.key_tap(key)
                held_duration = now - press_time
                if held_duration >= DPAD_ACCEL_THRESHOLD:
                    rate = DPAD_REPEAT_ACCEL
                else:
                    rate = DPAD_REPEAT_FAST
                self.held[key] = (press_time, now + rate)

    def release_all(self):
        self.held.clear()


# ─── Media Control Helpers ───────────────────────────────────────────────────

class MediaController:
    """Handle media-specific actions."""

    HOME_BIN = "/home/danielmatthews-ferrero/bin"

    @staticmethod
    def _run(script, *args):
        try:
            env = {
                **os.environ,
                "XDG_RUNTIME_DIR": "/run/user/1000",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
                "WAYLAND_DISPLAY": "wayland-0",
            }
            subprocess.Popen(
                [os.path.join(MediaController.HOME_BIN, script)] + list(args),
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except Exception as e:
            log(f"Media helper error ({script}): {e}")

    @classmethod
    def play_pause(cls):
        cls._run("media-playpause.sh")

    @classmethod
    def seek_forward(cls):
        cls._run("media-seek.sh", "forward")

    @classmethod
    def seek_backward(cls):
        cls._run("media-seek.sh", "backward")

    @classmethod
    def volume_up(cls):
        cls._run("media-volume.sh", "up")

    @classmethod
    def volume_down(cls):
        cls._run("media-volume.sh", "down")

    @classmethod
    def subtitle_next(cls):
        cls._run("media-subtitle.sh", "next")

    @classmethod
    def subtitle_prev(cls):
        cls._run("media-subtitle.sh", "prev")

    @classmethod
    def fullscreen_toggle(cls):
        cls._run("media-fullscreen.sh")

    @classmethod
    def quit_player(cls):
        cls._run("media-quit.sh")


# ─── Analog Stick Processing ────────────────────────────────────────────────

def stick_to_mouse(value, dead, speed, max_val):
    if abs(value) <= dead:
        return 0
    sign = 1 if value > 0 else -1
    magnitude = abs(value) - dead
    max_range = max_val - dead
    if max_range <= 0:
        return 0
    normalized = magnitude / max_range
    return int(sign * (normalized ** 1.5) * speed)


def stick_to_scroll(value, dead, speed, max_val):
    if abs(value) <= dead:
        return 0.0
    sign = 1 if value > 0 else -1
    magnitude = abs(value) - dead
    max_range = max_val - dead
    if max_range <= 0:
        return 0.0
    normalized = magnitude / max_range
    return sign * normalized * speed


def stick_to_digital(value, threshold):
    if value < -threshold:
        return -1
    elif value > threshold:
        return 1
    return 0


# ─── Main Controller Loop ───────────────────────────────────────────────────

class UnifiedController:
    """Main controller state machine."""

    def __init__(self):
        self.controller = None
        self.vinput = None
        self.haptic = None
        self.dpad = None
        self.accel = None
        self.mode = Mode.LAUNCHER
        self.grabbed = False
        self.running = True
        self.last_activity = time.monotonic()
        self.last_mode_check = 0
        self.media_mode_active = False
        self.is_fullscreen = False      # toggled by L2
        self.y_press_time = 0           # for hold-to-quit in fullscreen

        # Analog state
        self.lx = 0
        self.ly = 0
        self.rx = 0
        self.ry = 0
        self.hat_x = 0
        self.hat_y = 0
        self.lstick_digital_x = 0
        self.lstick_digital_y = 0

        # Scroll accumulator
        self.scroll_accum_x = 0.0
        self.scroll_accum_y = 0.0

        # Right stick arrow key tracking (for JMP/Jellyfin mode)
        self.rstick_digital_y = 0
        self._rstick_next_fire = 0.0
        self._jmp_foreground = False

        # Button state tracking (for edge detection)
        self.btn_state = defaultdict(bool)

    def _accel_action(self, action, tick):
        """Callback for AccelHold — fires accelerating seek/volume."""
        if action == "seek_fwd":
            idx = min(tick, len(SEEK_STEPS) - 1)
            secs = SEEK_STEPS[idx]
            self._mpv_seek(secs)
        elif action == "seek_bwd":
            idx = min(tick, len(SEEK_STEPS) - 1)
            secs = SEEK_STEPS[idx]
            self._mpv_seek(-secs)
        elif action == "vol_up":
            step = min(VOLUME_STEP_INITIAL + (tick // VOLUME_ACCEL_EVERY), VOLUME_STEP_MAX)
            if not self._mpv_volume(step):
                self.vinput.key_tap(ecodes.KEY_VOLUMEUP)
        elif action == "vol_down":
            step = min(VOLUME_STEP_INITIAL + (tick // VOLUME_ACCEL_EVERY), VOLUME_STEP_MAX)
            if not self._mpv_volume(-step):
                self.vinput.key_tap(ecodes.KEY_VOLUMEDOWN)

    def setup(self):
        """Find controller and create virtual devices."""
        self.controller = find_controller()
        if not self.controller:
            return False

        log(f"Found: {self.controller.name} at {self.controller.path}")

        # Kill all vibration at kernel level
        try:
            self.controller.write(ecodes.EV_FF, ecodes.FF_GAIN, 0)
            log("FF_GAIN set to 0 (vibration disabled at kernel level)")
        except Exception as e:
            log(f"FF_GAIN disable failed (non-fatal): {e}")

        self.vinput = VirtualInput()
        self.haptic = HapticEngine(self.controller)
        self.dpad = DpadRepeat(self.vinput, self.haptic)
        self.accel = AccelHold(self._accel_action)
        self.last_activity = time.monotonic()

        # Detect initial mode
        apps = get_foreground_app()
        self.mode = detect_mode(apps)
        self._apply_grab()
        log(f"Initial mode: {self.mode.value} (apps: {apps})")

        return True

    def _apply_grab(self):
        should_grab = True  # Always grab — we send keyboard events in all modes

        if should_grab and not self.grabbed:
            try:
                self.controller.grab()
                self.grabbed = True
                log("Controller grabbed")
            except Exception as e:
                log(f"Grab failed: {e}")
        elif not should_grab and self.grabbed:
            try:
                self.controller.ungrab()
                self.grabbed = False
                log("Controller ungrabbed (LAUNCHER mode)")
            except Exception as e:
                log(f"Ungrab failed: {e}")

    def _check_mode(self):
        now = time.monotonic()
        if now - self.last_mode_check < FOREGROUND_POLL_S:
            return

        self.last_mode_check = now
        apps = get_foreground_app()
        new_mode = detect_mode(apps)

        # Track if JMP is in foreground (suppresses mouse, uses arrow keys)
        self._jmp_foreground = bool(set(a.lower() for a in apps) & JELLYFIN_APPS)

        if self.media_mode_active and new_mode == Mode.NAVIGATION:
            new_mode = Mode.MEDIA

        if new_mode != self.mode:
            old = self.mode
            self.mode = new_mode
            self.dpad.release_all()
            self.accel.release_all()
            self._apply_grab()
            log(f"Mode: {old.value} → {new_mode.value}")
        else:
            self._launcher_switch_count = 0

    def _go_home(self):
        """Minimize media apps and return to flex-launcher (elegant transition)."""
        wl_env = {**os.environ, "WAYLAND_DISPLAY": "wayland-0",
                  "XDG_RUNTIME_DIR": "/run/user/1000"}
        # Minimize persistent apps — they stay running for instant re-entry
        for app_id in ["com.github.iwalton3.jellyfin-media-player",
                       "com.moonlight_stream.Moonlight"]:
            try:
                subprocess.run(["wlrctl", "toplevel", "minimize",
                               f"app_id:{app_id}"],
                               env=wl_env, capture_output=True, timeout=2)
            except Exception:
                pass
        # Kill only ephemeral players (VLC, mpv)
        for proc_name in ["vlc", "mpv"]:
            subprocess.run(["pkill", "-x", proc_name],
                           capture_output=True, timeout=2)
        self.is_fullscreen = False
        self.media_mode_active = False
        self.mode = Mode.LAUNCHER
        # Reset right stick tracking
        self.rstick_digital_y = 0
        self._rstick_next_fire = 0.0
        self._jmp_foreground = False
        # Focus flex-launcher (also restores from minimize on labwc)
        try:
            subprocess.run(["wlrctl", "toplevel", "focus",
                           "app_id:flex-launcher"],
                           env=wl_env, capture_output=True, timeout=2)
        except Exception:
            pass
        # Ensure flex-launcher is running
        try:
            r = subprocess.run(["pgrep", "-f", "flex-launcher"],
                               capture_output=True, timeout=2)
            if r.returncode != 0:
                subprocess.Popen(
                    ["flex-launcher", "-c",
                     "/home/danielmatthews-ferrero/.config/flex-launcher/config.ini"],
                    env=wl_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                time.sleep(1)
        except Exception:
            pass
        # Write state file
        try:
            with open("/tmp/foreground-app", "w") as f:
                f.write("flex-launcher")
        except Exception:
            pass
        log("GO HOME: minimized apps, focused launcher")

    def _mpv_seek(self, seconds):
        """Seek via MPRIS first (for JMP), then mpv IPC, then arrow keys."""
        if _mpris_seek(seconds):
            return
        if _mpv_command(["seek", seconds, "relative"]):
            return
        # Last resort: arrow keys
        if seconds < 0:
            self.vinput.key_tap(ecodes.KEY_PAGEUP)
        else:
            self.vinput.key_tap(ecodes.KEY_PAGEDOWN)

    def _mpv_volume(self, delta):
        """Adjust volume via mpv IPC. Falls back to keyboard volume keys."""
        if _mpv_command(["add", "volume", delta]):
            return True
        return False

    def _handle_button(self, code, value):
        """Process a button event. value: 1=press, 0=release."""
        pressed = (value == 1)
        was_pressed = self.btn_state[code]
        self.btn_state[code] = pressed
        edge = pressed and not was_pressed  # rising edge only

        if not edge and pressed:
            return  # ignore held (repeat handled by accel engine)
        if not edge and not pressed:
            # Release events
            self._handle_button_release(code)
            return

        # ── Home button: always active in all modes ──
        if code == ecodes.BTN_MODE:
            self.vinput.key_tap(ecodes.KEY_HOME)
            self.media_mode_active = False
            return

        # ── Y button: ALWAYS go home, regardless of mode ──
        if code == ecodes.BTN_WEST:
            if mpv_is_fullscreen():
                self.y_press_time = time.monotonic()
                log("Y pressed — mpv fullscreen, hold 3s to go home")
            else:
                self._go_home()
                self.y_press_time = 0
            return

        # ── LAUNCHER mode: A button -> Enter for flex-launcher selection ──
        if self.mode == Mode.LAUNCHER:
            if code == ecodes.BTN_EAST:  # A (Nintendo) -> Enter/Select
                self.vinput.key_tap(ecodes.KEY_ENTER)
            elif code == ecodes.BTN_SOUTH:  # B (Nintendo) -> Escape
                self.vinput.key_tap(ecodes.KEY_ESC)
            return

        # ── ZR press: play/pause (KEY_SPACE) ──
        if code == ecodes.BTN_TR2:  # ZR
            if not _mpris_play_pause():
                self.vinput.key_tap(ecodes.KEY_SPACE)
            return

        # ── MEDIA mode buttons ──
        if self.mode == Mode.MEDIA:
            if code == ecodes.BTN_TL:      # L bumper → seek backward (accel)
                self.accel.press("seek_bwd")
            elif code == ecodes.BTN_TR:     # R bumper → seek forward (accel)
                self.accel.press("seek_fwd")
            elif code == ecodes.BTN_TL2:    # ZL → toggle fullscreen (universal)
                self.is_fullscreen = not self.is_fullscreen
                _mpv_command(["cycle", "fullscreen"])
                self.vinput.key_tap(ecodes.KEY_F11)
                log(f"Fullscreen toggled: {self.is_fullscreen}")
            elif code == ecodes.BTN_EAST:   # A (Nintendo) -> play/pause
                MediaController.play_pause()

            elif code == ecodes.BTN_SOUTH:  # B (Nintendo) -> exit media mode
                self.media_mode_active = False
                self.mode = Mode.NAVIGATION
                self.dpad.release_all()
                self.accel.release_all()

                log("Media mode OFF (B back)")
            # Y handled globally above (go home)
            return

        # ── NAVIGATION mode buttons ──
        if self.mode == Mode.NAVIGATION:
            if code == ecodes.BTN_EAST:      # A (Nintendo) -> mouse click (select under cursor)
                self.vinput.key_tap(ecodes.BTN_LEFT)

            elif code == ecodes.BTN_SOUTH:   # B (Nintendo) -> Backspace (back in JMP)
                self.vinput.key_tap(ecodes.KEY_BACKSPACE)

            elif code == ecodes.BTN_NORTH:   # X → Backspace
                self.vinput.key_tap(ecodes.KEY_BACKSPACE)

            # Y handled globally above (go home)
            elif code == ecodes.BTN_TL:      # L bumper → seek back (accel)
                self.accel.press("seek_bwd")

            elif code == ecodes.BTN_TR:      # R bumper → seek forward (accel)
                self.accel.press("seek_fwd")

            elif code == ecodes.BTN_START:   # + → Space (play/pause fallback)
                self.vinput.key_tap(ecodes.KEY_SPACE)

            elif code == ecodes.BTN_SELECT:  # - → Tab
                self.vinput.key_tap(ecodes.KEY_TAB)

            elif code == ecodes.BTN_THUMBL:  # L stick click → Enter
                self.vinput.key_tap(ecodes.KEY_ENTER)

            elif code == ecodes.BTN_THUMBR:  # R stick click → right-click
                self.vinput.key_tap(ecodes.BTN_RIGHT)

            elif code == ecodes.BTN_TL2:     # ZL → toggle fullscreen (universal)
                self.is_fullscreen = not self.is_fullscreen
                _mpv_command(["cycle", "fullscreen"])  # mpv
                self.vinput.key_tap(ecodes.KEY_F11)    # Chromium/Firefox/most Linux apps
                log(f"Fullscreen toggled: {self.is_fullscreen}")

    def _handle_button_release(self, code):
        """Handle button release events for held-state buttons."""
        # Release accelerating hold actions
        if code == ecodes.BTN_TL:
            self.accel.release("seek_bwd")
        elif code == ecodes.BTN_TR:
            self.accel.release("seek_fwd")
        elif code == ecodes.BTN_WEST:
            # Y release: check if held 3s in fullscreen
            if self.y_press_time > 0:
                held = time.monotonic() - self.y_press_time
                if held >= 3.0:
                    self._go_home()
                else:
                    log(f"Y held {held:.1f}s in fullscreen (need 3s) — ignored")
                self.y_press_time = 0

    def _handle_hat(self, axis, value):
        """Process D-pad (hat) events."""
        if self.mode == Mode.LAUNCHER:
            # Send keyboard arrows for flex-launcher navigation
            if axis == ecodes.ABS_HAT0X:
                if value < 0:
                    self.vinput.key_tap(ecodes.KEY_LEFT)
                elif value > 0:
                    self.vinput.key_tap(ecodes.KEY_RIGHT)
            elif axis == ecodes.ABS_HAT0Y:
                if value < 0:
                    self.vinput.key_tap(ecodes.KEY_UP)
                elif value > 0:
                    self.vinput.key_tap(ecodes.KEY_DOWN)
            return

        if self.mode == Mode.MEDIA:
            # Media mode d-pad: volume up/down (accel), subtitle left/right
            if axis == ecodes.ABS_HAT0Y:
                if value == -1:
                    self.accel.press("vol_up")
    
                elif value == 1:
                    self.accel.press("vol_down")
    
                else:
                    self.accel.release("vol_up")
                    self.accel.release("vol_down")
            elif axis == ecodes.ABS_HAT0X:
                if value == 1:
                    MediaController.subtitle_next()
    
                elif value == -1:
                    MediaController.subtitle_prev()
    
            return

        # NAVIGATION mode: d-pad up/down = arrows (volume only when fullscreen)
        if axis == ecodes.ABS_HAT0Y:
            if self.is_fullscreen or mpv_is_fullscreen():
                # Fullscreen playback: volume control
                if value == -1:
                    self.accel.press("vol_up")
    
                elif value == 1:
                    self.accel.press("vol_down")
    
                else:
                    self.accel.release("vol_up")
                    self.accel.release("vol_down")
            else:
                # UI navigation: arrow keys
                if value == -1:
                    self.dpad.release(ecodes.KEY_DOWN)
                    self.dpad.press(ecodes.KEY_UP)
                elif value == 1:
                    self.dpad.release(ecodes.KEY_UP)
                    self.dpad.press(ecodes.KEY_DOWN)
                else:
                    self.dpad.release(ecodes.KEY_UP)
                    self.dpad.release(ecodes.KEY_DOWN)

        elif axis == ecodes.ABS_HAT0X:
            # Left/right always arrow keys (for navigation)
            if value < 0:
                self.dpad.release(ecodes.KEY_RIGHT)
                self.dpad.press(ecodes.KEY_LEFT)
                self.hat_x = -1
            elif value > 0:
                self.dpad.release(ecodes.KEY_LEFT)
                self.dpad.press(ecodes.KEY_RIGHT)
                self.hat_x = 1
            else:
                self.dpad.release(ecodes.KEY_LEFT)
                self.dpad.release(ecodes.KEY_RIGHT)
                self.hat_x = 0

    def _handle_stick(self, axis, value):
        """Process analog stick events."""
        if self.mode == Mode.LAUNCHER:
            return

        if axis == ecodes.ABS_X:
            self.lx = value
            new_digital = stick_to_digital(value, STICK_DIGITAL_DEAD)
            if new_digital != self.lstick_digital_x:
                if self.lstick_digital_x == -1:
                    self.dpad.release(ecodes.KEY_LEFT)
                elif self.lstick_digital_x == 1:
                    self.dpad.release(ecodes.KEY_RIGHT)
                if self.mode == Mode.NAVIGATION and new_digital != 0:
                    if new_digital == -1:
                        self.dpad.press(ecodes.KEY_LEFT)
                    else:
                        self.dpad.press(ecodes.KEY_RIGHT)
                self.lstick_digital_x = new_digital

        elif axis == ecodes.ABS_Y:
            self.ly = value
            new_digital = stick_to_digital(value, STICK_DIGITAL_DEAD)
            if new_digital != self.lstick_digital_y:
                if self.lstick_digital_y == -1:
                    self.dpad.release(ecodes.KEY_UP)
                elif self.lstick_digital_y == 1:
                    self.dpad.release(ecodes.KEY_DOWN)
                if self.mode == Mode.NAVIGATION and new_digital != 0:
                    if new_digital == -1:
                        self.dpad.press(ecodes.KEY_UP)
                    else:
                        self.dpad.press(ecodes.KEY_DOWN)
                self.lstick_digital_y = new_digital

        elif axis == ecodes.ABS_RX:
            self.rx = value
        elif axis == ecodes.ABS_RY:
            self.ry = value

    def _output_mouse_scroll(self):
        """Called at MOUSE_POLL_S interval to output mouse movement and scroll."""
        if self.mode == Mode.LAUNCHER:
            return

        # JMP/Jellyfin TV mode: suppress ALL mouse events to keep keyboard
        # navigation mode active. Right stick → arrow keys for page scrolling.
        if self._jmp_foreground:
            # Right stick Y → UP/DOWN arrow key taps with repeat
            ry_digital = stick_to_digital(self.ry, SCROLL_DEAD)
            now = time.monotonic()
            if ry_digital != self.rstick_digital_y:
                # Direction changed — fire immediately
                self.rstick_digital_y = ry_digital
                if ry_digital != 0:
                    key = ecodes.KEY_UP if ry_digital == -1 else ecodes.KEY_DOWN
                    self.vinput.key_tap(key)
                    self._rstick_next_fire = now + DPAD_INITIAL_DELAY
            elif ry_digital != 0 and now >= self._rstick_next_fire:
                # Held — repeat with acceleration
                key = ecodes.KEY_UP if ry_digital == -1 else ecodes.KEY_DOWN
                self.vinput.key_tap(key)
                held_duration = now - (self._rstick_next_fire - DPAD_INITIAL_DELAY) if self._rstick_next_fire > 0 else 0
                rate = DPAD_REPEAT_ACCEL if held_duration > DPAD_ACCEL_THRESHOLD else DPAD_REPEAT_FAST
                self._rstick_next_fire = now + rate
            # No mouse movement, no scroll — keeps JMP in keyboard nav mode
            return

        # Normal mode: left stick → mouse cursor
        dx = stick_to_mouse(self.lx, MOUSE_DEAD, MOUSE_SPEED, STICK_MAX)
        dy = stick_to_mouse(self.ly, MOUSE_DEAD, MOUSE_SPEED, STICK_MAX)
        if dx != 0 or dy != 0:
            self.vinput.mouse_move(dx, dy)

        # Normal mode: right stick → mouse scroll
        sy = stick_to_scroll(self.ry, SCROLL_DEAD, SCROLL_SPEED, STICK_MAX)
        sx = stick_to_scroll(self.rx, SCROLL_DEAD, SCROLL_SPEED, STICK_MAX)

        if sy != 0.0:
            self.scroll_accum_y += sy
        else:
            self.scroll_accum_y = 0.0

        if sx != 0.0:
            self.scroll_accum_x += sx
        else:
            self.scroll_accum_x = 0.0

        v_scroll = 0
        h_scroll = 0
        if abs(self.scroll_accum_y) >= 1.0:
            v_scroll = int(self.scroll_accum_y)
            self.scroll_accum_y -= v_scroll
            v_scroll = -v_scroll
        if abs(self.scroll_accum_x) >= 1.0:
            h_scroll = int(self.scroll_accum_x)
            self.scroll_accum_x -= h_scroll

        if v_scroll != 0 or h_scroll != 0:
            self.vinput.scroll(v_scroll, h_scroll)

    def cleanup(self):
        log("Cleaning up...")
        if self.dpad:
            self.dpad.release_all()
        if self.accel:
            self.accel.release_all()
        if self.haptic:
            self.haptic.cleanup()
        if self.grabbed and self.controller:
            try:
                self.controller.ungrab()
            except Exception:
                pass
            self.grabbed = False
        if self.vinput:
            self.vinput.close()
            self.vinput = None
        self.controller = None

    async def run(self):
        """Main async event loop."""
        last_mouse = time.monotonic()

        try:
            while self.running:
                idle = time.monotonic() - self.last_activity
                if idle >= IDLE_TIMEOUT:
                    bt_disconnect()
                    self.cleanup()
                    await asyncio.sleep(30)
                    return "idle_disconnect"

                self._check_mode()

                try:
                    event = await asyncio.wait_for(
                        self.controller.async_read_one(),
                        timeout=0.010
                    )

                    if event is not None:
                        self.last_activity = time.monotonic()

                        if event.type == ecodes.EV_KEY:
                            self._handle_button(event.code, event.value)
                        elif event.type == ecodes.EV_ABS:
                            if event.code in (ecodes.ABS_HAT0X, ecodes.ABS_HAT0Y):
                                self._handle_hat(event.code, event.value)
                            elif event.code in (ecodes.ABS_X, ecodes.ABS_Y,
                                                ecodes.ABS_RX, ecodes.ABS_RY):
                                self._handle_stick(event.code, event.value)

                except asyncio.TimeoutError:
                    pass

                # D-pad repeat tick
                if self.dpad:
                    self.dpad.tick()

                # Accelerating hold tick
                if self.accel:
                    self.accel.tick()

                # Mouse/scroll output at fixed rate
                now = time.monotonic()
                if now - last_mouse >= MOUSE_POLL_S:
                    self._output_mouse_scroll()
                    last_mouse = now

        except OSError as e:
            log(f"Controller disconnected: {e}")
            return "disconnected"
        except Exception as e:
            log(f"Error in main loop: {e}")
            import traceback
            traceback.print_exc()
            return "error"
        finally:
            self.cleanup()

        return "stopped"


# ─── Entry Point ─────────────────────────────────────────────────────────────

async def main_loop():
    """Outer loop: handle connect/disconnect/reconnect."""
    log("unified-controller.py starting")
    log(f"Idle timeout: {IDLE_TIMEOUT}s ({IDLE_TIMEOUT // 60} min)")
    log(f"D-pad repeat: {DPAD_INITIAL_DELAY*1000:.0f}ms delay, "
        f"{DPAD_REPEAT_FAST*1000:.0f}ms rate, "
        f"accel to {DPAD_REPEAT_ACCEL*1000:.0f}ms after {DPAD_ACCEL_THRESHOLD}s")
    log(f"AccelHold: seek steps {SEEK_STEPS}, vol step {VOLUME_STEP_INITIAL}-{VOLUME_STEP_MAX}")

    while True:
        if not is_bt_connected() and find_controller() is None:
            log("Controller not connected, waiting...")
            await asyncio.sleep(RECONNECT_POLL_S)
            continue

        uc = UnifiedController()
        if not uc.setup():
            log("Controller detected but evdev device not ready, retrying...")
            await asyncio.sleep(RECONNECT_POLL_S)
            continue

        result = await uc.run()
        log(f"Session ended: {result}")

        if result == "idle_disconnect":
            log("Waiting for user to wake controller...")
            await asyncio.sleep(10)
        else:
            await asyncio.sleep(RECONNECT_POLL_S)


def handle_signal(signum, frame):
    log(f"Received signal {signum}, exiting")
    sys.exit(0)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, handle_signal)
    signal.signal(signal.SIGINT, handle_signal)

    try:
        asyncio.run(main_loop())
    except KeyboardInterrupt:
        log("Interrupted, exiting")
    except SystemExit:
        pass
