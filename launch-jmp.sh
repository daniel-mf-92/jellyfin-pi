#!/bin/bash
# =============================================================================
# launch-jmp.sh — Launch Jellyfin Media Player in TV mode on Pi 5
# =============================================================================
# Kills any existing JMP instance, starts a fresh one under Wayland, and uses
# Chrome DevTools Protocol (CDP) to verify the web view loaded correctly.
# If the page is blank (common on first boot), it force-navigates to the server.
# If the page is on the login screen, it triggers the auto-login helper.
#
# Usage:    ./launch-jmp.sh
# Env:      JELLYFIN_SERVER=host:port  (default: localhost:8096)
# Requires: curl, python3, python3-websocket (for CDP navigation)
# =============================================================================
set -euo pipefail

JELLYFIN_SERVER="${JELLYFIN_SERVER:-localhost:8096}"

# --- Kill existing instance ---
killall jellyfinmediaplayer 2>/dev/null || true
sleep 1

# --- Wayland environment ---
# WAYLAND_DISPLAY / XDG_RUNTIME_DIR: connect to labwc compositor.
# QT_QPA_PLATFORM=wayland: force Qt Wayland backend (no XWayland).
# QTWEBENGINE_REMOTE_DEBUGGING=9222: expose CDP for page inspection.
# QTWEBENGINE_CHROMIUM_FLAGS: disable GPU compositing to avoid Mesa V3D
#   GL errors, and disable smooth scrolling to reduce GPU load.
# JMP_EXTERNAL_PLAYER=vlc: route unsupported formats to VLC (custom patch).
export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000
export QT_QPA_PLATFORM=wayland
export QTWEBENGINE_REMOTE_DEBUGGING=9222
export QTWEBENGINE_CHROMIUM_FLAGS="--disable-gpu-compositing --disable-smooth-scrolling"
export JMP_EXTERNAL_PLAYER=vlc

# --- Start JMP ---
nohup jellyfinmediaplayer --fullscreen --tv > /tmp/jmp.log 2>&1 &
JMP_PID=$!

# --- Wait for CDP endpoint (max 30s) ---
# JMP's embedded Chromium takes a few seconds to start the CDP server.
for i in $(seq 1 30); do
    if curl -s http://localhost:9222/json >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

sleep 3

# --- Verify page loaded ---
# On first launch or after a config wipe, JMP's web view can be blank.
# Detect this and force-navigate to the Jellyfin server via CDP WebSocket.
URL=$(curl -s http://localhost:9222/json | python3 -c "import json,sys; pages=json.load(sys.stdin); print(pages[0].get('url',''))" 2>/dev/null || echo "")

if [ -z "$URL" ] || [ "$URL" = "" ]; then
    # Web view didn't navigate — push it to the server via CDP
    WS_URL=$(curl -s http://localhost:9222/json | python3 -c "import json,sys; pages=json.load(sys.stdin); print(pages[0]['webSocketDebuggerUrl'])" 2>/dev/null)
    if [ -n "$WS_URL" ]; then
        python3 -c "
import json, os, websocket
server = os.environ.get('JELLYFIN_SERVER', 'localhost:8096')
ws = websocket.create_connection('${WS_URL}', suppress_origin=True)
ws.send(json.dumps({'id':1,'method':'Page.navigate','params':{'url':f'http://{server}/web/#/home'}}))
print(ws.recv())
ws.close()
" 2>/dev/null || true
    fi
fi

# --- Auto-login if on login page ---
# jmp-autologin-cdp.py is a local-only helper (not in this repo).
sleep 5
URL=$(curl -s http://localhost:9222/json | python3 -c "import json,sys; pages=json.load(sys.stdin); print(pages[0].get('url',''))" 2>/dev/null || echo "")
if echo "$URL" | grep -q "#/login"; then
    python3 ~/bin/jmp-autologin-cdp.py 2>/dev/null || true
fi

echo "JMP running (PID $JMP_PID)"
