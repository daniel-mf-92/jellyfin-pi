#!/bin/bash
# =============================================================================
# @section jellyfin-ram-buffer
# @frequency L0 (every 2 min)
# @description Download movies to tmpfs for instant local playback + rewind
# =============================================================================

BUFFER_MIN_FREE_RAM_MB=2048
BUFFER_MAX_SINGLE_GB=6

jellyfin_buffer_cleanup() {
    local avail_ram_mb
    avail_ram_mb=$(free -m | awk '/^Mem:/{print $7}')
    if [[ "${avail_ram_mb:-0}" -lt "$BUFFER_MIN_FREE_RAM_MB" ]]; then
        log "BUFFER" "RAM pressure: ${avail_ram_mb}MB free < ${BUFFER_MIN_FREE_RAM_MB}MB threshold"
        local current_id
        current_id=$(cat "$BUFFER_DIR/.current_id" 2>/dev/null || echo "")
        for old_file in $(ls -t "$BUFFER_DIR"/*.mkv 2>/dev/null | tail -n +2); do
            local old_basename
            old_basename=$(basename "$old_file" .mkv)
            if [[ "$old_basename" != "$current_id" ]]; then
                local old_size_mb=$(( $(stat -c%s "$old_file" 2>/dev/null || echo 0) / 1048576 ))
                rm -f "$old_file"
                log "BUFFER" "Evicted ${old_basename} (${old_size_mb}MB) — RAM pressure"
                avail_ram_mb=$(free -m | awk '/^Mem:/{print $7}')
                [[ "${avail_ram_mb:-0}" -ge "$BUFFER_MIN_FREE_RAM_MB" ]] && break
            fi
        done
    fi
}

jellyfin_buffer_download() {
    local cached_id="$1"
    local buffer_file="$BUFFER_DIR/${cached_id}.mkv"

    local video_br=500000
    local audio_br=128000
    if [[ -f "$BW_FILE" ]]; then
        video_br=$(python3 -c "import json; d=json.load(open('$BW_FILE')); print(d.get('video_bitrate', 500000))" 2>/dev/null || echo 500000)
        audio_br=$(python3 -c "import json; d=json.load(open('$BW_FILE')); print(d.get('audio_bitrate', 128000))" 2>/dev/null || echo 128000)
    fi
    [[ "$video_br" -lt 150000 ]] 2>/dev/null && video_br=150000

    if ! pgrep -f "curl.*${cached_id}" >/dev/null 2>&1; then
        local file_size
        file_size=$(stat -c%s "$buffer_file" 2>/dev/null || echo 0)

        local expected_size=0
        local item_info
        item_info=$(curl -sf --max-time 5 "${JELLYFIN_API}/Items/${cached_id}?Fields=MediaSources&api_key=${JELLYFIN_API_KEY}" 2>/dev/null)
        if [[ -n "$item_info" ]]; then
            expected_size=$(echo "$item_info" | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
    ticks = d.get('RunTimeTicks', 0)
    dur_secs = ticks / 10000000 if ticks else 7200
    est = int(($video_br + $audio_br) * dur_secs / 8)
    print(est)
except: print(0)
" 2>/dev/null || echo 0)
        fi

        if [[ "$file_size" -eq 0 ]] || { [[ "$expected_size" -gt 0 ]] && [[ "$file_size" -lt $(( expected_size * 90 / 100 )) ]]; }; then
            local avail_ram_mb
            avail_ram_mb=$(free -m | awk '/^Mem:/{print $7}')
            if [[ "${avail_ram_mb:-0}" -gt "$BUFFER_MIN_FREE_RAM_MB" ]]; then
                # Build transcode URL with subtitles if available
                local sub_params=""
                if [[ -n "$item_info" ]]; then
                    local has_subs
                    has_subs=$(echo "$item_info" | python3 -c "
import json,sys
try:
    d=json.load(sys.stdin)
    for ms in d.get('MediaSources',[]):
        for s in ms.get('MediaStreams',[]):
            if s.get('Type')=='Subtitle':
                print(s.get('Index','')); break
        break
except: pass
" 2>/dev/null)
                    [[ -n "$has_subs" ]] && sub_params="&SubtitleStreamIndex=${has_subs}&SubtitleMethod=Encode"
                fi

                local stream_url="${JELLYFIN_API}/Videos/${cached_id}/stream.mkv?Static=false&VideoCodec=h264&AudioCodec=aac&MaxVideoBitDepth=8&VideoBitRate=${video_br}&AudioBitRate=${audio_br}&MaxWidth=1280&MaxHeight=720${sub_params}&api_key=${JELLYFIN_API_KEY}"

                if [[ "$file_size" -gt 0 ]]; then
                    nohup curl -s -C "$file_size" -o "$buffer_file" --max-time 7200 "$stream_url" > /tmp/jellyfin-dl.log 2>&1 &
                    log "BUFFER" "Resumed download for $cached_id from $(( file_size / 1048576 ))MB (target ~$(( expected_size / 1048576 ))MB) at ${video_br}bps"
                else
                    nohup curl -s -o "$buffer_file" --max-time 7200 "$stream_url" > /tmp/jellyfin-dl.log 2>&1 &
                    log "BUFFER" "Started download for $cached_id (target ~$(( expected_size / 1048576 ))MB) at ${video_br}bps"
                fi
            else
                log "BUFFER" "Skipping download — RAM too low (${avail_ram_mb}MB free)"
            fi
        else
            log "BUFFER" "Cache complete: ${cached_id} at $(( file_size / 1048576 ))MB"
        fi
    else
        local file_size
        file_size=$(stat -c%s "$buffer_file" 2>/dev/null || echo 0)
        log "BUFFER" "Download in progress: ${cached_id} at $(( file_size / 1048576 ))MB"
    fi
}

jellyfin_buffer_autoplay() {
    if [[ -f "$BUFFER_DIR/.play_when_ready" ]]; then
        local play_id
        play_id=$(cat "$BUFFER_DIR/.play_when_ready" 2>/dev/null)
        local play_file="$BUFFER_DIR/${play_id}.mkv"
        local play_size
        play_size=$(stat -c%s "$play_file" 2>/dev/null || echo 0)

        if [[ "$play_size" -gt 104857600 ]] && ! pgrep -x vlc >/dev/null 2>&1 && ! pgrep -x mpv >/dev/null 2>&1; then
            nohup mpv \
                --fullscreen \
                --cache=yes \
                --demuxer-max-bytes=500M \
                --demuxer-readahead-secs=300 \
                --cache-secs=600 \
                --vo=gpu \
                --gpu-api=opengl \
                --gpu-dumb-mode=yes \
                --hwdec=drm-copy \
                --opengl-glfinish=yes \
                --log-file=/tmp/mpv-buffer-play.log \
                "$play_file" >/dev/null 2>&1 &
            disown
            rm -f "$BUFFER_DIR/.play_when_ready"
            log "BUFFER" "Auto-playing ${play_id} from buffer (${play_size} bytes)"
        fi
    fi
}

jellyfin_buffer_run() {
    mkdir -p "$BUFFER_DIR"
    jellyfin_buffer_cleanup

    if [[ -f "$BUFFER_DIR/.current_id" ]]; then
        local cached_id
        cached_id=$(cat "$BUFFER_DIR/.current_id")
        jellyfin_buffer_download "$cached_id"
        jellyfin_buffer_autoplay
    fi
}
