#!/usr/bin/env bash
set -euo pipefail

# Jellyfin Media Player — ARM64 (aarch64) build script
# Target: Raspberry Pi 5, Debian Trixie (13)
# Build time: ~65 seconds on Pi 5 @ 2.8GHz
# Usage: sudo ./build-arm64.sh

JMP_VERSION="${1:-1.11.1}"
BUILD_DIR="/tmp/jellyfin-pi-build"
NPROC=$(nproc)

echo "=== Jellyfin Media Player ARM64 Build ==="
echo "JMP version: ${JMP_VERSION}"
echo "Build cores: ${NPROC}"
echo ""

# --- Step 1: Install build dependencies ---
echo "[1/4] Installing build dependencies..."
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
    libdrm-dev libgbm-dev libegl-dev 2>&1 | tail -3
echo "[1/4] Done."

# --- Step 2: Clone JMP source ---
echo "[2/4] Cloning jellyfin-media-player v${JMP_VERSION}..."
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
echo "[2/4] Done."

# --- Step 3: Configure & Build ---
echo "[3/4] Building with ${NPROC} cores..."
cd "${BUILD_DIR}/jellyfin-media-player"
rm -rf build && mkdir build && cd build

cmake .. \
    -GNinja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr/local \
    -DQTROOT=/usr \
    -DLINUX_X11POWER=ON 2>&1 | tail -5

time ninja -j${NPROC} 2>&1
echo "[3/4] Done."

# --- Step 4: Install ---
echo "[4/4] Installing..."
ninja install 2>&1 | tail -5

echo ""
echo "============================================"
echo "  BUILD COMPLETE!"
echo "  Version: $(jellyfinmediaplayer --version 2>&1)"
echo "  Binary:  /usr/local/bin/jellyfinmediaplayer"
echo "  Run:     jellyfinmediaplayer"
echo "============================================"
