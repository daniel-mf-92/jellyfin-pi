# jellyfin-pi

**A complete TV media center for Raspberry Pi 5 -- Jellyfin Media Player built from source for ARM64, with gamepad navigation, hardware video decoding, and a TV-friendly launcher.**

Jellyfin Media Player (JMP) does not ship ARM64 Linux builds. This project builds JMP from source in about 60 seconds on a Pi 5 using Debian Trixie's pre-built QtWebEngine packages, then wraps it in a full living-room setup: a TV home screen (flex-launcher), Nintendo Switch Pro Controller support with mode-aware input mapping, Pi 5 V4L2 hardware decoding, HDMI audio with lip-sync compensation, Moonlight game streaming, and self-healing systemd services. Every config file is included -- clone, install, sit on the couch.

---

## Architecture

```
                                    +-------------------+
                                    |  Jellyfin Server  |
                                    |  (network/local)  |
                                    +--------+----------+
                                             |
                                             | HTTP/WebSocket
                                             |
+---------------------+            +--------v----------+
| Switch Pro          |            | Jellyfin Media    |
| Controller (BT)     |            | Player (JMP)      |
|                     |            |                   |
| evdev raw events    |            |  QtWebEngine UI   |
+----------+----------+            |  mpv video backend|
           |                       |  v4l2m2m-copy HW  |
           |                       +--------+----------+
           |                                |
+----------v----------+                     |
| unified-controller  |            +--------v----------+
| .py (systemd)       |            | labwc (Wayland)   |
|                     |            | compositor        |
|  Mode detection:    |            +--------+----------+
|  LAUNCHER / NAV /   |                     |
|  MEDIA              |            +--------v----------+
+----------+----------+            | HDMI output       |
           |                       | 1080p + audio     |
           | UInput               +-------------------+
           |
+----------v----------+
| Virtual Keyboard    |   switch-pro-virtual.json
| Virtual Mouse       |   (JMP input map)
+---------------------+

Modes auto-switch based on foreground app (/tmp/foreground-app):
  LAUNCHER    -- flex-launcher visible: d-pad = arrows, A = enter
  NAVIGATION  -- JMP/browser/Moonlight: sticks = mouse/scroll, A = click
  MEDIA       -- fullscreen playback: d-pad = volume, bumpers = seek
```

---

## What Makes This Different

- **60-second ARM64 build.** Debian Trixie ships QtWebEngine5 as a system package. No cross-compiling, no hours-long Chromium builds. `cmake + ninja` against system libs and you have a working JMP binary.
- **One controller, three modes.** The unified-controller daemon reads evdev directly, creates virtual keyboard and mouse devices via UInput, and auto-switches between launcher navigation, UI browsing, and media playback modes. No antimicrox, no SDL remapping layers.
- **Hardware decode that works.** Pi 5's V4L2 stateless decoder needs `v4l2m2m-copy` (not `v4l2m2m`). This copies decoded frames back to system memory so the GPU compositor can display them. Both mpv.conf and JMP's jellyfinmediaplayer.conf are pre-configured for this.
- **GL compositing disabled.** `QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu-compositing"` prevents the black-screen / render-corruption bug on Pi's VideoCore GPU. The web UI renders in software; video playback still uses hardware decode.
- **HDMI audio with lip-sync fix.** Pi HDMI audio has inherent latency. `audio-delay=-0.3` in mpv.conf and jmp-mpv.conf compensates, with `video-sync=audio` to keep frames locked.
- **Self-healing.** HDMI audio jack detection breaks on hot-plug. `fix-hdmi-audio.sh` forces DRM re-detect, restarts WirePlumber if needed, and sets the HDMI sink as default at full volume.

---

## Quick Start

### 1. Clone

```bash
git clone https://github.com/daniel-mf-92/jellyfin-pi.git
cd jellyfin-pi
```

### 2. Build JMP from source

```bash
sudo ./build-arm64.sh
```

This installs build dependencies, clones JMP v1.11.1, compiles with Ninja, and installs to `/usr/local/bin/jellyfinmediaplayer`. Takes about 60 seconds on Pi 5.

To build a different version:

```bash
sudo ./build-arm64.sh 1.12.0
```

### 3. Install configs and services

```bash
./install.sh
```

This copies all configuration files to the correct locations, creates the systemd service for the controller daemon, and enables it. See the install script section below for details.

To build and install in one step:

```bash
./install.sh --build
```

### 4. Start the compositor

Log in on a TTY (or configure autologin) and start labwc:

