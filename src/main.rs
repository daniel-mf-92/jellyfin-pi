mod api;
mod player;
mod input;
mod state;
mod config;
mod tracking;
use tracking::PlaybackTracker;
mod daemon;
mod device_profile;
mod power;
mod mpris;
mod trickplay;
mod segments;
mod queue;
mod audio;

use segments::SegmentManager;
use queue::{PlaybackQueue, QueueItem};
use player::PlaybackControls;
use api::{JellyfinClient, ImageCache};
use api::models::*;

/// Load .env file (gitignored) for credentials.
fn load_dotenv() {
    let mut paths = vec![std::path::PathBuf::from(".env")];
    if let Ok(home) = std::env::var("HOME") {
        paths.push(std::path::Path::new(&home).join("jellyfin-pi/.env"));
    }

    for p in paths {
        if p.exists() {
            if let Ok(contents) = std::fs::read_to_string(&p) {
                for line in contents.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') { continue; }
                    if let Some((key, val)) = line.split_once('=') {
                        let val = val.trim().trim_matches('"').trim_matches('\'');
                        if std::env::var(key.trim()).is_err() {
                            std::env::set_var(key.trim(), val);
                        }
                    }
                }
                log::info!("Loaded env from {}", p.display());
                return;
            }
        }
    }
}
use player::VlcPlayer;
use player::PlayerWrapper;
use player::vlc::PlayerEvent;
use input::ControllerManager;
use input::controller::InputAction;
use state::{StateManager, Screen};
use config::AppConfig;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, Mutex};
use tokio::sync::mpsc;
use log::{info, error, warn, debug};
use slint::{Image as SlintImage, ModelRc, VecModel, SharedString};

const RSS_WARN_MB: u64 = 2500;
const RSS_SOFT_LIMIT_MB: u64 = 4000;
const RSS_CACHE_CLEAR_MB: u64 = 1800;
const RSS_EMERGENCY_EXIT_MB: u64 = 6500;

slint::include_modules!();

// =============================================================================
// Type conversion helpers
// =============================================================================

/// Convert a `BaseItemDto` into the Slint `MediaItem` struct.
fn base_item_to_media_item(
    item: &BaseItemDto,
    server_url: &str,
    poster_image: SlintImage,
    backdrop_image: SlintImage,
) -> MediaItem {
    MediaItem {
        id: SharedString::from(&item.id),
        title: SharedString::from(&item.name),
        subtitle: SharedString::from(
            item.series_name.as_deref().unwrap_or_default(),
        ),
        item_type: SharedString::from(&item.item_type),
        image_source: poster_image,
        backdrop_source: backdrop_image,
        progress: item.progress(),
        is_played: item
            .user_data
            .as_ref()
            .map(|ud| ud.played)
            .unwrap_or(false),
        is_favorite: item
            .user_data
            .as_ref()
            .map(|ud| ud.is_favorite)
            .unwrap_or(false),
        unplayed_count: item
            .user_data
            .as_ref()
            .and_then(|ud| ud.unplayed_item_count)
            .unwrap_or(0),
        year: SharedString::from(
            item.production_year
                .map(|y| y.to_string())
                .unwrap_or_default(),
        ),
        rating: SharedString::from(
            item.community_rating
                .map(|r| format!("{:.1}", r))
                .unwrap_or_default(),
        ),
        official_rating: SharedString::from(
            item.official_rating.as_deref().unwrap_or_default(),
        ),
        runtime: SharedString::from(
            item.runtime_string().unwrap_or_default(),
        ),
        overview: SharedString::from(
            item.overview.as_deref().unwrap_or_default(),
        ),
        genres: SharedString::from(
            item.genres
                .as_ref()
                .map(|g| g.join(", "))
                .unwrap_or_default(),
        ),
    }
}

/// Convert a `SearchHint` into the Slint `SearchResult` struct.
fn search_hint_to_result(
    hint: &SearchHint,
    poster_image: SlintImage,
) -> SearchResult {
    SearchResult {
        id: SharedString::from(&hint.item_id),
        title: SharedString::from(&hint.name),
        subtitle: SharedString::from(
            hint.series.as_deref().unwrap_or_default(),
        ),
        item_type: SharedString::from(&hint.item_type),
        image_source: poster_image,
        year: SharedString::from(
            hint.production_year
                .map(|y| y.to_string())
                .unwrap_or_default(),
        ),
    }
}

/// Convert a `UserDto` into the Slint `UserInfo` struct.
fn user_dto_to_user_info(
    user: &UserDto,
    server_url: &str,
    avatar: SlintImage,
) -> UserInfo {
    UserInfo {
        id: SharedString::from(&user.id),
        name: SharedString::from(&user.name),
        avatar,
        has_password: user.has_password,
    }
}


/// Map UI sort label to Jellyfin API sort field name.
fn map_sort_label(label: &str) -> &str {
    match label {
        "Name" => "SortName",
        "Date Added" => "DateCreated",
        "Rating" => "CommunityRating",
        "Year" => "ProductionYear",
        "Runtime" => "Runtime",
        _ => "SortName",
    }
}

/// Map UI filter label to Jellyfin API filter value.
fn map_filter_label(label: &str) -> &str {
    match label {
        "Unplayed" => "IsUnplayed",
        "Played" => "IsPlayed",
        "Favorites" => "IsFavorite",
        "Resumable" => "IsResumable",
        _ => "",
    }
}
/// Load a poster image for an item through the image cache.
async fn load_poster_image(
    item: &BaseItemDto,
    server_url: &str,
    image_cache: &ImageCache,
    max_height: i32,
) -> SlintImage {
    if let Some(url) = item.primary_image_url(server_url, max_height) {
        image_cache
            .load_image(&url)
            .await
            .unwrap_or_default()
    } else {
        SlintImage::default()
    }
}

/// Load a backdrop image for an item through the image cache.
async fn load_backdrop_image(
    item: &BaseItemDto,
    server_url: &str,
    image_cache: &ImageCache,
    max_width: i32,
) -> SlintImage {
    if let Some(url) = item.backdrop_image_url(server_url, max_width) {
        image_cache
            .load_image(&url)
            .await
            .unwrap_or_default()
    } else {
        SlintImage::default()
    }
}

/// Load a user avatar image through the image cache.
async fn load_user_avatar(
    user: &UserDto,
    server_url: &str,
    image_cache: &ImageCache,
) -> SlintImage {
    if let Some(tag) = &user.primary_image_tag {
        let url = format!(
            "{}/Users/{}/Images/Primary?maxHeight=96&quality=90&tag={}",
            server_url, user.id, tag
        );
        image_cache
            .load_image(&url)
            .await
            .unwrap_or_default()
    } else {
        SlintImage::default()
    }
}

/// Convert a list of `BaseItemDto` into a `Vec<MediaItem>` with loaded images.
async fn items_to_media_items(
    items: &[BaseItemDto],
    server_url: &str,
    image_cache: &ImageCache,
) -> Vec<MediaItem> {
    // Load poster images concurrently in batches of 20 (no backdrops for grid view)
    let mut result = Vec::with_capacity(items.len());
    for chunk in items.chunks(20) {
        let futures: Vec<_> = chunk
            .iter()
            .map(|item| {
                let server_url = server_url.to_string();
                async move {
                    let poster = load_poster_image(item, &server_url, image_cache, 225).await;
                    let backdrop = SlintImage::default(); // defer backdrop until detail page
                    base_item_to_media_item(item, &server_url, poster, backdrop)
                }
            })
            .collect();
        let batch_results = futures::future::join_all(futures).await;
        result.extend(batch_results);
    }
    result
}

