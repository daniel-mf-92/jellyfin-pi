# Jellyfin Pi — UI Specification & E2E Verification Guide

This document is the single source of truth for what the Slint/Rust jellyfin-pi app
should look and behave like. **The target is the standard Jellyfin web/Android TV UI.**

## Reference: Standard Jellyfin UI Flow

Open any Jellyfin client (web at http://10.100.0.2:8096, Android TV, iOS) and observe:

### 1. Login Screen
- Shows user avatars in a row (poster-style circles or squares)
- Click a user -> password prompt (if set) -> auto-navigates to Home
- **No loading spinner should appear during login** -- it should feel instant
- If token is saved, skip login entirely and go straight to Home

### 2. Home Screen
- **Top row: "My Media" library cards** -- Movies, TV Shows, Collections, Music
  - These are LARGE landscape cards with the library poster/icon image
  - Clicking one navigates to that library grid (NOT a detail page)
- **Below: Content rows** -- "Continue Watching", "Next Up", "Latest in Movies", etc.
  - Horizontal scrolling rows of poster cards
  - Each card shows: poster image, title below
  - **Scrolling follows focus** -- when dpad moves right past visible edge, the row scrolls
- D-pad navigation: Up/Down moves between rows, Left/Right moves within a row
- Pressing A/Enter on a media card -> Detail screen
- Pressing A/Enter on a library card -> Library grid screen

### 3. Detail Screen (Movie/Episode/Series)
- **Hero backdrop** image at top (~40% of screen)
- Title, year, rating, runtime overlay on backdrop
- Poster image on right side
- **Overview/synopsis** text
- **Action buttons row**: Play, Resume (if progress), Favorite, Mark Played
  - Play button is focused by default -> pressing A starts VLC playback
- **Genres** row (clickable tags)
- **Cast & Crew** horizontal scroll row with headshots
- **Seasons** row (for series) -> clicking loads episodes
- **Episodes** row (landscape cards)
- **Similar** row
- Escape/B -> back to Home

### 4. Library Grid Screen
- Header with library name + Sort/Filter buttons
- Grid of poster cards (6 columns)
- D-pad navigates the grid (Left/Right within row, Up/Down between rows)
- Pressing A on a card -> Detail screen
- Escape/B -> back to Home

### 5. Player Screen
- VLC or mpv launched as subprocess with the stream URL
- Overlay shows: progress bar, title, time, play/pause
- D-pad: Left/Right = seek, Up/Down = volume
- A = play/pause, B = stop and go back

### 6. Search Screen
- On-screen keyboard (or just text input)
- Results appear as poster cards below

## Controller Mapping (unified-controller.py)

When jellyfin-pi is the foreground app (_jmp_foreground == True):
- **A button (BTN_EAST)** -> KEY_ENTER (select/confirm)
- **B button (BTN_SOUTH)** -> KEY_ESCAPE (back)
- **D-pad** -> Arrow keys
- **Y button** -> F2 (settings)
- **X button** -> F3 (search)
- **L/R bumpers** -> Seek back/forward (in player mode)

The controller is handled by unified-controller.py -- the Slint app only sees keyboard events.

## Critical Behaviors

### Focus & Scrolling
- **Every screen must have a FocusScope** that captures keyboard input
- **Horizontal rows must scroll** when the focused item moves past the visible edge
  - viewport-width in Flickable MUST be total content width, not self.width
  - viewport-x must be bound to -scroll_target + overscan
- **Vertical scroll** on Home/Detail must follow focused row/section

### Loading States
- Loading spinner appears briefly during API calls
- **Loading MUST have a timeout** -- if API call takes >10s, cancel and show error
- **Escape MUST work during loading** -- clears loading state and goes back
- Loading should NEVER be a permanent state -- every code path must set is_loading(false)

### Navigation Contract
- item-selected(id) on Home -> check if CollectionFolder -> library screen, else -> detail screen
- play-item(id) -> get playback info from Jellyfin API -> launch VLC with stream URL
- go-back() -> pop navigation stack -> return to previous screen
- **Every navigate must set is_loading(true), and every completion/error must set is_loading(false)**

### Error Handling
- API errors -> show error overlay with message, Escape dismisses
- Playback errors -> show error, return to detail screen
- Network timeout -> show "Cannot connect" with retry option

## E2E Verification Checklist

After making changes, verify ALL of these work:

### Build Verification
```bash
# 1. Check Slint compiles (on Mac Mini -- fast, catches UI errors)
cd ~/Documents/local-codebases/jellyfin-pi
cargo check 2>&1 | grep "^error" | head -5
# Must show: NO errors (warnings OK)

# 2. Full release build (on Pi -- takes ~2 min)
ssh danielmatthews-ferrero@10.100.0.17 \
  "cd ~/jellyfin-pi && git pull origin slint-rewrite && source ~/.cargo/env && cargo build --release 2>&1 | tail -3"
# Must show: "Finished release" with 0 errors

# 3. Install and launch
ssh danielmatthews-ferrero@10.100.0.17 \
  "kill -9 \$(pgrep -x jellyfin-pi) 2>/dev/null; sleep 1; \
   echo 5991 | sudo -S cp ~/jellyfin-pi/target/release/jellyfin-pi /usr/local/bin/jellyfin-pi; \
   echo jellyfin-pi > /tmp/foreground-app; \
   WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 SLINT_BACKEND=winit WINIT_UNIX_BACKEND=wayland \
   nohup /usr/local/bin/jellyfin-pi > /tmp/jmp-slint.log 2>&1 &"
```

### Functional Verification (check Pi log after each action)
```bash
# Read log
ssh danielmatthews-ferrero@10.100.0.17 "cat /tmp/jmp-slint.log"
```

1. **App starts** -> log shows "Jellyfin TV starting...", "Home data loaded successfully"
2. **Home screen loads** -> log shows "Auto-login with saved token succeeded"
3. **Library cards visible** -> "My Libraries" row appears at top with poster images
4. **No stuck loading** -> log never shows is_loading staying true
5. **API calls succeed** -> no "Failed to load" or "Server error" in log
6. **Navigate to detail** -> log shows "Item detail loaded: <title>"
7. **Navigate to library** -> log shows "Loaded N library items for <name>"
8. **Playback works** -> log shows "Created vlc player" and "Play item requested"

### What the Log Should NOT Contain
- "Failed to load public users" -- login broken
- "Failed to get item" -- API auth broken  
- "Failed to get playback info: Not found" -- wrong API endpoint format
- "Server error: 503" -- Jellyfin not ready
- "Player error" -- VLC/mpv not installed or wrong args
- Any panic or crash trace

## File Structure Reference

```
ui/
  app.slint          -- Main window, screen switching, loading overlay, global keys
  home.slint         -- Home screen with content rows
  detail.slint       -- Movie/series detail page
  library.slint      -- Library grid browser
  player.slint       -- Player overlay
  content-row.slint  -- Horizontal scrolling card row (used by home + detail)
  card.slint         -- Individual media card component
  theme.slint        -- Colors, sizes, spacing constants
  library-tiles.slint -- (DEPRECATED -- use content row with landscape cards instead)
  
src/
  main.rs            -- All callbacks, navigation, API calls, player management
  api/client.rs      -- Jellyfin REST API client
  api/models.rs      -- Data structures (BaseItemDto, etc.)
  api/images.rs      -- Image cache (downloads + caches poster/backdrop images)
  player/mod.rs      -- PlayerWrapper (VLC or mpv)
  player/vlc.rs      -- VLC subprocess management
  player/mpv.rs      -- mpv subprocess management
  state.rs           -- Navigation stack, app state
  config.rs          -- Config file (config.toml)
  
unified-controller.py -- Nintendo Switch Pro controller -> keyboard events
```

## Jellyfin API Quick Reference

Server: http://10.100.0.2:8096
Auth: X-Emby-Token header with saved token from config.toml

Key endpoints:
- GET /Users/Public -- public user list (login screen)
- POST /Users/AuthenticateByName -- login
- GET /Users/{userId}/Views -- library folders (Movies, TV Shows, etc.)
- GET /Users/{userId}/Items/Latest?parentId={libraryId} -- latest in library
- GET /Users/{userId}/Items/Resume -- continue watching
- GET /Shows/NextUp?userId={userId} -- next up
- GET /Users/{userId}/Items/{itemId} -- item detail
- GET /Items/{itemId}/Similar -- similar items
- GET /Items/{itemId}/PlaybackInfo -- stream URL for playback
- GET /Users/{userId}/Items?parentId={libraryId}&SortBy=Name -- library grid items
- GET /Items/{itemId}/Images/Primary -- poster image

## Summary

Build a Jellyfin client that looks and works like the standard Jellyfin Android TV app.
Every screen must be navigable with a game controller (dpad + A/B buttons).
Every action must complete (no stuck loading). Every error must be recoverable (Escape goes back).
The app must start, show the home screen with library cards and content rows, allow browsing
into libraries and detail pages, and play media via VLC. That is the minimum viable product.


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
