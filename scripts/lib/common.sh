#!/bin/bash
# =============================================================================
# jellyfin-pi/scripts/lib/common.sh
# Shared infrastructure for JMP automation scripts.
# Sourced by jellyfin-cron.sh (and transitively by the master script).
# Provides fallback implementations when running standalone (not from master).
# =============================================================================

# Guard against double-sourcing
[[ -n "$_JELLYFIN_COMMON_LOADED" ]] && return 0
_JELLYFIN_COMMON_LOADED=1

# --- Resolve repo root ---
JELLYFIN_TV_DIR="${JELLYFIN_TV_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"

# --- Load .env (credentials) ---
if [[ -f "$JELLYFIN_TV_DIR/.env" ]]; then
    set -a
    source "$JELLYFIN_TV_DIR/.env"
    set +a
fi

# --- Derived constants ---
JELLYFIN_API="${JELLYFIN_URL:-http://10.100.0.2:8096}"
JELLYFIN_API_KEY="${JELLYFIN_API_KEY:-}"
BUFFER_DIR="/tmp/jellyfin-buffer"
BW_FILE="/tmp/pi-home-wg-bandwidth.json"

# --- Wayland environment (needed by most scripts) ---
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/1000}"

# --- Fallback functions (only defined if master script hasn't set them) ---
# When sourced from the master script, these are already defined and won't
# be overwritten. When running standalone, these provide basic functionality.

if ! type log &>/dev/null; then
    _JF_LOG_FILE="${LOG_FILE:-/dev/stderr}"
    log() {
        echo "$(date '+%Y-%m-%d %H:%M:%S') [$1] $2" >> "$_JF_LOG_FILE"
    }
fi

if ! type check_circuit_breaker &>/dev/null; then
    STATE_DIR="${STATE_DIR:-/tmp/jellyfin-cron-state}"
    mkdir -p "$STATE_DIR"

    check_circuit_breaker() {
        local component="$1"
        local max_restarts="${2:-3}"
        local cb_file="$STATE_DIR/cb_${component}"
        local now=$(date +%s)
        local cutoff=$((now - 3600))

        if [[ -f "$cb_file" ]]; then
            awk -v cutoff="$cutoff" '$1 > cutoff' "$cb_file" > "$cb_file.tmp" && mv "$cb_file.tmp" "$cb_file"
        fi

        local count=0
        [[ -f "$cb_file" ]] && count=$(wc -l < "$cb_file" | tr -d ' ')

        if [[ "$count" -ge "$max_restarts" ]]; then
            log "CIRCUIT" "$component: $count restarts in last hour (max $max_restarts). Breaker OPEN."
            return 1
        fi
        return 0
    }
fi

if ! type record_restart &>/dev/null; then
    record_restart() {
        local component="$1"
        echo "$(date +%s)" >> "$STATE_DIR/cb_${component}"
    }
fi

# --- Time gating fallback ---
if [[ -z "${RUN_5MIN+x}" ]]; then
    _MINUTE=$(date +%-M)
    RUN_5MIN=false
    (( _MINUTE % 5 < 2 )) && RUN_5MIN=true
fi
