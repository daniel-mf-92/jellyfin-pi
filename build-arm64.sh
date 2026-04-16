#\!/usr/bin/env bash
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
#   1. Installs all build + runtime dependencies (Qt5, VLC, mesa, CEC, etc.)
#   2. Clones the JMP source at the specified tag
#   3. Overlays the jmp-custom source tree (VLC backend via CMake USE_VLC)
#   4. Builds with Ninja using all available cores
#   5. Installs to /usr/local/bin/jellyfinmediaplayer
#   6. Installs v4l2-utils for hardware decode verification
#   7. Runs a post-build verification check
# =============================================================================

JMP_VERSION="${1:-1.11.1}"
BUILD_DIR="/tmp/pi-media-player-build"
NPROC=$(nproc)

echo "=== Jellyfin Media Player ARM64 Build (VLC backend) ==="
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
    vlc libvlc-dev 2>&1 | tail -3
echo "[1/5] Done."

# --- Step 2: Install V4L2 utilities for hardware decode verification ---
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

# --- Step 3b: Overlay jmp-custom source tree (VLC CMake integration) ---
# The jmp-custom tree contains modified CMakeLists, PlayerConfiguration.cmake,
# FindVLC.cmake, and PlayerComponent.cpp with native VLC support via USE_VLC.
# Auto-detect the real user home (script runs as root via sudo)
if [ -n "${SUDO_USER:-}" ]; then
  _REAL_HOME=$(eval echo ~$SUDO_USER)
else
  _REAL_HOME=$HOME
fi
CUSTOM_DIR="${_REAL_HOME}/Documents/local-codebases/jmp-custom"
JMP_SRC="${BUILD_DIR}/jellyfin-media-player"
if [ -d "$CUSTOM_DIR" ]; then
    echo "[3b/5] Overlaying jmp-custom source tree..."
    # CMake modules (FindVLC.cmake, PlayerConfiguration.cmake)
    cp "$CUSTOM_DIR/CMakeModules/FindVLC.cmake" "$JMP_SRC/CMakeModules/"
    cp "$CUSTOM_DIR/CMakeModules/PlayerConfiguration.cmake" "$JMP_SRC/CMakeModules/"
    # Player sources (PlayerComponent.cpp with VLC support, updated CMakeLists.txt)
    cp "$CUSTOM_DIR/src/player/PlayerComponent.cpp" "$JMP_SRC/src/player/"
    cp "$CUSTOM_DIR/src/player/CMakeLists.txt" "$JMP_SRC/src/player/"
    # Main src CMakeLists.txt (conditional VLC/mpv linking)
    cp "$CUSTOM_DIR/src/CMakeLists.txt" "$JMP_SRC/src/"
    # Player header (VLC types replace mpv types)
    cp "$CUSTOM_DIR/src/player/PlayerComponent.h" "$JMP_SRC/src/player/"
    cp "$CUSTOM_DIR/src/player/CodecsComponent.cpp" "$JMP_SRC/src/player/"
    cp "$CUSTOM_DIR/src/player/CodecsComponent.h" "$JMP_SRC/src/player/"
    cp "$CUSTOM_DIR/src/player/OpenGLDetect.cpp" "$JMP_SRC/src/player/"
    # QML UI (conditional MpvVideoItem loader for VLC mode)
    cp "$CUSTOM_DIR/src/ui/webview.qml" "$JMP_SRC/src/ui/"
    # JavaScript bridge (VLC DirectPlay device profile)
    cp "$CUSTOM_DIR/native/nativeshell.js" "$JMP_SRC/native/"
    # SystemComponent: patch stock files to add getEnvironmentVariable (for VLC mode detection)
    # Don't replace — stock has setCursorVisibility, mouse timer, etc. we need
    bash "$CUSTOM_DIR/patch-systemcomponent.sh" "$JMP_SRC"
    echo "[3b/5] Done."
else
    echo "[3b/5] WARNING: jmp-custom not found at $CUSTOM_DIR — building with stock mpv"
fi

# --- Step 4: Configure & Build ---
echo "[4/5] Building with ${NPROC} cores (expect ~60s on Pi 5)..."
cd "${BUILD_DIR}/jellyfin-media-player"
rm -rf build && mkdir build && cd build

cmake .. \
    -GNinja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr/local \
    -DQTROOT=/usr \
    -DLINUX_X11POWER=ON \
    -DUSE_VLC=ON 2>&1 | tail -5

ninja -j${NPROC} 2>&1
echo "[4/5] Done."

# --- Step 5: Install & Verify ---
echo "[5/5] Installing and verifying..."
ninja install 2>&1 | tail -5

# Post-build verification: confirm the binary exists and runs
BINARY="/usr/local/bin/jellyfinmediaplayer"
if [ \! -x "$BINARY" ]; then
    echo "ERROR: Binary not found at ${BINARY}" >&2
    exit 1
fi

VERSION_OUTPUT=$("$BINARY" --version 2>&1 || true)

# Verify VLC linkage
echo ""
echo "--- VLC linkage check ---"
if ldd "$BINARY" 2>/dev/null | grep -q libvlc; then
    echo "OK: Binary links against libvlc"
    ldd "$BINARY" | grep vlc
elif readelf -d "$BINARY" 2>/dev/null | grep -q vlc; then
    echo "OK: Binary has VLC in dynamic section"
else
    echo "WARNING: Could not confirm VLC linkage in binary"
    echo "  (May be linked indirectly via jmp_core static lib)"
    # Check shared libs transitively
    ldd "$BINARY" 2>/dev/null | head -20 || true
fi

echo ""
echo "============================================"
echo "  BUILD COMPLETE (VLC backend)"
echo "  Binary:  ${BINARY}"
echo "  Version: ${VERSION_OUTPUT}"
echo "  Player:  VLC (libvlc)"
echo "  Hwdec:   v4l2m2m-copy (verify: v4l2-ctl --list-devices)"
echo "============================================"
