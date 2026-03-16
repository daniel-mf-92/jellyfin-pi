# jellyfin-pi

**Jellyfin Media Player for ARM64 — AI-first, headless-controllable.**

Build and run [Jellyfin Media Player](https://github.com/jellyfin/jellyfin-media-player) on Raspberry Pi 5 and other ARM64 Linux devices, with built-in support for programmatic control via Chrome DevTools Protocol (CDP).

## Why this exists

The official JMP does not ship ARM64 Linux builds. This project provides:

1. **One-command ARM64 build** — installs deps, compiles, and installs JMP on Debian Trixie/Pi OS
2. **AI-first control interface** — `jmp-ctl.py` talks to JMP via CDP WebSocket, no GUI interaction needed
3. **Headless-friendly** — set server, login, browse, search, play, pause, seek, take screenshots — all from the command line or from any automation/AI agent

Perfect for Raspberry Pi media centers, kiosk displays, home automation, and AI-controlled entertainment systems.

## Quick Start

### Build & Install

```bash
git clone https://github.com/daniel-mf-92/jellyfin-pi.git
cd jellyfin-pi
sudo ./build-arm64.sh
```

Build time: ~60s on Pi 5 (Debian Trixie packages provide QtWebEngine5, no source compilation needed).

### Launch with CDP enabled

```bash
export QTWEBENGINE_REMOTE_DEBUGGING=9222
export QT_QPA_PLATFORM=wayland  # or xcb for X11
jellyfinmediaplayer --fullscreen --tv
```

### Control via CLI

```bash
# Set your Jellyfin server
./jmp-ctl.py set-server https://jellyfin.example.com

# Login
./jmp-ctl.py login myuser mypassword

# Check what's playing
./jmp-ctl.py status

# Search and browse
./jmp-ctl.py search "The Matrix"
./jmp-ctl.py items

# Playback control
./jmp-ctl.py play
./jmp-ctl.py pause
./jmp-ctl.py seek 120
./jmp-ctl.py volume 80
./jmp-ctl.py mute

# Navigate the web UI
./jmp-ctl.py navigate /web/#/home
./jmp-ctl.py navigate /web/#/movies

# Inspect and debug
./jmp-ctl.py screenshot output.png
./jmp-ctl.py dom
./jmp-ctl.py eval "document.title"
```

## AI / Automation Integration

JMP exposes the full Jellyfin web UI via CDP on port 9222. Any tool that speaks the Chrome DevTools Protocol can control it:

- **Python** — `websocket-client` with `suppress_origin=True` (see `jmp-ctl.py`)
- **Node.js** — `chrome-remote-interface` or Puppeteer (`browserWSEndpoint`)
- **Any language** — HTTP `GET http://localhost:9222/json` returns WebSocket URLs

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QTWEBENGINE_REMOTE_DEBUGGING` | - | **Required.** Set to port number (e.g. `9222`) to enable CDP |
| `JMP_CDP_PORT` | `9222` | Port for `jmp-ctl.py` to connect to |
| `JMP_CDP_HOST` | `localhost` | Host for `jmp-ctl.py` (for remote control over SSH tunnel) |
| `QT_QPA_PLATFORM` | - | Set to `wayland` or `xcb` depending on your display server |

### CDP capabilities

Since JMP uses QtWebEngine (Chromium), CDP gives you full access to:

- **Runtime.evaluate** — execute any JavaScript in the Jellyfin web UI context
- **Page.navigate** — go to any page
- **Page.captureScreenshot** — get PNG screenshots without external tools
- **DOM inspection** — read the full page DOM tree
- **Network monitoring** — watch API calls between the web UI and Jellyfin server
- **Input simulation** — dispatch keyboard/mouse events

This makes JMP fully controllable by AI agents, home automation systems (Home Assistant, Node-RED), voice assistants, or any custom software — without needing screen scraping or OCR.

## Files

| File | Purpose |
|------|---------|
| `build-arm64.sh` | Build script — installs deps, compiles JMP, installs to `/usr/local` |
| `jmp-ctl.py` | CLI controller — all JMP operations via CDP |
| `jmp-ctl-launch.sh` | Launcher script — starts JMP with CDP enabled + optional auto-connect |

## Compatibility

| Component | Tested |
|-----------|--------|
| **Hardware** | Raspberry Pi 5 (4GB/8GB) |
| **OS** | Debian 13 (Trixie), Raspberry Pi OS (64-bit) |
| **Display** | Wayland (labwc, sway) and X11 |
| **Audio** | PipeWire, ALSA |
| **Video** | Hardware decode via V4L2 |
| **Controller** | SDL2 gamepad, CEC (HDMI-CEC for TV remotes) |

## License

Jellyfin Media Player is GPL-2.0. `jmp-ctl.py` and build scripts in this repo are MIT.
