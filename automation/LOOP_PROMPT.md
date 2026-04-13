You are an autonomous Codex agent auditing and fixing the jellyfin-pi Slint/Rust TV media browser app.

Repository: ~/Documents/local-codebases/jellyfin-pi
Branch: slint-rewrite
Config (on Pi): ~/.config/jellyfin-pi/config.toml → url = "http://10.100.0.2:8096"
Pi binary: /usr/local/bin/jellyfin-pi (built natively on Pi5 via cargo build --release)
Logs (on Pi): /tmp/jmp-slint.log
Pi SSH: ssh danielmatthews-ferrero@10.100.0.17 (via WireGuard)

## Architecture
- Rust backend: src/main.rs (3000+ lines) — Jellyfin API client, player management, callbacks
- Slint UI: ui/*.slint — home, detail, library, player, search, settings screens
- Controller: unified-controller.py — Switch Pro → virtual keyboard/mouse for Slint
- Player: VLC (cvlc) or mpv launched as subprocess
- Jellyfin server: http://10.100.0.2:8096 on Mac Mini

## Known Issues (PRIORITY ORDER)

### P0 — CRITICAL (app is unusable without these)
1. **Selecting any media item causes permanent "Loading..." black screen** — The navigate("detail", id) callback hangs. Load never completes. Escape should cancel loading and go back (FocusScope added but may not receive focus over the loading overlay). Debug by checking /tmp/jmp-slint.log on Pi after pressing A on an item.
2. **A button does nothing on home screen** — In NAVIGATION mode, unified-controller.py sends KEY_ENTER when _jmp_foreground is true, but Slint FocusScope may not be receiving it. Verify the focus chain works.
3. **Selecting user at login also causes Loading... crash** — Same pattern as detail navigation.

### P1 — HIGH (core UX broken)
4. **Horizontal scroll does not follow focused item** — content-row.slint viewport-width fix was applied but may not work. When navigating right with dpad, items go off-screen. Test by checking if viewport-x changes when focused-index changes.
5. **"My Libraries" cards (Movies, TV Shows, Collections) should be wider** — Currently set to landscape row_type but may still be poster-sized. Cards should be clearly distinct from regular media cards.
6. **"Error cannot play — not found"** — play-item callback gets 404 from Jellyfin playback info API. Check if item IDs are correct, if the auth token is valid, if the API endpoint format matches Jellyfin 10.11.5.

### P2 — MEDIUM (polish)
7. **CollectionFolder items should navigate to library grid, not detail** — Redirect logic added but needs testing.
8. **B button (Escape) must ALWAYS go back** — Even from loading screens, error screens, any state.

## Execution Contract

1. Read the code and logs. SSH to Pi to check /tmp/jmp-slint.log.
2. Pick the highest-priority unfixed issue.
3. Identify root cause by reading the Rust/Slint code.
4. Fix it with minimal, targeted changes.
5. Run `cargo check` locally (Mac Mini, will have compile errors for linux-only deps but catches Slint/logic errors).
6. Commit with descriptive message. Push to origin/slint-rewrite.
7. Report what you fixed and what to test.

## Safety
- Do NOT modify config.toml on Pi
- Do NOT restart Jellyfin server
- Do NOT run cargo build on Pi (user will do that manually)
- Do NOT force push or delete branches
- Keep changes minimal and focused — one issue per iteration
