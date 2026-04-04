#!/bin/bash
set -euo pipefail

LOCK_FILE=/tmp/jmp-launch.lock

# Clean stale lock (older than 2 minutes — previous launch crashed or timed out)
if [ -f "$LOCK_FILE" ]; then
  lock_age=$(( $(date +%s) - $(stat -c %Y "$LOCK_FILE" 2>/dev/null || echo 0) ))
  if [ "$lock_age" -gt 120 ]; then
    rm -f "$LOCK_FILE"
  fi
fi

exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  exit 0
fi

# --- Environment ---
export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000

# FIX: Raise file descriptor limit — Chromium/QtWebEngine needs thousands of FDs.
# Default 1024 causes "Too many open files" crash in /dev/shm shared memory.
ulimit -n 65536

# FIX: mpv (embedded in JMP) requires LC_NUMERIC=C to parse numbers correctly.
# Without this: "Non-C locale detected" → "Unhandled FatalException: Failed to parse
# application engine script" → instant crash.
export LC_NUMERIC=C

export QT_QPA_PLATFORM=wayland
export QTWEBENGINE_REMOTE_DEBUGGING=9222
export JMP_EXTERNAL_PLAYER=vlc
export JELLYFIN_SERVER="http://localhost:8096"
export LIBCEC_DISABLE=1

# Move QtWebEngine cache to tmpfs (RAM) for faster access
mkdir -p /tmp/jellyfin-qtwebengine-cache
export QTWEBENGINE_CACHE_DIRECTORY="/tmp/jellyfin-qtwebengine-cache"
export QTWEBENGINE_DISK_CACHE_DIR="/tmp/jellyfin-qtwebengine-cache"
export QTWEBENGINE_DISK_CACHE_SIZE=4294967296
export QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu-compositing --disable-smooth-scrolling --disk-cache-size=4294967296 --media-cache-size=2147483648 --aggressive-cache-discard=no"

echo jellyfinmediaplayer > /tmp/foreground-app

# --- Clean stale Chromium shared memory (prevents EMFILE buildup across restarts) ---
rm -f /dev/shm/.org.chromium.Chromium.* 2>/dev/null || true

# --- Start JMP if not already running ---
if ! pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  nohup /usr/local/bin/jellyfinmediaplayer --fullscreen > /tmp/jmp.log 2>&1 &
  sleep 0.5
fi

# Second check — if still not up, wait a bit longer (cold start can be slow)
if ! pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  sleep 1.5
fi

if ! pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  echo "JMP process failed to start" >&2
  exit 1
fi

# --- Wait for JMP window to appear in Wayland toplevel list (up to 10s) ---
JMP_WINDOW_FOUND=0
for i in $(seq 1 20); do
  for target in \
    "app_id:org.jellyfin.JellyfinDesktop" \
    "app_id:com.github.iwalton3.jellyfin-media-player" \
    "app_id:jellyfin-media-player" \
    "app_id:jellyfinmediaplayer" \
    "title:Jellyfin"; do
    if wlrctl toplevel find "$target" >/dev/null 2>&1; then
      JMP_WINDOW_FOUND=1
      break 2
    fi
  done
  sleep 0.5
done

if [ "$JMP_WINDOW_FOUND" -eq 0 ]; then
  echo "JMP process running but window did not appear within 10s" >&2
  # Still try to proceed — process is running, window may appear later
fi

# --- Minimize flex-launcher so it does not confuse mode detection ---
wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || \
wlrctl toplevel minimize title:"Flex Launcher" >/dev/null 2>&1 || true

# --- Focus JMP window ---
for target in \
  "app_id:org.jellyfin.JellyfinDesktop" \
  "app_id:com.github.iwalton3.jellyfin-media-player" \
  "app_id:jellyfin-media-player" \
  "app_id:jellyfinmediaplayer" \
  "title:Jellyfin"; do
  wlrctl toplevel focus "$target" >/dev/null 2>&1 || true
done

echo jellyfinmediaplayer > /tmp/foreground-app

exit 0
