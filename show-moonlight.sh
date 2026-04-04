#!/bin/bash
# =============================================================================
# show-moonlight.sh — Bring Moonlight to foreground, with gaming VM orchestration
# =============================================================================
# Called by: flex-launcher menu "Games"
#
# If a gaming VM backend is configured, this script can:
#   1) check if the VM is already ready,
#   2) start it when down,
#   3) wait until it's reachable,
#   4) publish state updates for the UI.
#
# Status output (for pi-home-a / launcher UI overlays):
#   /tmp/pi-home-a-games-status (override via PI_HOME_GAMES_STATUS_FILE)
#   Format: <unix_ts>|<state>|<message>
# =============================================================================

set -euo pipefail

# --- Wayland environment ---
export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000
export QT_QPA_PLATFORM=wayland

# Track foreground app for controller logic
echo com.moonlight_stream.moonlight > /tmp/foreground-app

# --- Optional config file ---
# This file can define GAMING_VM_* variables below.
CONFIG_FILE="${GAMING_VM_CONFIG_FILE:-$HOME/.config/pi-home-a/gaming-vm.env}"
if [ -f "$CONFIG_FILE" ]; then
    # shellcheck disable=SC1090
    source "$CONFIG_FILE"
fi

# --- Runtime knobs (with safe defaults) ---
: "${PI_HOME_GAMES_STATUS_FILE:=/tmp/pi-home-a-games-status}"
: "${GAMING_VM_WAIT_TIMEOUT_SEC:=180}"
: "${GAMING_VM_POLL_INTERVAL_SEC:=3}"
: "${GAMING_VM_AUTO_START:=1}"
: "${GAMING_VM_NOTIFY:=1}"

# Autodiscover helper scripts when explicit commands are not configured.
if [ -z "${GAMING_VM_READY_CHECK_CMD:-}" ] && [ -x "$HOME/bin/gaming-vm-ready.sh" ]; then
    GAMING_VM_READY_CHECK_CMD="$HOME/bin/gaming-vm-ready.sh"
fi
if [ -z "${GAMING_VM_START_CMD:-}" ] && [ -x "$HOME/bin/gaming-vm-start.sh" ]; then
    GAMING_VM_START_CMD="$HOME/bin/gaming-vm-start.sh"
fi

# Validate numeric inputs
if ! [[ "$GAMING_VM_WAIT_TIMEOUT_SEC" =~ ^[0-9]+$ ]]; then
    GAMING_VM_WAIT_TIMEOUT_SEC=180
fi
if ! [[ "$GAMING_VM_POLL_INTERVAL_SEC" =~ ^[0-9]+$ ]] || [ "$GAMING_VM_POLL_INTERVAL_SEC" -le 0 ]; then
    GAMING_VM_POLL_INTERVAL_SEC=3
fi

status_update() {
    local state="$1"
    local message="$2"
    printf '%s|%s|%s\n' "$(date +%s)" "$state" "$message" > "$PI_HOME_GAMES_STATUS_FILE"
}

notify_update() {
    local title="$1"
    local message="$2"
    if [ "$GAMING_VM_NOTIFY" = "1" ] && command -v notify-send >/dev/null 2>&1; then
        notify-send "$title" "$message" >/dev/null 2>&1 || true
    fi
}

is_vm_ready() {
    if [ -z "${GAMING_VM_READY_CHECK_CMD:-}" ]; then
        # No readiness check configured => keep existing behavior (assume ready).
        return 0
    fi
    bash -lc "$GAMING_VM_READY_CHECK_CMD" >/dev/null 2>&1
}

start_vm() {
    if [ -z "${GAMING_VM_START_CMD:-}" ]; then
        return 1
    fi
    bash -lc "$GAMING_VM_START_CMD" >/tmp/gaming-vm-start.log 2>&1
}

wait_for_vm_ready() {
    local elapsed=0

    while [ "$elapsed" -lt "$GAMING_VM_WAIT_TIMEOUT_SEC" ]; do
        if is_vm_ready; then
            return 0
        fi

        local remaining=$((GAMING_VM_WAIT_TIMEOUT_SEC - elapsed))
        status_update "waiting" "Gaming VM booting... (${remaining}s left)"
        sleep "$GAMING_VM_POLL_INTERVAL_SEC"
        elapsed=$((elapsed + GAMING_VM_POLL_INTERVAL_SEC))
    done

    return 1
}

# --- VM orchestration path ---
status_update "checking" "Checking gaming VM status"

if ! is_vm_ready; then
    if [ "$GAMING_VM_AUTO_START" != "1" ]; then
        status_update "error" "Gaming VM is offline"
        notify_update "Games unavailable" "Gaming VM is offline"
        exit 1
    fi

    status_update "starting" "Starting gaming VM"
    notify_update "Starting gaming VM" "Launching backend VM for Moonlight"

    if ! start_vm; then
        status_update "error" "Failed to start gaming VM"
        notify_update "Games launch failed" "Failed to start gaming VM"
        exit 1
    fi

    if ! wait_for_vm_ready; then
        status_update "error" "Gaming VM startup timed out"
        notify_update "Games launch timed out" "Gaming VM did not become ready in time"
        exit 1
    fi

    status_update "ready" "Gaming VM is ready"
    notify_update "Gaming VM ready" "Opening Moonlight"
else
    status_update "ready" "Gaming VM already ready"
fi

# --- Ensure Moonlight process/window is available ---
if ! pgrep -f moonlight-qt >/dev/null 2>&1; then
    nohup moonlight-qt --fullscreen >/tmp/moonlight.log 2>&1 &
    # Wait for window to appear (max 10s)
    for i in $(seq 1 20); do
        sleep 0.5
        wlrctl toplevel find app_id:com.moonlight_stream.Moonlight >/dev/null 2>&1 && break
    done
fi

# Focus Moonlight (restores from minimized on labwc)
wlrctl toplevel focus app_id:com.moonlight_stream.Moonlight >/dev/null 2>&1 || true
status_update "launching" "Opening Moonlight"
