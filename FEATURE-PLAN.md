# Jellyfin Pi — Feature Gap Implementation Plan
## Compiled from 5 research agents — 2026-04-06

---

## Phase 1: Wire Up Existing Components (1-2 sessions)
Everything here already exists in code but is broken or disconnected.

### 1.1 Detail Page Data Wiring
- Cast/crew: detail.slint has CastCrewRow, models.rs has People — extract item.people into CastMember structs in load_item_detail() and pass via AppBridge.cast-members
- Genres as tags: detail.slint has GenreRow with GenreTag — populate AppBridge.genres from item.genres
- Critic rating: Model has critic_rating — add display in detail info row
- Community rating: Show as score badge (e.g., 8.5/10)
- Tagline: Model has taglines — display as italic text below title
- Studio name: Model has studios — display below genres
- Add to ITEM_FIELDS: People, Studios, CommunityRating, CriticRating, RemoteTrailers, ExternalUrls, Tags

### 1.2 Library Tiles on Home Screen
- API endpoint: get_views() returns user media folders (Movies, TV Shows, Music, Collections)
- Load on login, populate AppBridge.library-tiles
- LibraryTilesRow component already exists in home.slint — just needs data

### 1.3 Fix Sort/Filter Wiring
- Map UI labels to API values: Name->SortName, Date Added->DateCreated, Rating->CommunityRating, Year->ProductionYear, Runtime->Runtime
- Fix sort-changed/filter-changed callbacks — they pass empty library_id
- Map filter labels: Unplayed->IsUnplayed, Played->IsPlayed, Favorites->IsFavorite
- Add ascending/descending toggle

### 1.4 Buffering Spinner
- Buffering(i32) event already emitted by VLC player
- Add loading overlay in player.slint when buffering < 100%

---

## Phase 2: New Detail Page Features (2-3 sessions)

### 2.1 Add to Rust Model (models.rs)
- ExternalUrls: Vec of ExternalUrl (Name, Url) — for IMDb/TMDB links
- RemoteTrailers: Vec of RemoteTrailer (Url) — for trailer playback
- ProviderIds: HashMap of String to String — for deep linking

### 2.2 Trailer Playback
- Parse YouTube URL from RemoteTrailers
- Watch Trailer button on detail page
- Play trailer via VLC (yt-dlp to get stream URL)

### 2.3 External Links
- Show IMDb/TMDB info as text badges
- Display IMDb rating if available

### 2.4 Media Info Badge
- Show resolution (1080p/4K), video codec (H.264/HEVC), audio (5.1/Atmos) as small badges
- Data already in MediaStreams from API

### 2.5 Detail Page Layout (matching Android TV)
- Backdrop image full width with gradient overlay
- Title (year) + official rating + runtime in info row
- Star rating + critic score
- Tagline in italics
- Action buttons: Play, Resume, Favorite, Played
- Overview text
- Genre tags + Studio
- Cast and crew horizontal scroll row with photos
- Similar items row
- Media info badges at bottom

---

## Phase 3: Playback Improvements (1-2 sessions)

### 3.1 High Priority
- Buffering spinner: Wire Buffering event to UI overlay (Small)
- Subtitle delay UI: Backend exists in controls.rs — add to player overlay (Small)
- OSD toast for seek: Brief +10s/-30s text overlay when seeking (Small)
- Aspect ratio control: VLC ratio and crop commands (Small-Medium)

### 3.2 Medium Priority
- Quality/bitrate picker: Popup to select quality during playback (Medium)
- Subtitle appearance: VLC args for font size/color in Settings (Medium)
- Media info overlay: Show codec/bitrate/resolution during playback (Medium)

### 3.3 Already Working
- Skip intro/outro/recap (segments.rs)
- Trickplay thumbnail preview (trickplay.rs)
- Auto-next episode (queue.rs)
- Audio/subtitle track selection
- Playback speed control
- Chapter navigation
- Audio passthrough (AC3, DTS, TrueHD)
- Resume from position
- Queue management

---

## Phase 4: Navigation and UX Polish (1-2 sessions)

### 4.1 Focus State Preservation
- Save focused_row/focused_col in Screen enum variants in state.rs
- Restore on go_back() — most impactful single UX fix

### 4.2 Context Menu (X Button)
- Currently mapped but unimplemented
- Popup overlay: Play, Add to Queue, Mark Played, Toggle Favorite, Go to Series
- Context-sensitive based on item type

### 4.3 Consistent Focus Ring
- Unify 4 different focus indicator styles into one: 2px accent border + 1.15x scale

### 4.4 Now-Playing Bar Integration
- Component exists (now-playing-bar.slint) but not navigable
- Show on Home/Library/Search/Detail when media is playing
- Add to focus chain

### 4.5 Screen Transition Animations
- Fade/slide transitions between screens
- Card selection pulse before navigating to detail

---

## Phase 5: Library Browsing (1-2 sessions)

### 5.1 Pagination / Infinite Scroll
- load_more callback exists in UI but never wired
- Track total_count from API, load next 100 on scroll-to-bottom

### 5.2 Genre Filtering
- Add genre dropdown/sidebar to library screen

### 5.3 Letter Jump (A-Z)
- Map L1/R1 bumpers to jump between alphabet letters in library

### 5.4 Collections and Playlists
- Add Collections (BoxSet) as browseable library type
- Add server-side Playlists browser

---

## Priority Summary

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| P0 | Wire cast/crew/genres to detail page | Small | High |
| P0 | Load library tiles on home screen | Small | High |
| P0 | Fix sort/filter wiring | Small | High |
| P0 | Buffering spinner | Tiny | High |
| P1 | Ratings, tagline, studio on detail | Small | Medium |
| P1 | Focus preservation on back nav | Medium | High |
| P1 | Pagination for large libraries | Medium | High |
| P1 | Context menu (X button) | Medium | Medium |
| P2 | Trailer playback | Medium | Medium |
| P2 | External links / media info | Small | Low |
| P2 | Quality picker | Medium | Medium |
| P2 | Subtitle delay/appearance UI | Small | Medium |
| P2 | Screen transitions | Medium | Medium |
| P3 | Genre filtering | Medium | Medium |
| P3 | Letter jump A-Z | Small | Medium |
| P3 | Collections/playlists | Medium | Low |
| P3 | Now-playing bar focus | Small | Low |
| P3 | Search improvements | Small | Low |
