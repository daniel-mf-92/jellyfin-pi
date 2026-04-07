#!/bin/bash
# =============================================================================
# @section streaming-health
# @frequency L0 (every 2 min)
# @description JMP stall detection, mpv fallback, bitrate adaptation, dual-stream
# =============================================================================

STREAMING_CB="streaming-heal"
STREAMING_MAX_ADJUST=5
JMP_LOG="$HOME/.local/share/jellyfinmediaplayer/logs/mediaplayer.log"
MPV_LOG="/tmp/mpv-jellyfin.log"

JELLYFIN_API_HOST="${JELLYFIN_API#http://}"
JELLYFIN_API_HOST="${JELLYFIN_API_HOST#https://}"
JELLYFIN_API_HOST="${JELLYFIN_API_HOST%%/*}"

is_jellyfin_mpv_running() {
    [[ -n "$JELLYFIN_API_HOST" ]] && pgrep -f "mpv.*${JELLYFIN_API_HOST}" >/dev/null 2>&1 && return 0
    pgrep -f "mpv.*/tmp/jellyfin-buffer" >/dev/null 2>&1
}

kill_jellyfin_mpv_processes() {
    [[ -n "$JELLYFIN_API_HOST" ]] && pkill -f "mpv.*${JELLYFIN_API_HOST}" 2>/dev/null
    pkill -f "mpv.*/tmp/jellyfin-buffer" 2>/dev/null
}

get_jellyfin_playing_item() {
    curl -sf "${JELLYFIN_API}/Sessions?api_key=${JELLYFIN_API_KEY}" 2>/dev/null | python3 -c '
import sys, json
try:
    sessions = json.load(sys.stdin)
    for s in sessions:
        npi = s.get("NowPlayingItem")
        if npi:
            print(npi.get("Id", ""))
            break
except: pass
' 2>/dev/null
}

launch_mpv_jellyfin() {
    local item_id="$1"
    local video_br="${2:-2000000}"
    local audio_br="${3:-128000}"

    if [[ -z "$item_id" ]] || [[ -z "$video_br" ]]; then
        log "STREAM" "Cannot launch mpv: missing item_id or video_br"
        return 1
    fi

    # Adaptive resolution based on video bitrate
    local max_w=1280 max_h=720
    if [[ "$video_br" -lt 400000 ]] 2>/dev/null; then
        max_w=640; max_h=360
    elif [[ "$video_br" -lt 700000 ]] 2>/dev/null; then
        max_w=854; max_h=480
    fi

    # Adaptive audio: low bitrate = 64kbps audio
    [[ "$video_br" -lt 300000 ]] 2>/dev/null && audio_br=64000

    # Check for local buffer file first (> 50MB)
    local local_buffer="/tmp/jellyfin-buffer/${item_id}.mkv"
    local source_url=""
    if [[ -f "$local_buffer" ]]; then
        local buf_size
        buf_size=$(stat -c%s "$local_buffer" 2>/dev/null || echo 0)
        if [[ "$buf_size" -gt 52428800 ]]; then
            source_url="$local_buffer"
            log "STREAM" "Playing from local buffer: ${local_buffer} ($(( buf_size / 1048576 ))MB)"
        fi
    fi

    if [[ -z "$source_url" ]]; then
        source_url="${JELLYFIN_API}/Videos/${item_id}/stream.mkv?Static=false&VideoCodec=h264&AudioCodec=aac&MaxVideoBitDepth=8&VideoBitRate=${video_br}&AudioBitRate=${audio_br}&MaxWidth=${max_w}&MaxHeight=${max_h}&api_key=${JELLYFIN_API_KEY}"
    fi

    # Adaptive cache-pause-wait: lower bitrate = longer initial buffer
    local cache_wait
    cache_wait=$(python3 -c "print(max(30, min(120, int($video_br / 5000))))" 2>/dev/null || echo 60)

    > "$MPV_LOG"

    nohup mpv \
        --fullscreen \
        --cache=yes \
        --demuxer-max-bytes=100M \
        --cache-secs=60 \
        --cache-pause-wait="$cache_wait" \
        --vo=gpu \
        --gpu-api=opengl \
        --gpu-dumb-mode=yes \
        --hwdec=drm-copy \
        --opengl-glfinish=yes \
        --log-file="$MPV_LOG" \
        "$source_url" >/dev/null 2>&1 &
    disown

    echo "$video_br" > "$STATE_DIR/mpv-launched-bitrate"
    log "STREAM" "Launched mpv for item $item_id at ${video_br}bps video / ${audio_br}bps audio (${max_w}x${max_h}, cache-wait=${cache_wait}s)"
}

