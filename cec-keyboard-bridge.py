#!/usr/bin/env python3
"""
CEC Keyboard Bridge
- Listens for HDMI-CEC remote control events via cec-client
- Maps CEC key presses to keyboard input via uinput
- Allows TV remote to navigate Jellyfin UI without a gamepad

Requirements:
  apt install cec-utils python3-evdev
  Must run as root (or user in 'input' group with uinput access)

Usage:
  sudo python3 cec-keyboard-bridge.py

Or as a systemd service:
  [Unit]
  Description=CEC Keyboard Bridge
  After=graphical.target

  [Service]
  ExecStart=/usr/bin/python3 /path/to/cec-keyboard-bridge.py
  Restart=always
  RestartSec=5

  [Install]
  WantedBy=graphical.target
"""

import subprocess
import sys
import time
from evdev import UInput, ecodes

# CEC user control codes (from CEC spec) -> keyboard keys
# These are the hex codes reported by cec-client in "key pressed:" lines
CEC_KEY_MAP = {
    # Navigation
    "up":           ecodes.KEY_UP,
    "down":         ecodes.KEY_DOWN,
    "left":         ecodes.KEY_LEFT,
    "right":        ecodes.KEY_RIGHT,
    "select":       ecodes.KEY_ENTER,
    "exit":         ecodes.KEY_ESC,
    "back":         ecodes.KEY_ESC,       # Some remotes send "back" instead of "exit"
    "root menu":    ecodes.KEY_HOME,

    # Playback
    "play":         ecodes.KEY_SPACE,
    "pause":        ecodes.KEY_SPACE,
    "stop":         ecodes.KEY_S,
    "rewind":       ecodes.KEY_REWIND,
    "Fast forward": ecodes.KEY_FASTFORWARD,
    "forward":      ecodes.KEY_FASTFORWARD,

    # Volume (passed through to Jellyfin, TV usually handles its own volume)
    "volume up":    ecodes.KEY_VOLUMEUP,
    "volume down":  ecodes.KEY_VOLUMEDOWN,
    "mute":         ecodes.KEY_MUTE,

    # Number keys (for search/input)
    "0":            ecodes.KEY_0,
    "1":            ecodes.KEY_1,
    "2":            ecodes.KEY_2,
    "3":            ecodes.KEY_3,
    "4":            ecodes.KEY_4,
    "5":            ecodes.KEY_5,
    "6":            ecodes.KEY_6,
    "7":            ecodes.KEY_7,
    "8":            ecodes.KEY_8,
    "9":            ecodes.KEY_9,

    # Color buttons (as F-keys for shortcuts)
    "F1 (blue)":    ecodes.KEY_F1,
    "F2 (red)":     ecodes.KEY_F2,
    "F3 (green)":   ecodes.KEY_F3,
    "F4 (yellow)":  ecodes.KEY_F4,

    # Channel (page navigation in Jellyfin)
    "channel up":   ecodes.KEY_PAGEUP,
    "channel down": ecodes.KEY_PAGEDOWN,
}


def create_virtual_keyboard():
    """Create a uinput virtual keyboard with all mapped keys."""
    all_keys = list(set(CEC_KEY_MAP.values()))
    cap = {ecodes.EV_KEY: all_keys}
    return UInput(cap, name="cec-keyboard-bridge")


def press_key(ui, key):
    """Send a key press and release."""
    ui.write(ecodes.EV_KEY, key, 1)
    ui.syn()
    time.sleep(0.05)
    ui.write(ecodes.EV_KEY, key, 0)
    ui.syn()


def run_cec_client():
    """Start cec-client and yield parsed key events."""
    proc = subprocess.Popen(
        ["cec-client", "-d", "8", "-t", "r"],  # -d 8 = minimal debug, -t r = recording device
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    return proc


def parse_cec_line(line):
    """
    Parse a cec-client output line for key press events.
    Returns the key name (lowercase) or None.

    cec-client outputs lines like:
      key pressed: up (1)
      key pressed: select (1)
      key released: select
    """
    line = line.strip().lower()
    if "key pressed:" in line:
        # Extract key name between "key pressed: " and " ("
        try:
            start = line.index("key pressed:") + len("key pressed:")
            rest = line[start:].strip()
            # Remove the duration suffix like " (1)"
            if " (" in rest:
                key_name = rest[:rest.index(" (")].strip()
            else:
                key_name = rest.strip()
            return key_name
        except (ValueError, IndexError):
            return None
    return None


def main():
    print("CEC Keyboard Bridge starting...", flush=True)

    ui = create_virtual_keyboard()
    print("Virtual keyboard 'cec-keyboard-bridge' created", flush=True)

    while True:
        try:
            print("Starting cec-client...", flush=True)
            proc = run_cec_client()

            for line in proc.stdout:
                key_name = parse_cec_line(line)
                if key_name and key_name in CEC_KEY_MAP:
                    key_code = CEC_KEY_MAP[key_name]
                    press_key(ui, key_code)
                    print(f"CEC: {key_name} -> {ecodes.KEY[key_code]}", flush=True)

            # cec-client exited, restart
            proc.wait()
            print("cec-client exited, restarting in 3s...", flush=True)
            time.sleep(3)

        except KeyboardInterrupt:
            print("Shutting down...", flush=True)
            if proc and proc.poll() is None:
                proc.terminate()
            break
        except Exception as e:
            print(f"Error: {e}, restarting in 5s...", flush=True)
            time.sleep(5)

    ui.close()


if __name__ == "__main__":
    main()
