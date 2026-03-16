#!/usr/bin/env bash
set -euo pipefail

# Jellyfin Media Player — ARM64 (aarch64) build script
# Target: Raspberry Pi 5, Debian Trixie (13)
# Usage: sudo ./build-arm64.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="${SCRIPT_DIR}/build"
JMP_VERSION="1.11.1"
JMP_REPO="https://github.com/jellyfin/jellyfin-media-player.git"
WEBCLIENT_VERSION="10.10.7"
WEBCLIENT_URL="https://repo.jellyfin.org/files/client/jellyfin-web/${WEBCLIENT_VERSION}/jellyfin-web_${WEBCLIENT_VERSION}_portable.tar.gz"
NPROC=$(nproc)

echo "=== Jellyfin Media Player ARM64 Build ==="
echo "JMP version: ${JMP_VERSION}"
echo "Web client:  ${WEBCLIENT_VERSION}"
echo "Build cores: ${NPROC}"
echo ""

# --- Step 1: Install build dependencies ---
echo "[1/6] Installing build dependencies..."
apt-get update -qq
apt-get install -y --no-install-recommends \
    build-essential cmake git pkg-config \
    qtwebengine5-dev qtwebengine5-dev-tools qt5-qmake qtbase5-dev qtbase5-private-dev \
    libqt5webchannel5-dev libqt5x11extras5-dev qtdeclarative5-dev qtquickcontrols2-5-dev \
    qml-module-qtquick-controls qml-module-qtquick-controls2 qml-module-qtwebengine \
    qml-module-qtwebchannel qml-module-qtquick-layouts qml-module-qtquick-window2 \
    libmpv-dev mpv \
    libgl1-mesa-dev libgles2-mesa-dev \
    libx11-dev libxrandr-dev libxi-dev \
    libcec-dev libprotobuf-dev protobuf-compiler \
    ninja-build curl wget \
    python3 python3-pip \
    zlib1g-dev libfreetype6-dev libfontconfig-dev \
    libdrm-dev libgbm-dev libegl-dev 2>&1 | tail -5

echo "[1/6] Done."

# --- Step 2: Clone JMP source ---
echo "[2/6] Cloning jellyfin-media-player v${JMP_VERSION}..."
mkdir -p "${BUILD_DIR}"
cd "${BUILD_DIR}"

if [ -d "jellyfin-media-player" ]; then
    echo "  Source dir exists, pulling latest..."
    cd jellyfin-media-player
    git fetch --all
    git checkout v${JMP_VERSION} 2>/dev/null || git checkout master
    cd ..
else
    git clone --depth 1 --branch v${JMP_VERSION} "${JMP_REPO}" 2>/dev/null || \
    git clone --depth 1 "${JMP_REPO}"
fi

echo "[2/6] Done."

# --- Step 3: Download Jellyfin web client ---
echo "[3/6] Downloading Jellyfin web client v${WEBCLIENT_VERSION}..."
cd "${BUILD_DIR}/jellyfin-media-player"

if [ ! -d "dist/web-client/jellyfin-web" ]; then
    mkdir -p dist/web-client
    cd dist/web-client
    wget -q --show-progress -O jellyfin-web.tar.gz "${WEBCLIENT_URL}" || {
        echo "  Trying alternative web client URL..."
        # Try GitHub releases as fallback
        wget -q --show-progress -O jellyfin-web.tar.gz \
            "https://github.com/jellyfin/jellyfin-web/releases/download/v${WEBCLIENT_VERSION}/jellyfin-web_${WEBCLIENT_VERSION}_portable.tar.gz" || {
            echo "  ERROR: Could not download web client. Trying latest..."
            LATEST_WC=$(curl -sL https://api.github.com/repos/jellyfin/jellyfin-web/releases/latest | grep tag_name | head -1 | sed 's/.*"v\(.*\)".*/\1/')
            wget -q --show-progress -O jellyfin-web.tar.gz \
                "https://repo.jellyfin.org/files/client/jellyfin-web/${LATEST_WC}/jellyfin-web_${LATEST_WC}_portable.tar.gz"
        }
    }
    tar xzf jellyfin-web.tar.gz
    rm jellyfin-web.tar.gz
    cd "${BUILD_DIR}/jellyfin-media-player"
else
    echo "  Web client already downloaded."
fi

echo "[3/6] Done."

# --- Step 4: Configure ---
echo "[4/6] Configuring CMake build..."
mkdir -p build
cd build

cmake .. \
    -GNinja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr/local \
    -DQTROOT=/usr \
    -DLINUX_X11POWER=ON 2>&1 | tail -20

echo "[4/6] Done."

# --- Step 5: Build ---
echo "[5/6] Building (this may take 30-60 min on Pi 5)..."
echo "  Using ${NPROC} cores..."
ninja -j${NPROC} 2>&1 | tail -30

echo "[5/6] Done."

# --- Step 6: Install ---
echo "[6/6] Installing..."
ninja install 2>&1 | tail -10

echo ""
echo "============================================"
echo "  BUILD COMPLETE!"
echo "  Binary: /usr/local/bin/jellyfinmediaplayer"
echo "  Run:    jellyfinmediaplayer"
echo "============================================"