/// Convert a list of `BaseItemDto` into a `Vec<MediaItem>` without loading
/// any remote images. Useful for memory-safe fallback rows on constrained
/// devices when rich image rows would trigger excessive RSS growth.
fn items_to_media_items_no_images(
    items: &[BaseItemDto],
    server_url: &str,
) -> Vec<MediaItem> {
    items
        .iter()
        .map(|item| {
            base_item_to_media_item(
                item,
                server_url,
                SlintImage::default(),
                SlintImage::default(),
            )
        })
        .collect()
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv();
    env_logger::init();
    info!("Jellyfin TV starting...");

    // Clean up stale /dev/shm/jmp-cache from previous runs (was RAM disk cache, now removed)
    if let Ok(dir) = std::fs::read_dir("/dev/shm/jmp-cache") {
        let mut cleaned = 0u64;
        for entry in dir.flatten() {
            if let Ok(meta) = entry.metadata() {
                cleaned += meta.len();
                let _ = std::fs::remove_file(entry.path());
            }
        }
        if cleaned > 0 {
            info!("Cleaned {}MB from stale /dev/shm/jmp-cache", cleaned / (1024 * 1024));
        }
        let _ = std::fs::remove_dir("/dev/shm/jmp-cache");
    }

    // 1. Load config
    let config = AppConfig::load();

    // 2. Create services
    let client = Arc::new(RwLock::new(JellyfinClient::new(&config)));
    let image_cache = Arc::new(ImageCache::new(reqwest::Client::new()));
    let state = Arc::new(StateManager::new(config.server.url.clone()));
    let daemon_cb_max = config.daemon.circuit_breaker_max_per_hour;
    let config = Arc::new(RwLock::new(config));

    // 3. Create MpvPlayer (lazy: created when playback starts)
    let player: Arc<Mutex<Option<PlayerWrapper>>> = Arc::new(Mutex::new(None));

    // 3a. Create playback tracker (local SQLite)
    let tracker = match PlaybackTracker::new() {
        Ok(t) => {
            info!("Playback tracker initialized");
            Arc::new(t)
        }
        Err(e) => {
            error!("Failed to init playback tracker: {}", e);
            return Err(e.into());
        }
    };

    // 3c. Create new module instances
    let segments: Arc<Mutex<SegmentManager>> = Arc::new(Mutex::new(SegmentManager::new()));
    let playback_controls: Arc<Mutex<PlaybackControls>> = Arc::new(Mutex::new(PlaybackControls::new()));
    let queue: Arc<Mutex<PlaybackQueue>> = Arc::new(Mutex::new(PlaybackQueue::new()));

    // 3b. Create daemon manager (baked-in background tasks)
    let mut daemon_mgr = daemon::DaemonManager::new(daemon_cb_max);
    let daemon_event_rx = daemon_mgr.take_event_receiver();
    let _daemon_shared = daemon_mgr.shared();
    let daemon_player_tx = daemon_mgr.player_event_sender();
    let daemon_screen_tx = daemon_mgr.screen_watch_sender();
    // DEFERRED: daemon tasks start AFTER login, not at startup (saves ~100s of MB)
    let daemon_mgr = Arc::new(Mutex::new(daemon_mgr));


    // 4. Create Slint UI
    let ui = AppWindow::new()?;
    let ui_weak = ui.as_weak();

    // 5. Set up controller input
    let mut controller = ControllerManager::new();
    let input_rx = controller.take_receiver().expect("Controller receiver must be available at startup");

    // 6. Connect AppBridge callbacks
    setup_navigation_callbacks(
        &ui,
        client.clone(),
        image_cache.clone(),
        state.clone(),
        daemon_screen_tx,
    );
    setup_auth_callbacks(
        &ui,
        client.clone(),
        image_cache.clone(),
        state.clone(),
        config.clone(),
        daemon_mgr.clone(),
    );
    setup_playback_callbacks(
        &ui,
        client.clone(),
        state.clone(),
        player.clone(),
        config.clone(),
        daemon_player_tx.clone(),
        tracker.clone(),
        segments.clone(),
        playback_controls.clone(),
        queue.clone(),
    );
    setup_content_callbacks(
        &ui,
        client.clone(),
        image_cache.clone(),
        state.clone(),
    );
    setup_user_action_callbacks(&ui, client.clone());

    // 7-8. Auto-login: saved token first, then hardcoded credentials.
    // If auto-login fails, load public users for the login screen.
    {
        let ui_handle = ui_weak.clone();
        let client_clone = client.clone();
        let image_clone = image_cache.clone();
        let state_clone = state.clone();
        let config_clone = config.clone();
        let daemon_mgr_clone = daemon_mgr.clone();
        let _ = slint::spawn_local(async move {
            let mut authenticated = false;

            // --- Try saved token (fast path) ---
            let (saved_user_id, saved_token) = {
                let cfg = config_clone.read().await;
                (cfg.server.saved_user_id.clone(), cfg.server.saved_token.clone())
            };

            if let (Some(user_id), Some(token)) = (saved_user_id, saved_token) {
                {
                    let mut c = client_clone.write().await;
                    c.access_token = Some(token.clone());
                    c.user_id = Some(user_id.clone());
                }
                match load_home_data(
                    ui_handle.clone(),
                    client_clone.clone(),
                    image_clone.clone(),
                    state_clone.clone(),
                )
                .await
                {
                    Ok(()) => {
                        info!("Auto-login with saved token succeeded");
                        // Start daemon tasks now that we're authenticated
                        daemon_mgr_clone.lock().await.start(client_clone.clone(), config_clone.clone(), state_clone.clone());
                        state_clone.navigate_replace(Screen::Home).await;
                        let _ = ui_handle.upgrade_in_event_loop(|ui| {
                            ui.global::<AppBridge>().set_current_screen("home".into());
                        });
                        authenticated = true;
                    }
                    Err(e) => {
                        let err_text = e.to_string();
                        let auth_failure = {
                            let lower = err_text.to_ascii_lowercase();
                            lower.contains("auth error")
                                || lower.contains("unauthorized")
                                || lower.contains("not authenticated")
                        };

                        if auth_failure {
                            warn!("Saved token is no longer valid: {}", err_text);
                        } else {
                            warn!(
                                "Saved token auto-login failed due to server/network issue (keeping token in config): {}",
                                err_text
                            );
                        }

                        let mut c = client_clone.write().await;
                        c.access_token = None;
                        c.user_id = None;

                        if auth_failure {
                            let mut cfg = config_clone.write().await;
                            cfg.clear_auth();
                        }
                    }
                }
            }

            // --- Fallback: authenticate with hardcoded credentials ---
            if !authenticated {
                let username = match std::env::var("JELLYFIN_USERNAME") {
                    Ok(u) if !u.is_empty() => u,
                    _ => {
                        warn!("JELLYFIN_USERNAME not set in .env");
                        String::new()
                    }
                };
                let password = match std::env::var("JELLYFIN_PASSWORD") {
                    Ok(p) if !p.is_empty() => p,
                    _ => {
                        warn!("JELLYFIN_PASSWORD not set in .env");
                        String::new()
                    }
                };

                if !username.is_empty() && !password.is_empty() {
                    info!("Auto-login with credentials for user: {}", username);

                    let auth_result = {
                        let mut c = client_clone.write().await;
                        c.authenticate(&username, &password).await
                    };

                    match auth_result {
                        Ok(result) => {
                            info!("Auto-login succeeded for user: {}", username);
                            state_clone
                                .set_user(result.user.clone(), result.access_token.clone())
                                .await;

                            // Save token for faster login next time
                            {
                                let mut cfg = config_clone.write().await;
                                cfg.save_auth(&result.user.id, &result.access_token);
                            }

                            // Set current user in UI
                            let server_url = {
                                let c = client_clone.read().await;
                                c.server_url.clone()
                            };
                            let avatar =
                                load_user_avatar(&result.user, &server_url, &image_clone).await;
                            let user_info =
                                user_dto_to_user_info(&result.user, &server_url, avatar);
                            if let Some(ui) = ui_handle.upgrade() {
                                ui.global::<AppBridge>().set_current_user(user_info);
                            }

                            // Start daemon tasks now that we're authenticated
                            daemon_mgr_clone.lock().await.start(client_clone.clone(), config_clone.clone(), state_clone.clone());

                            state_clone.navigate_replace(Screen::Home).await;
                            let _ = ui_handle.upgrade_in_event_loop(|ui| {
                                ui.global::<AppBridge>().set_current_screen("home".into());
                            });

                            if let Err(e) = load_home_data(
                                ui_handle.clone(),
                                client_clone.clone(),
                                image_clone.clone(),
                                state_clone.clone(),
                            )
                            .await
                            {
                                error!("Failed to load home after auto-login: {}", e);
                            }
                            authenticated = true;
                        }
                        Err(e) => {
                            warn!("Auto-login failed: {}. Showing login screen.", e);
                        }
                    }
                }
            }

            if !authenticated {
                load_public_users(ui_handle, client_clone, image_clone).await;
                }
        });
    }

    // 9-10. Controller input handled by external unified-controller.py
    // which sends keyboard events via uinput. The Slint FocusScope
    // handles arrow keys, enter, escape natively. No internal evdev needed.
    drop(controller);
    drop(input_rx);

    // 11. Spawn idle timer for screensaver
    {
        let ui_weak = ui.as_weak();
        let state_clone = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                if state_clone.tick_idle().await {
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_show_screensaver(true);
                    });
                }
            }
        });
    }

    // 11a. RSS self-monitoring: prevent OOM
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            if let Ok(status) = tokio::fs::read_to_string("/proc/self/status").await {
                for line in status.lines() {
                    if line.starts_with("VmRSS:") {
                        let kb: u64 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                        let mb = kb / 1024;
                        if mb > RSS_EMERGENCY_EXIT_MB {
                            log::error!(
                                "RSS {}MB exceeds {}MB emergency limit — exiting to prevent OOM",
                                mb,
                                RSS_EMERGENCY_EXIT_MB
                            );
                            std::process::exit(1);
                        } else if mb > RSS_SOFT_LIMIT_MB {
                            log::error!(
                                "RSS {}MB exceeds {}MB soft limit — keeping app alive while cache trimming runs",
                                mb,
                                RSS_SOFT_LIMIT_MB
                            );
                        } else if mb > RSS_WARN_MB {
                            log::warn!("RSS {}MB above warning threshold {}MB", mb, RSS_WARN_MB);
                        } else if mb > 500 {
                            log::info!("RSS: {}MB", mb);
                        }
                    }
                }
            }
        }
    });

    // 11a-bis. Periodic cache clearing when RSS is high (runs on Slint event loop since ImageCache is !Send)
    {
        let image_cache_rss = image_cache.clone();
        let _ = slint::spawn_local(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
                if let Ok(status) = tokio::fs::read_to_string("/proc/self/status").await {
                    for line in status.lines() {
                        if line.starts_with("VmRSS:") {
                            let kb: u64 = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                            let mb = kb / 1024;
                            if mb > RSS_CACHE_CLEAR_MB {
                                log::warn!(
                                    "RSS {}MB > {}MB — clearing image memory cache",
                                    mb,
                                    RSS_CACHE_CLEAR_MB
                                );
                                image_cache_rss.clear_memory_cache().await;
                            }
                        }
                    }
                }
            }
        });
    }

    // 11b. Spawn daemon event consumer
    if let Some(mut daemon_rx) = daemon_event_rx {
        tokio::spawn(async move {
            while let Some(event) = daemon_rx.recv().await {
                match event {
                    daemon::DaemonEvent::BufferReady { item_id, path } => {
                        info!("Daemon: buffer ready for {}: {}", item_id, path);
                    }
                    daemon::DaemonEvent::BandwidthUpdated(profile) => {
                        debug!("Daemon: bandwidth updated: {}bps video", profile.video_bitrate);
                    }
                    daemon::DaemonEvent::BitrateAdapted { video_bitrate, audio_bitrate } => {
                        info!("Daemon: bitrate adapted: video={}bps audio={}bps", video_bitrate, audio_bitrate);
                    }
                    daemon::DaemonEvent::QosEnabled => {
                        info!("Daemon: QoS streaming mode enabled");
                    }
                    daemon::DaemonEvent::QosDisabled => {
                        info!("Daemon: QoS streaming mode disabled");
                    }
                }
            }
        });
    }

    // 12. Run Slint event loop (blocks)
    ui.run()?;

    info!("Jellyfin TV shutting down");
    Ok(())
}

// =============================================================================
// Callback Setup Functions
// =============================================================================

