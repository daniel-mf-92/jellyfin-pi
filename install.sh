#!/usr/bin/env bash
set -euo pipefail

# Jellyfin TV - Install Script for Raspberry Pi 5
# Installs the Slint + Rust client as a systemd service

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_NAME="jellyfin-tv"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="$HOME/.config/jellyfin-tv"
SERVICE_DIR="$HOME/.config/systemd/user"

echo "=== Jellyfin TV Installer ==="

# 1. Find binary
BINARY=""
if [ -f "$SCRIPT_DIR/target/release/$BINARY_NAME" ]; then
    BINARY="$SCRIPT_DIR/target/release/$BINARY_NAME"
elif [ -f "$SCRIPT_DIR/target/debug/$BINARY_NAME" ]; then
    BINARY="$SCRIPT_DIR/target/debug/$BINARY_NAME"
else
    echo "Error: Binary not found. Run build-pi5.sh first."
    exit 1
fi

echo "Binary: $BINARY"

# 2. Install binary
echo "--- Installing binary to $INSTALL_DIR ---"
sudo cp "$BINARY" "$INSTALL_DIR/$BINARY_NAME"
sudo chmod +x "$INSTALL_DIR/$BINARY_NAME"

# 3. Create config directory and default config
echo "--- Setting up configuration ---"
mkdir -p "$CONFIG_DIR"

if [ ! -f "$CONFIG_DIR/config.toml" ]; then
    cat > "$CONFIG_DIR/config.toml" << 'TOML'
[server]
url = "https://jellyfin-macmini.oneclickwebsite.io"
device_name = "Jellyfin TV (Pi)"
client_name = "Jellyfin TV"
client_version = "1.0.0"

[playback]
hwdec = "v4l2m2m-copy"
vo = "gpu"
gpu_context = "drm"
audio_device = "alsa/default"
audio_delay_ms = -300.0
subtitle_size = 48
max_streaming_bitrate = 120000000
prefer_direct_play = true

[controller]
deadzone = 12000
repeat_delay_ms = 400
repeat_rate_ms = 150
idle_disconnect_min = 15

[ui]
screensaver_timeout_sec = 300
theme = "dark"
TOML
    echo "Default config created at $CONFIG_DIR/config.toml"
else
    echo "Config already exists, skipping."
fi

# 4. Create systemd user service
echo "--- Creating systemd service ---"
mkdir -p "$SERVICE_DIR"

cat > "$SERVICE_DIR/jellyfin-tv.service" << EOF
[Unit]
Description=Jellyfin TV Client (Slint + Rust)
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/$BINARY_NAME
Environment=RUST_LOG=info
Environment=SLINT_BACKEND=linuxkms
Environment=SLINT_FULLSCREEN=1
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF

# 5. Create DRM/KMS launcher (alternative to systemd, for direct boot)
cat > "$CONFIG_DIR/launch.sh" << 'LAUNCHER'
#!/usr/bin/env bash
# Launch Jellyfin TV directly on DRM/KMS (no desktop environment needed)
export RUST_LOG=info
export SLINT_BACKEND=linuxkms
export SLINT_FULLSCREEN=1

# Ensure we have GPU access
if [ ! -w /dev/dri/card1 ]; then
    echo "Warning: No write access to /dev/dri/card1"
    echo "Run: sudo usermod -a -G video,render $USER"
fi

exec /usr/local/bin/jellyfin-tv "$@"
LAUNCHER
chmod +x "$CONFIG_DIR/launch.sh"

# 6. Set up permissions
echo "--- Setting up permissions ---"
# Add user to video and render groups for DRM access
sudo usermod -a -G video,render,input "$USER" 2>/dev/null || true

# Create udev rule for controller access without root
sudo tee /etc/udev/rules.d/99-switch-pro.rules > /dev/null << 'UDEV'
# Nintendo Switch Pro Controller
SUBSYSTEM=="input", ATTRS{name}=="*Pro Controller*", MODE="0666"
SUBSYSTEM=="input", ATTRS{id/vendor}=="057e", MODE="0666"
UDEV
sudo udevadm control --reload-rules

# 7. Set up automation scripts
echo "--- Setting up automation scripts ---"
chmod +x "$SCRIPT_DIR"/scripts/*.sh "$SCRIPT_DIR"/scripts/lib/common.sh 2>/dev/null || true

# Symlink bandwidth-measure for standalone use
ln -sf "$SCRIPT_DIR/scripts/bandwidth-measure.sh" "$HOME/bin/measure-streaming-bw.sh" 2>/dev/null || true

# Ensure .env has JELLYFIN_API_KEY
if [ -f "$SCRIPT_DIR/.env" ]; then
    if ! grep -q "JELLYFIN_API_KEY" "$SCRIPT_DIR/.env"; then
        echo ""
        echo "WARNING: JELLYFIN_API_KEY not found in .env"
        echo "Add it: echo 'JELLYFIN_API_KEY=your-key' >> $SCRIPT_DIR/.env"
    fi
else
    echo ""
    echo "WARNING: No .env file found. Copy .env.example and fill in values:"
    echo "  cp $SCRIPT_DIR/.env.example $SCRIPT_DIR/.env"
fi

echo "Automation scripts ready. Add to master script:"
echo "  JELLYFIN_TV_DIR=\"\$HOME/jellyfin-tv\""
echo "  [ -d \"\$JELLYFIN_TV_DIR/scripts\" ] && source \"\$JELLYFIN_TV_DIR/scripts/jellyfin-cron.sh\""

# 8. Enable and start service
echo "--- Enabling service ---"
systemctl --user daemon-reload
systemctl --user enable jellyfin-tv.service

echo ""
echo "=== Installation complete ==="
echo ""
echo "Commands:"
echo "  Start:   systemctl --user start jellyfin-tv"
echo "  Stop:    systemctl --user stop jellyfin-tv"
echo "  Status:  systemctl --user status jellyfin-tv"
echo "  Logs:    journalctl --user -u jellyfin-tv -f"
echo "  Direct:  $CONFIG_DIR/launch.sh"
echo ""
echo "Config: $CONFIG_DIR/config.toml"
echo ""
echo "NOTE: Log out and back in for group changes to take effect."
