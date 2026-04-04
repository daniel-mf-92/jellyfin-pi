#!/bin/bash
# =============================================================================
# jellyfin-pi/scripts/jellyfin-cron.sh
# Entry point sourced by pi-home-a master script.
# Sources all JMP automation scripts and calls them in order.
#
# Usage from master script:
#   JELLYFIN_TV_DIR="$HOME/jellyfin-tv"
#   if [ -d "$JELLYFIN_TV_DIR/scripts" ]; then
#       source "$JELLYFIN_TV_DIR/scripts/jellyfin-cron.sh"
#   fi
#
# =============================================================================
# SECTION MANIFEST — Which master script sections this repo owns
# =============================================================================
# The @section tags below match tags in each script file. Any section listed
# here is REPO-MANAGED and should NOT also exist inline in the master script.
# Sections NOT listed here remain LOCAL to the master script.
#
# REPO-MANAGED (@section tags):
#   jmp-self-healing         — JMP crash/freeze detection + recovery
#   jellyfin-ram-buffer      — RAM buffer download, eviction, auto-play
#   media-streaming-qos      — SIGSTOP go2rtc, tc wg0, renice, Azure QoS
#   streaming-health         — JMP stall->mpv, bitrate adapt, dual-stream
#   audio-healing            — PipeWire/WirePlumber, HDMI sink, lip-sync
#   wireguard-bandwidth-measure — WG speed test, /tmp/pi-home-wg-bandwidth.json
#   jellyfin-tv-launch       — Ensure JMP running (L1)
#   flex-launcher-health     — Restart flex-launcher if dead
#
# LOCAL (stays in master script — sensitive or non-JMP):
#   go2rtc-self-healing      — Camera relay (has credentials in config path)
#   tapo-camera-ip-monitor   — Camera IP scan (has credentials in sed)
#   rtsp-congestion-monitor  — Camera RTSP send queue
#   tapo-recorder            — Camera recording service
#   doorbell-recorder        — Doorbell recording service
#   doorbell-motion-clipper  — Doorbell motion detection
#   wireguard-monitor        — WG tunnel infrastructure
#   bluetooth-controller     — BT adapter reset
#   pro-controller-gyro      — IMU device blocking
#   gui-self-healing         — pcmanfm, wf-panel-pi desktop
#   moonlight-stale-session  — Gaming session monitor
#   game-launcher-server     — Game launcher HTTP server
#   wifi-signal-monitor      — AX210 signal strength
#   hey-jarvis               — Voice assistant
#   led-night-mode           — LED dimming 22:00-06:00
#   ax210-power-save         — WiFi power save disable
#   health-endpoint          — JSON status file
#   log-rotation             — Log file trimming
# =============================================================================

JELLYFIN_TV_DIR="${JELLYFIN_TV_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

# --- Source all modules ---
source "$JELLYFIN_TV_DIR/scripts/lib/common.sh"
source "$JELLYFIN_TV_DIR/scripts/jmp-heal.sh"
source "$JELLYFIN_TV_DIR/scripts/jellyfin-buffer.sh"
source "$JELLYFIN_TV_DIR/scripts/media-qos.sh"
source "$JELLYFIN_TV_DIR/scripts/streaming-health.sh"
source "$JELLYFIN_TV_DIR/scripts/audio-heal.sh"
source "$JELLYFIN_TV_DIR/scripts/bandwidth-measure.sh"
source "$JELLYFIN_TV_DIR/scripts/jellyfin-launch.sh"
source "$JELLYFIN_TV_DIR/scripts/flex-launcher-heal.sh"

# --- L0: every 2 minutes ---
jmp_self_heal
jellyfin_buffer_run
streaming_health_run
audio_pipewire_heal
audio_hdmi_sink_heal
audio_lipsync_heal
flex_launcher_heal
media_qos_controller

# --- L1: every 5 minutes (gated by $RUN_5MIN from master script) ---
if ${RUN_5MIN:-false}; then
    bandwidth_measure
    jellyfin_ensure_running
fi