fn setup_navigation_callbacks(
    ui: &AppWindow,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    state: Arc<StateManager>,
    daemon_screen_tx: tokio::sync::watch::Sender<String>,
) {
    let detail_load_in_flight = Arc::new(AtomicBool::new(false));

    // navigate(screen, param)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    let state_clone = state.clone();
    let detail_flag_clone = detail_load_in_flight.clone();
    ui.global::<AppBridge>().on_navigate(move |screen, param| {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let image_cache = image_clone.clone();
        let state = state_clone.clone();
        let detail_load_in_flight = detail_flag_clone.clone();
        let screen_str = screen.to_string();
        let param_str = param.to_string();

        // Notify daemon of screen change (for foreground-app tracking)
        let _ = daemon_screen_tx.send(screen_str.clone());

        let _ = slint::spawn_local(async move {
            debug!("Navigate requested: screen={}, param={}", screen_str, param_str);

            // Reset idle timer on navigation
            state.reset_idle().await;

            match screen_str.as_str() {
                "home" => {
                    state.navigate_to(Screen::Home).await;
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_current_screen("home".into());
                        ui.global::<AppBridge>().set_is_loading(true);
                    });
                    if let Err(e) = load_home_data(
                        ui_weak.clone(),
                        client,
                        image_cache,
                        state,
                    )
                    .await
                    {
                        error!("Failed to load home data: {}", e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.global::<AppBridge>()
                                .set_error_message(format!("Failed to load home: {}", e).into());
                            ui.global::<AppBridge>().set_is_loading(false);
                        });
                    }
                }
                "detail" => {
                    let item_id = param_str.clone();

                    // Prevent duplicate detail loads from repeated A/Enter presses.
                    if detail_load_in_flight.swap(true, Ordering::AcqRel) {
                        debug!("Ignoring duplicate detail navigation while load is in flight: {}", item_id);
                        return;
                    }

                    // Check if this is a CollectionFolder (library) — redirect to library screen
                    let is_collection_folder = {
                        let c = client.read().await;
                        match c.get_item(&item_id).await {
                            Ok(item) => item.collection_type.is_some() || item.item_type == "CollectionFolder",
                            Err(_) => false,
                        }
                    };

                    if is_collection_folder {
                        // Navigate to library screen instead
                        state
                            .navigate_to(Screen::Library {
                                library_id: item_id.clone(),
                                title: String::new(),
                            })
                            .await;
                        let _ = ui_weak.upgrade_in_event_loop(|ui| {
                            ui.global::<AppBridge>().set_current_screen("library".into());
                            ui.global::<AppBridge>().set_is_loading(true);
                        });
                        if let Err(e) = load_library(
                            ui_weak.clone(),
                            client,
                            image_cache,
                            &item_id,
                            None,
                            None,
                        )
                        .await
                        {
                            error!("Failed to load library {}: {}", item_id, e);
                            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                ui.global::<AppBridge>()
                                    .set_error_message(format!("Failed to load library: {}", e).into());
                                ui.global::<AppBridge>().set_is_loading(false);
                            });
                        }
                        detail_load_in_flight.store(false, Ordering::Release);
                        return;
                    }

                    state
                        .navigate_to(Screen::Detail {
                            item_id: item_id.clone(),
                        })
                        .await;
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_current_screen("detail".into());
                        ui.global::<AppBridge>().set_is_loading(true);
                    });
                    if let Err(e) = load_item_detail(
                        ui_weak.clone(),
                        client,
                        image_cache,
                        &item_id,
                    )
                    .await
                    {
                        error!("Failed to load detail for {}: {}", item_id, e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.global::<AppBridge>()
                                .set_error_message(format!("Failed to load details: {}", e).into());
                            ui.global::<AppBridge>().set_is_loading(false);
                        });
                    }

                    detail_load_in_flight.store(false, Ordering::Release);
                }
                "library" => {
                    let library_id = param_str.clone();
                    // We get the title later from the API
                    state
                        .navigate_to(Screen::Library {
                            library_id: library_id.clone(),
                            title: String::new(),
                        })
                        .await;
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_current_screen("library".into());
                        ui.global::<AppBridge>().set_is_loading(true);
                    });
                    if let Err(e) = load_library(
                        ui_weak.clone(),
                        client,
                        image_cache,
                        &library_id,
                        None,
                        None,
                    )
                    .await
                    {
                        error!("Failed to load library {}: {}", library_id, e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.global::<AppBridge>()
                                .set_error_message(format!("Failed to load library: {}", e).into());
                            ui.global::<AppBridge>().set_is_loading(false);
                        });
                    }
                }
                "search" => {
                    state.navigate_to(Screen::Search).await;
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_current_screen("search".into());
                    });
                }
                "settings" => {
                    state.navigate_to(Screen::Settings).await;
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_current_screen("settings".into());
                    });
                }
                "player" => {
                    // Play item is handled by the play-item callback
                    // but if navigated directly, treat param as item_id
                    if !param_str.is_empty() {
                        let _ = ui_weak.upgrade_in_event_loop(|ui| {
                            ui.global::<AppBridge>().set_is_loading(true);
                        });
                        // Playback is initiated by play-item callback
                    }
                }
                other => {
                    warn!("Unknown navigation target: {}", other);
                }
            }
        });
    });

    // go-back()
    let ui_weak = ui.as_weak();
    let state_clone = state.clone();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    let detail_flag_clone = detail_load_in_flight.clone();
    ui.global::<AppBridge>().on_go_back(move || {
        let ui_weak = ui_weak.clone();
        let state = state_clone.clone();
        let client = client_clone.clone();
        let image_cache = image_clone.clone();
        let detail_load_in_flight = detail_flag_clone.clone();

        let _ = slint::spawn_local(async move {
            // Clear error message on back
            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                ui.global::<AppBridge>().set_error_message("".into());
                ui.global::<AppBridge>().set_is_loading(false);
            });

            // Allow navigating to detail again immediately after a cancellation/back action.
            detail_load_in_flight.store(false, Ordering::Release);

            // Dismiss screensaver if active
            {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    if ui.global::<AppBridge>().get_show_screensaver() {
                        ui.global::<AppBridge>().set_show_screensaver(false);
                        return;
                    }
                });
            }

            state.reset_idle().await;

            if let Some(screen) = state.go_back().await {
                let screen_name = screen.name().to_string();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>()
                        .set_current_screen(SharedString::from(&screen_name));
                });

                // Keep back-navigation immediate. Home content is already cached in
                // the UI model, so avoid forcing a synchronous reload here.
            }
        });
    });

    // library-selected(library_id)
    let ui_weak = ui.as_weak();
    ui.global::<AppBridge>().on_library_selected({
        let ui_weak = ui_weak.clone();
        move |library_id| {
            let ui_weak = ui_weak.clone();
            let library_id_str = library_id.to_string();
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<AppBridge>().invoke_navigate("library".into(), library_id_str.into());
            });
        }
    });
}

fn setup_auth_callbacks(
    ui: &AppWindow,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    state: Arc<StateManager>,
    config: Arc<RwLock<AppConfig>>,
    daemon_mgr: Arc<Mutex<daemon::DaemonManager>>,
) {
    // login(user_id, password)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    let state_clone = state.clone();
    let config_clone = config.clone();
    let daemon_mgr_clone = daemon_mgr.clone();
    ui.global::<AppBridge>().on_login(move |user_id, password| {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let image_cache = image_clone.clone();
        let state = state_clone.clone();
        let config = config_clone.clone();
        let daemon_mgr = daemon_mgr_clone.clone();
        let user_id_str = user_id.to_string();
        let password_str = password.to_string();

        let _ = slint::spawn_local(async move {
            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                ui.global::<AppBridge>().set_is_loading(true);
                ui.global::<AppBridge>().set_error_message("".into());
            });

            info!("Login attempt for user_id: {}", user_id_str);

            // Fetch the user's name for authentication
            // The login screen passes user_id; we need to find the username
            let username = {
                let c = client.read().await;
                match c.get_public_users().await {
                    Ok(users) => users
                        .iter()
                        .find(|u| u.id == user_id_str)
                        .map(|u| u.name.clone())
                        .unwrap_or_else(|| user_id_str.clone()),
                    Err(_) => user_id_str.clone(),
                }
            };

            let auth_result = {
                let mut c = client.write().await;
                c.authenticate(&username, &password_str).await
            };

            match auth_result {
                Ok(result) => {
                    info!("Login successful for user: {}", username);

                    // Update state with authenticated user
                    state
                        .set_user(result.user.clone(), result.access_token.clone())
                        .await;

                    // Save credentials for auto-login
                    {
                        let mut cfg = config.write().await;
                        cfg.save_auth(&result.user.id, &result.access_token);
                    }

                    // Set current user in UI
                    let server_url = {
                        let c = client.read().await;
                        c.server_url.clone()
                    };
                    let avatar =
                        load_user_avatar(&result.user, &server_url, &image_cache).await;
                    let user_info = user_dto_to_user_info(&result.user, &server_url, avatar);
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.global::<AppBridge>().set_current_user(user_info);
                    }

                    // Start daemon tasks now that we're authenticated
                    daemon_mgr.lock().await.start(client.clone(), config.clone(), state.clone());

                    // Navigate to home and load data
                    state.navigate_replace(Screen::Home).await;
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().set_current_screen("home".into());
                    });

                    if let Err(e) = load_home_data(
                        ui_weak.clone(),
                        client.clone(),
                        image_cache,
                        state,
                    )
                    .await
                    {
                        error!("Failed to load home after login: {}", e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.global::<AppBridge>()
                                .set_error_message(format!("Failed to load home: {}", e).into());
                            ui.global::<AppBridge>().set_is_loading(false);
                        });
                    }
                }
                Err(e) => {
                    error!("Login failed: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>()
                            .set_error_message(format!("Login failed: {}", e).into());
                        ui.global::<AppBridge>().set_is_loading(false);
                    });
                }
            }
        });
    });

    // logout()
    let ui_weak = ui.as_weak();
    let state_clone = state.clone();
    let config_clone = config.clone();
    let client_clone = client.clone();
    ui.global::<AppBridge>().on_logout(move || {
        let ui_weak = ui_weak.clone();
        let state = state_clone.clone();
        let config = config_clone.clone();
        let client = client_clone.clone();

        tokio::spawn(async move {
            info!("Logout requested");

            // Clear client credentials
            {
                let mut c = client.write().await;
                c.access_token = None;
                c.user_id = None;
            }

            // Clear saved credentials
            {
                let mut cfg = config.write().await;
                cfg.clear_auth();
            }

            // Reset state
            state.logout().await;

            // Navigate to login screen
            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                ui.global::<AppBridge>().set_current_screen("login".into());
                ui.global::<AppBridge>().set_home_rows(ModelRc::default());
                ui.global::<AppBridge>().set_search_results(ModelRc::default());
                ui.global::<AppBridge>().set_library_items(ModelRc::default());
                ui.global::<AppBridge>().set_error_message("".into());
            });

            info!("Logout complete");
        });
    });
}

