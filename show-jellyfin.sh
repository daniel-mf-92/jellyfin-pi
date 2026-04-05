#!/bin/bash
set -euo pipefail

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000

# Kill other media players but NOT jellyfin-pi
for proc in vlc mpv mpvpaper; do
  pkill -x "$proc" >/dev/null 2>&1 || true
done

echo jellyfin-pi > /tmp/foreground-app

BINARY="/usr/local/bin/jellyfin-pi"

jtv_has_toplevel() {
  wlrctl toplevel find "title:Jellyfin" >/dev/null 2>&1
}

focus_jtv() {
  wlrctl toplevel unfullscreen "title:Jellyfin" >/dev/null 2>&1 || true
  wlrctl toplevel focus "title:Jellyfin" >/dev/null 2>&1 || true
  wlrctl toplevel fullscreen "title:Jellyfin" >/dev/null 2>&1 || true
}

restore_launcher() {
  echo flex-launcher > /tmp/foreground-app
  wlrctl toplevel focus app_id:flex-launcher >/dev/null 2>&1 || true
}

# If process is running and has a window, just focus it
if pgrep -f "$BINARY" >/dev/null 2>&1; then
  if jtv_has_toplevel; then
    wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || true
    focus_jtv
    sleep 0.3
    if jtv_has_toplevel; then
      exit 0
    fi
    restore_launcher
    exit 1
  else
    # Zombie: process alive but no window
    pkill -f "$BINARY" >/dev/null 2>&1 || true
    sleep 1
    pkill -9 -f "jellyfin-pi" >/dev/null 2>&1 || true
    sleep 0.5
  fi
fi

# Full launch
if command -v timeout >/dev/null 2>&1; then
  timeout 15s "$HOME/jellyfin-pi/launch-jmp.sh" >/tmp/show-jellyfin.log 2>&1 || true
else
  "$HOME/jellyfin-pi/launch-jmp.sh" >/tmp/show-jellyfin.log 2>&1 || true
fi

sleep 1
if pgrep -f "$BINARY" >/dev/null 2>&1 && jtv_has_toplevel; then
  exit 0
fi

restore_launcher
exit 1