```bash
labwc -s ~/.config/labwc/autostart
```

The autostart script launches flex-launcher, pre-starts JMP and Moonlight (minimized), hides the cursor, and configures HDMI output.

---

## File Reference

| File | Description | Install location |
|------|-------------|-----------------|
| `build-arm64.sh` | Builds JMP from source for ARM64. Installs deps, clones, compiles, installs. | Run from repo (sudo) |
| `install.sh` | Copies all configs to the right places, creates systemd service. | Run from repo |
| `unified-controller.py` | Switch Pro Controller daemon. Reads evdev, outputs virtual keyboard/mouse. Three auto-switching modes. | `~/bin/unified-controller.py` |
| `switch-pro-virtual.json` | JMP input map for the virtual keyboard device. Maps keys to JMP actions (seek, play/pause, back, fullscreen). | `~/.local/share/jellyfinmediaplayer/inputmaps/` |
| `jellyfinmediaplayer.conf` | JMP settings: TV layout, v4l2m2m-copy hardware decode, Jellyfin server URL, audio config. | `~/.local/share/jellyfinmediaplayer/` |
| `mpv.conf` | mpv config: v4l2m2m-copy decode, HDMI audio via ALSA, lip-sync compensation, 2GB cache. | `~/.config/mpv/mpv.conf` |
| `jmp-mpv.conf` | JMP-specific mpv overrides (audio delay and video sync only). | `~/.local/share/jellyfinmediaplayer/mpv.conf` |
| `labwc-autostart` | Wayland compositor autostart: display scaling, cursor hiding, flex-launcher, JMP/Moonlight pre-launch. | `~/.config/labwc/autostart` |
| `flex-launcher-config.ini` | TV home screen config: two entries (Jellyfin, Games), gamepad enabled, dark theme. | `~/.config/flex-launcher/config.ini` |
| `go-home.sh` | Kills media apps and returns to flex-launcher. Called by unified-controller on Y press. | `~/bin/go-home.sh` |
| `launch-jmp.sh` | Launches JMP with CDP enabled, auto-navigates to server, triggers auto-login if needed. | `~/bin/launch-jmp.sh` |
| `show-jellyfin.sh` | Brings pre-launched JMP to foreground (or starts it if not running). Used by flex-launcher. | `~/bin/show-jellyfin.sh` |
| `fix-hdmi-audio.sh` | Forces HDMI jack detection, restarts WirePlumber if needed, sets HDMI as default sink. | `~/bin/fix-hdmi-audio.sh` |
| `jmp-ctl.py` | CLI tool to control JMP via Chrome DevTools Protocol. Set server, login, play, seek, screenshot, etc. | `~/bin/jmp-ctl.py` |
| `jmp-ctl-launch.sh` | Launches JMP with CDP and optionally sets server/login in one command. | `~/bin/jmp-ctl-launch.sh` |
| `cec-keyboard-bridge.py` | Maps HDMI-CEC remote control buttons to keyboard via uinput. Alternative to gamepad. | `~/bin/cec-keyboard-bridge.py` |
| `home-button-daemon.py` | **Deprecated.** Original controller daemon, replaced by unified-controller.py. | Not installed |
| `switch-pro-tv.gamecontroller.amgp` | antimicrox profile. **Deprecated** -- unified-controller.py replaces this. | Not installed |

---

## Controller Button Mapping

The unified-controller daemon uses **physical button positions** (Nintendo convention), not Xbox label mapping.

### Navigation Mode (JMP, Moonlight, browsers)

| Button | Physical Position | Action |
|--------|-------------------|--------|
| A | Right (East) | Left mouse click (select) |
| B | Bottom (South) | Backspace (back) |
| X | Top (North) | Backspace (back) |
| Y | Left (West) | Kill app, go to launcher |
| D-pad | -- | Arrow keys (UI navigation) |
| D-pad (fullscreen) | -- | Volume up/down |
| Left stick | -- | Mouse cursor |
| Right stick | -- | Scroll (vertical/horizontal) |
| L bumper | Left shoulder | Seek backward (accelerating) |
| R bumper | Right shoulder | Seek forward (accelerating) |
| ZL | Left trigger | Toggle fullscreen |
| ZR | Right trigger | Play/Pause (MPRIS) |
| + | Start | Space (play/pause fallback) |
| - | Select | Tab (menu) |
| L stick click | -- | Enter |
| R stick click | -- | Right mouse click |

### Launcher Mode (flex-launcher)

