#!/bin/bash
# =============================================================================
# @section flex-launcher-health
# @frequency L0 (every 2 min)
# @description Restart flex-launcher if dead (skip if moonlight active)
# =============================================================================

flex_launcher_heal() {
    if pgrep -x flex-launcher >/dev/null 2>&1; then
        return 0
    fi

    # Only restart if no moonlight-qt session is active (don't interrupt gaming)
    if pgrep -x moonlight-qt >/dev/null 2>&1; then
        log "FLEX" "flex-launcher not running but moonlight-qt active. Skipping (game session)."
        return 0
    fi

    if ! pgrep -x labwc >/dev/null 2>&1; then
        log "FLEX" "flex-launcher not running but labwc not active. Skipping."
        return 0
    fi

    log "FLEX" "flex-launcher not running (no moonlight session). Restarting..."
    if check_circuit_breaker flex-launcher; then
        nohup /usr/local/bin/flex-launcher >/dev/null 2>&1 &
        disown
        record_restart flex-launcher
        sleep 2
        if pgrep -x flex-launcher >/dev/null 2>&1; then
            log "FLEX" "flex-launcher restarted successfully."
        else
            log "FLEX" "flex-launcher failed to restart."
        fi
    fi
}
