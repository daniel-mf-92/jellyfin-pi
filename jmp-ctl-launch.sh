#!/usr/bin/env bash
# Launch JMP with CDP enabled for AI/CLI control.
# Usage: ./jmp-ctl-launch.sh [--server URL] [--login USER:PASS]

set -euo pipefail

CDP_PORT="${JMP_CDP_PORT:-9222}"
PLATFORM="${QT_QPA_PLATFORM:-wayland}"
SERVER=""
LOGIN=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --server) SERVER="$2"; shift 2 ;;
        --login)  LOGIN="$2"; shift 2 ;;
        --port)   CDP_PORT="$2"; shift 2 ;;
        --x11)    PLATFORM="xcb"; shift ;;
        *)        echo "Unknown option: $1"; exit 1 ;;
    esac
done

export QTWEBENGINE_REMOTE_DEBUGGING="$CDP_PORT"
export QT_QPA_PLATFORM="$PLATFORM"

echo "Starting JMP with CDP on port $CDP_PORT ($PLATFORM)..."
jellyfinmediaplayer --fullscreen --tv &
JMP_PID=$!

# Wait for CDP to become available
echo "Waiting for CDP..."
for i in $(seq 1 30); do
    if curl -s "http://localhost:$CDP_PORT/json" >/dev/null 2>&1; then
        echo "CDP ready on port $CDP_PORT"
        break
    fi
    sleep 1
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [[ -n "$SERVER" ]]; then
    echo "Setting server: $SERVER"
    python3 "$SCRIPT_DIR/jmp-ctl.py" set-server "$SERVER"
fi

if [[ -n "$LOGIN" ]]; then
    IFS=':' read -r user pass <<< "$LOGIN"
    echo "Logging in as: $user"
    python3 "$SCRIPT_DIR/jmp-ctl.py" login "$user" "$pass"
fi

echo "JMP running (PID $JMP_PID). Use jmp-ctl.py to control."
wait $JMP_PID
