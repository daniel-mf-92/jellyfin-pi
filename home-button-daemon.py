#!/usr/bin/env python3
"""
Home Button Daemon - Switch Pro Controller Y button returns to flex-launcher.
Only triggers when an app (JMP/Moonlight) is running on top of flex-launcher.
On the launchpad itself, Y does nothing.
"""
import subprocess
import time
import evdev
import os

BTN_WEST_Y = 308
CONTROLLER_NAME = "Pro Controller"
COOLDOWN = 2

APP_PROCESSES = ["jellyfinmediaplayer", "moonlight", "Moonlight"]

def find_controller():
    for path in evdev.list_devices():
        dev = evdev.InputDevice(path)
        if CONTROLLER_NAME in dev.name:
            return dev
    return None

def app_is_running():
    """Check if any app is running on top of flex-launcher."""
    for app in APP_PROCESSES:
        result = subprocess.run(["pgrep", "-f", app], capture_output=True)
        if result.returncode == 0:
            return True
    return False

def go_home():
    """Kill foreground apps, ensure flex-launcher is running."""
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

def main():
    last_trigger = 0
    while True:
        dev = find_controller()
        if not dev:
            print("Waiting for Pro Controller...", flush=True)
            time.sleep(5)
            continue
        print(f"Listening on {dev.path}: {dev.name}", flush=True)
        try:
            for event in dev.read_loop():
                if event.type == evdev.ecodes.EV_KEY and event.code == BTN_WEST_Y and event.value == 1:
                    now = time.time()
                    if now - last_trigger > COOLDOWN and app_is_running():
                        print("Y pressed - going home", flush=True)
                        go_home()
                        last_trigger = now
        except OSError:
            print("Controller disconnected, retrying...", flush=True)
            time.sleep(3)

if __name__ == "__main__":
    main()
