#!/bin/bash
set -euo pipefail

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000

echo jellyfinmediaplayer > /tmp/foreground-app

# --- If JMP is already running, just focus it (no kill/restart) ---
if pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  # Minimize flex-launcher
  wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || \
  wlrctl toplevel minimize title:"Flex Launcher" >/dev/null 2>&1 || true

  # Focus JMP
  for target in \
    "app_id:org.jellyfin.JellyfinDesktop" \
    "app_id:com.github.iwalton3.jellyfin-media-player" \
    "app_id:jellyfin-media-player" \
    "app_id:jellyfinmediaplayer" \
    "title:Jellyfin"; do
    wlrctl toplevel focus "$target" >/dev/null 2>&1 && break
  done
  exit 0
fi

# --- JMP not running: stop any other media, then cold-start ---
MEDIA_QUIT="$HOME/bin/media-quit.sh"
if [ -x "$MEDIA_QUIT" ]; then
  "$MEDIA_QUIT" >/dev/null 2>&1 || true
fi

"$HOME/jellyfin-tv/launch-jmp.sh" >/tmp/show-jellyfin.log 2>&1 || true

# If JMP still not up after launch attempt, recover flex-launcher
if ! pgrep -f /usr/local/bin/jellyfinmediaplayer >/dev/null 2>&1; then
  systemctl --user restart flex-launcher.service >/dev/null 2>&1 || true
  sleep 0.6
  wlrctl toplevel focus app_id:flex-launcher >/dev/null 2>&1 || \
  wlrctl toplevel focus title:"Flex Launcher" >/dev/null 2>&1 || true
  echo flex-launcher > /tmp/foreground-app
  exit 1
fi

exit 0
