#!/bin/bash
# =============================================================================
# @section jmp-self-healing
# @frequency L0 (every 2 min)
# @description JMP crash/freeze detection + auto-recovery
# =============================================================================

jmp_self_heal() {
    local foreground_app
    foreground_app=$(cat /tmp/foreground-app 2>/dev/null || echo "")

    if [[ "$foreground_app" == "jellyfinmediaplayer" ]] || [[ "$foreground_app" == "jellyfin" ]]; then
        # JMP should be running
        if ! pgrep -x jellyfinmediaplayer >/dev/null 2>&1; then
            log "JMP-HEAL" "JMP crashed while active — auto-restarting"
            check_circuit_breaker "jmp-heal" 5 && {
                "$JELLYFIN_TV_DIR/show-jellyfin.sh" >/dev/null 2>&1 &
                record_restart "jmp-heal"
                log "JMP-HEAL" "JMP restarted"
            }
        else
            # JMP running, check if window visible
            if ! wlrctl toplevel list 2>/dev/null | grep -qi "jellyfin"; then
                log "JMP-HEAL" "JMP window lost — forcing focus"
                wlrctl toplevel focus app_id:com.github.iwalton3.jellyfin-media-player 2>/dev/null || true
                wlrctl toplevel fullscreen app_id:com.github.iwalton3.jellyfin-media-player 2>/dev/null || true
            fi
            # Check if JMP frozen (CDP not responding)
            if ! curl -s --max-time 1 http://127.0.0.1:9222/json >/dev/null 2>&1; then
                log "JMP-HEAL" "JMP frozen (CDP dead) — killing and restarting"
                pkill -9 jellyfinmediaplayer 2>/dev/null || true
                sleep 0.5
                "$JELLYFIN_TV_DIR/show-jellyfin.sh" >/dev/null 2>&1 &
            fi
        fi
    fi
}
