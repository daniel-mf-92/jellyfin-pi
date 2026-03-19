#!/bin/bash
# =============================================================================
# fix-hdmi-audio.sh — Force HDMI audio jack detection on Raspberry Pi 5
# =============================================================================
# Problem: Pi 5's DRM/KMS driver sometimes fails to detect the HDMI audio jack
# at boot, especially on HDMI-A-2. This leaves PipeWire/WirePlumber with no
# HDMI sink, so all audio goes nowhere.
#
# Solution: Force DRM connector re-detection, then verify the ALSA jack state.
# If WirePlumber still has no HDMI sink, restart it. Finally, set the HDMI sink
# as default at 100% volume (the TV controls actual volume via CEC).
#
# Run via: labwc-autostart (at boot), master script, or systemd timer.
# Logs to: /tmp/hdmi-audio-heal.log
# =============================================================================
set -euo pipefail

LOG=/tmp/hdmi-audio-heal.log

# --- Step 1: Force DRM connector re-detection ---
# Writing "detect" to the DRM connector status sysfs node forces the kernel
# to re-probe HDMI hotplug. This wakes up audio on cold-boot scenarios.
for connector in /sys/class/drm/card?-HDMI-A-*/status; do
    if [ -e "$connector" ]; then
        echo detect | sudo tee "$connector" > /dev/null 2>&1
    fi
done
sleep 2

# --- Step 2: Check ALSA HDMI jack state ---
# card 1 = vc4hdmi1 (HDMI-A-2) on Pi 5. The jack must report "on" for audio.
JACK_STATUS=$(amixer -c 1 contents 2>/dev/null | grep -A1 'HDMI Jack' | grep 'values=' | awk -F= '{print $2}' || echo "unknown")
if [ "$JACK_STATUS" != "on" ]; then
    echo "$(date) WARN: HDMI Jack status='${JACK_STATUS}' after force-detect" >> "$LOG"
    exit 1
fi

# --- Step 3: Ensure WirePlumber has an HDMI sink ---
# If PipeWire/WirePlumber started before the jack was detected, it may have
# no HDMI sink registered. Restarting WirePlumber forces re-enumeration.
SINK_COUNT=$(pactl list sinks short 2>/dev/null | grep -c 'hdmi' || echo "0")
if [ "$SINK_COUNT" -eq 0 ]; then
    echo "$(date) HEAL: No HDMI sink found, restarting WirePlumber..." >> "$LOG"
    systemctl --user restart wireplumber
    sleep 3
fi

# --- Step 4: Set HDMI as default sink, unmute, 100% volume ---
# Pin volume at 100% — the TV handles actual volume. Mute state can persist
# across reboots so we explicitly unmute every time.
HDMI_SINK=$(pactl list sinks short 2>/dev/null | grep hdmi | awk '{print $2}' | head -1)
if [ -n "$HDMI_SINK" ]; then
    pactl set-default-sink "$HDMI_SINK" 2>/dev/null
    pactl set-sink-volume "$HDMI_SINK" 100% 2>/dev/null
    pactl set-sink-mute "$HDMI_SINK" 0 2>/dev/null
fi
