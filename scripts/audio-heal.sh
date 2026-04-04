#!/bin/bash
# =============================================================================
# @section audio-healing
# @frequency L0 (every 2 min)
# @description PipeWire/WirePlumber self-healing, HDMI sink, mpv.conf lip-sync
# =============================================================================

CORRECT_AUDIO_DELAY="audio-delay=-0.3"

audio_pipewire_heal() {
    # Only run if labwc compositor is active
    pgrep -x labwc >/dev/null 2>&1 || return 0

    local pw_healthy=true
    local sink_output
    sink_output=$(timeout 3 bash -c "XDG_RUNTIME_DIR=/run/user/1000 pactl list sinks short 2>/dev/null")

    if [[ $? -ne 0 ]] || [[ -z "$sink_output" ]]; then
        pw_healthy=false
        log "PIPEWIRE" "PipeWire/pactl not responding. Restarting audio stack..."
        if check_circuit_breaker pipewire; then
            systemctl --user restart pipewire pipewire-pulse wireplumber
            record_restart pipewire
            sleep 3
            sink_output=$(timeout 3 bash -c "XDG_RUNTIME_DIR=/run/user/1000 pactl list sinks short 2>/dev/null")
            if [[ -n "$sink_output" ]]; then
                log "PIPEWIRE" "Audio stack restored."
                pw_healthy=true
            else
                log "PIPEWIRE" "Audio stack still broken after restart."
            fi
        fi
    fi

    # Ensure HDMI is default sink (not null/network sink)
    if $pw_healthy; then
        local hdmi_sink
        hdmi_sink=$(echo "$sink_output" | grep "hdmi" | awk "{print \$2}" | head -1)
        if [[ -n "$hdmi_sink" ]]; then
            local current_default
            current_default=$(XDG_RUNTIME_DIR=/run/user/1000 pactl get-default-sink 2>/dev/null)
            if [[ "$current_default" != "$hdmi_sink" ]]; then
                XDG_RUNTIME_DIR=/run/user/1000 pactl set-default-sink "$hdmi_sink"
                log "PIPEWIRE" "Set default sink to HDMI: $hdmi_sink (was: $current_default)"
            fi
            # Ensure volume is at 100%
            XDG_RUNTIME_DIR=/run/user/1000 pactl set-sink-volume "$hdmi_sink" 100% 2>/dev/null
        else
            log "PIPEWIRE" "No HDMI sink found in: $sink_output"
        fi
    fi
}

audio_hdmi_sink_heal() {
    # Only run if labwc compositor is active
    pgrep -x labwc >/dev/null 2>&1 || return 0

    export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/1000/bus"

    local hdmi_sink_id
    hdmi_sink_id=$(wpctl status 2>/dev/null | grep "Built-in Audio Digital Stereo (HDMI)" | grep -oP "^\s+\*?\s+\K\d+")

    if [[ -z "$hdmi_sink_id" ]]; then
        log "AUDIO" "HDMI sink missing. Restarting WirePlumber..."
        if check_circuit_breaker wireplumber; then
            systemctl --user restart wireplumber
            record_restart wireplumber
            sleep 3
            hdmi_sink_id=$(wpctl status 2>/dev/null | grep "Built-in Audio Digital Stereo (HDMI)" | grep -oP "^\s+\*?\s+\K\d+")
            if [[ -n "$hdmi_sink_id" ]]; then
                wpctl set-default "$hdmi_sink_id"
                log "AUDIO" "HDMI sink restored (id: $hdmi_sink_id) and set as default."
            else
                log "AUDIO" "HDMI sink still missing after WirePlumber restart."
            fi
        fi
    else
        local current_default
        current_default=$(wpctl status 2>/dev/null | grep -A5 "Sinks:" | grep "^\s\+\*" | grep -oP "\d+" | head -1)
        if [[ "$current_default" != "$hdmi_sink_id" ]]; then
            wpctl set-default "$hdmi_sink_id"
            log "AUDIO" "Reset default sink to HDMI (id: $hdmi_sink_id, was: $current_default)."
        fi
    fi
}

audio_lipsync_heal() {
    local mpv_conf_main="$HOME/.config/mpv/mpv.conf"
    local mpv_conf_jmp="$HOME/.local/share/jellyfinmediaplayer/mpv.conf"

    for conf_file in "$mpv_conf_main" "$mpv_conf_jmp"; do
        if [[ -f "$conf_file" ]]; then
            local current_val
            current_val=$(grep -oP "^audio-delay=.*" "$conf_file" 2>/dev/null)
            if [[ "$current_val" != "$CORRECT_AUDIO_DELAY" ]]; then
                if [[ -n "$current_val" ]]; then
                    sed -i "s|^audio-delay=.*|$CORRECT_AUDIO_DELAY|" "$conf_file"
                    log "LIPSYNC" "Fixed $conf_file: $current_val -> $CORRECT_AUDIO_DELAY"
                else
                    echo "$CORRECT_AUDIO_DELAY" >> "$conf_file"
                    log "LIPSYNC" "Added $CORRECT_AUDIO_DELAY to $conf_file"
                fi
            fi
        fi
    done
}
