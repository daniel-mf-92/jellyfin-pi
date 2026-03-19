#!/bin/bash
# Kill foreground apps and return to flex-launcher
# Triggered by Y button on Switch Pro controller

# Kill Jellyfin Media Player
killall jellyfinmediaplayer 2>/dev/null
killall chromium 2>/dev/null

# Kill Moonlight
killall moonlight-qt 2>/dev/null

# Kill VLC (external player)
killall vlc 2>/dev/null

# Small delay for cleanup
sleep 0.5

# Restart flex-launcher if not running
if ! pgrep -x flex-launcher >/dev/null; then
    export WAYLAND_DISPLAY=wayland-0
    export XDG_RUNTIME_DIR=/run/user/1000
    nohup flex-launcher -c ~/.config/flex-launcher/config.ini > /tmp/flex-launcher.log 2>&1 &
fi
