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

echo pi-media-player > /tmp/foreground-app

BINARY="/usr/local/bin/pi-media-player"
app_running() {
  pgrep -f "$BINARY" >/dev/null 2>&1 || pgrep -x pi-media-player >/dev/null 2>&1
}

# --- Start Pi-Media-Player if not already running ---
if ! app_running; then
  pkill -x pi-media-player >/dev/null 2>&1 || true
  sleep 0.3
  nohup "$BINARY" > /tmp/jmp.log 2>&1 &
  sleep 0.5
fi

if ! app_running; then
  sleep 1.5
fi

if ! app_running; then
  echo "pi-media-player process failed to start" >&2
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
  echo "Pi-Media-Player window did not appear within 10s" >&2
fi

# --- Minimize flex-launcher, focus Pi-Media-Player ---
wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || true
wlrctl toplevel focus "title:Jellyfin" >/dev/null 2>&1 || true

echo pi-media-player > /tmp/foreground-app
exit 0