read_current_bitrates() {
    VIDEO_BR_CURRENT=2000000
    AUDIO_BR_CURRENT=128000
    if [[ -f "$BW_FILE" ]]; then
        VIDEO_BR_CURRENT=$(python3 -c "import json; d=json.load(open('$BW_FILE')); print(d.get('video_bitrate', 2000000))" 2>/dev/null || echo 2000000)
        AUDIO_BR_CURRENT=$(python3 -c "import json; d=json.load(open('$BW_FILE')); print(d.get('audio_bitrate', 128000))" 2>/dev/null || echo 128000)
    fi
}

streaming_dual_stream_kill() {
    if pgrep -x vlc >/dev/null 2>&1 && pgrep -x jellyfinmediaplayer >/dev/null 2>&1; then
        local dual_sessions
        dual_sessions=$(curl -sf "${JELLYFIN_API}/Sessions?api_key=${JELLYFIN_API_KEY}" 2>/dev/null | python3 -c '
import sys, json
try:
    sessions = json.load(sys.stdin)
    playing = [s for s in sessions if s.get("NowPlayingItem")]
    if len(playing) > 1:
        for s in playing:
            if s.get("Client") != "VLC Bridge":
                print(s.get("Id",""))
except: pass
' 2>/dev/null)

        if [[ -n "$dual_sessions" ]]; then
            log "STREAM" "DUAL-STREAM DETECTED — stopping JMP server sessions"
            for sid in $dual_sessions; do
                curl -sf -X POST "${JELLYFIN_API}/Sessions/$sid/Playing/Stop" \
                    -H "X-Emby-Authorization: MediaBrowser Token=\"$JELLYFIN_API_KEY\"" 2>/dev/null
                log "STREAM" "Stopped JMP session: $sid"
            done
        fi
    fi
}

streaming_jmp_stall_check() {
    # Skip if VLC is managing playback
    if pgrep -x vlc >/dev/null 2>&1; then
        return 0
    fi

    if pgrep -x jellyfinmediaplayer >/dev/null 2>&1 && [[ -f "$JMP_LOG" ]]; then
        local stall_count
        stall_count=$(tail -200 "$JMP_LOG" 2>/dev/null | grep -c "bufferStalledError" || echo 0)

        if [[ "$stall_count" -gt 0 ]]; then
            log "STREAM" "JMP detected $stall_count bufferStalledError(s). Switching to mpv..."

            if check_circuit_breaker "$STREAMING_CB" "$STREAMING_MAX_ADJUST"; then
                local playing_item
                playing_item=$(get_jellyfin_playing_item)
                pkill -x jellyfinmediaplayer 2>/dev/null
                sleep 2

                if [[ -n "$playing_item" ]]; then
                    read_current_bitrates
                    launch_mpv_jellyfin "$playing_item" "$VIDEO_BR_CURRENT" "$AUDIO_BR_CURRENT"
                    record_restart "$STREAMING_CB"
                else
                    log "STREAM" "No currently playing item found. Cannot resume in mpv."
                fi
            fi
        fi
    fi
}

streaming_mpv_monitor() {
    if ! is_jellyfin_mpv_running; then
        return 0
    fi

    [[ ! -f "$MPV_LOG" ]] && return 0

    # --- Hysteresis: read last bitrate-change timestamp ---
    local last_change_epoch=0 now_epoch elapsed_since_change
    now_epoch=$(date +%s)
    if [[ -f "$STATE_DIR/bitrate-last-change" ]]; then
        last_change_epoch=$(cat "$STATE_DIR/bitrate-last-change" 2>/dev/null || echo 0)
    fi
    elapsed_since_change=$(( now_epoch - last_change_epoch ))

    local buffering_count
    buffering_count=$(tail -100 "$MPV_LOG" 2>/dev/null | awk '/(Buffering)/{c++} END{print c+0}')

    if [[ "$buffering_count" -gt 3 ]]; then
        # Downscale hold: require 2 min since last bitrate change
        if [[ "$elapsed_since_change" -lt 120 ]]; then
            log "STREAM" "mpv buffering ($buffering_count) but hold-time not met (${elapsed_since_change}s < 120s). Skipping downscale."
        elif check_circuit_breaker "$STREAMING_CB" "$STREAMING_MAX_ADJUST"; then
            log "STREAM" "mpv excessive buffering ($buffering_count occurrences). Reducing bitrate..."

            local playing_item
            playing_item=$(get_jellyfin_playing_item)
            read_current_bitrates
            local new_video_br
            new_video_br=$(python3 -c "print(max(150000, int($VIDEO_BR_CURRENT * 0.60)))" 2>/dev/null || echo 150000)

            local new_audio_br=128000
            [[ "$new_video_br" -lt 300000 ]] 2>/dev/null && new_audio_br=64000

            local new_max_w=1280 new_max_h=720
            if [[ "$new_video_br" -lt 400000 ]] 2>/dev/null; then
                new_max_w=640; new_max_h=360
            elif [[ "$new_video_br" -lt 700000 ]] 2>/dev/null; then
                new_max_w=854; new_max_h=480
            fi

            # Update bandwidth file
            if [[ -f "$BW_FILE" ]]; then
                python3 -c "
import json
with open('$BW_FILE', 'r') as f:
    d = json.load(f)
d['video_bitrate'] = $new_video_br
d['audio_bitrate'] = $new_audio_br
d['max_width'] = $new_max_w
d['max_height'] = $new_max_h
d['adjusted_by'] = 'streaming-heal'
d['adjusted_at'] = '$(date -u +%Y-%m-%dT%H:%M:%SZ)'
with open('$BW_FILE', 'w') as f:
    json.dump(d, f, indent=2)
" 2>/dev/null
            fi

            log "STREAM" "Reduced video bitrate: ${VIDEO_BR_CURRENT} -> ${new_video_br} (audio: ${new_audio_br}, res: ${new_max_w}x${new_max_h})"

            > "$MPV_LOG"
            kill_jellyfin_mpv_processes
            sleep 2

            # Record change timestamp and reset upscale counter
            echo "$now_epoch" > "$STATE_DIR/bitrate-last-change"
            echo "0" > "$STATE_DIR/upscale-good-count"

            if [[ -n "$playing_item" ]]; then
                launch_mpv_jellyfin "$playing_item" "$new_video_br" "$new_audio_br"
                record_restart "$STREAMING_CB"
            else
                log "STREAM" "No playing item found. mpv killed but not relaunched."
            fi
        fi

    # Auto-upscale when bandwidth improves (with hysteresis)
    elif [[ "$buffering_count" -eq 0 ]] && [[ -f "$STATE_DIR/mpv-launched-bitrate" ]]; then
        local launched_br
        launched_br=$(cat "$STATE_DIR/mpv-launched-bitrate" 2>/dev/null || echo 0)
        read_current_bitrates

        # Threshold raised to 2.0x (was 1.5x) — only upscale when bandwidth genuinely doubles
        local meets_threshold
        meets_threshold=$(python3 -c "
lb = int('$launched_br' or 0)
cb = int('$VIDEO_BR_CURRENT' or 0)
print('yes' if lb > 0 and cb > lb * 2.0 else 'no')
" 2>/dev/null || echo "no")

        if [[ "$meets_threshold" == "yes" ]]; then
            # Require 3 consecutive good readings before upscaling
            local good_count=0
            if [[ -f "$STATE_DIR/upscale-good-count" ]]; then
                good_count=$(cat "$STATE_DIR/upscale-good-count" 2>/dev/null || echo 0)
            fi
            good_count=$(( good_count + 1 ))
            echo "$good_count" > "$STATE_DIR/upscale-good-count"

            if [[ "$good_count" -ge 3 ]] && [[ "$elapsed_since_change" -ge 300 ]]; then
                local playing_item
                playing_item=$(get_jellyfin_playing_item)
                if [[ -n "$playing_item" ]]; then
                    log "STREAM" "Bandwidth stable: launched at ${launched_br}, now ${VIDEO_BR_CURRENT} (${good_count} good readings, ${elapsed_since_change}s hold). Upscaling..."
                    > "$MPV_LOG"
                    kill_jellyfin_mpv_processes
                    sleep 2
                    launch_mpv_jellyfin "$playing_item" "$VIDEO_BR_CURRENT" "$AUDIO_BR_CURRENT"
                    # Record change timestamp and reset counter
                    echo "$now_epoch" > "$STATE_DIR/bitrate-last-change"
                    echo "0" > "$STATE_DIR/upscale-good-count"
                fi
            elif [[ "$good_count" -ge 3 ]]; then
                log "STREAM" "Upscale conditions met (${good_count} readings) but hold-time not met (${elapsed_since_change}s < 300s). Waiting..."
            else
                log "STREAM" "Upscale candidate: ${good_count}/3 consecutive good readings (bw ${VIDEO_BR_CURRENT} > 2x launched ${launched_br})"
            fi
        else
            # Conditions not met — reset consecutive counter
            if [[ -f "$STATE_DIR/upscale-good-count" ]]; then
                local prev_count
                prev_count=$(cat "$STATE_DIR/upscale-good-count" 2>/dev/null || echo 0)
                if [[ "$prev_count" -gt 0 ]]; then
                    log "STREAM" "Upscale good-count reset (was ${prev_count}, bandwidth no longer 2x threshold)"
                fi
            fi
            echo "0" > "$STATE_DIR/upscale-good-count"
        fi
    fi
}

streaming_health_run() {
    streaming_dual_stream_kill
    streaming_jmp_stall_check
    streaming_mpv_monitor
}
