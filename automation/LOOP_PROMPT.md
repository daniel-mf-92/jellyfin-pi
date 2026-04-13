You are an autonomous Codex agent auditing and fixing the jellyfin-pi Slint/Rust TV media browser app.

Repository: ~/Documents/local-codebases/jellyfin-pi
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
- Pi binary: /usr/local/bin/jellyfin-pi
- Pi logs: /tmp/jmp-slint.log
- Pi config: ~/.config/jellyfin-pi/config.toml

## Execution Contract

1. Read JELLYFIN_UI_SPEC.md
2. SSH to Pi and read /tmp/jmp-slint.log to see current errors
3. Identify the highest-priority broken behavior from the spec
4. Read the relevant Rust/Slint code
5. Fix it with minimal, targeted changes
6. Run `cargo check` on Mac Mini to verify compilation
7. Commit and push to origin/slint-rewrite
8. SSH to Pi: pull, build, install, restart, read log to verify fix
9. Report what you fixed and verification result

## E2E Verification (MUST DO after every fix)

```bash
# Build on Pi
ssh danielmatthews-ferrero@10.100.0.17 "cd ~/jellyfin-pi && git pull origin slint-rewrite && source ~/.cargo/env && cargo build --release 2>&1 | tail -3"

# Install and restart
ssh danielmatthews-ferrero@10.100.0.17 "kill -9 \$(pgrep -x jellyfin-pi) 2>/dev/null; sleep 1; echo 5991 | sudo -S cp ~/jellyfin-pi/target/release/jellyfin-pi /usr/local/bin/jellyfin-pi; rm -f /tmp/jmp-slint.log; echo jellyfin-pi > /tmp/foreground-app; WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 SLINT_BACKEND=winit WINIT_UNIX_BACKEND=wayland nohup /usr/local/bin/jellyfin-pi > /tmp/jmp-slint.log 2>&1 &"

# Wait and check log
sleep 8
ssh danielmatthews-ferrero@10.100.0.17 "cat /tmp/jmp-slint.log" | grep -E "INFO|ERROR|WARN" | grep -v "winit\|sctk\|tracing\|hyper\|reqwest"
```

The fix is NOT done until the Pi log shows the issue is resolved.

## Safety
- Do NOT modify config.toml on Pi
- Do NOT restart Jellyfin server on Mac Mini
- Do NOT force push or delete branches
- Keep changes minimal -- one issue per iteration
- Always commit before moving to next issue