| Button | Action |
|--------|--------|
| D-pad | Arrow keys |
| A | Enter (select) |
| B | Escape |

### Media Mode (overlay during playback)

| Button | Action |
|--------|--------|
| D-pad up/down | Volume (accelerating) |
| D-pad left/right | Subtitle track |
| A | Play/Pause |
| B | Exit media mode |
| L/R bumpers | Seek (accelerating: 5s, 10s, 20s, 40s, 80s, 120s) |
| ZL | Toggle fullscreen |

### Y Button (Home) -- Special Behavior

- **Not fullscreen:** Immediately kills media apps and returns to flex-launcher.
- **In fullscreen:** Must hold Y for 3 seconds to confirm (prevents accidental exits during playback).

---

## Hardware Video Decode

The Raspberry Pi 5 uses a stateless V4L2 video decoder. The correct hwdec mode is `v4l2m2m-copy`, which decodes on hardware then copies frames to system memory for the compositor.

**Why `-copy`?** Without it, decoded frames stay in DMA buffers that labwc (wlroots) cannot composite. The `-copy` variant adds a small CPU overhead but works reliably with Wayland compositors.

Supported codecs via V4L2 on Pi 5:
- **H.264** (AVC) -- most common, fully hardware accelerated
- **HEVC** (H.265) -- hardware accelerated, used by many modern encodes

Configuration is set in three places:

| File | Setting |
|------|---------|
| `mpv.conf` | `hwdec=v4l2m2m-copy` |
| `jellyfinmediaplayer.conf` | `"hardwareDecoding": "v4l2m2m-copy"` |
| `jmp-mpv.conf` | Audio sync only (inherits hwdec from JMP config) |

---

## HDMI Audio

Pi 5 HDMI audio requires some care:

1. **Jack detection.** The HDMI audio jack status can report "off" even when a TV is connected. `fix-hdmi-audio.sh` forces DRM connector re-detection and restarts WirePlumber if no HDMI sink appears.

2. **Lip-sync compensation.** HDMI audio on Pi has inherent latency (~300ms). Both `mpv.conf` and `jmp-mpv.conf` set `audio-delay=-0.3` with `video-sync=audio` to keep audio and video aligned.

3. **Direct ALSA output.** `mpv.conf` uses `ao=alsa` with `audio-device=alsa/hdmi:CARD=vc4hdmi1,DEV=0` to bypass PulseAudio/PipeWire for lowest latency. JMP uses PipeWire by default via its own audio stack.

---

## CDP Remote Control

JMP embeds QtWebEngine (Chromium). Setting `QTWEBENGINE_REMOTE_DEBUGGING=9222` exposes the Chrome DevTools Protocol on port 9222, allowing full programmatic control.

### Using jmp-ctl.py

```bash
./jmp-ctl.py status                          # Current URL, player state
./jmp-ctl.py set-server https://jf.example.com  # Configure server
./jmp-ctl.py login myuser mypass             # Authenticate
./jmp-ctl.py navigate /web/#/movies          # Browse library
./jmp-ctl.py search "The Matrix"             # Search
./jmp-ctl.py play                            # Resume
./jmp-ctl.py pause                           # Pause
./jmp-ctl.py seek 120                        # Seek to 2:00
./jmp-ctl.py volume 80                       # Set volume
./jmp-ctl.py screenshot grab.png             # Capture screen
./jmp-ctl.py eval "document.title"           # Run arbitrary JS
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QTWEBENGINE_REMOTE_DEBUGGING` | -- | Port for CDP (set to `9222`) |
| `JMP_CDP_PORT` | `9222` | Port jmp-ctl.py connects to |
| `JMP_CDP_HOST` | `localhost` | Host for remote control (e.g. over SSH tunnel) |
| `QT_QPA_PLATFORM` | -- | `wayland` or `xcb` |
| `QTWEBENGINE_CHROMIUM_FLAGS` | -- | Set `--disable-gpu-compositing` on Pi |

---

## CEC Remote Control

`cec-keyboard-bridge.py` maps HDMI-CEC commands from your TV remote to keyboard input via uinput. This lets you navigate Jellyfin with the TV remote alone, no gamepad needed.

Mapped buttons: D-pad (arrows), Select (enter), Back (escape), Play/Pause (space), volume keys, number keys (0-9), color buttons (F1-F4), channel up/down (page up/down).

Requires `cec-utils` and `python3-evdev`:

```bash
sudo apt install cec-utils python3-evdev
```

---

## Moonlight Game Streaming

