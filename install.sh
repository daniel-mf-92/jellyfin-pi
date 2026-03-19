#!/usr/bin/env bash
set -euo pipefail

# install.sh -- Install jellyfin-pi configs, scripts, and services
#
# Copies all configuration files to the correct locations on the Pi,
# creates the unified-controller systemd user service, and enables it.
#
# Usage:
#   ./install.sh            Install configs and services only
#   ./install.sh --build    Also build JMP from source first
#
# Safe to run multiple times (idempotent).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HOME_DIR="$HOME"
BIN_DIR="$HOME_DIR/bin"
CONFIG_DIR="$HOME_DIR/.config"
LOCAL_SHARE="$HOME_DIR/.local/share"

BUILD=false

for arg in "$@"; do
    case "$arg" in
        --build) BUILD=true ;;
        -h|--help)
            echo "Usage: $0 [--build]"
            echo ""
            echo "  --build    Build JMP from source before installing configs"
            echo ""
            echo "Install all jellyfin-pi configs, scripts, and systemd services."
            echo "Safe to run multiple times."
            exit 0
            ;;
        *) echo "Unknown option: $arg"; exit 1 ;;
    esac
done

# --- Colors for output ---
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

step() {
    echo -e "${GREEN}[*]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[!]${NC} $1"
}

# --- Build JMP if requested ---
if [ "$BUILD" = true ]; then
    step "Building Jellyfin Media Player from source..."
    if [ "$(id -u)" -eq 0 ]; then
        "$SCRIPT_DIR/build-arm64.sh"
    else
        sudo "$SCRIPT_DIR/build-arm64.sh"
    fi
    echo ""
fi

# --- Create directories ---
step "Creating directories..."
mkdir -p "$BIN_DIR"
mkdir -p "$CONFIG_DIR/labwc"
mkdir -p "$CONFIG_DIR/flex-launcher"
mkdir -p "$CONFIG_DIR/mpv"
mkdir -p "$CONFIG_DIR/systemd/user"
mkdir -p "$LOCAL_SHARE/jellyfinmediaplayer/inputmaps"

# --- Install scripts to ~/bin ---
step "Installing scripts to $BIN_DIR..."

install_script() {
    local src="$1"
    local dst="$2"
    cp "$src" "$dst"
    chmod +x "$dst"
}

install_script "$SCRIPT_DIR/unified-controller.py"     "$BIN_DIR/unified-controller.py"
install_script "$SCRIPT_DIR/go-home.sh"                "$BIN_DIR/go-home.sh"
install_script "$SCRIPT_DIR/launch-jmp.sh"             "$BIN_DIR/launch-jmp.sh"
install_script "$SCRIPT_DIR/show-jellyfin.sh"          "$BIN_DIR/show-jellyfin.sh"
install_script "$SCRIPT_DIR/fix-hdmi-audio.sh"         "$BIN_DIR/fix-hdmi-audio.sh"
install_script "$SCRIPT_DIR/jmp-ctl.py"                "$BIN_DIR/jmp-ctl.py"
install_script "$SCRIPT_DIR/jmp-ctl-launch.sh"         "$BIN_DIR/jmp-ctl-launch.sh"
install_script "$SCRIPT_DIR/cec-keyboard-bridge.py"    "$BIN_DIR/cec-keyboard-bridge.py"

echo "  Installed 8 scripts to $BIN_DIR"

# --- Install labwc autostart ---
step "Installing labwc autostart to $CONFIG_DIR/labwc/autostart..."
cp "$SCRIPT_DIR/labwc-autostart" "$CONFIG_DIR/labwc/autostart"
chmod +x "$CONFIG_DIR/labwc/autostart"

# --- Install flex-launcher config ---
step "Installing flex-launcher config to $CONFIG_DIR/flex-launcher/config.ini..."
cp "$SCRIPT_DIR/flex-launcher-config.ini" "$CONFIG_DIR/flex-launcher/config.ini"

# --- Install mpv config ---
step "Installing mpv config to $CONFIG_DIR/mpv/mpv.conf..."
cp "$SCRIPT_DIR/mpv.conf" "$CONFIG_DIR/mpv/mpv.conf"

