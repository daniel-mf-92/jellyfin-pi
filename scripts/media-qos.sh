#!/bin/bash
# =============================================================================
# @section media-streaming-qos
# @frequency L0 (every 2 min)
# @description QoS: SIGSTOP go2rtc, priority management, bandwidth-hog killing
# =============================================================================

QOS_STATE_FILE="$STATE_DIR/media-qos-active"
QOS_GRACE_PERIOD=600  # 10 min grace after stream ends

is_media_playing() {
    pgrep -x vlc >/dev/null 2>&1 && return 0
    pgrep -x ffplay >/dev/null 2>&1 && return 0
    pgrep -x mpv >/dev/null 2>&1 && return 0
    return 1
}

media_qos_enable() {
    log "QOS" "=== ENABLING STREAMING MODE ==="

    # SIGSTOP go2rtc (most effective — immediate bandwidth relief)
    if pgrep -x go2rtc >/dev/null 2>&1; then
        sudo killall -STOP go2rtc 2>/dev/null && log "QOS" "go2rtc SIGSTOP sent (frozen)"
    fi

    # Notify Azure VM to enable peer-aware tc shaping (background, non-blocking)
    ssh -o ConnectTimeout=3 -o BatchMode=yes -o StrictHostKeyChecking=no relay-host.local \
        "\$HOME/bin/jellyfin-qos.sh enable" >>/tmp/azure-qos-call.log 2>&1 &
    log "QOS" "Azure VM QoS enable requested (background)"

    # Boost media player priority
    for pid in $(pgrep -x vlc 2>/dev/null) $(pgrep -x ffplay 2>/dev/null) $(pgrep -x mpv 2>/dev/null); do
        sudo renice -15 -p $pid 2>/dev/null
    done

    # tc rate limiting removed: tc only shapes egress traffic, but the Pi is a
    # *receiver* for Jellyfin streams — the bottleneck is inbound bandwidth, not
    # outbound. The htb qdisc added kernel overhead on every outbound packet with
    # zero benefit for inbound stream quality. SIGSTOP on go2rtc (above) is the
    # effective mechanism — it stops inbound camera relay traffic at the source.
    log "QOS" "Skipping tc (egress-only, ineffective for inbound streams — SIGSTOP is the effective control)"

    # Deprioritize go2rtc (fallback if SIGSTOP failed)
    for pid in $(pgrep -f go2rtc 2>/dev/null); do
        sudo renice 19 -p $pid 2>/dev/null
    done

    # Deprioritize Chromium
    local chrome_depri=0
    for pid in $(pgrep -f chromium 2>/dev/null | head -10); do
        sudo renice 15 -p $pid 2>/dev/null && ((chrome_depri++))
    done
    [[ $chrome_depri -gt 0 ]] && log "QOS" "Deprioritized Chromium ($chrome_depri processes)"

    # Deprioritize Kodi
    for pid in $(pgrep -f kodi 2>/dev/null); do
        sudo renice 15 -p $pid 2>/dev/null
    done

    # Kill bandwidth hogs
    local killed=0
    for proc in wget aria2c; do
        for pid in $(pgrep -x "$proc" 2>/dev/null); do
            kill $pid 2>/dev/null && ((killed++))
        done
    done
    # Kill large curl transfers (not Jellyfin buffer or API calls)
    for pid in $(pgrep -x curl 2>/dev/null); do
        local cl
        cl=$(ps -p $pid -o args= 2>/dev/null)
        [[ "$cl" =~ "localhost" ]] && continue
        [[ "$cl" =~ "jellyfin-buffer" ]] && continue
        [[ "$cl" =~ "localhost:8096" ]] && continue
        local cpu
        cpu=$(ps -p "$pid" -o %cpu= 2>/dev/null | tr -d ' ' | cut -d. -f1)
        [[ "${cpu:-0}" -gt 15 ]] && { kill "$pid" 2>/dev/null && ((killed++)); }
    done
    [[ $killed -gt 0 ]] && log "QOS" "Killed $killed bandwidth hogs"

    # Fresh bandwidth measurement on stream start (background)
    if [[ -x "$JELLYFIN_TV_DIR/scripts/bandwidth-measure.sh" ]]; then
        ( sleep 3; bash "$JELLYFIN_TV_DIR/scripts/bandwidth-measure.sh" ) &
    elif [[ -x "$HOME/bin/measure-streaming-bw.sh" ]]; then
        ( sleep 3; "$HOME/bin/measure-streaming-bw.sh" qos-fresh ) &
    fi

    echo "$(date +%s)" > "$QOS_STATE_FILE"
    log "QOS" "Streaming mode ACTIVE"
}

