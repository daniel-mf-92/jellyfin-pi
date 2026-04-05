#!/bin/bash
set -euo pipefail

LOCK_FILE=/tmp/jmp-launch.lock

# Clean stale lock (older than 2 minutes)
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
export SLINT_BACKEND=winit
export WINIT_UNIX_BACKEND=wayland
ulimit -n 65536

echo jellyfin-pi > /tmp/foreground-app

BINARY="/usr/local/bin/jellyfin-pi"

# --- Start jellyfin-pi if not already running ---
if ! pgrep -f "$BINARY" >/dev/null 2>&1; then
  pkill -f "jellyfin-pi" >/dev/null 2>&1 || true
  sleep 0.3
  nohup "$BINARY" > /tmp/jmp.log 2>&1 &
  sleep 0.5
fi

if ! pgrep -f "$BINARY" >/dev/null 2>&1; then
  sleep 1.5
fi

if ! pgrep -f "$BINARY" >/dev/null 2>&1; then
  echo "jellyfin-pi process failed to start" >&2
  exit 1
fi

# --- Wait for window (up to 10s) ---
WINDOW_FOUND=0
for i in $(seq 1 20); do
  if wlrctl toplevel find "title:Jellyfin" >/dev/null 2>&1; then
    WINDOW_FOUND=1
    break
  fi
  sleep 0.5
done

if [ "$WINDOW_FOUND" -eq 0 ]; then
  echo "jellyfin-pi window did not appear within 10s" >&2
fi

# --- Minimize flex-launcher, focus jellyfin-pi ---
wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || true
wlrctl toplevel focus "title:Jellyfin" >/dev/null 2>&1 || true

echo jellyfin-pi > /tmp/foreground-app
exit 0
