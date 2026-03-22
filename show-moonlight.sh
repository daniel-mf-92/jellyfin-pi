#!/bin/bash
# Bring Moonlight to foreground (pre-launched at boot)
export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000
export QT_QPA_PLATFORM=wayland

echo com.moonlight_stream.moonlight > /tmp/foreground-app

# If Moonlight is not running, launch it
if ! pgrep -f moonlight-qt > /dev/null 2>&1; then
    nohup moonlight-qt --fullscreen > /tmp/moonlight.log 2>&1 &
    # Wait for window to appear (max 10s)
    for i in $(seq 1 20); do
        sleep 0.5
        wlrctl toplevel find app_id:com.moonlight_stream.Moonlight 2>/dev/null && break
    done
fi

# Focus Moonlight (also restores from minimized on labwc)
wlrctl toplevel focus app_id:com.moonlight_stream.Moonlight 2>/dev/null
