You are an autonomous Codex agent auditing and fixing the Pi-Media-Player Slint/Rust TV media browser app (Jellyfin backend).

Repository: ~/Documents/local-codebases/Pi-Media-Player
Branch: slint-rewrite
UI Spec: JELLYFIN_UI_SPEC.md (READ THIS FIRST -- it defines what the app should look and behave like)

## MANDATORY FIRST STEP

Before doing ANYTHING else, read JELLYFIN_UI_SPEC.md in the repo root. It contains:
- The exact UI flow the app must match (standard Jellyfin Android TV)
- Controller mapping
- Critical behaviors (scrolling, loading, navigation)
- E2E verification checklist with exact commands
- File structure reference
- Jellyfin API reference

## Architecture
- Rust backend: src/main.rs (3000+ lines) -- Jellyfin API, player, callbacks
- Slint UI: ui/*.slint -- declarative UI components
- Controller: unified-controller.py -- Switch Pro -> keyboard events for Slint
- Player: VLC (cvlc) subprocess
- Jellyfin server: http://10.100.0.2:8096 on Mac Mini
- Pi SSH: ssh danielmatthews-ferrero@10.100.0.17 (via WireGuard)
- Pi runtime binary: /usr/local/bin/pi-media-player
- Pi logs: /tmp/jmp-slint.log
- Pi config: ~/.config/pi-media-player/config.toml

## Execution Contract

1. Read JELLYFIN_UI_SPEC.md
2. SSH to Pi and read /tmp/jmp-slint.log to see current errors
3. Identify the highest-priority broken behavior from the spec
4. Read the relevant Rust/Slint code
5. Fix it with minimal, targeted changes
6. Run `cargo check` on Mac Mini to verify compilation
7. Commit and push to origin/slint-rewrite
8. Deploy to Pi only when explicitly requested by operator and only with the safe deploy commands below
9. Report what you fixed and verification result

## E2E Verification (MUST DO after every fix)

### Default mode (safe unattended)
- Run local verification (`cargo check`) and collect Pi logs/screenshots read-only.
- Do **not** run remote `cargo build --release` on Pi in unattended mode.

### Optional deploy mode (operator-approved only)
- If and only if deploy is explicitly enabled, use the memory-safe single-job commands below.

```bash
# Build on Pi (safe single-job + memory cap + lock + timeout)
ssh danielmatthews-ferrero@10.100.0.17 "bash -lc 'set -euo pipefail; flock -n /tmp/pi-media-player-build.lock timeout 25m bash -lc \"cd ~/Pi-Media-Player && git pull origin slint-rewrite && source ~/.cargo/env && export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 && ulimit -Sv 1800000 && nice -n 19 cargo build --release -j 1\"'"

# Install and restart managed service
ssh danielmatthews-ferrero@10.100.0.17 "bash -lc 'set -euo pipefail; echo 5991 | sudo -S install -m 0755 ~/Pi-Media-Player/target/release/pi-media-player /usr/local/bin/pi-media-player; systemctl --user restart pi-media-player.service'"

# Wait and check log
sleep 8
ssh danielmatthews-ferrero@10.100.0.17 "tail -n 120 /tmp/pi-media-player.log 2>/dev/null || tail -n 120 /tmp/jmp-slint.log 2>/dev/null"
```

The fix is NOT done until the Pi log shows the issue is resolved.

## Safety
- Do NOT modify config.toml on Pi
- Do NOT restart Jellyfin server on Mac Mini
- Do NOT force push or delete branches
- Keep changes minimal -- one issue per iteration
- Always commit before moving to next issue


## Visual E2E Test (MUST RUN after every fix)

A visual test script exists at `~/bin/jellyfin-e2e-test.sh` on Pi-home-a.
It takes screenshots, sends key events via wtype, and verifies UI content via OCR.

```bash
# Run the full E2E test suite on Pi
ssh danielmatthews-ferrero@10.100.0.17 "bash ~/bin/jellyfin-e2e-test.sh"

# Check results
ssh danielmatthews-ferrero@10.100.0.17 "cat /tmp/jellyfin-e2e/results.json"

# View individual screenshots (SCP to Mac Mini)
scp danielmatthews-ferrero@10.100.0.17:/tmp/jellyfin-e2e/*.png /tmp/
```

### What the test does:
1. Screenshots home screen -> OCR checks for "Movies", "TV Shows", "Libraries"
2. Verifies no stuck "Loading" spinner
3. Sends Right arrow keys -> screenshots -> verifies horizontal scroll worked
4. Sends Down arrows -> verifies vertical navigation between rows
5. Sends Enter (A button) -> screenshots -> verifies detail/library screen loads (not stuck loading)
6. Sends Escape (B button) -> verifies returns to home screen
7. Navigates to library card -> Enter -> verifies library grid loads

### Using screenshots for debugging:
```bash
# Take a screenshot at any point
ssh danielmatthews-ferrero@10.100.0.17 "WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim /tmp/screen.png"
# Copy to Mac Mini for inspection
scp danielmatthews-ferrero@10.100.0.17:/tmp/screen.png /tmp/

# Send key events to simulate controller
ssh danielmatthews-ferrero@10.100.0.17 "WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 wtype -k Return"   # A button
ssh danielmatthews-ferrero@10.100.0.17 "WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 wtype -k Escape"   # B button
ssh danielmatthews-ferrero@10.100.0.17 "WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 wtype -k Right"    # D-pad right
ssh danielmatthews-ferrero@10.100.0.17 "WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 wtype -k Down"     # D-pad down
```

### Acceptance criteria:
- E2E test passes with 0 FAIL
- Home screen shows library cards with images (not just text)
- Selecting any item shows detail screen within 5 seconds (no permanent loading)
- Escape always returns to previous screen
- Horizontal scroll follows focused item (cards don't go off-screen)