fn setup_playback_callbacks(
    ui: &AppWindow,
    client: Arc<RwLock<JellyfinClient>>,
    state: Arc<StateManager>,
    player: Arc<Mutex<Option<PlayerWrapper>>>,
    config: Arc<RwLock<AppConfig>>,
    daemon_player_tx: mpsc::UnboundedSender<PlayerEvent>,
    tracker: Arc<PlaybackTracker>,
    segments: Arc<Mutex<SegmentManager>>,
    playback_controls: Arc<Mutex<PlaybackControls>>,
    queue: Arc<Mutex<PlaybackQueue>>,
) {
    // play-item(item_id)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let state_clone = state.clone();
    let player_clone = player.clone();
    let config_for_play = config.clone();
    let tracker_clone = tracker.clone();
    let segments_clone = segments.clone();
    let playback_controls_clone = playback_controls.clone();
    let queue_clone = queue.clone();
    ui.global::<AppBridge>().on_play_item(move |item_id| {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let state = state_clone.clone();
        let player = player_clone.clone();
        let config_for_play2 = config_for_play.clone();
        let daemon_player_tx = daemon_player_tx.clone();        let tracker = tracker_clone.clone();
        let segments = segments_clone.clone();
        let playback_controls = playback_controls_clone.clone();
        let queue = queue_clone.clone();
        let item_id_str = item_id.to_string();

        tokio::spawn(async move {
            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                ui.global::<AppBridge>().set_is_loading(true);
            });

            info!("Play item requested: {}", item_id_str);

            // Get playback info from Jellyfin
            let playback_info = {
                let c = client.read().await;
                c.get_playback_info(&item_id_str).await
            };

            match playback_info {
                Ok(info) => {
                    // Pick the best media source
                    let media_source = match info.media_sources.first() {
                        Some(ms) => ms,
                        None => {
                            error!("No media sources available for item {}", item_id_str);
                            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                                ui.global::<AppBridge>()
                                    .set_error_message("No media sources available".into());
                                ui.global::<AppBridge>().set_is_loading(false);
                            });
                            return;
                        }
                    };

                    let session_id = info
                        .play_session_id
                        .clone()
                        .unwrap_or_default();
                    let media_source_id = media_source.id.clone();

                    // Build the stream URL
                    let server_url = {
                        let c = client.read().await;
                        c.server_url.clone()
                    };
                    let access_token = {
                        let c = client.read().await;
                        c.access_token.clone().unwrap_or_default()
                    };

                    let stream_url = if media_source
                        .supports_direct_play
                        .unwrap_or(false)
                    {
                        // Direct play
                        format!(
                            "{}/Videos/{}/stream?Static=true&MediaSourceId={}&api_key={}",
                            server_url, item_id_str, media_source_id, access_token
                        )
                    } else if let Some(ref transcode_url) = media_source.transcoding_url {
                        // Transcoding
                        format!("{}{}", server_url, transcode_url)
                    } else if media_source
                        .supports_direct_stream
                        .unwrap_or(false)
                    {
                        // Direct stream
                        format!(
                            "{}/Videos/{}/stream?Static=true&MediaSourceId={}&api_key={}",
                            server_url, item_id_str, media_source_id, access_token
                        )
                    } else {
                        error!("No playable source found for item {}", item_id_str);
                        let _ = ui_weak.upgrade_in_event_loop(|ui| {
                            ui.global::<AppBridge>()
                                .set_error_message("No playable source found".into());
                            ui.global::<AppBridge>().set_is_loading(false);
                        });
                        return;
                    };

                    let play_method = if media_source
                        .supports_direct_play
                        .unwrap_or(false)
                    {
                        "DirectPlay"
                    } else if media_source.transcoding_url.is_some() {
                        "Transcode"
                    } else {
                        "DirectStream"
                    };

                    // Get the item details for title display
                    let item_detail = {
                        let c = client.read().await;
                        c.get_item(&item_id_str).await.ok()
                    };

                    // Get resume position
                    let start_position_ms = item_detail
                        .as_ref()
                        .and_then(|item| item.user_data.as_ref())
                        .map(|ud| ud.playback_position_ticks / 10_000)
                        .unwrap_or(0);

                    // Update state
                    state
                        .start_playback(
                            item_id_str.clone(),
                            session_id.clone(),
                            media_source_id.clone(),
                        )
                        .await;

                    // Create player (always fresh — respects current default_player setting)
                    let vlc_result = {
                        let mut p = player.lock().await;
                        // Stop existing player if any
                        if let Some(ref old) = *p {
                            let _ = old.stop().await;
                        }
                        *p = None;

                        let cfg = config_for_play2.read().await;
                        let player_type = cfg.playback.default_player.clone();
                        drop(cfg);

                        let new_result = if player_type == "mpv" {
                            PlayerWrapper::new_mpv()
                        } else {
                            PlayerWrapper::new_vlc()
                        };
                        match new_result {
                            Ok(new_player) => {
                                info!("Created {} player", player_type);
                                *p = Some(new_player);
                            }
                            Err(e) => {
                                error!("Failed to create {} player: {}", player_type, e);
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                    ui.global::<AppBridge>().set_error_message(
                                        format!("Player error: {}", e).into(),
                                    );
                                    ui.global::<AppBridge>().set_is_loading(false);
                                });
                                return;
                            }
                        }
                        Ok::<(), ()>(())
                    };

                    if vlc_result.is_err() {
                        return;
                    }

                    // Start playback
                    {
                        let p = player.lock().await;
                        if let Some(ref vlc) = *p {
                            let start_ms = if start_position_ms > 0 {
                                Some(start_position_ms)
                            } else {
                                None
                            };
                            if let Err(e) = vlc.play_url(&stream_url, start_ms).await {
                                error!("Failed to start playback: {}", e);
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                    ui.global::<AppBridge>()
                                        .set_error_message(format!("Playback error: {}", e).into());
                                    ui.global::<AppBridge>().set_is_loading(false);
                                });
                                return;
                            }
                        }
                    }

                    // Report playback start to Jellyfin
                    {
                        let c = client.read().await;
                        let start_info = PlaybackStartInfo {
                            item_id: item_id_str.clone(),
                            media_source_id: Some(media_source_id.clone()),
                            play_session_id: Some(session_id.clone()),
                            play_method: play_method.to_string(),
                            position_ticks: start_position_ms * 10_000,
                            can_seek: true,
                            is_paused: false,
                            is_muted: false,
                            audio_stream_index: None,
                            subtitle_stream_index: None,
                        };
                        if let Err(e) = c.report_playback_start(&start_info).await {
                            warn!("Failed to report playback start: {}", e);
                        }
                    }

                    // Record in local playback tracker
                    {
                        let user_name = {
                            let s = state.get_state().await;
                            s.current_user.as_ref().map(|u| u.name.clone()).unwrap_or_default()
                        };
                        let user_id = {
                            let s = state.get_state().await;
                            s.current_user.as_ref().map(|u| u.id.clone()).unwrap_or_default()
                        };
                        let title_str = item_detail.as_ref().map(|i| i.name.as_str()).unwrap_or("Unknown");
                        let series = item_detail.as_ref().and_then(|i| i.series_name.as_deref());
                        let se = item_detail.as_ref().and_then(|i| {
                            let s = i.parent_index_number?;
                            let e = i.index_number?;
                            Some(format!("S{:02}E{:02}", s, e))
                        });
                        let runtime = item_detail.as_ref().and_then(|i| i.run_time_ticks);
                        match tracker.start_session(
                            &user_name,
                            &user_id,
                            &item_id_str,
                            title_str,
                            series,
                            se.as_deref(),
                            "pi5-home-A",
                            play_method,
                            runtime,
                        ) {
                            Ok(sid) => {
                                state.set_tracking_session(Some(sid)).await;
                                info!("Tracking session started: #{}", sid);
                            }
                            Err(e) => warn!("Tracking: failed to start session: {}", e),
                        }
                    }

                    // Load media segments for intro/credits skip
                    {
                        let c = client.read().await;
                        if let Ok(segs) = c.get_media_segments(&item_id_str).await {
                            let mut sm = segments.lock().await;
                            sm.set_segments(segs);
                        }
                    }
                    // Reset playback controls for new item
                    {
                        let mut pc = playback_controls.lock().await;
                        let cmds = pc.reset_all();
                        let p = player.lock().await;
                        if let Some(ref vlc) = *p {
                            for cmd in cmds {
                                let _ = vlc.send_command(&cmd).await;
                            }
                        }
                    }

                    // Set up player state in UI
                    let title = item_detail
                        .as_ref()
                        .map(|i| i.name.clone())
                        .unwrap_or_default();
                    let subtitle = item_detail
                        .as_ref()
                        .and_then(|i| i.series_name.clone())
                        .unwrap_or_default();

                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>().set_current_screen("player".into());
                        ui.global::<AppBridge>().set_is_loading(false);
                        let ps = PlayerState {
                            is_playing: true,
                            is_paused: false,
                            position_ms: start_position_ms as i32,
                            duration_ms: 0,
                            title: SharedString::from(title),
                            subtitle: SharedString::from(subtitle),
                            audio_tracks: ModelRc::default(),
                            subtitle_tracks: ModelRc::default(),
                            current_audio: 0,
                            current_subtitle: 0,
                            volume: 100.0,
                            is_muted: false,
                            buffering_percent: 0,
                            is_buffering: false,
                        };
                        ui.global::<AppBridge>().set_player_state(ps);
                    });

                    // Spawn VLC event loop handler
                    let ui_weak_ev = ui_weak.clone();
                    let client_ev = client.clone();
                    let state_ev = state.clone();
                    let player_ev = player.clone();
                    let daemon_tx_ev = daemon_player_tx.clone();
                    let tracker_ev = tracker.clone();
                    let segments_ev = segments.clone();
                    let controls_ev = playback_controls.clone();
                    let queue_ev = queue.clone();
                    tokio::spawn(async move {
                        handle_player_events(
                            ui_weak_ev,
                            client_ev,
                            state_ev,
                            player_ev,
                            daemon_tx_ev,
                            tracker_ev,
                            segments_ev,
                            controls_ev,
                            queue_ev,
                        )
                        .await;
                    });
                }
                Err(e) => {
                    error!("Failed to get playback info: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>()
                            .set_error_message(format!("Cannot play: {}", e).into());
                        ui.global::<AppBridge>().set_is_loading(false);
                    });
                }
            }
        });
    });

    // play-pause()
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_play_pause(move || {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                if let Err(e) = vlc.toggle_pause().await {
                    error!("Toggle pause failed: {}", e);
                }
            }
        });
    });

    // seek(position_ms)
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_seek(move |position_ms| {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                if let Err(e) = vlc.seek_to(position_ms as i64).await {
                    error!("Seek failed: {}", e);
                }
            }
        });
    });

    // stop-playback()
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let state_clone = state.clone();
    let player_clone = player.clone();
    let tracker_clone2 = tracker.clone();
    ui.global::<AppBridge>().on_stop_playback(move || {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let state = state_clone.clone();
        let player = player_clone.clone();
        let tracker = tracker_clone2.clone();

        tokio::spawn(async move {
            info!("Stop playback requested");

            // Get current position for reporting
            let position_ticks = {
                let p = player.lock().await;
                if let Some(ref vlc) = *p {
                    vlc.get_position_ms().await.unwrap_or(0) * 10_000
                } else {
                    0
                }
            };

            // Stop VLC and drop player (terminates event loop task)
            {
                let mut p = player.lock().await;
                if let Some(ref vlc) = *p {
                    let _ = vlc.stop().await;
                }
                *p = None;
            }

            // Report playback stopped to Jellyfin
            let app_state = state.get_state().await;
            if let (Some(item_id), Some(session_id)) = (
                app_state.playing_item_id.as_ref(),
                app_state.play_session_id.as_ref(),
            ) {
                let stop_info = PlaybackStopInfo {
                    item_id: item_id.clone(),
                    media_source_id: app_state.playing_media_source_id.clone(),
                    play_session_id: Some(session_id.clone()),
                    position_ticks,
                };
                let c = client.read().await;
                if let Err(e) = c.report_playback_stopped(&stop_info).await {
                    warn!("Failed to report playback stopped: {}", e);
                }
            }

            // End tracking session
            if let Some(tid) = state.get_tracking_session().await {
                let runtime = {
                    let c = client.read().await;
                    if let Some(ref item_id) = state.get_state().await.playing_item_id {
                        c.get_item(item_id).await.ok().and_then(|i| i.run_time_ticks)
                    } else {
                        None
                    }
                };
                tracker.end_session(tid, position_ticks, runtime);
            }

            // Update state and navigate back
            state.stop_playback().await;
            let current = state.current_screen_name().await;
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<AppBridge>()
                    .set_current_screen(SharedString::from(&current));
            });
        });
    });

    // next-track()
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_next_track(move || {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                // Cycle to next audio track
                if let Ok((audio_tracks, _)) = vlc.get_tracks().await {
                    let current_aid: i32 = 1; // default
                    let next = audio_tracks
                        .iter()
                        .find(|t| t.id > current_aid)
                        .or_else(|| audio_tracks.first());
                    if let Some(track) = next {
                        let _ = vlc.set_audio_track(track.id).await;
                        info!("Switched to audio track {}: {}", track.id, track.title);
                    }
                }
            }
        });
    });

    // prev-track()
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_prev_track(move || {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                // Cycle subtitle tracks
                if let Ok((_, sub_tracks)) = vlc.get_tracks().await {
                    let current_sid: i32 = 0;
                    if sub_tracks.is_empty() {
                        return;
                    }
                    let next = sub_tracks
                        .iter()
                        .find(|t| t.id > current_sid)
                        .or_else(|| sub_tracks.first());
                    if let Some(track) = next {
                        let _ = vlc.set_subtitle_track(track.id).await;
                        info!("Switched to subtitle track {}: {}", track.id, track.title);
                    }
                }
            }
        });
    });

    // set-audio-track(index)
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_set_audio_track(move |index| {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                if let Err(e) = vlc.set_audio_track(index).await {
                    error!("Set audio track failed: {}", e);
                }
            }
        });
    });

    // set-subtitle-track(index)
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_set_subtitle_track(move |index| {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                if let Err(e) = vlc.set_subtitle_track(index).await {
                    error!("Set subtitle track failed: {}", e);
                }
            }
        });
    });

    // set-volume(level)
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_set_volume(move |level| {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                if let Err(e) = vlc.set_volume(level as f64).await {
                    error!("Set volume failed: {}", e);
                }
            }
        });
    });

    // toggle-mute()
    let player_clone = player.clone();
    ui.global::<AppBridge>().on_toggle_mute(move || {
        let player = player_clone.clone();
        tokio::spawn(async move {
            let p = player.lock().await;
            if let Some(ref vlc) = *p {
                if let Err(e) = vlc.toggle_mute().await {
                    error!("Toggle mute failed: {}", e);
                }
            }
        });
    });

    // =========================================================================
    // New module callbacks: segments, controls, queue
    // =========================================================================

    // skip-segment() -- skip intro/credits
    {
        let segments_clone = segments.clone();
        let player_clone = player.clone();
        ui.global::<AppBridge>().on_skip_segment(move || {
            let segments = segments_clone.clone();
            let player = player_clone.clone();
            tokio::spawn(async move {
                let position_ms = {
                    let p = player.lock().await;
                    if let Some(ref vlc) = *p {
                        vlc.get_position_ms().await.unwrap_or(0)
                    } else { 0 }
                };
                let position_ticks = position_ms * 10_000;
                let skip_target = {
                    let mut sm = segments.lock().await;
                    if let Some(seg_id) = sm.active_segment_id(position_ticks).map(|s| s.to_owned()) {
                        sm.mark_skipped(&seg_id);
                    }
                    sm.get_skip_target(position_ticks)
                };
                if let Some(target_ticks) = skip_target {
                    let target_ms = target_ticks / 10_000;
                    let p = player.lock().await;
                    if let Some(ref vlc) = *p {
                        if let Err(e) = vlc.seek_to(target_ms).await {
                            error!("Skip segment seek failed: {}", e);
                        } else {
                            info!("Skipped segment, seeked to {}ms", target_ms);
                        }
                    }
                }
            });
        });
    }

    // speed-up()
    {
        let controls_clone = playback_controls.clone();
        let player_clone = player.clone();
        let ui_weak = ui.as_weak();
        ui.global::<AppBridge>().on_speed_up(move || {
            let controls = controls_clone.clone();
            let player = player_clone.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                let (cmd, label) = {
                    let mut pc = controls.lock().await;
                    let cmd = pc.speed_up();
                    let label = pc.speed_label();
                    (cmd, label)
                };
                let p = player.lock().await;
                if let Some(ref vlc) = *p {
                    let _ = vlc.send_command(&cmd).await;
                }
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>().set_speed_label(label.into());
                });
            });
        });
    }

    // speed-down()
    {
        let controls_clone = playback_controls.clone();
        let player_clone = player.clone();
        let ui_weak = ui.as_weak();
        ui.global::<AppBridge>().on_speed_down(move || {
            let controls = controls_clone.clone();
            let player = player_clone.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                let (cmd, label) = {
                    let mut pc = controls.lock().await;
                    let cmd = pc.speed_down();
                    let label = pc.speed_label();
                    (cmd, label)
                };
                let p = player.lock().await;
                if let Some(ref vlc) = *p {
                    let _ = vlc.send_command(&cmd).await;
                }
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>().set_speed_label(label.into());
                });
            });
        });
    }


    // Toggle media player (VLC <-> MPV)
    {
        let config_toggle = config.clone();
        let ui_weak = ui.as_weak();
        ui.global::<AppBridge>().on_toggle_media_player(move || {
            let config_inner = config_toggle.clone();
            let ui_weak_inner = ui_weak.clone();
            let _ = slint::spawn_local(async move {
                let mut cfg = config_inner.write().await;
                cfg.playback.default_player = if cfg.playback.default_player == "vlc" {
                    "mpv".to_string()
                } else {
                    "vlc".to_string()
                };
                let player_name = cfg.playback.default_player.clone();
                let _ = cfg.save();
                info!("Default media player changed to: {}", player_name);
                let _ = ui_weak_inner.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>().set_media_player(
                        slint::SharedString::from(&player_name)
                    );
                });
            });
        });
    }

    // subtitle-delay-adjust(delta_ms)
    {
        let controls_clone = playback_controls.clone();
        let player_clone = player.clone();
        ui.global::<AppBridge>().on_subtitle_delay_adjust(move |delta_ms| {
            let controls = controls_clone.clone();
            let player = player_clone.clone();
            tokio::spawn(async move {
                let cmd = {
                    let mut pc = controls.lock().await;
                    pc.adjust_subtitle_delay(delta_ms as i64)
                };
                let p = player.lock().await;
                if let Some(ref vlc) = *p {
                    let _ = vlc.send_command(&cmd).await;
                }
            });
        });
    }

    // show-chapters()
    {
        let player_clone = player.clone();
        let ui_weak = ui.as_weak();
        ui.global::<AppBridge>().on_show_chapters(move || {
            let player = player_clone.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                let p = player.lock().await;
                if let Some(ref vlc) = *p {
                    match vlc.get_chapter_count().await {
                        Ok(count) => {
                            info!("Chapter count: {}", count);
                            // Chapter data is provided through the chapters property
                            // which is populated elsewhere
                        }
                        Err(e) => {
                            warn!("Failed to get chapters: {}", e);
                        }
                    }
                }
            });
        });
    }

    // seek-to-chapter(index)
    {
        let player_clone = player.clone();
        ui.global::<AppBridge>().on_seek_to_chapter(move |index| {
            let player = player_clone.clone();
            tokio::spawn(async move {
                let p = player.lock().await;
                if let Some(ref vlc) = *p {
                    if let Err(e) = vlc.set_chapter(index).await {
                        error!("Seek to chapter {} failed: {}", index, e);
                    } else {
                        info!("Seeked to chapter {}", index);
                    }
                }
            });
        });
    }

    // queue-play-item(index)
    {
        let queue_clone = queue.clone();
        let ui_weak = ui.as_weak();
        ui.global::<AppBridge>().on_queue_play_item(move |index| {
            let queue = queue_clone.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                let item_id = {
                    let mut q = queue.lock().await;
                    q.skip_to(index as usize).map(|item| item.item_id.clone())
                };
                if let Some(id) = item_id {
                    info!("Queue: playing item at index {}: {}", index, id);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>().invoke_play_item(id.into());
                    });
                }
            });
        });
    }

    // queue-remove-item(index)
    {
        let queue_clone = queue.clone();
        ui.global::<AppBridge>().on_queue_remove_item(move |index| {
            let queue = queue_clone.clone();
            tokio::spawn(async move {
                let mut q = queue.lock().await;
                q.remove(index as usize);
                info!("Queue: removed item at index {}", index);
            });
        });
    }

    // toggle-shuffle()
    {
        let queue_clone = queue.clone();
        ui.global::<AppBridge>().on_toggle_shuffle(move || {
            let queue = queue_clone.clone();
            tokio::spawn(async move {
                let mut q = queue.lock().await;
                let shuffled = q.toggle_shuffle();
                info!("Queue: shuffle = {}", shuffled);
            });
        });
    }

    // cycle-repeat()
    {
        let queue_clone = queue.clone();
        let ui_weak = ui.as_weak();
        ui.global::<AppBridge>().on_cycle_repeat(move || {
            let queue = queue_clone.clone();
            let ui_weak = ui_weak.clone();
            tokio::spawn(async move {
                let mode = {
                    let mut q = queue.lock().await;
                    q.cycle_repeat()
                };
                let mode_int = match mode {
                    queue::RepeatMode::None => 0,
                    queue::RepeatMode::RepeatOne => 1,
                    queue::RepeatMode::RepeatAll => 2,
                };
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>().set_repeat_mode(mode_int);
                });
                info!("Queue: repeat mode = {:?}", mode);
            });
        });
    }

    // enqueue-next(item_id) -- add item to play after current
    {
        let queue_clone = queue.clone();
        let client_clone = client.clone();
        ui.global::<AppBridge>().on_enqueue_next(move |item_id| {
            let queue = queue_clone.clone();
            let client = client_clone.clone();
            let item_id_str = item_id.to_string();
            tokio::spawn(async move {
                // Fetch item details to build QueueItem
                let c = client.read().await;
                if let Ok(item) = c.get_item(&item_id_str).await {
                    let server_url = c.server_url.clone();
                    drop(c);
                    let qi = QueueItem::from_dto(&item, &server_url);
                    let mut q = queue.lock().await;
                    q.play_next(qi);
                    info!("Queue: enqueued '{}' to play next", item.name);
                }
            });
        });
    }
}

