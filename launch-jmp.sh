#!/bin/bash
set -euo pipefail

# Disable gate — touch ~/.pi-media-player-disabled to stop auto-launches.
if [ -f "$HOME/.pi-media-player-disabled" ]; then
  exit 0
fi

LOCK_FILE=/tmp/jmp-launch.lock

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

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR}/bus}"
export SLINT_BACKEND=winit
export WINIT_UNIX_BACKEND=wayland
ulimit -n 65536

echo pi-media-player > /tmp/foreground-app

BINARY="/usr/local/bin/pi-media-player"
app_running() {
  pgrep -f "$BINARY" >/dev/null 2>&1 || pgrep -x pi-media-player >/dev/null 2>&1
}

MEM_MAX="${PI_MEDIA_PLAYER_MEM_MAX:-8G}"
MEM_HIGH="${PI_MEDIA_PLAYER_MEM_HIGH:-5G}"

if ! app_running; then
  pkill -x pi-media-player >/dev/null 2>&1 || true
  sleep 0.3

  systemctl --user reset-failed pi-media-player.scope >/dev/null 2>&1 || true

  user_systemd_ok=false
  if [ -S "${XDG_RUNTIME_DIR}/systemd/private" ]; then
    state=$(systemctl --user is-system-running 2>/dev/null || echo offline)
    [ "$state" != "offline" ] && user_systemd_ok=true
  fi

  if [ "$user_systemd_ok" = true ]; then
    nohup systemd-run --user --scope --unit=pi-media-player --collect \
      -p MemoryMax="$MEM_MAX" \
      -p MemoryHigh="$MEM_HIGH" \
      -p MemorySwapMax=0 \
      -p OOMPolicy=kill \
      --quiet \
      "$BINARY" > /tmp/pi-media-player.log 2>&1 &
  else
    echo "$(date -Iseconds) WARN: systemd-run --user unreachable, starting pi-media-player WITHOUT cgroup cap" >> /tmp/pi-media-player.log
    nohup "$BINARY" > /tmp/pi-media-player.log 2>&1 &
  fi

  sleep 0.5
fi

if ! app_running; then
  sleep 1.5
fi

if ! app_running; then
  echo "pi-media-player process failed to start" >&2
  exit 1
fi

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

wlrctl toplevel minimize app_id:flex-launcher >/dev/null 2>&1 || true
wlrctl toplevel focus "app_id:pi-media-player" >/dev/null 2>&1 || \
wlrctl toplevel focus "title:Jellyfin" >/dev/null 2>&1 || true

echo pi-media-player > /tmp/foreground-app
exit 0
