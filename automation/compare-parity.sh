#!/usr/bin/env bash
# Visual parity comparison: Jellyfin web UI vs Pi-Media-Player Slint app
# Run on Mac Mini. Requires: reference screenshots + Pi screenshots already taken.
set -uo pipefail

REF_DIR="$HOME/Documents/local-codebases/Pi-Media-Player/automation/reference-screenshots"
TEST_DIR="/tmp/jellyfin-e2e"
REPORT_DIR="$HOME/Documents/local-codebases/Pi-Media-Player/automation/parity-reports"
REPORT_FILE="$REPORT_DIR/$(date +%Y%m%d-%H%M%S).md"
PI_HOST="danielmatthews-ferrero@10.100.0.17"
WL_ENV="WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000"

mkdir -p "$REPORT_DIR" "$TEST_DIR"

echo "# Visual Parity Report — $(date)" > "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# --- Step 1: Capture reference screenshots if missing ---
if [[ ! -f "$REF_DIR/02-home.png" ]]; then
    echo "Capturing Jellyfin web UI reference screenshots..."
    node "$HOME/Documents/local-codebases/Pi-Media-Player/automation/capture-jellyfin-reference.js"
fi

# --- Step 2: Capture Slint app screenshots via Pi ---
echo "Capturing Pi-Media-Player Slint app screenshots from Pi..."

take_pi_screenshot() {
    local name="$1"
    ssh -o ConnectTimeout=5 "$PI_HOST" "$WL_ENV grim /tmp/jellyfin-e2e/${name}.png" 2>/dev/null
    scp -q "$PI_HOST:/tmp/jellyfin-e2e/${name}.png" "$TEST_DIR/${name}.png" 2>/dev/null
}

send_pi_key() {
    ssh -o ConnectTimeout=5 "$PI_HOST" "$WL_ENV wtype -k $1" 2>/dev/null
    sleep 1
}

# Reset to home first
send_pi_key "Escape"
send_pi_key "Escape"
send_pi_key "Escape"
sleep 2

# Home screen
take_pi_screenshot "slint-home"

# Navigate down to content, take screenshot
send_pi_key "Down"
send_pi_key "Down"
sleep 1
take_pi_screenshot "slint-content-rows"

# Select first item (should go to detail)
send_pi_key "Return"
sleep 3
take_pi_screenshot "slint-detail"

# Go back
send_pi_key "Escape"
sleep 2

# Go to first library card and select (row 0 = My Libraries)
send_pi_key "Up"
send_pi_key "Up"
send_pi_key "Up"
sleep 1
send_pi_key "Return"
sleep 3
take_pi_screenshot "slint-library-grid"

# Go back
send_pi_key "Escape"
sleep 2

# --- Step 3: Compare screenshots ---
echo "" >> "$REPORT_FILE"
echo "## Screen Comparisons" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

compare_screens() {
    local ref_name="$1"
    local test_name="$2"
    local label="$3"
    local ref_img="$REF_DIR/$ref_name"
    local test_img="$TEST_DIR/$test_name"

    echo "### $label" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"

    if [[ ! -f "$ref_img" ]]; then
        echo "- Reference: MISSING ($ref_name)" >> "$REPORT_FILE"
        return
    fi
    if [[ ! -f "$test_img" ]]; then
        echo "- Slint app: MISSING ($test_name)" >> "$REPORT_FILE"
        return
    fi

    # OCR both images
    local ref_text test_text
    ref_text=$(tesseract "$ref_img" stdout 2>/dev/null | tr '\n' ' ' | head -c 500)
    test_text=$(tesseract "$test_img" stdout 2>/dev/null | tr '\n' ' ' | head -c 500)

    echo "- **Reference (Jellyfin web)**: $(echo "$ref_text" | head -c 200)" >> "$REPORT_FILE"
    echo "- **Slint app**: $(echo "$test_text" | head -c 200)" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"

    # Check for key elements that should appear in both
    local missing=""
    for keyword in $4; do
        if ! echo "$test_text" | grep -qi "$keyword"; then
            missing="$missing $keyword"
        fi
    done

    if [[ -z "$missing" ]]; then
        echo "- **PARITY: PASS** — all expected elements found" >> "$REPORT_FILE"
    else
        echo "- **PARITY: FAIL** — missing in Slint:$missing" >> "$REPORT_FILE"
    fi

    # Structural comparison (resize both to same size, compare)
    if command -v compare &>/dev/null; then
        local diff_img="$REPORT_DIR/diff-${test_name}"
        convert "$ref_img" -resize 1920x1080! "/tmp/ref-resized.png" 2>/dev/null
        convert "$test_img" -resize 1920x1080! "/tmp/test-resized.png" 2>/dev/null
        local metric
        metric=$(compare -metric RMSE "/tmp/ref-resized.png" "/tmp/test-resized.png" "$diff_img" 2>&1 | grep -oP '[\d.]+' | head -1 || echo "N/A")
        echo "- **Visual difference (RMSE)**: $metric (lower = more similar)" >> "$REPORT_FILE"
    fi

    echo "" >> "$REPORT_FILE"
}

# Compare each screen pair
compare_screens "02-home.png" "slint-home.png" "Home Screen" "Movies Shows Continue Latest Libraries"
compare_screens "05-movie-detail.png" "slint-detail.png" "Detail Screen" "Play Overview Cast"
compare_screens "03-movies-library.png" "slint-library-grid.png" "Library Grid" "Sort Filter"

# --- Step 4: Summary ---
echo "" >> "$REPORT_FILE"
echo "## Element Checklist" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"
echo "| Screen | Element | Jellyfin Web | Slint App | Parity |" >> "$REPORT_FILE"
echo "|--------|---------|-------------|-----------|--------|" >> "$REPORT_FILE"

check_element() {
    local screen="$1" element="$2" ref_file="$3" test_file="$4"
    local ref_has="NO" test_has="NO" parity="FAIL"

    if [[ -f "$REF_DIR/$ref_file" ]]; then
        tesseract "$REF_DIR/$ref_file" stdout 2>/dev/null | grep -qi "$element" && ref_has="YES"
    fi
    if [[ -f "$TEST_DIR/$test_file" ]]; then
        tesseract "$TEST_DIR/$test_file" stdout 2>/dev/null | grep -qi "$element" && test_has="YES"
    fi
    [[ "$ref_has" == "$test_has" ]] && parity="PASS"

    echo "| $screen | $element | $ref_has | $test_has | $parity |" >> "$REPORT_FILE"
}

# Home screen elements
check_element "Home" "Movies" "02-home.png" "slint-home.png"
check_element "Home" "TV Shows" "02-home.png" "slint-home.png"
check_element "Home" "Continue Watching" "02-home.png" "slint-home.png"
check_element "Home" "Latest" "02-home.png" "slint-home.png"
check_element "Home" "Next Up" "02-home.png" "slint-home.png"

# Detail elements
check_element "Detail" "Play" "05-movie-detail.png" "slint-detail.png"
check_element "Detail" "Overview" "05-movie-detail.png" "slint-detail.png"
check_element "Detail" "Cast" "05-movie-detail.png" "slint-detail.png"
check_element "Detail" "Genre" "05-movie-detail.png" "slint-detail.png"
check_element "Detail" "Similar" "05-movie-detail.png" "slint-detail.png"

# Library elements
check_element "Library" "Sort" "03-movies-library.png" "slint-library-grid.png"
check_element "Library" "Filter" "03-movies-library.png" "slint-library-grid.png"

echo ""
echo "Report: $REPORT_FILE"
cat "$REPORT_FILE"
