# jellyfin-pi

Jellyfin Media Player (JMP) built for Raspberry Pi 5 (ARM64/aarch64) on Debian Trixie.

## What is this?

The official [Jellyfin Media Player](https://github.com/jellyfin/jellyfin-media-player) does not provide ARM64 Linux builds. This repo provides a build script and pre-built releases for Raspberry Pi 5.

## Quick Install (Pre-built)

Check the [Releases](https://github.com/daniel-mf-92/jellyfin-pi/releases) page for .deb packages.

## Build from Source

Requirements: Raspberry Pi 5 (or any aarch64 Debian Trixie system), 8GB+ RAM recommended.

```bash
git clone https://github.com/daniel-mf-92/jellyfin-pi.git
cd jellyfin-pi
sudo ./build-arm64.sh
```

The script handles everything: installs dependencies, clones JMP source, downloads the web client, and builds with Ninja.

Build time: ~30-60 min on Pi 5 at stock clocks.

## What's included

- `build-arm64.sh` — Full build script for Debian Trixie ARM64
- Pre-built releases when available

## Compatibility

- **Tested on:** Raspberry Pi 5 (8GB), Debian 13 (Trixie), Raspberry Pi OS
- **Desktop:** Wayland (labwc) and X11
- **Audio:** PipeWire / ALSA
- **Video:** Hardware decode via V4L2

## License

Jellyfin Media Player is licensed under GPL-2.0. This repo only contains build scripts.
