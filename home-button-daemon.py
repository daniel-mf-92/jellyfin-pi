#!/usr/bin/env python3
"""
Pi Home Controller Daemon
- Maps Switch Pro Controller to keyboard via uinput (works on Wayland + X11)
- Y button returns to flex-launcher when an app is running
- Full D-pad, stick, and button mapping for Jellyfin TV mode
"""
import subprocess
import time
import evdev
from evdev import UInput, ecodes, AbsInfo
import os
import signal
import sys

CONTROLLER_NAME = "Pro Controller"
COOLDOWN = 2

# Switch Pro Controller button codes (evdev)
BTN_B = 304       # B (bottom) - South
BTN_A = 305       # A (right) - East
BTN_X = 307       # X (top) - North
BTN_Y = 308       # Y (left) - West
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

# D-pad axes
ABS_HAT0X = 16    # D-pad left/right
ABS_HAT0Y = 17    # D-pad up/down

# Stick axes
ABS_X = 0         # Left stick X
ABS_Y = 1         # Left stick Y
ABS_RX = 3        # Right stick X
ABS_RY = 4        # Right stick Y

# Keyboard mapping
BUTTON_MAP = {
    BTN_A: ecodes.KEY_ENTER,       # A = Select/Enter
    BTN_B: ecodes.KEY_ESC,         # B = Back/Escape
    BTN_X: ecodes.KEY_J,           # X = J (confirm in some UIs)
    # BTN_Y handled separately for home
    BTN_L: ecodes.KEY_PAGEUP,      # L = Page Up
    BTN_R: ecodes.KEY_PAGEDOWN,    # R = Page Down
    BTN_ZL: ecodes.KEY_REWIND,     # ZL = Rewind
    BTN_ZR: ecodes.KEY_FASTFORWARD,# ZR = Fast Forward
    BTN_PLUS: ecodes.KEY_SPACE,    # + = Play/Pause
    BTN_MINUS: ecodes.KEY_TAB,     # - = Menu/Tab
    BTN_LSTICK: ecodes.KEY_F,      # L stick = Fullscreen
    BTN_RSTICK: ecodes.KEY_M,      # R stick = Mute
}

DPAD_MAP_X = {-1: ecodes.KEY_LEFT, 1: ecodes.KEY_RIGHT}
DPAD_MAP_Y = {-1: ecodes.KEY_UP, 1: ecodes.KEY_DOWN}

# Stick dead zone and repeat
STICK_DEADZONE = 12000
STICK_REPEAT_DELAY = 0.4
STICK_REPEAT_RATE = 0.12

APP_PROCESSES = ["jellyfinmediaplayer", "moonlight", "Moonlight"]

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
    subprocess.run(["killall", "jellyfinmediaplayer"], capture_output=True)
    subprocess.run(["killall", "chromium"], capture_output=True)
    subprocess.run(["flatpak", "kill", "com.moonlight_stream.Moonlight"], capture_output=True)
    time.sleep(0.5)
    result = subprocess.run(["pgrep", "-x", "flex-launcher"], capture_output=True)
    if result.returncode != 0:
        env = os.environ.copy()
        env["WAYLAND_DISPLAY"] = "wayland-0"
        env["XDG_RUNTIME_DIR"] = "/run/user/1000"
        subprocess.Popen(
            ["flex-launcher", "-c", os.path.expanduser("~/.config/flex-launcher/config.ini")],
            env=env, stdout=open("/tmp/flex-launcher.log", "w"), stderr=subprocess.STDOUT
        )

def create_virtual_keyboard():
    cap = {ecodes.EV_KEY: list(set(
        list(BUTTON_MAP.values()) +
        list(DPAD_MAP_X.values()) +
        list(DPAD_MAP_Y.values()) +
        [ecodes.KEY_BACKSPACE]
    ))}
    return UInput(cap, name="pi-home-controller-kbd")

def press_key(ui, key):
    ui.write(ecodes.EV_KEY, key, 1)
    ui.syn()
    time.sleep(0.05)
    ui.write(ecodes.EV_KEY, key, 0)
    ui.syn()

def main():
    last_home = 0
    dpad_x_state = 0
    dpad_y_state = 0
    dpad_x_key = None
    dpad_y_key = None

    # Left stick state for keyboard arrow repeat
    lstick_x_dir = 0
    lstick_y_dir = 0
    lstick_x_time = 0
    lstick_y_time = 0
    lstick_x_repeating = False
    lstick_y_repeating = False

    ui = create_virtual_keyboard()
    print("Virtual keyboard created", flush=True)

    while True:
        dev = find_controller()
        if not dev:
            print("Waiting for Pro Controller...", flush=True)
            time.sleep(5)
            continue

        print(f"Listening on {dev.path}: {dev.name}", flush=True)
        try:
            for event in dev.read_loop():
                # Button events
                if event.type == ecodes.EV_KEY:
                    if event.code == BTN_Y:
                        if event.value == 1:  # Press
                            now = time.time()
                            if now - last_home > COOLDOWN and app_is_running():
                                print("Y pressed - going home", flush=True)
                                go_home()
                                last_home = now
                            # On launcher, Y does nothing
                    elif event.code in BUTTON_MAP:
                        if event.value == 1:  # Press
                            ui.write(ecodes.EV_KEY, BUTTON_MAP[event.code], 1)
                            ui.syn()
                        elif event.value == 0:  # Release
                            ui.write(ecodes.EV_KEY, BUTTON_MAP[event.code], 0)
                            ui.syn()

                # D-pad events (absolute axes)
                elif event.type == ecodes.EV_ABS:
                    if event.code == ABS_HAT0X:
                        # Release old key
                        if dpad_x_key:
                            ui.write(ecodes.EV_KEY, dpad_x_key, 0)
                            ui.syn()
                            dpad_x_key = None
                        # Press new key
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

        except OSError:
            print("Controller disconnected, retrying...", flush=True)
            time.sleep(3)

    ui.close()

if __name__ == "__main__":
    main()