Moonlight is installed as a native `.deb` package (`moonlight-qt`), not via Flatpak. This provides better performance and simpler process management. The launcher's "Games" entry brings Moonlight to the foreground. The Y button on the controller kills Moonlight and returns to the launcher, same as for JMP.

---

## Prerequisites

| Component | Package / Source |
|-----------|-----------------|
| OS | Debian 13 (Trixie) or Raspberry Pi OS (64-bit, bookworm+) |
| Display server | labwc (Wayland compositor) |
| Launcher | flex-launcher |
| Audio | PipeWire + WirePlumber (system default) |
| Controller | python3-evdev (`sudo apt install python3-evdev`) |
| CEC (optional) | cec-utils (`sudo apt install cec-utils`) |
| Cursor hiding | unclutter-xfixes (`sudo apt install unclutter`) |
| Window control | wlrctl (`sudo apt install wlrctl`) |
| Display config | wlr-randr |
| Game streaming | moonlight-qt (optional) |

---

## Troubleshooting

### Black screen or GL render errors in JMP

JMP's web UI uses Chromium's GPU compositor by default, which does not work reliably on Pi's VideoCore VII GPU.

**Fix:** Ensure labwc-autostart exports:
```bash
export QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu-compositing --disable-smooth-scrolling"
```
This forces software rendering for the UI layer only. Video playback still uses hardware decode.

### No audio / HDMI audio not detected

HDMI audio jack status can get stuck as "off" after a reboot or TV power cycle.

**Fix:**
```bash
~/bin/fix-hdmi-audio.sh
```
This forces DRM re-detection, restarts WirePlumber if no HDMI sink is found, and sets HDMI as the default sink at 100% volume.

### Audio out of sync (lips move before/after sound)

**Fix:** Adjust `audio-delay` in `~/.config/mpv/mpv.conf` and `~/.local/share/jellyfinmediaplayer/mpv.conf`. Negative values mean "play audio earlier." Default is `-0.3` (300ms). Adjust in 0.05s increments.

### Controller not detected

1. Pair the Switch Pro Controller via `bluetoothctl`:
   ```bash
   bluetoothctl
   scan on
   pair 98:41:5C:XX:XX:XX
   trust 98:41:5C:XX:XX:XX
   connect 98:41:5C:XX:XX:XX
   ```
2. Verify evdev sees it:
   ```bash
   python3 -c "import evdev; [print(evdev.InputDevice(p).name, p) for p in evdev.list_devices()]"
   ```
3. Check the unified-controller service:
   ```bash
   systemctl --user status unified-controller
   journalctl --user -u unified-controller -f
   ```

### Controller drift (cursor moves on its own)

The analog stick dead zones are configured in `unified-controller.py`:
- `MOUSE_DEAD = 6000` (left stick cursor)
- `SCROLL_DEAD = 6000` (right stick scroll)
- `STICK_DIGITAL_DEAD = 8000` (left stick arrow key threshold)

If you experience drift, increase these values. The evdev axis range is -32768 to 32767.

### Controller auto-disconnects

The idle timeout is 15 minutes (`IDLE_TIMEOUT = 900` in unified-controller.py). After 15 minutes with no input, the daemon disconnects the controller via bluetoothctl to save battery. Press any button to wake it -- the daemon will reconnect automatically.

### JMP does not connect to Jellyfin server

Use CDP to set the server:
```bash
./jmp-ctl.py set-server https://your-jellyfin-server.com
./jmp-ctl.py login youruser yourpass
```

Or edit `jellyfinmediaplayer.conf` and set `startupurl_desktop` and `startupurl_extension` to your server URL.

### Display scaling looks wrong

The labwc-autostart sets `wlr-randr --output HDMI-A-2 --scale 1`. For 4K TVs at couch distance, try `--scale 2`. Edit `~/.config/labwc/autostart` to adjust.

---

## Compatibility

| Component | Tested |
|-----------|--------|
| Hardware | Raspberry Pi 5 (4GB / 8GB) |
| OS | Debian 13 (Trixie), Raspberry Pi OS 64-bit |
| Display | Wayland (labwc) -- X11 possible with `QT_QPA_PLATFORM=xcb` |
| Audio | PipeWire, ALSA |
| Video decode | V4L2 (H.264, HEVC) via v4l2m2m-copy |
| Controller | Nintendo Switch Pro Controller (Bluetooth), HDMI-CEC TV remotes |
| Game streaming | Moonlight (native .deb) |

---

## License

Jellyfin Media Player is GPL-2.0. Scripts and configuration files in this repository are MIT.
