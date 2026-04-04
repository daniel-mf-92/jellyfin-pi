#!/bin/bash
# measure-streaming-bw.sh — Quick bandwidth measurement for adaptive streaming
# Called by master script on QoS enable and periodically during playback.
# Writes /tmp/pi-home-wg-bandwidth.json with conservative bitrate targets.

BW_FILE="/tmp/pi-home-wg-bandwidth.json"
LOG_FILE="$HOME/logs/pi-home-master-script.log"
JELLYFIN_API="http://localhost:8096"
API_KEY="33bbc5c52aa74f07a8b7b07f5c89b37b"
# Use a known static file on Jellyfin for speed test
TEST_URL="${JELLYFIN_API}/Videos/e6067924303046d641ce61f9f80e260d/stream?Static=true&api_key=${API_KEY}"

log() {
    echo "$(date '+%Y-%m-%d %H:%M:%S') [ADAPTIVE-BW] $1" >> "$LOG_FILE"
}

# Download 2MB chunk and measure speed
bw_bytes=$(curl -o /dev/null -w '%{speed_download}' --max-time 10 --range 0-2097151 "$TEST_URL" 2>/dev/null)

if [ -z "$bw_bytes" ] || [ "$bw_bytes" = "0" ] || [ "$bw_bytes" = "0.000" ]; then
    log "Speed test failed, keeping previous config"
    exit 1
fi

# Calculate conservative bitrate targets
read video_bps audio_bps max_w max_h total_bps <<< $(python3 -c "
bw = float($bw_bytes)
total_bps = int(bw * 8)
# Use 55% of measured bandwidth for video (leave room for audio + TCP overhead + jitter)
video_bps = int(total_bps * 0.55)
video_bps = max(200000, min(8000000, video_bps))
video_bps = (video_bps // 50000) * 50000
if video_bps < 200000: video_bps = 200000

# Adaptive audio
audio_bps = 64000 if video_bps < 300000 else 128000

# Adaptive resolution
if video_bps < 400000:
    max_w, max_h = 640, 360
elif video_bps < 700000:
    max_w, max_h = 854, 480
else:
    max_w, max_h = 1280, 720

print(f'{video_bps} {audio_bps} {max_w} {max_h} {total_bps}')
" 2>/dev/null)

if [ -z "$video_bps" ]; then
    log "Python calc failed"
    exit 1
fi

# Read previous bitrate for comparison
prev_video=$(python3 -c "import json; print(json.load(open('$BW_FILE')).get('video_bitrate',0))" 2>/dev/null || echo 0)

# Write new config
cat > "$BW_FILE" << EOF
{
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "raw_bytes_per_sec": $bw_bytes,
  "total_bps": $total_bps,
  "video_bitrate": $video_bps,
  "audio_bitrate": $audio_bps,
  "max_width": $max_w,
  "max_height": $max_h,
  "source": "${1:-manual}"
}
EOF

log "Measured ${bw_bytes} B/s (${total_bps}bps total) -> video=${video_bps}bps audio=${audio_bps}bps ${max_w}x${max_h} (prev=${prev_video})"
