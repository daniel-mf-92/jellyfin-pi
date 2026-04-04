#!/bin/bash
set -euo pipefail

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000

# Kill other media players (mpv, vlc) but NOT JMP
for proc in vlc mpv mpvpaper; do
  pkill -x "$proc" >/dev/null 2>&1 || true
done

echo jellyfinmediaplayer > /tmp/foreground-app

# If JMP is already running, just focus it (<1s)
if pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || true
  for target in \
    "app_id:org.jellyfin.JellyfinDesktop" \
    "app_id:com.github.iwalton3.jellyfin-media-player" \
    "app_id:jellyfin-media-player" \
    "app_id:jellyfinmediaplayer" \
    "title:Jellyfin"; do
    wlrctl toplevel unfullscreen "$target" >/dev/null 2>&1 || true
    wlrctl toplevel focus "$target" >/dev/null 2>&1 || true
    wlrctl toplevel fullscreen "$target" >/dev/null 2>&1 || true
  done
  exit 0
fi

# JMP is dead — do full launch
if command -v timeout >/dev/null 2>&1; then
  timeout 15s "$HOME/jellyfin-tv/launch-jmp.sh" >/tmp/show-jellyfin.log 2>&1 || true
else
  "$HOME/jellyfin-tv/launch-jmp.sh" >/tmp/show-jellyfin.log 2>&1 || true
fi

# If JMP still not up, recover launcher
if ! pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  systemctl --user restart flex-launcher.service >/dev/null 2>&1 || true
  sleep 0.6
  wlrctl toplevel focus app_id:flex-launcher >/dev/null 2>&1 || \
  wlrctl toplevel focus title:"Flex Launcher" >/dev/null 2>&1 || true
  echo flex-launcher > /tmp/foreground-app
  exit 1
fi

exit 0
