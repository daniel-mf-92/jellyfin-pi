# CODEX_CONTEXT.md — Pi-Media-Player 24/7 Autonomous Agent

Authoritative reference for the always-on Codex agent iterating on this repo.
Last updated: 2026-04-23 (post-legacy-removal).

## Identity & Naming

- **Canonical project name:** Pi-Media-Player
- **Legacy app identifiers:** retired (migration completed)
- **GitHub repo:** https://github.com/daniel-mf-92/Pi-Media-Player
- **Primary branch:** slint-rewrite
- **Agent LaunchAgent label:** com.jellyfinpi.codex.loop (label NOT renamed — runtime state)
- **Agent AGENT_NAME:** jellyfinpi-codex (backoff/lock files under /tmp keyed on this)

## Three Canonical Locations

1. **M4 local clone** (primary dev workspace):
   `~/Documents/local-codebases/Pi-Media-Player/`
   — This is where the Codex loop runs. It edits here, commits, and pushes.

2. **Pi runtime clone** (deployed target):
   `pi5-home-a:~/Pi-Media-Player/`  (reach via `ssh pi5-home-a` from M4, via WireGuard 10.100.0.17)
   — Build + runtime host. Loop MUST pull here after every successful commit.

3. **GitHub remote:**
   `git@github.com:daniel-mf-92/Pi-Media-Player.git`
   — Source of truth. Push origin/slint-rewrite after every iteration.

## Sync Pattern (every successful iteration)

```bash
# On M4, after committing a working change:
cd ~/Documents/local-codebases/Pi-Media-Player
git push origin slint-rewrite

# Then propagate to Pi:
ssh pi5-home-a 'cd ~/Pi-Media-Player && git pull --rebase origin slint-rewrite && bash build-pi5.sh && systemctl --user restart pi-media-player.service'

# Verify runtime log is clean:
ssh pi5-home-a 'tail -40 /tmp/jmp-slint.log' | grep -Ev 'winit|sctk|tracing|hyper|reqwest'
```

**Build command on Pi:** `bash build-pi5.sh` (NOT `~/bin/build-arm64.sh` — that belongs to the upstream Qt JMP project).

## Runtime Paths (Canonical)

Canonical runtime identifiers and paths:

- Service/runtime binary: `/usr/local/bin/pi-media-player`
- Config dir: `~/.config/pi-media-player/`
- IPC sockets: `/tmp/pi-media-player-*.sock`
- Wayland app id: `pi-media-player`
- Runtime log file: `/tmp/pi-media-player.log`

Legacy app identifiers have been removed from runtime paths.

## Goal — e2e Jellyfin UX Parity

Iterate the Slint/Rust UI toward feature-parity with the official Jellyfin Media Player (Android TV) UX:
- Browsing (libraries, rows, shelves, detail pages)
- Playback controls (play/pause/seek/next/prev)
- Resume from last position
- Transcoding negotiation + HLS fallback
- Subtitle track selection
- Audio track selection
- Remote control via unified-controller.py (Switch Pro -> keyboard -> Slint)

**Per iteration:** add ONE capability. Commit. Test on Pi. If build fails, revert the last commit and try a smaller change.

## Loop Orchestration

- **LaunchAgent plist:** `~/Library/LaunchAgents/com.jellyfinpi.codex.loop.plist` (KeepAlive=true)
- **Wrapper:** `~/bin/jellyfinpi-codex-loop-wrapper.sh` (sets REPO_DIR, sources codex-loop-common.sh)
- **Loop script:** `automation/codex-jellyfinpi-loop.sh` (in this repo)
- **Prompt:** `automation/LOOP_PROMPT.md` (in this repo — **read this first** every iteration)
- **UI Spec:** `JELLYFIN_UI_SPEC.md` (repo root — defines target UX)
- **Sleep between iterations:** 180s (overridable via SLEEP_SECONDS env)
- **Exponential backoff on failure:** 30s -> 60s -> ... -> 6h max

## Credentials / Safety

- **NEVER** rotate, revoke, or regenerate any API keys or tokens.
- **NEVER** force-push. Never rewrite history. Never delete branches.
- **NEVER** modify config.toml on the Pi (user state).
- **NEVER** restart the Mac Mini Jellyfin server.
- Commit frequently (one logical change per commit).
- Always test build before committing.
- On conflicts with Pi-side changes: `git pull --rebase` and resolve.

## What Has Been Tried / What's Still TODO

**Recent history (pre-rename, on branch slint-rewrite):**
- Iteration 9 (2026-04-13): auto-fix for detail load timeout (false timeout on nested 10s)
- Iteration 8 (2026-04-13): earlier detail-page work
- 2026-04-16: repo rename completed to Pi-Media-Player (commit d5f2433)

**Still TODO (high level):**
- Full JELLYFIN_UI_SPEC.md review and gap identification
- Horizontal shelf scrolling parity
- Continue Watching row
- Series detail / season / episode drill-down
- Subtitle picker UI
- Audio track picker UI
- Playback resume on launch

## Verification Commands (quick reference)

```bash
# Confirm loop is alive:
ssh macmini-azure 'launchctl list | grep jellyfinpi; ps auxww | grep jellyfinpi-codex | grep -v grep'

# Tail loop log:
ssh macmini-azure 'tail -40 ~/logs/jellyfinpi-codex-loop.out.log'

# Check current iteration in flight:
ssh macmini-azure 'ls -la ~/Documents/local-codebases/Pi-Media-Player/automation/logs/ | tail'

# Peek at Pi runtime log:
ssh macmini-azure "ssh pi5-home-a 'tail -30 /tmp/jmp-slint.log'"
```