media_qos_disable() {
    log "QOS" "=== DISABLING STREAMING MODE ==="

    # SIGCONT go2rtc (resume camera relay)
    sudo killall -CONT go2rtc 2>/dev/null && log "QOS" "go2rtc SIGCONT sent (resumed)"

    # Notify Azure VM to disable peer-aware tc shaping
    ssh -o ConnectTimeout=3 -o BatchMode=yes -o StrictHostKeyChecking=no relay-host.local \
        "\$HOME/bin/jellyfin-qos.sh disable" >>/tmp/azure-qos-call.log 2>&1 &
    log "QOS" "Azure VM QoS disable requested (background)"

    # tc teardown removed: no local tc rules to clean up (see media_qos_enable)

    # Restore go2rtc priority
    for pid in $(pgrep -f go2rtc 2>/dev/null); do
        sudo renice 0 -p $pid 2>/dev/null
    done

    # Restore Chromium priority
    for pid in $(pgrep -f chromium 2>/dev/null | head -10); do
        sudo renice 0 -p $pid 2>/dev/null
    done

    # Restore Kodi priority
    for pid in $(pgrep -f kodi 2>/dev/null); do
        sudo renice 0 -p $pid 2>/dev/null
    done

    # Restore media player priority
    for pid in $(pgrep -x vlc 2>/dev/null) $(pgrep -x ffplay 2>/dev/null) $(pgrep -x mpv 2>/dev/null); do
        sudo renice 0 -p $pid 2>/dev/null
    done

    rm -f "$QOS_STATE_FILE"
    log "QOS" "Normal operation resumed"
}

media_qos_controller() {
    local now_qos
    now_qos=$(date +%s)

    if is_media_playing; then
        if [[ ! -f "$QOS_STATE_FILE" ]]; then
            media_qos_enable
        else
            # Re-enforce priorities every cycle (processes may respawn)
            for pid in $(pgrep -x vlc 2>/dev/null) $(pgrep -x ffplay 2>/dev/null) $(pgrep -x mpv 2>/dev/null); do
                local cur_nice
                cur_nice=$(ps -o nice= -p $pid 2>/dev/null | tr -d ' ')
                [[ -n "$cur_nice" && "$cur_nice" != "-15" ]] && sudo renice -15 -p $pid 2>/dev/null
            done
            for pid in $(pgrep -f go2rtc 2>/dev/null); do
                sudo renice 19 -p $pid 2>/dev/null
            done
        fi
        echo "$now_qos" > "$QOS_STATE_FILE"

        # Keep screen alive during media playback
        wlopm --on $(wlopm 2>/dev/null | awk "{print \$1}" | head -1) 2>/dev/null
        # Simulate activity to prevent flex-launcher screensaver (300s idle)
        if command -v wlrctl >/dev/null 2>&1; then
            wlrctl pointer move 0 0 2>/dev/null
        elif [[ -e /dev/uinput ]]; then
            python3 -c "from evdev import UInput, ecodes; u=UInput(); u.syn(); u.close()" 2>/dev/null || true
        fi
    else
        if [[ -f "$QOS_STATE_FILE" ]]; then
            local last_active
            last_active=$(cat "$QOS_STATE_FILE" 2>/dev/null || echo 0)
            local elapsed=$((now_qos - last_active))
            if [[ "$elapsed" -gt "$QOS_GRACE_PERIOD" ]]; then
                media_qos_disable
            fi
        fi
    fi
}
