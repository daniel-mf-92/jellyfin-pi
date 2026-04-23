#!/usr/bin/env bash
# Pi-Media-Player E2E visual test — takes screenshots, sends key events, verifies UI via OCR
# Run on Pi-home-a or via SSH from Mac Mini
set -euo pipefail

export WAYLAND_DISPLAY=wayland-0
export XDG_RUNTIME_DIR=/run/user/1000

SCREENSHOT_DIR="/tmp/jellyfin-e2e"
LOG="/tmp/jellyfin-e2e/test.log"
PASS=0
FAIL=0

mkdir -p "$SCREENSHOT_DIR"
echo "$(date) — Pi-Media-Player E2E test starting" > "$LOG"

screenshot() {
    local name="$1"
    local path="$SCREENSHOT_DIR/${name}.png"
    grim "$path" 2>/dev/null
    echo "$path"
}

ocr() {
    local img="$1"
    tesseract "$img" stdout 2>/dev/null || echo ""
}

send_key() {
    local key="$1"
    wtype -k "$key" 2>/dev/null
    sleep 0.5
}

focus_app_window() {
    wlrctl toplevel focus title:Pi-Media-Player 2>/dev/null || true
    wlrctl toplevel focus title:Jellyfin 2>/dev/null || true
    wlrctl toplevel focus app_id:pi-media-player 2>/dev/null || true
    sleep 1
}

check_text() {
    local img="$1"
    local expected="$2"
    local label="$3"
    local text
    text=$(ocr "$img")
    if echo "$text" | grep -qi "$expected"; then
        echo "PASS: $label — found '$expected'" | tee -a "$LOG"
        PASS=$((PASS + 1))
        return 0
    else
        echo "FAIL: $label — expected '$expected' not found in OCR output" | tee -a "$LOG"
        echo "  OCR text (first 200 chars): $(echo "$text" | head -c 200)" | tee -a "$LOG"
        FAIL=$((FAIL + 1))
        return 1
    fi
}

check_not_text() {
    local img="$1"
    local unexpected="$2"
    local label="$3"
    local text
    text=$(ocr "$img")
    if echo "$text" | grep -qi "$unexpected"; then
        echo "FAIL: $label — unexpected '$unexpected' found" | tee -a "$LOG"
        FAIL=$((FAIL + 1))
        return 1
    else
        echo "PASS: $label — '$unexpected' not present (good)" | tee -a "$LOG"
        PASS=$((PASS + 1))
        return 0
    fi
}

check_not_black() {
    local img="$1"
    local label="$2"
    # Check if image is mostly black (>95% dark pixels)
    local dark_pct
    dark_pct=$(convert "$img" -colorspace Gray -threshold 10% -format "%[fx:mean*100]" info: 2>/dev/null || echo "50")
    if (( $(echo "$dark_pct < 5" | bc -l 2>/dev/null || echo 0) )); then
        echo "FAIL: $label — screen is mostly black (${dark_pct}% bright)" | tee -a "$LOG"
        FAIL=$((FAIL + 1))
        return 1
    else
        echo "PASS: $label — screen has content (${dark_pct}% bright)" | tee -a "$LOG"
        PASS=$((PASS + 1))
        return 0
    fi
}

# ============================================================================
# TEST SUITE
# ============================================================================

echo ""
echo "=== Pi-Media-Player E2E Visual Test ==="
echo ""

is_app_running() {
    pgrep -x pi-media-player >/dev/null 2>&1 ||
    pgrep -x jellyfin-pi >/dev/null 2>&1 ||
    systemctl --user is-active --quiet pi-media-player.service
}

# Verify app is running
if ! is_app_running; then
    echo "FATAL: pi-media-player/jellyfin-pi not running" | tee -a "$LOG"
    systemctl --user status --no-pager pi-media-player.service 2>/dev/null | sed -n "1,8p" | tee -a "$LOG" || true
    exit 1
fi

# --- TEST 1: Home screen is visible ---
echo "--- Test 1: Home screen visible ---"
sleep 2
focus_app_window
IMG=$(screenshot "01-home")
check_not_black "$IMG" "Home screen not black"
# Check for expected home screen elements
check_text "$IMG" "Libraries\|Movies\|TV\|Shows\|Continue\|Latest\|Next Up\|Watching" "Home screen has library/content text" || true

# --- TEST 2: No stuck loading spinner ---
echo "--- Test 2: No stuck loading ---"
check_not_text "$IMG" "Loading" "No loading spinner on home screen" || true

# --- TEST 3: Navigate right (horizontal scroll test) ---
echo "--- Test 3: Horizontal navigation ---"
send_key "Right"
send_key "Right"
send_key "Right"
sleep 1
IMG=$(screenshot "02-scroll-right")
check_not_black "$IMG" "Screen after scrolling right"

# --- TEST 4: Navigate down to content rows ---
echo "--- Test 4: Vertical navigation ---"
send_key "Down"
send_key "Down"
sleep 1
IMG=$(screenshot "03-content-rows")
check_not_black "$IMG" "Content rows visible"

# --- TEST 5: Select item (A button = Enter) ---
echo "--- Test 5: Select item ---"
send_key "Return"
sleep 3
IMG=$(screenshot "04-after-select")
check_not_black "$IMG" "Screen after selecting item"
# Should be detail screen or library screen, NOT loading
check_not_text "$IMG" "Loading" "Not stuck on loading after select" || true
# Check for detail screen elements
check_text "$IMG" "Play\|Overview\|Cast\|Genre\|Similar\|Season\|Episode\|Sort\|Filter" "Detail or library content visible" || true

# --- TEST 6: Escape goes back ---
echo "--- Test 6: Escape goes back ---"
send_key "Escape"
sleep 2
IMG=$(screenshot "05-after-escape")
check_not_black "$IMG" "Screen after escape"
check_text "$IMG" "Libraries\|Movies\|TV\|Continue\|Latest\|Next Up" "Back on home screen" || true

# --- TEST 7: Navigate to first library card and select ---
echo "--- Test 7: Library card navigation ---"
# Go to top (home) first
send_key "Up"
send_key "Up"
send_key "Up"
send_key "Home" 2>/dev/null || true
sleep 1
send_key "Return"
sleep 3
IMG=$(screenshot "06-library-grid")
check_not_black "$IMG" "Library grid visible"
check_not_text "$IMG" "Loading" "Library not stuck loading" || true

# Go back
send_key "Escape"
sleep 2

# ============================================================================
# RESULTS
# ============================================================================

echo ""
echo "=== RESULTS ==="
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "Screenshots: $SCREENSHOT_DIR/"
echo "" | tee -a "$LOG"
echo "$(date) — E2E test complete: $PASS pass, $FAIL fail" >> "$LOG"

# Copy results summary
echo "{\"pass\": $PASS, \"fail\": $FAIL, \"timestamp\": \"$(date -Iseconds)\", \"screenshots\": \"$SCREENSHOT_DIR/\"}" > "$SCREENSHOT_DIR/results.json"

if [[ $FAIL -gt 0 ]]; then
    exit 1
else
    exit 0
fi