fn setup_content_callbacks(
    ui: &AppWindow,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    state: Arc<StateManager>,
) {
    // request-home-data()
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    let state_clone = state.clone();
    ui.global::<AppBridge>().on_request_home_data(move || {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let image_cache = image_clone.clone();
        let state = state_clone.clone();

        let _ = slint::spawn_local(async move {
            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                ui.global::<AppBridge>().set_is_loading(true);
            });
            if let Err(e) =
                load_home_data(ui_weak.clone(), client, image_cache, state).await
            {
                error!("Failed to load home data: {}", e);
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>()
                        .set_error_message(format!("Failed to refresh: {}", e).into());
                    ui.global::<AppBridge>().set_is_loading(false);
                });
            }
        });
    });

    // request-library(library_id, sort, filter)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    let state_clone = state.clone();
    ui.global::<AppBridge>().on_request_library(
        move |library_id, sort, filter| {
            let ui_weak = ui_weak.clone();
            let client = client_clone.clone();
            let image_cache = image_clone.clone();
            let state = state_clone.clone();
            let library_id_str = library_id.to_string();
            let sort_str = sort.to_string();
            let filter_str = filter.to_string();

            let _ = slint::spawn_local(async move {
                // If library_id is empty (from sort/filter change), get it from state
                let library_id_str = if library_id_str.is_empty() {
                    state.get_screen_param().await.unwrap_or_default()
                } else {
                    library_id_str
                };
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>().set_is_loading(true);
                });
                // Map UI labels to API values
                let mapped_sort = if sort_str.is_empty() {
                    String::new()
                } else {
                    map_sort_label(&sort_str).to_string()
                };
                let sort_opt = if mapped_sort.is_empty() {
                    None
                } else {
                    Some(mapped_sort.as_str())
                };
                let mapped_filter = if filter_str.is_empty() {
                    String::new()
                } else {
                    map_filter_label(&filter_str).to_string()
                };
                let filter_opt = if mapped_filter.is_empty() {
                    None
                } else {
                    Some(mapped_filter.as_str())
                };
                if let Err(e) = load_library(
                    ui_weak.clone(),
                    client,
                    image_cache,
                    &library_id_str,
                    sort_opt,
                    filter_opt,
                )
                .await
                {
                    error!("Failed to load library: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>()
                            .set_error_message(format!("Failed to load library: {}", e).into());
                        ui.global::<AppBridge>().set_is_loading(false);
                    });
                }
            });
        },
    );

    // request-item-detail(item_id)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    ui.global::<AppBridge>()
        .on_request_item_detail(move |item_id| {
            let ui_weak = ui_weak.clone();
            let client = client_clone.clone();
            let image_cache = image_clone.clone();
            let item_id_str = item_id.to_string();

            let _ = slint::spawn_local(async move {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>().set_is_loading(true);
                });
                if let Err(e) = load_item_detail(
                    ui_weak.clone(),
                    client,
                    image_cache,
                    &item_id_str,
                )
                .await
                {
                    error!("Failed to load item detail: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>()
                            .set_error_message(format!("Failed to load details: {}", e).into());
                        ui.global::<AppBridge>().set_is_loading(false);
                    });
                }
            });
        });

    // request-search(query)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    ui.global::<AppBridge>().on_request_search(move |query| {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let image_cache = image_clone.clone();
        let query_str = query.to_string();

        let _ = slint::spawn_local(async move {
            if query_str.trim().is_empty() {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>()
                        .set_search_results(ModelRc::default());
                });
                return;
            }

            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                ui.global::<AppBridge>().set_is_loading(true);
            });

            if let Err(e) =
                perform_search(ui_weak.clone(), client, image_cache, &query_str).await
            {
                error!("Search failed: {}", e);
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>()
                        .set_error_message(format!("Search failed: {}", e).into());
                    ui.global::<AppBridge>().set_is_loading(false);
                });
            }
        });
    });

    // request-seasons(series_id)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    ui.global::<AppBridge>().on_request_seasons(move |series_id| {
        let ui_weak = ui_weak.clone();
        let client = client_clone.clone();
        let image_cache = image_clone.clone();
        let series_id_str = series_id.to_string();

        let _ = slint::spawn_local(async move {
            if let Err(e) =
                load_seasons(ui_weak.clone(), client, image_cache, &series_id_str).await
            {
                error!("Failed to load seasons: {}", e);
            }
        });
    });

    // request-episodes(series_id, season_id)
    let ui_weak = ui.as_weak();
    let client_clone = client.clone();
    let image_clone = image_cache.clone();
    ui.global::<AppBridge>()
        .on_request_episodes(move |series_id, season_id| {
            let ui_weak = ui_weak.clone();
            let client = client_clone.clone();
            let image_cache = image_clone.clone();
            let series_id_str = series_id.to_string();
            let season_id_str = season_id.to_string();

            let _ = slint::spawn_local(async move {
                if let Err(e) = load_episodes(
                    ui_weak.clone(),
                    client,
                    image_cache,
                    &series_id_str,
                    &season_id_str,
                )
                .await
                {
                    error!("Failed to load episodes: {}", e);
                }
            });
        });
}

