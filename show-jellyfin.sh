#!/bin/bash
# =============================================================================
# show-jellyfin.sh — Bring Jellyfin Media Player to the foreground
# =============================================================================
# JMP is pre-launched and minimized at boot (see labwc-autostart).
# This script focuses (un-minimizes) it. If JMP crashed or was killed,
# it re-launches and waits for the window to appear before focusing.
#
# Called by: flex-launcher menu, unified-controller.py, go-home.sh
# =============================================================================

# --- Wayland environment ---
# These must be set for wlrctl and JMP to find the labwc compositor.
export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000

# Track which app is in the foreground (used by unified-controller.py)
echo com.github.iwalton3.jellyfin-media-player > /tmp/foreground-app

# --- Ensure JMP is running ---
# wlrctl toplevel find exits 0 if the window exists, 1 if not.
if ! wlrctl toplevel find app_id:com.github.iwalton3.jellyfin-media-player 2>/dev/null; then
    # JMP is not running — launch it fresh
    export QT_QPA_PLATFORM=wayland
    export QTWEBENGINE_REMOTE_DEBUGGING=9222
    export QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu-compositing --disable-smooth-scrolling"
    nohup jellyfinmediaplayer --fullscreen --tv > /tmp/jmp.log 2>&1 &
    # Wait for window to appear (max 10s)
    for i in $(seq 1 20); do
        sleep 0.5
        wlrctl toplevel find app_id:com.github.iwalton3.jellyfin-media-player 2>/dev/null && break
    done
fi

# --- Focus JMP ---
# On labwc, "focus" also restores a minimized window to visible + foreground.
wlrctl toplevel focus app_id:com.github.iwalton3.jellyfin-media-player 2>/dev/null
