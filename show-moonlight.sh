#!/bin/bash
# Start Moonlight and jump directly to the games grid

set -u

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000
export QT_QPA_PLATFORM=wayland

MOONLIGHT_APP_ID="com.moonlight_stream.Moonlight"

echo "$MOONLIGHT_APP_ID" > /tmp/foreground-app

# Always relaunch Moonlight for deterministic state (Computers -> Games grid)
pkill -x moonlight-qt 2>/dev/null || true
sleep 0.3
nohup moonlight-qt > /tmp/moonlight.log 2>&1 &

# Wait for window (max 12s)
for _ in $(seq 1 24); do
    sleep 0.5
    if wlrctl toplevel find app_id:"$MOONLIGHT_APP_ID" 2>/dev/null; then
        break
    fi
done

if ! wlrctl toplevel find app_id:"$MOONLIGHT_APP_ID" 2>/dev/null; then
    echo "Moonlight window did not appear" >> /tmp/moonlight.log
    exit 1
fi

# Bring to front and fullscreen
wlrctl toplevel focus app_id:"$MOONLIGHT_APP_ID" 2>/dev/null || true
wlrctl toplevel fullscreen app_id:"$MOONLIGHT_APP_ID" 2>/dev/null || true

# If decoder warning popup appears, click its OK button (safe no-op if absent)
sleep 1.2
wlrctl pointer move -10000 -10000 2>/dev/null || true
wlrctl pointer move 1150 625 2>/dev/null || true
wlrctl pointer click 2>/dev/null || true

# Open the single host card to jump to the full games grid
sleep 0.3
wlrctl pointer move -10000 -10000 2>/dev/null || true
wlrctl pointer move 120 150 2>/dev/null || true
wlrctl pointer click 2>/dev/null || true

# Keep it foregrounded
sleep 1.0
wlrctl toplevel focus app_id:"$MOONLIGHT_APP_ID" 2>/dev/null || true
wlrctl toplevel fullscreen app_id:"$MOONLIGHT_APP_ID" 2>/dev/null || true

exit 0