fn setup_user_action_callbacks(
    ui: &AppWindow,
    client: Arc<RwLock<JellyfinClient>>,
) {
    // toggle-favorite(item_id)
    let client_clone = client.clone();
    let ui_weak = ui.as_weak();
    ui.global::<AppBridge>().on_toggle_favorite(move |item_id| {
        let client = client_clone.clone();
        let item_id_str = item_id.to_string();
        let ui_weak = ui_weak.clone();

        tokio::spawn(async move {
            // First get current favorite status
            let (is_favorite, ok) = {
                let c = client.read().await;
                match c.get_item(&item_id_str).await {
                    Ok(item) => {
                        let fav = item
                            .user_data
                            .as_ref()
                            .map(|ud| ud.is_favorite)
                            .unwrap_or(false);
                        (fav, true)
                    }
                    Err(e) => {
                        error!("Failed to get item for favorite toggle: {}", e);
                        (false, false)
                    }
                }
            };

            if !ok {
                return;
            }

            // Toggle the opposite
            let new_state = !is_favorite;
            let c = client.read().await;
            match c.toggle_favorite(&item_id_str, new_state).await {
                Ok(()) => {
                    info!(
                        "Toggled favorite for {}: {} -> {}",
                        item_id_str, is_favorite, new_state
                    );
                    // Update the detail item if it's currently displayed
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        let mut detail = ui.global::<AppBridge>().get_detail_item();
                        if detail.id.as_str() == item_id_str {
                            detail.is_favorite = new_state;
                            ui.global::<AppBridge>().set_detail_item(detail);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to toggle favorite: {}", e);
                }
            }
        });
    });

    // mark-played(item_id)
    let client_clone = client.clone();
    let ui_weak = ui.as_weak();
    ui.global::<AppBridge>().on_mark_played(move |item_id| {
        let client = client_clone.clone();
        let item_id_str = item_id.to_string();
        let ui_weak = ui_weak.clone();

        tokio::spawn(async move {
            let c = client.read().await;
            match c.mark_played(&item_id_str).await {
                Ok(()) => {
                    info!("Marked {} as played", item_id_str);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        let mut detail = ui.global::<AppBridge>().get_detail_item();
                        if detail.id.as_str() == item_id_str {
                            detail.is_played = true;
                            ui.global::<AppBridge>().set_detail_item(detail);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to mark played: {}", e);
                }
            }
        });
    });

    // mark-unplayed(item_id)
    let client_clone = client.clone();
    let ui_weak = ui.as_weak();
    ui.global::<AppBridge>().on_mark_unplayed(move |item_id| {
        let client = client_clone.clone();
        let item_id_str = item_id.to_string();
        let ui_weak = ui_weak.clone();

        tokio::spawn(async move {
            let c = client.read().await;
            match c.mark_unplayed(&item_id_str).await {
                Ok(()) => {
                    info!("Marked {} as unplayed", item_id_str);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        let mut detail = ui.global::<AppBridge>().get_detail_item();
                        if detail.id.as_str() == item_id_str {
                            detail.is_played = false;
                            detail.progress = 0.0;
                            ui.global::<AppBridge>().set_detail_item(detail);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to mark unplayed: {}", e);
                }
            }
        });
    });
}

// =============================================================================
// Data Loading Functions
// =============================================================================

/// Fetch and display public users on the login screen.
async fn load_public_users(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
) {
    let server_url = {
        let c = client.read().await;
        c.server_url.clone()
    };

    let max_attempts_before_background_retry = 24;
    let is_transient_startup_error = |err_text: &str| {
        let lower = err_text.to_ascii_lowercase();
        lower.contains("503")
            || lower.contains("server is starting")
            || lower.contains("service unavailable")
            || lower.contains("network error")
            || lower.contains("timed out")
            || lower.contains("connection")
    };
    let mut users_result = None;
    let mut attempt: usize = 0;
    loop {
        attempt += 1;
        let result = {
            let c = client.read().await;
            c.get_public_users().await
        };

        match result {
            Ok(users) => {
                users_result = Some(Ok(users));
                break;
            }
            Err(e) => {
                let transient = is_transient_startup_error(&e.to_string());
                warn!(
                    "Failed to load public users (attempt {}/{}): {}",
                    attempt, max_attempts_before_background_retry, e
                );
                if !transient {
                    users_result = Some(Err(e));
                    break;
                }

                if attempt >= max_attempts_before_background_retry {
                    users_result = Some(Err(e));
                    break;
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        }
    }

    match users_result {
        Some(Ok(users)) => {
            info!("Loaded {} public users", users.len());
            let mut user_infos = Vec::with_capacity(users.len());
            for user in &users {
                let avatar = load_user_avatar(user, &server_url, &image_cache).await;
                user_infos.push(user_dto_to_user_info(user, &server_url, avatar));
            }

            if let Some(ui) = ui_weak.upgrade() {
                let model = VecModel::from(user_infos);
                ui.global::<AppBridge>()
                    .set_users(ModelRc::new(model));
            }
        }
        Some(Err(e)) => {
            error!("Failed to load public users: {}", e);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<AppBridge>()
                    .set_error_message(
                        format!(
                            "Cannot connect to server: {}",
                            e
                        )
                        .into(),
                    );
            });
        }
        None => {
            error!("Failed to load public users: exhausted retries with no result");
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<AppBridge>()
                    .set_error_message("Cannot connect to server".into());
            });
        }
    }
}

/// Fetch all home screen data (resume, next up, latest per library).
async fn load_home_data(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    state: Arc<StateManager>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let c = client.read().await;
    let server_url = c.server_url.clone();

    // Fetch all data concurrently
    let (views_result, resume_result, next_up_result) = tokio::join!(
        c.get_user_views(),
        c.get_resume_items(8),
        c.get_next_up(8),
    );

    let views = views_result.map_err(|e| format!("Failed to get views: {}", e))?;
    let resume_items = resume_result.unwrap_or_default();
    let next_up_items = next_up_result.unwrap_or_default();

    drop(c); // Release the read lock before loading images

    let mut rows: Vec<ContentRowData> = Vec::new();

    // "Continue Watching" row
    if !resume_items.is_empty() {
        let media_items = items_to_media_items(&resume_items, &server_url, &image_cache).await;
        rows.push(ContentRowData {
            title: SharedString::from("Continue Watching"),
            items: ModelRc::new(VecModel::from(media_items)),
            row_type: SharedString::from("landscape"),
        });
    }

    // "Next Up" row
    if !next_up_items.is_empty() {
        let media_items = items_to_media_items(&next_up_items, &server_url, &image_cache).await;
        rows.push(ContentRowData {
            title: SharedString::from("Next Up"),
            items: ModelRc::new(VecModel::from(media_items)),
            row_type: SharedString::from("landscape"),
        });
    }

    // "Latest in {Library}" rows for each library view
    for view in &views {
        let c = client.read().await;
        if rows.len() >= 5 { break; } // Cap total rows to limit memory
        match c.get_latest_media(&view.id, 8).await {
            Ok(latest) if !latest.is_empty() => {
                drop(c);
                let media_items =
                    items_to_media_items(&latest, &server_url, &image_cache).await;
                let row_type = match view.collection_type.as_deref() {
                    Some("movies") => "poster",
                    Some("tvshows") => "poster",
                    Some("music") => "square",
                    _ => "poster",
                };
                rows.push(ContentRowData {
                    title: SharedString::from(format!("Latest in {}", view.name)),
                    items: ModelRc::new(VecModel::from(media_items)),
                    row_type: SharedString::from(row_type),
                });
            }
            Ok(_) => {
                drop(c);
                debug!("No latest items for library: {}", view.name);
            }
            Err(e) => {
                drop(c);
                warn!("Failed to get latest for {}: {}", view.name, e);
            }
        }
    }

    // Update UI
    if let Some(ui) = ui_weak.upgrade() {
        // Populate library tiles from views (as media cards with images)
        let tiles: Vec<LibraryTile> = views.iter().map(|v| {
            LibraryTile {
                id: SharedString::from(&v.id),
                name: SharedString::from(&v.name),
                collection_type: SharedString::from(v.collection_type.as_deref().unwrap_or("")),
            }
        }).collect();
        ui.global::<AppBridge>()
            .set_library_tiles(ModelRc::new(VecModel::from(tiles)));

        // Also add a "My Libraries" row at the TOP of home rows with poster images
        let mut library_cards: Vec<MediaItem> = Vec::new();
        for view in &views {
            let poster = {
                let img_tag = view.image_tags.as_ref()
                    .and_then(|t| t.get("Primary"))
                    .cloned()
                    .unwrap_or_default();
                if !img_tag.is_empty() {
                    let url = format!("{}/Items/{}/Images/Primary?maxHeight=300&quality=80&tag={}",
                        server_url, view.id, img_tag);
                    image_cache.load_image(&url).await.unwrap_or_default()
                } else {
                    slint::Image::default()
                }
            };
            library_cards.push(MediaItem {
                id: SharedString::from(&view.id),
                title: SharedString::from(&view.name),
                image_source: poster,
                item_type: SharedString::from("CollectionFolder"),
                ..Default::default()
            });
        }
        if !library_cards.is_empty() {
            let mut all_rows: Vec<ContentRowData> = Vec::with_capacity(rows.len() + 1);
            all_rows.push(ContentRowData {
                title: SharedString::from("My Libraries"),
                items: ModelRc::new(VecModel::from(library_cards)),
                row_type: SharedString::from("landscape"),
            });
            all_rows.extend(rows);
            ui.global::<AppBridge>()
                .set_home_rows(ModelRc::new(VecModel::from(all_rows)));
        } else {
            ui.global::<AppBridge>()
                .set_home_rows(ModelRc::new(VecModel::from(rows)));
        }
        ui.global::<AppBridge>().set_is_loading(false);
    }

    info!("Home data loaded successfully");
    Ok(())
}

/// Load item detail, including similar items.
async fn load_item_detail(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    item_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let c = client.read().await;
    let server_url = c.server_url.clone();

    // Fetch the primary item first so we can render the detail screen quickly.
    // Related/cast data is loaded after the initial detail payload is displayed.
    let item = c
        .get_item(item_id)
        .await
        .map_err(|e| format!("Failed to get item: {}", e))?;
    drop(c);

    // Load the poster first for quick first paint; backdrop is loaded lazily after
    // the detail content is shown so we don't block on large image downloads.
    let poster = load_poster_image(&item, &server_url, &image_cache, 300).await;
    let detail_item = base_item_to_media_item(&item, &server_url, poster, SlintImage::default());

    // If this is a series, auto-load seasons
    let is_series = item.item_type == "Series";
    let series_id = item.id.clone();
    let item_id_owned = item.id.clone();

    // Build genre tags from item data
    let genre_tags: Vec<GenreTag> = item.genres
        .as_ref()
        .map(|genres| {
            genres.iter().map(|g| GenreTag {
                name: SharedString::from(g.as_str()),
            }).collect()
        })
        .unwrap_or_default();

    if let Some(ui) = ui_weak.upgrade() {
        ui.global::<AppBridge>().set_detail_item(detail_item);
        ui.global::<AppBridge>()
            .set_detail_related(ModelRc::default());
        // Clear previous seasons/episodes
        ui.global::<AppBridge>()
            .set_detail_seasons(ModelRc::default());
        ui.global::<AppBridge>()
            .set_detail_episodes(ModelRc::default());
        // Set genres
        ui.global::<AppBridge>()
            .set_genres(ModelRc::new(VecModel::from(genre_tags)));
        // Set cast & crew
        ui.global::<AppBridge>()
            .set_cast_members(ModelRc::default());

        // Unblock the loading overlay as soon as primary detail content is ready.
        ui.global::<AppBridge>().set_is_loading(false);
    }

    // Load backdrop after initial render. If it times out or fails, keep the
    // default backdrop and continue.
    let backdrop = tokio::time::timeout(
        tokio::time::Duration::from_secs(3),
        load_backdrop_image(&item, &server_url, &image_cache, 800),
    )
    .await
    .unwrap_or_default();

    if let Some(ui) = ui_weak.upgrade() {
        let mut current_detail = ui.global::<AppBridge>().get_detail_item();
        if current_detail.id.as_str() == item_id_owned.as_str() {
            current_detail.backdrop_source = backdrop;
            ui.global::<AppBridge>().set_detail_item(current_detail);
        }
    }

    // Load similar items after initial render.
    let similar = {
        let c = client.read().await;
        c.get_similar(item_id, 12).await.unwrap_or_default()
    };
    let related_items = items_to_media_items_no_images(&similar, &server_url);
    if let Some(ui) = ui_weak.upgrade() {
        let current_id = ui.global::<AppBridge>().get_detail_item().id;
        if current_id.as_str() == item_id_owned.as_str() {
            ui.global::<AppBridge>()
                .set_detail_related(ModelRc::new(VecModel::from(related_items)));
        }
    }

    // Build cast & crew with images after initial render.
    let filtered_people: Vec<_> = item
        .people
        .as_ref()
        .map(|people| {
            people
                .iter()
                .filter(|p| {
                    let pt = p.person_type.as_deref().unwrap_or("");
                    pt == "Actor" || pt == "Director" || pt == "Writer" || pt == "GuestStar"
                })
                .take(20)
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let mut cast_members: Vec<CastMember> = Vec::with_capacity(filtered_people.len());
    for p in &filtered_people {
        cast_members.push(CastMember {
            id: SharedString::from(p.id.as_deref().unwrap_or("")),
            name: SharedString::from(p.name.as_str()),
            role: SharedString::from(p.role.as_deref().unwrap_or("")),
            image: SlintImage::default(),
        });
    }

    if let Some(ui) = ui_weak.upgrade() {
        let current_id = ui.global::<AppBridge>().get_detail_item().id;
        if current_id.as_str() == item_id_owned.as_str() {
            ui.global::<AppBridge>()
                .set_cast_members(ModelRc::new(VecModel::from(cast_members)));
        }
    }

    // Auto-load seasons for series
    if is_series {
        let _ = load_seasons(ui_weak, client, image_cache, &series_id).await;
    }

    info!("Item detail loaded: {}", item.name);
    Ok(())
}

/// Load seasons for a series.
async fn load_seasons(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    series_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    const MAX_SEASONS_RENDERED: usize = 50;

    let c = client.read().await;
    let server_url = c.server_url.clone();
    let seasons = c
        .get_seasons(series_id)
        .await
        .map_err(|e| format!("Failed to get seasons: {}", e))?;
    drop(c);

    let filtered_seasons: Vec<BaseItemDto> = seasons
        .into_iter()
        .filter(|item| item.item_type == "Season")
        .take(MAX_SEASONS_RENDERED)
        .collect();

    if filtered_seasons.is_empty() {
        warn!(
            "No season items returned for series {} (API payload may be malformed)",
            series_id
        );
    }

    let season_items = if filtered_seasons.len() > 20 {
        info!(
            "Rendering {} seasons without posters for series {} to control memory",
            filtered_seasons.len(),
            series_id
        );
        items_to_media_items_no_images(&filtered_seasons, &server_url)
    } else {
        items_to_media_items(&filtered_seasons, &server_url, &image_cache).await
    };

    if let Some(ui) = ui_weak.upgrade() {
        ui.global::<AppBridge>()
            .set_detail_seasons(ModelRc::new(VecModel::from(season_items)));
    }

    info!("Loaded {} seasons for series {}", filtered_seasons.len(), series_id);
    Ok(())
}

/// Load episodes for a series/season.
async fn load_episodes(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    series_id: &str,
    season_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let c = client.read().await;
    let server_url = c.server_url.clone();
    let episodes = c
        .get_episodes(series_id, season_id)
        .await
        .map_err(|e| format!("Failed to get episodes: {}", e))?;
    drop(c);

    let episode_items = items_to_media_items(&episodes, &server_url, &image_cache).await;

    if let Some(ui) = ui_weak.upgrade() {
        ui.global::<AppBridge>()
            .set_detail_episodes(ModelRc::new(VecModel::from(episode_items)));
    }

    info!(
        "Loaded {} episodes for series {} season {}",
        episodes.len(),
        series_id,
        season_id
    );
    Ok(())
}

/// Load library items with optional sorting and filtering.
async fn load_library(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    library_id: &str,
    sort_by: Option<&str>,
    filters: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let c = client.read().await;
    let server_url = c.server_url.clone();

    // First get the library's name
    let library_name = match c.get_item(library_id).await {
        Ok(lib_item) => lib_item.name.clone(),
        Err(_) => String::from("Library"),
    };

    let result = c
        .get_items(
            Some(library_id),
            None,
            sort_by.or(Some("SortName")),
            Some("Ascending"),
            0,
            100,
            filters,
        )
        .await
        .map_err(|e| format!("Failed to get library items: {}", e))?;
    drop(c);

    let media_items = items_to_media_items(&result.items, &server_url, &image_cache).await;

    if let Some(ui) = ui_weak.upgrade() {
        ui.global::<AppBridge>()
            .set_library_items(ModelRc::new(VecModel::from(media_items)));
        ui.global::<AppBridge>()
            .set_library_title(SharedString::from(&library_name));
        ui.global::<AppBridge>().set_is_loading(false);
    }

    info!(
        "Loaded {} library items for '{}'",
        result.items.len(),
        library_name
    );
    Ok(())
}

/// Perform a search and update the UI.
async fn perform_search(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    image_cache: Arc<ImageCache>,
    query: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let c = client.read().await;
    let server_url = c.server_url.clone();

    let hints = c
        .search(query, 30)
        .await
        .map_err(|e| format!("Search failed: {}", e))?;
    drop(c);

    let mut search_results = Vec::with_capacity(hints.len());
    for hint in &hints {
        let poster = if let Some(ref tag) = hint.primary_image_tag {
            let url = format!(
                "{}/Items/{}/Images/Primary?maxHeight=225&quality=90&tag={}",
                server_url, hint.item_id, tag
            );
            image_cache.load_image(&url).await.unwrap_or_default()
        } else {
            SlintImage::default()
        };
        search_results.push(search_hint_to_result(hint, poster));
    }

    if let Some(ui) = ui_weak.upgrade() {
        let model = VecModel::from(search_results);
        ui.global::<AppBridge>()
            .set_search_results(ModelRc::new(model));
        ui.global::<AppBridge>().set_is_loading(false);
    }

    info!("Search for '{}' returned {} results", query, hints.len());
    Ok(())
}

// =============================================================================
// Player Event Handling
// =============================================================================

/// Handle VLC player events and relay them to the UI + Jellyfin server.
async fn handle_player_events(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<RwLock<JellyfinClient>>,
    state: Arc<StateManager>,
    player: Arc<Mutex<Option<PlayerWrapper>>>,
    daemon_player_tx: mpsc::UnboundedSender<PlayerEvent>,
    tracker: Arc<PlaybackTracker>,
    segments: Arc<Mutex<SegmentManager>>,
    playback_controls: Arc<Mutex<PlaybackControls>>,
    queue: Arc<Mutex<PlaybackQueue>>,
) {
    // Take the event receiver from the player
    let mut event_rx = {
        let mut p = player.lock().await;
        match p.as_mut() {
            Some(vlc) => match vlc.take_event_receiver() {
                Some(rx) => rx,
                None => {
                    debug!("Player event receiver already taken");
                    return;
                }
            },
            None => return,
        }
    };

    // Also start the VLC event loop to generate events.
    // We check if the player exists, then spawn a task that briefly locks
    // the mutex only to get what it needs, releasing it before the long-running loop.
    {
        let has_player = {
            let p = player.lock().await;
            p.is_some()
        };
        if has_player {
            // Note: run_event_loop holds the Mutex for its duration.
            // This is acceptable because it runs in a dedicated task, and
            // other player operations (pause/seek/stop) use try_lock or
            // short-lived locks that will retry. The event loop MUST have
            // exclusive access to poll VLC events safely.
            let player_for_loop = player.clone();
            tokio::spawn(async move {
                loop {
                    // Acquire lock briefly to check if player exists and run one event poll cycle
                    let should_continue = {
                        let p = player_for_loop.lock().await;
                        if let Some(ref vlc) = *p {
                            vlc.run_event_loop().await;
                            true
                        } else {
                            false
                        }
                    };
                    // Lock is dropped here - other tasks can access the player
                    if !should_continue {
                        break;
                    }
                    // Small yield to let other tasks acquire the lock
                    tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
                }
            });
        }
    }

    // Progress reporting interval
    let mut last_progress_report = tokio::time::Instant::now();
    let progress_interval = tokio::time::Duration::from_secs(10);

    while let Some(event) = event_rx.recv().await {
        // Forward all player events to daemon (QoS, streaming health, screen-alive)
        let _ = daemon_player_tx.send(event.clone());

        match event {
            PlayerEvent::PositionChanged {
                position_ms,
                duration_ms,
            } => {
                // Update UI
                let pos = position_ms;
                let dur = duration_ms;
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.position_ms = pos as i32;
                    ps.duration_ms = dur as i32;
                    ui.global::<AppBridge>().set_player_state(ps);
                });

                // Report progress to Jellyfin periodically
                let now = tokio::time::Instant::now();
                if now.duration_since(last_progress_report) >= progress_interval {
                    last_progress_report = now;
                    let app_state = state.get_state().await;
                    if let (Some(item_id), Some(session_id)) = (
                        app_state.playing_item_id.as_ref(),
                        app_state.play_session_id.as_ref(),
                    ) {
                        let progress_info = PlaybackProgressInfo {
                            item_id: item_id.clone(),
                            media_source_id: app_state.playing_media_source_id.clone(),
                            play_session_id: Some(session_id.clone()),
                            play_method: "DirectPlay".to_string(),
                            position_ticks: position_ms * 10_000,
                            can_seek: true,
                            is_paused: false,
                            is_muted: false,
                            audio_stream_index: None,
                            subtitle_stream_index: None,
                        };
                        let c = client.read().await;
                        if let Err(e) = c.report_playback_progress(&progress_info).await {
                            warn!("Failed to report progress: {}", e);
                        }
                    }
                }

                // Check for skippable segments (intro/credits)
                {
                    let sm = segments.lock().await;
                    let position_ticks = position_ms * 10_000;
                    if let Some((_seg_type, _end_ticks)) = sm.check_position(position_ticks) {
                        let label = sm.skip_label(position_ticks).unwrap_or_else(|| "Skip".to_string());
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.global::<AppBridge>().set_show_skip_button(true);
                            ui.global::<AppBridge>().set_skip_button_label(label.into());
                        });
                    } else {
                        let _ = ui_weak.upgrade_in_event_loop(|ui| {
                            ui.global::<AppBridge>().set_show_skip_button(false);
                        });
                    }
                }
            }
            PlayerEvent::Playing => {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.is_playing = true;
                    ps.is_paused = false;
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::Paused => {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.is_paused = true;
                    ui.global::<AppBridge>().set_player_state(ps);
                });

                // Report paused state to Jellyfin
                let app_state = state.get_state().await;
                if let (Some(item_id), Some(session_id)) = (
                    app_state.playing_item_id.as_ref(),
                    app_state.play_session_id.as_ref(),
                ) {
                    let p = player.lock().await;
                    let position_ms = if let Some(ref vlc) = *p {
                        vlc.get_position_ms().await.unwrap_or(0)
                    } else {
                        0
                    };
                    drop(p);

                    let progress_info = PlaybackProgressInfo {
                        item_id: item_id.clone(),
                        media_source_id: app_state.playing_media_source_id.clone(),
                        play_session_id: Some(session_id.clone()),
                        play_method: "DirectPlay".to_string(),
                        position_ticks: position_ms * 10_000,
                        can_seek: true,
                        is_paused: true,
                        is_muted: false,
                        audio_stream_index: None,
                        subtitle_stream_index: None,
                    };
                    let c = client.read().await;
                    let _ = c.report_playback_progress(&progress_info).await;
                }
            }
            PlayerEvent::Stopped | PlayerEvent::EndOfFile => {
                info!("Playback ended (event: {:?})", if matches!(event, PlayerEvent::EndOfFile) { "EndOfFile" } else { "Stopped" });

                // Report playback stopped
                let app_state = state.get_state().await;
                if let (Some(item_id), Some(session_id)) = (
                    app_state.playing_item_id.as_ref(),
                    app_state.play_session_id.as_ref(),
                ) {
                    let p = player.lock().await;
                    let position_ticks = if let Some(ref vlc) = *p {
                        vlc.get_position_ms().await.unwrap_or(0) * 10_000
                    } else {
                        0
                    };
                    drop(p);

                    let stop_info = PlaybackStopInfo {
                        item_id: item_id.clone(),
                        media_source_id: app_state.playing_media_source_id.clone(),
                        play_session_id: Some(session_id.clone()),
                        position_ticks,
                    };
                    let c = client.read().await;
                    let _ = c.report_playback_stopped(&stop_info).await;
                }

                // End tracking session
                if let Some(tid) = state.get_tracking_session().await {
                    let position_ticks_track = {
                        let p = player.lock().await;
                        if let Some(ref vlc) = *p {
                            vlc.get_position_ms().await.unwrap_or(0) * 10_000
                        } else { 0 }
                    };
                    let runtime = {
                        let c = client.read().await;
                        if let Some(ref iid) = app_state.playing_item_id {
                            c.get_item(iid).await.ok().and_then(|i| i.run_time_ticks)
                        } else { None }
                    };
                    tracker.end_session(tid, position_ticks_track, runtime);
                }

                // Clear segments
                {
                    let mut sm = segments.lock().await;
                    sm.clear();
                }
                // Hide skip button
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>().set_show_skip_button(false);
                });

                // Auto-advance to next queue item
                let next_item = {
                    let mut q = queue.lock().await;
                    q.advance().map(|item| item.item_id.clone())
                };
                if let Some(next_id) = next_item {
                    info!("Queue: auto-advancing to next item: {}", next_id);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<AppBridge>().invoke_play_item(next_id.into());
                    });
                    // Don't stop playback or navigate back - new item will start
                    break;
                }

                // Navigate back from player (no more queue items)
                state.stop_playback().await;
                let current = state.current_screen_name().await;
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>()
                        .set_current_screen(SharedString::from(&current));
                });

                break; // Exit the event loop
            }
            PlayerEvent::VolumeChanged(vol) => {
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.volume = vol as f32;
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::MuteChanged(muted) => {
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.is_muted = muted;
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::AudioTrackChanged(id) => {
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.current_audio = id;
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::SubtitleTrackChanged(id) => {
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.current_subtitle = id;
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::TracksAvailable { audio, subtitles } => {
                let audio_names: Vec<SharedString> = audio
                    .iter()
                    .map(|t| {
                        let label = if let Some(ref lang) = t.language {
                            format!("{} ({})", t.title, lang)
                        } else {
                            t.title.clone()
                        };
                        SharedString::from(label)
                    })
                    .collect();
                let sub_names: Vec<SharedString> = subtitles
                    .iter()
                    .map(|t| {
                        let label = if let Some(ref lang) = t.language {
                            format!("{} ({})", t.title, lang)
                        } else {
                            t.title.clone()
                        };
                        SharedString::from(label)
                    })
                    .collect();

                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.audio_tracks = ModelRc::new(VecModel::from(audio_names));
                    ps.subtitle_tracks = ModelRc::new(VecModel::from(sub_names));
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::Buffering(percent) => {
                debug!("Buffering: {}%", percent);
                let pct = percent;
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    let mut ps = ui.global::<AppBridge>().get_player_state();
                    ps.buffering_percent = pct;
                    ps.is_buffering = pct < 100;
                    ui.global::<AppBridge>().set_player_state(ps);
                });
            }
            PlayerEvent::Error(msg) => {
                error!("Player error: {}", msg);
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppBridge>()
                        .set_error_message(SharedString::from(format!("Player error: {}", msg)));
                });
            }
        }
    }
}

