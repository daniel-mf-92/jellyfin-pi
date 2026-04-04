#!/bin/bash
# =============================================================================
# @section wireguard-bandwidth-measure
# @frequency L1 (every 5 min)
# @description WireGuard bandwidth test, writes /tmp/pi-home-wg-bandwidth.json
# =============================================================================

bandwidth_measure() {
    local bw_file="${BW_FILE:-/tmp/pi-home-wg-bandwidth.json}"
    local api="${JELLYFIN_API:-http://10.100.0.2:8096}"
    local api_key="${JELLYFIN_API_KEY:-}"

    # Bandwidth test: 3 samples of 512KB, use MINIMUM for reliability over flaky WiFi
    local bw_samples="" _s
    for _i in 1 2 3; do
        _s=$(curl -o /dev/null -w %{speed_download} --max-time 8 --range 0-524287 \
            "${api}/Videos/e6067924303046d641ce61f9f80e260d/stream?Static=true&api_key=${api_key}" 2>/dev/null)
        [[ -n "$_s" ]] && [[ "$_s" != "0" ]] && bw_samples="$bw_samples $_s"
    done
    local bw_bytes
    bw_bytes=$(echo "$bw_samples" | tr ' ' '
' | sort -n | head -1)

    if [[ -n "$bw_bytes" ]] && [[ "$bw_bytes" != "0" ]]; then
        local bw_bps
        bw_bps=$(python3 -c "
bw = float($bw_bytes)
total_bps = int(bw * 8)
video_bps = int(total_bps * 0.49)  # 65% base minus 25% safety margin
# Clamp between 200kbps and 8Mbps
video_bps = max(200000, min(8000000, video_bps))
# Round to nearest 50kbps for finer granularity at low bitrates
video_bps = (video_bps // 50000) * 50000
if video_bps < 200000: video_bps = 200000
print(video_bps)
" 2>/dev/null)

        # Adaptive audio: if video < 300kbps, use 64kbps audio; else 128kbps
        local audio_bps
        audio_bps=$(python3 -c "
vbr = int('${bw_bps:-2000000}')
print(64000 if vbr < 300000 else 128000)
" 2>/dev/null || echo 128000)

        # Fixed 720p - projector max resolution
        local max_w=1280 max_h=720

        cat > "$bw_file" << BWEOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "raw_bytes_per_sec": $bw_bytes,
  "total_bps": $(python3 -c "print(int(float($bw_bytes) * 8))" 2>/dev/null),
  "video_bitrate": ${bw_bps:-2000000},
  "audio_bitrate": $audio_bps,
  "max_width": $max_w,
  "max_height": $max_h
}
BWEOF
        log "BANDWIDTH" "WG speed: ${bw_bytes} B/s, video bitrate: ${bw_bps:-2000000} bps, audio: ${audio_bps} bps, res: ${max_w}x${max_h}"
    else
        log "BANDWIDTH" "Speed test failed, keeping previous config."
    fi
}

# Run directly if not being sourced
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    JELLYFIN_TV_DIR="${JELLYFIN_TV_DIR:-$(cd "$(dirname "$0")/.." && pwd)}"
    source "$JELLYFIN_TV_DIR/scripts/lib/common.sh"
    bandwidth_measure "$@"
fi
