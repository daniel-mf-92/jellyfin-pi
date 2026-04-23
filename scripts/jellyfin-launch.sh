#!/bin/bash
# =============================================================================
# @section pi-media-player-launch
# @frequency L1 (every 5 min)
# @description Ensure native Jellyfin Media Player is running
# =============================================================================

jellyfin_ensure_running() {
    # Kill legacy launchers if they are still around
    pkill -f "chromium.*jellyfin" 2>/dev/null || true
    pkill -f "kodi --standalone" 2>/dev/null || true

    # Don't launch if mpv/vlc is actively playing (would steal focus)
    if pgrep -x mpv >/dev/null 2>&1 || pgrep -x vlc >/dev/null 2>&1; then
        log "JELLYFIN" "Skipping JMP launch — media player active"
        return 0
    fi

    if ! pgrep -x jellyfinmediaplayer >/dev/null 2>&1; then
        log "JELLYFIN" "Native Jellyfin Media Player not running. Launching..."

        export QT_QPA_PLATFORM=wayland
        export QTWEBENGINE_REMOTE_DEBUGGING=9222

        nohup "$JELLYFIN_TV_DIR/launch-jmp.sh" >/tmp/jmp-launch.log 2>&1 &
        disown
        sleep 2

        if pgrep -x jellyfinmediaplayer >/dev/null 2>&1; then
            log "JELLYFIN" "Native Jellyfin Media Player launched."
        else
            log "JELLYFIN" "Failed to launch native Jellyfin Media Player."
        fi
    fi
}