# --- Install JMP configs ---
step "Installing JMP configs to $LOCAL_SHARE/jellyfinmediaplayer/..."
cp "$SCRIPT_DIR/jellyfinmediaplayer.conf" "$LOCAL_SHARE/jellyfinmediaplayer/jellyfinmediaplayer.conf"
cp "$SCRIPT_DIR/jmp-mpv.conf" "$LOCAL_SHARE/jellyfinmediaplayer/mpv.conf"
cp "$SCRIPT_DIR/switch-pro-virtual.json" "$LOCAL_SHARE/jellyfinmediaplayer/inputmaps/switch-pro-virtual.json"
echo "  Installed jellyfinmediaplayer.conf, mpv.conf, switch-pro-virtual.json"

# --- Create unified-controller systemd service ---
step "Creating unified-controller systemd user service..."

cat > "$CONFIG_DIR/systemd/user/unified-controller.service" << 'EOF'
[Unit]
Description=Unified Switch Pro Controller Daemon
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
ExecStart=%h/bin/unified-controller.py
Restart=always
RestartSec=5
Environment=XDG_RUNTIME_DIR=/run/user/%U
Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/%U/bus
Environment=WAYLAND_DISPLAY=wayland-0

[Install]
WantedBy=default.target
EOF

echo "  Created $CONFIG_DIR/systemd/user/unified-controller.service"

# --- Enable the service ---
step "Enabling unified-controller service..."
systemctl --user daemon-reload
systemctl --user enable unified-controller.service

if systemctl --user is-active unified-controller.service > /dev/null 2>&1; then
    warn "unified-controller is already running. Restarting..."
    systemctl --user restart unified-controller.service
else
    step "Starting unified-controller service..."
    systemctl --user start unified-controller.service || warn "Could not start service (controller may not be connected)"
fi

# --- Verify ---
step "Verifying installation..."
MISSING=""

check_file() {
    if [ ! -f "$1" ]; then
        MISSING="$MISSING\n  $1"
    fi
}

check_file "$BIN_DIR/unified-controller.py"
check_file "$BIN_DIR/go-home.sh"
check_file "$BIN_DIR/show-jellyfin.sh"
check_file "$BIN_DIR/fix-hdmi-audio.sh"
check_file "$BIN_DIR/jmp-ctl.py"
check_file "$CONFIG_DIR/labwc/autostart"
check_file "$CONFIG_DIR/flex-launcher/config.ini"
check_file "$CONFIG_DIR/mpv/mpv.conf"
check_file "$LOCAL_SHARE/jellyfinmediaplayer/jellyfinmediaplayer.conf"
check_file "$LOCAL_SHARE/jellyfinmediaplayer/mpv.conf"
check_file "$LOCAL_SHARE/jellyfinmediaplayer/inputmaps/switch-pro-virtual.json"
check_file "$CONFIG_DIR/systemd/user/unified-controller.service"

if [ -n "$MISSING" ]; then
    warn "Missing files:$MISSING"
    exit 1
fi

# --- Check for python3-evdev ---
if ! python3 -c "import evdev" 2>/dev/null; then
    warn "python3-evdev is not installed. The controller daemon requires it."
    echo "  Install with: sudo apt install python3-evdev"
fi

# --- Check for JMP binary ---
if ! command -v jellyfinmediaplayer > /dev/null 2>&1; then
    warn "jellyfinmediaplayer binary not found in PATH."
    echo "  Build it with: sudo ./build-arm64.sh"
    echo "  Or run: ./install.sh --build"
fi

echo ""
echo "============================================"
echo "  INSTALL COMPLETE"
echo ""
echo "  Scripts:    $BIN_DIR/"
echo "  labwc:      $CONFIG_DIR/labwc/autostart"
echo "  flex:       $CONFIG_DIR/flex-launcher/config.ini"
echo "  mpv:        $CONFIG_DIR/mpv/mpv.conf"
echo "  JMP:        $LOCAL_SHARE/jellyfinmediaplayer/"
echo "  Service:    unified-controller.service (enabled)"
echo ""
echo "  Next steps:"
echo "    1. Pair your Switch Pro Controller via bluetoothctl"
echo "    2. Start labwc: labwc -s ~/.config/labwc/autostart"
echo "    3. Edit flex-launcher config if paths differ"
echo "    4. Edit jellyfinmediaplayer.conf to set your server URL"
echo "============================================"
