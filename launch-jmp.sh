#!/bin/bash
# Launch Jellyfin Media Player in TV mode
# Navigates to server via CDP if web view fails to auto-load
# Set JELLYFIN_SERVER env var to override default server address
set -euo pipefail

JELLYFIN_SERVER="${JELLYFIN_SERVER:-10.100.0.2:8096}"

killall jellyfinmediaplayer 2>/dev/null || true
sleep 1

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000
export QT_QPA_PLATFORM=wayland
export QTWEBENGINE_REMOTE_DEBUGGING=9222
export JMP_EXTERNAL_PLAYER=vlc

nohup jellyfinmediaplayer --fullscreen --tv > /tmp/jmp.log 2>&1 &
JMP_PID=$!

# Wait for CDP
for i in $(seq 1 30); do
    if curl -s http://localhost:9222/json >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

sleep 3

# Check if page loaded or is blank
URL=$(curl -s http://localhost:9222/json | python3 -c "import json,sys; pages=json.load(sys.stdin); print(pages[0].get('url',''))" 2>/dev/null || echo "")

if [ -z "$URL" ] || [ "$URL" = "" ]; then
    # Web view didn't navigate - force it via CDP
    WS_URL=$(curl -s http://localhost:9222/json | python3 -c "import json,sys; pages=json.load(sys.stdin); print(pages[0]['webSocketDebuggerUrl'])" 2>/dev/null)
    if [ -n "$WS_URL" ]; then
        python3 -c "
import json, os, websocket
server = os.environ.get('JELLYFIN_SERVER', '10.100.0.2:8096')
ws = websocket.create_connection('${WS_URL}', suppress_origin=True)
ws.send(json.dumps({'id':1,'method':'Page.navigate','params':{'url':f'http://{server}/web/#/home'}}))
print(ws.recv())
ws.close()
" 2>/dev/null || true
    fi
fi

# Auto-login if needed (jmp-autologin-cdp.py is local-only, not in repo)
sleep 5
URL=$(curl -s http://localhost:9222/json | python3 -c "import json,sys; pages=json.load(sys.stdin); print(pages[0].get('url',''))" 2>/dev/null || echo "")
if echo "$URL" | grep -q "#/login"; then
    python3 ~/bin/jmp-autologin-cdp.py 2>/dev/null || true
fi

echo "JMP running (PID $JMP_PID)"
