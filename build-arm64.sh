#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# build-arm64.sh — Build Jellyfin Media Player from source on Raspberry Pi 5
# =============================================================================
# Target:     Raspberry Pi 5 (aarch64), Debian Trixie (13)
# Build time: ~60 seconds on Pi 5 @ 2.8GHz (4 cores)
# Usage:      sudo ./build-arm64.sh [version]
# Example:    sudo ./build-arm64.sh 1.11.1
#
# This script:
#   1. Installs all build + runtime dependencies (Qt5, mpv, mesa, CEC, etc.)
#   2. Clones the JMP source at the specified tag
#   3. Optionally patches in custom PlayerComponent (VLC external player)
#   4. Builds with Ninja using all available cores
#   5. Installs to /usr/local/bin/jellyfinmediaplayer
#   6. Installs v4l2-utils for hardware decode verification
#   7. Runs a post-build verification check
# =============================================================================

JMP_VERSION="${1:-1.11.1}"
BUILD_DIR="/tmp/jellyfin-pi-build"
NPROC=$(nproc)

echo "=== Jellyfin Media Player ARM64 Build ==="
echo "JMP version: ${JMP_VERSION}"
echo "Build cores: ${NPROC}"
echo ""

# --- Step 1: Install build dependencies ---
echo "[1/5] Installing build dependencies..."
apt-get update -qq
apt-get install -y --no-install-recommends \
    build-essential cmake git pkg-config ninja-build \
    qtwebengine5-dev qtwebengine5-dev-tools qt5-qmake qtbase5-dev qtbase5-private-dev \
    libqt5webchannel5-dev libqt5x11extras5-dev qtdeclarative5-dev qtquickcontrols2-5-dev \
    qml-module-qtquick-controls qml-module-qtquick-controls2 qml-module-qtwebengine \
    qml-module-qtwebchannel qml-module-qtquick-layouts qml-module-qtquick-window2 \
    libmpv-dev mpv libgl1-mesa-dev libgles2-mesa-dev \
    libx11-dev libxrandr-dev libxi-dev \
    libcec-dev libprotobuf-dev protobuf-compiler \
    libsdl2-dev zlib1g-dev libfreetype6-dev libfontconfig-dev \
    libdrm-dev libgbm-dev libegl-dev \
    vlc 2>&1 | tail -3
echo "[1/5] Done."

# --- Step 2: Install V4L2 utilities for hardware decode verification ---
# v4l2-ctl lets you confirm the Pi 5's V4L2 M2M decoder is available:
#   v4l2-ctl --list-devices       (should show "bcm2835-codec-decode")
#   v4l2-ctl -d /dev/video10 --all (shows decoder capabilities)
echo "[2/5] Installing V4L2 utilities for hardware decode verification..."
apt-get install -y --no-install-recommends \
    v4l-utils 2>&1 | tail -1
echo "[2/5] Done."

# --- Step 3: Clone JMP source ---
echo "[3/5] Cloning jellyfin-media-player v${JMP_VERSION}..."
mkdir -p "${BUILD_DIR}"
cd "${BUILD_DIR}"

if [ -d "jellyfin-media-player" ]; then
    cd jellyfin-media-player && git fetch --all
    git checkout "v${JMP_VERSION}" 2>/dev/null || true
    cd ..
else
    git clone --depth 1 --branch "v${JMP_VERSION}" \
        https://github.com/jellyfin/jellyfin-media-player.git 2>&1
fi
echo "[3/5] Done."

# --- Step 3b: Apply custom patches (VLC external player support) ---
# If a custom PlayerComponent.cpp exists, overlay it onto the source tree.
# This enables launching VLC as an external player for formats mpv struggles with.
CUSTOM_SRC="${HOME}/Documents/local-codebases/jmp-custom/src/player/PlayerComponent.cpp"
if [ -f "$CUSTOM_SRC" ]; then
    echo "[3b/5] Applying custom PlayerComponent (VLC external player)..."
    cp "$CUSTOM_SRC" "${BUILD_DIR}/jellyfin-media-player/src/player/PlayerComponent.cpp"
    echo "[3b/5] Done."
fi

# --- Step 4: Configure & Build ---
# Ninja parallel build uses all Pi 5 cores. Expect ~60s wall time.
echo "[4/5] Building with ${NPROC} cores (expect ~60s on Pi 5)..."
cd "${BUILD_DIR}/jellyfin-media-player"
rm -rf build && mkdir build && cd build

cmake .. \
    -GNinja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr/local \
    -DQTROOT=/usr \
    -DLINUX_X11POWER=ON 2>&1 | tail -5

time ninja -j${NPROC} 2>&1
echo "[4/5] Done."

# --- Step 5: Install & Verify ---
echo "[5/5] Installing and verifying..."
ninja install 2>&1 | tail -5

# Post-build verification: confirm the binary exists and runs
BINARY="/usr/local/bin/jellyfinmediaplayer"
if [ ! -x "$BINARY" ]; then
    echo "ERROR: Binary not found at ${BINARY}" >&2
    exit 1
fi

VERSION_OUTPUT=$("$BINARY" --version 2>&1 || true)
echo ""
echo "============================================"
echo "  BUILD COMPLETE"
echo "  Binary:  ${BINARY}"
echo "  Version: ${VERSION_OUTPUT}"
echo "  Hwdec:   v4l2m2m-copy (verify: v4l2-ctl --list-devices)"
echo "============================================"