// =============================================================================
// Controller Input Handling
// =============================================================================

/// Convert a Slint Key enum variant to a SharedString for dispatch_event.
fn send_key(ui: &AppWindow, key: slint::platform::Key) {
    let text = slint::SharedString::from(String::from(char::from(key)));
    ui.window().dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
    ui.window().dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
}

/// Receive input actions from the controller and dispatch them to the UI.
async fn handle_controller_input(
    ui_weak: slint::Weak<AppWindow>,
    mut rx: mpsc::UnboundedReceiver<InputAction>,
    state: Arc<StateManager>,
) {
    while let Some(action) = rx.recv().await {
        // Reset idle timer on any input
        state.reset_idle().await;

        // Dismiss screensaver on any input
        {
            let ui_weak = ui_weak.clone();
            let _ = ui_weak.upgrade_in_event_loop(|ui| {
                if ui.global::<AppBridge>().get_show_screensaver() {
                    ui.global::<AppBridge>().set_show_screensaver(false);
                }
            });
        }

        let current_screen = state.current_screen_name().await;

        match action {
            // Navigation actions - simulate key presses in the Slint UI
            InputAction::Up => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    send_key(&ui, slint::platform::Key::UpArrow);
                });
            }
            InputAction::Down => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    send_key(&ui, slint::platform::Key::DownArrow);
                });
            }
            InputAction::Left => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    send_key(&ui, slint::platform::Key::LeftArrow);
                });
            }
            InputAction::Right => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    send_key(&ui, slint::platform::Key::RightArrow);
                });
            }
            InputAction::Select => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    send_key(&ui, slint::platform::Key::Return);
                });
            }
            InputAction::Back => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>().invoke_go_back();
                });
            }
            InputAction::Home => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>()
                        .invoke_navigate("home".into(), "".into());
                });
            }
            InputAction::Menu => {
                // Toggle settings
                let ui_weak = ui_weak.clone();
                if current_screen == "settings" {
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().invoke_go_back();
                    });
                } else {
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>()
                            .invoke_navigate("settings".into(), "".into());
                    });
                }
            }
            InputAction::Search => {
                let ui_weak = ui_weak.clone();
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.global::<AppBridge>()
                        .invoke_navigate("search".into(), "".into());
                });
            }
            InputAction::PlayPause => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().invoke_play_pause();
                    });
                }
            }
            InputAction::SeekForward => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        let ps = ui.global::<AppBridge>().get_player_state();
                        let new_pos = ps.position_ms + 30_000; // 30 seconds forward
                        ui.global::<AppBridge>().invoke_seek(new_pos);
                    });
                }
            }
            InputAction::SeekBack => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        let ps = ui.global::<AppBridge>().get_player_state();
                        let new_pos = (ps.position_ms - 10_000).max(0); // 10 seconds back
                        ui.global::<AppBridge>().invoke_seek(new_pos);
                    });
                }
            }
            InputAction::NextTrack => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().invoke_next_track();
                    });
                }
            }
            InputAction::PrevTrack => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().invoke_prev_track();
                    });
                }
            }
            InputAction::VolumeUp => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        let ps = ui.global::<AppBridge>().get_player_state();
                        let new_vol = (ps.volume + 5.0).min(100.0);
                        ui.global::<AppBridge>().invoke_set_volume(new_vol);
                    });
                }
            }
            InputAction::VolumeDown => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        let ps = ui.global::<AppBridge>().get_player_state();
                        let new_vol = (ps.volume - 5.0).max(0.0);
                        ui.global::<AppBridge>().invoke_set_volume(new_vol);
                    });
                }
            }
            InputAction::Mute => {
                if current_screen == "player" {
                    let ui_weak = ui_weak.clone();
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.global::<AppBridge>().invoke_toggle_mute();
                    });
                }
            }
            InputAction::ContextMenu => {
                // Could trigger a context menu popup in the future
                debug!("Context menu action (not yet implemented)");
            }
            InputAction::Screenshot => {
                debug!("Screenshot action (not yet implemented)");
            }
        }
    }

    info!("Controller input handler exiting");
}
