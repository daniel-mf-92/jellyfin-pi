use log::{debug, error, info, warn};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PlayerError {
    Vlc(String),
    NotInitialized,
    Socket(String),
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::Vlc(msg) => write!(f, "vlc error: {}", msg),
            PlayerError::NotInitialized => write!(f, "player not initialized"),
            PlayerError::Socket(msg) => write!(f, "socket error: {}", msg),
        }
    }
}

impl std::error::Error for PlayerError {}

pub type PlayerResult<T> = Result<T, PlayerError>;

// ---------------------------------------------------------------------------
// Player events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Playing,
    Paused,
    Stopped,
    EndOfFile,
    Error(String),
    PositionChanged { position_ms: i64, duration_ms: i64 },
    VolumeChanged(f64),
    MuteChanged(bool),
    AudioTrackChanged(i32),
    SubtitleTrackChanged(i32),
    TracksAvailable {
        audio: Vec<TrackInfo>,
        subtitles: Vec<TrackInfo>,
    },
    Buffering(i32),
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub id: i32,
    pub title: String,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub is_default: bool,
}

// ---------------------------------------------------------------------------
// VlcPlayer
// ---------------------------------------------------------------------------

const SOCKET_PATH: &str = "/tmp/jellyfin-tv-vlc.sock";
const VLC_MAX_VOLUME: f64 = 512.0;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

pub struct VlcPlayer {
    child: Arc<Mutex<Option<Child>>>,
    event_tx: mpsc::UnboundedSender<PlayerEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<PlayerEvent>>,
    socket_path: String,
    stored_volume: Arc<Mutex<f64>>,
    is_muted: Arc<AtomicBool>,
}

impl VlcPlayer {
    /// Create a new VlcPlayer.
    pub fn new() -> PlayerResult<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        info!("VlcPlayer created (socket={})", SOCKET_PATH);

        Ok(Self {
            child: Arc::new(Mutex::new(None)),
            event_tx,
            event_rx: Some(event_rx),
            socket_path: SOCKET_PATH.to_string(),
            stored_volume: Arc::new(Mutex::new(256.0)), // 50% of 512
            is_muted: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<PlayerEvent>> {
        self.event_rx.take()
    }

    // -----------------------------------------------------------------------
    // Socket communication
    // -----------------------------------------------------------------------

    /// Send a command to VLC via the RC Unix socket and read the response.
    async fn send_command(&self, cmd: &str) -> PlayerResult<String> {
        let stream = match timeout(COMMAND_TIMEOUT, UnixStream::connect(&self.socket_path)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(PlayerError::Socket(format!(
                    "failed to connect to VLC socket: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(PlayerError::Socket("socket connect timeout".to_string()));
            }
        };

        let (reader, mut writer) = stream.into_split();

        // Write command
        let cmd_bytes = format!("{}\n", cmd);
        if let Err(e) = timeout(COMMAND_TIMEOUT, writer.write_all(cmd_bytes.as_bytes())).await {
            return Err(PlayerError::Socket(format!("write timeout: {}", e)));
        }

        // Read response lines (VLC RC sends "> " prompt after each command)
        let buf_reader = BufReader::new(reader);
        let mut lines = buf_reader.lines();
        let mut response = String::new();

        // Read lines with a timeout -- VLC may send multiple lines
        loop {
            match timeout(Duration::from_millis(500), lines.next_line()).await {
                Ok(Ok(Some(line))) => {
                    let trimmed = line.trim().to_string();
                    // Skip the VLC prompt
                    if trimmed == ">" || trimmed.is_empty() {
                        continue;
                    }
                    if !response.is_empty() {
                        response.push('\n');
                    }
                    response.push_str(&trimmed);
                }
                Ok(Ok(None)) => break, // EOF
                Ok(Err(e)) => {
                    debug!("socket read error: {}", e);
                    break;
                }
                Err(_) => break, // Timeout -- we have all available data
            }
        }

        debug!("VLC RC: '{}' -> '{}'", cmd, response);
        Ok(response)
    }

    // -----------------------------------------------------------------------
    // Playback control
    // -----------------------------------------------------------------------

    /// Kill any existing VLC process and clean up the socket.
    async fn kill_existing(&self) {
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            info!("Killing existing VLC process");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *child_guard = None;

        // Remove stale socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;
    }

    /// Launch VLC and play a URL.
    pub async fn play_url(&self, url: &str, start_position_ms: Option<i64>) -> PlayerResult<()> {
        info!("play_url: {}", url);

        // Kill any existing VLC instance
        self.kill_existing().await;

        // Adaptive cache sizing based on available RAM
        let prefetch = adaptive_prefetch_bytes();

        // Build command — fast startup (1.5s network cache) + deep read-ahead (prefetch)
        let mut args = vec![
            "--fullscreen".to_string(),
            "--intf".to_string(),
            "rc".to_string(),
            "--rc-unix".to_string(),
            self.socket_path.clone(),
            "--rc-fake-tty".to_string(),
            "--avcodec-hw".to_string(),
            "any".to_string(),
            "--network-caching".to_string(),
            "1500".to_string(),          // 1.5s — fast first frame
            "--file-caching".to_string(),
            "300".to_string(),           // 300ms for local /dev/shm files
            "--live-caching".to_string(),
            "1500".to_string(),
            "--http-reconnect".to_string(),
            "--http-continuous".to_string(),
            "--input-fast-seek".to_string(),
            format!("--prefetch-buffer-size={}", prefetch),
            "--avcodec-threads=4".to_string(),
            "--no-osd".to_string(),
            "--no-video-title-show".to_string(),
            "--alsa-audio-device".to_string(),
            "default".to_string(),
        ];

        if let Some(ms) = start_position_ms {
            let secs = ms as f64 / 1000.0;
            args.push("--start-time".to_string());
            args.push(format!("{:.3}", secs));
            debug!("start position set to {}s", secs);
        }

        args.push(url.to_string());

        info!("Launching: cvlc {}", args.join(" "));

        let child = Command::new("cvlc")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| PlayerError::Vlc(format!("failed to launch cvlc: {}", e)))?;

        {
            let mut child_guard = self.child.lock().await;
            *child_guard = Some(child);
        }

        // Wait for the socket to become available
        for i in 0..20 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if tokio::fs::metadata(&self.socket_path).await.is_ok() {
                // Try a test command
                if self.send_command("status").await.is_ok() {
                    debug!("VLC socket ready after {}ms", (i + 1) * 250);
                    let _ = self.event_tx.send(PlayerEvent::Playing);
                    return Ok(());
                }
            }
        }

        Err(PlayerError::Socket(
            "VLC socket did not become available within 5 seconds".to_string(),
        ))
    }

    /// Pause playback (VLC pause toggles).
    pub async fn pause(&self) -> PlayerResult<()> {
        self.send_command("pause").await?;
        let _ = self.event_tx.send(PlayerEvent::Paused);
        Ok(())
    }

    /// Resume playback.
    pub async fn resume(&self) -> PlayerResult<()> {
        self.send_command("play").await?;
        let _ = self.event_tx.send(PlayerEvent::Playing);
        Ok(())
    }

    /// Toggle pause (VLC "pause" command toggles).
    pub async fn toggle_pause(&self) -> PlayerResult<()> {
        self.send_command("pause").await?;
        // We do not know the exact state; the event loop will detect it
        Ok(())
    }

    /// Stop playback, quit VLC, kill child process.
    pub async fn stop(&self) -> PlayerResult<()> {
        let _ = self.send_command("stop").await;
        let _ = self.send_command("quit").await;

        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *child_guard = None;

        let _ = tokio::fs::remove_file(&self.socket_path).await;
        let _ = self.event_tx.send(PlayerEvent::Stopped);
        info!("VLC stopped");
        Ok(())
    }

    /// Seek to an absolute position (milliseconds).
    pub async fn seek_to(&self, position_ms: i64) -> PlayerResult<()> {
        let secs = position_ms / 1000;
        self.send_command(&format!("seek {}", secs)).await?;
        Ok(())
    }

    /// Seek relative to the current position (seconds).
    pub async fn seek_relative(&self, offset_seconds: f64) -> PlayerResult<()> {
        // Get current position, add offset, seek absolute
        let current = self.get_position_ms().await.unwrap_or(0);
        let target = current + (offset_seconds * 1000.0) as i64;
        let target = target.max(0);
        self.seek_to(target).await
    }

    // -----------------------------------------------------------------------
    // Volume / mute
    // -----------------------------------------------------------------------

    /// Set volume (0.0 - 100.0). VLC uses 0-512 internally.
    pub async fn set_volume(&self, volume: f64) -> PlayerResult<()> {
        let vlc_vol = ((volume / 100.0) * VLC_MAX_VOLUME) as i32;
        let vlc_vol = vlc_vol.clamp(0, VLC_MAX_VOLUME as i32);
        self.send_command(&format!("volume {}", vlc_vol)).await?;

        // Store for unmute restore
        {
            let mut stored = self.stored_volume.lock().await;
            *stored = vlc_vol as f64;
        }
        self.is_muted.store(false, Ordering::SeqCst);

        let _ = self.event_tx.send(PlayerEvent::VolumeChanged(volume));
        Ok(())
    }

    /// Get current volume (0-100).
    pub async fn get_volume(&self) -> PlayerResult<f64> {
        let response = self.send_command("volume").await?;
        // VLC RC returns something like "( audio volume: 256 )" or just a number
        let vol = parse_number_from_response(&response).unwrap_or(256.0);
        Ok((vol / VLC_MAX_VOLUME) * 100.0)
    }

    /// Set mute state. VLC RC does not have a native mute, so we use volume 0 / restore.
    pub async fn set_mute(&self, muted: bool) -> PlayerResult<()> {
        if muted {
            // Save current volume and set to 0
            let current = self.get_volume().await.unwrap_or(50.0);
            {
                let mut stored = self.stored_volume.lock().await;
                *stored = (current / 100.0) * VLC_MAX_VOLUME;
            }
            self.send_command("volume 0").await?;
            self.is_muted.store(true, Ordering::SeqCst);
        } else {
            // Restore saved volume
            let restore = {
                let stored = self.stored_volume.lock().await;
                *stored as i32
            };
            self.send_command(&format!("volume {}", restore)).await?;
            self.is_muted.store(false, Ordering::SeqCst);
        }
        let _ = self.event_tx.send(PlayerEvent::MuteChanged(muted));
        Ok(())
    }

    /// Toggle mute.
    pub async fn toggle_mute(&self) -> PlayerResult<()> {
        let currently_muted = self.is_muted.load(Ordering::SeqCst);
        self.set_mute(!currently_muted).await
    }

    // -----------------------------------------------------------------------
    // Track selection
    // -----------------------------------------------------------------------

    /// Select an audio track by ID.
    pub async fn set_audio_track(&self, track_id: i32) -> PlayerResult<()> {
        self.send_command(&format!("atrack {}", track_id)).await?;
        let _ = self.event_tx.send(PlayerEvent::AudioTrackChanged(track_id));
        Ok(())
    }

    /// Select a subtitle track by ID. Pass 0 to disable subtitles.
    pub async fn set_subtitle_track(&self, track_id: i32) -> PlayerResult<()> {
        if track_id == 0 {
            self.send_command("strack -1").await?;
        } else {
            self.send_command(&format!("strack {}", track_id)).await?;
        }
        let _ = self
            .event_tx
            .send(PlayerEvent::SubtitleTrackChanged(track_id));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Position / duration queries
    // -----------------------------------------------------------------------

    /// Get the current playback position in milliseconds.
    pub async fn get_position_ms(&self) -> PlayerResult<i64> {
        let response = self.send_command("get_time").await?;
        let secs = parse_number_from_response(&response).unwrap_or(0.0);
        Ok((secs * 1000.0) as i64)
    }

    /// Get the total duration in milliseconds.
    pub async fn get_duration_ms(&self) -> PlayerResult<i64> {
        let response = self.send_command("get_length").await?;
        let secs = parse_number_from_response(&response).unwrap_or(0.0);
        Ok((secs * 1000.0) as i64)
    }

    /// Get track information from VLC via the "info" command.
    pub async fn get_tracks(&self) -> PlayerResult<(Vec<TrackInfo>, Vec<TrackInfo>)> {
        let response = self.send_command("info").await?;

        let mut audio_tracks = Vec::new();
        let mut sub_tracks = Vec::new();

        // Parse VLC info output for stream information
        // VLC info format has sections like:
        // +----[ Stream 0 ]
        // | Type: Audio
        // | Codec: mp3
        // | Language: eng
        let mut current_id: i32 = -1;
        let mut current_type = String::new();
        let mut current_codec = None;
        let mut current_lang = None;

        for line in response.lines() {
            let line = line.trim();

            if line.starts_with("+----[ Stream") {
                // Save previous track if any
                if current_id >= 0 && !current_type.is_empty() {
                    let info = TrackInfo {
                        id: current_id,
                        title: format!("Track {}", current_id),
                        language: current_lang.take(),
                        codec: current_codec.take(),
                        is_default: current_id == 0,
                    };
                    match current_type.as_str() {
                        "Audio" | "audio" => audio_tracks.push(info),
                        "Subtitle" | "subtitle" | "spu" => sub_tracks.push(info),
                        _ => {}
                    }
                }

                // Parse stream ID
                current_id = line
                    .trim_start_matches("+----[ Stream")
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(-1);
                current_type.clear();
                current_codec = None;
                current_lang = None;
            } else if line.starts_with("| Type:") || line.starts_with("|Type:") {
                current_type = line
                    .split(':')
                    .nth(1)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
            } else if line.starts_with("| Codec:") || line.starts_with("|Codec:") {
                current_codec = line.split(':').nth(1).map(|s| s.trim().to_string());
            } else if line.starts_with("| Language:") || line.starts_with("|Language:") {
                current_lang = line.split(':').nth(1).map(|s| s.trim().to_string());
            }
        }

        // Save the last track
        if current_id >= 0 && !current_type.is_empty() {
            let info = TrackInfo {
                id: current_id,
                title: format!("Track {}", current_id),
                language: current_lang,
                codec: current_codec,
                is_default: current_id == 0,
            };
            match current_type.as_str() {
                "Audio" | "audio" => audio_tracks.push(info),
                "Subtitle" | "subtitle" | "spu" => sub_tracks.push(info),
                _ => {}
            }
        }

        Ok((audio_tracks, sub_tracks))
    }

    /// Returns true if VLC is actively playing.
    pub async fn is_playing(&self) -> bool {
        match self.send_command("is_playing").await {
            Ok(response) => {
                let val = parse_number_from_response(&response).unwrap_or(0.0);
                val > 0.0
            }
            Err(_) => false,
        }
    }

    // -----------------------------------------------------------------------
    // Event loop
    // -----------------------------------------------------------------------

    /// Run the event loop: polls position every 500ms and detects when VLC exits.
    pub async fn run_event_loop(&self) {
        let tx = self.event_tx.clone();
        let child = self.child.clone();
        let socket_path = self.socket_path.clone();

        info!("VLC event loop started");

        loop {
            // Check if channel is closed
            if tx.is_closed() {
                info!("Event channel closed, stopping event loop");
                break;
            }

            // Check if VLC process is still alive
            {
                let mut child_guard = child.lock().await;
                if let Some(ref mut c) = *child_guard {
                    match c.try_wait() {
                        Ok(Some(status)) => {
                            info!("VLC process exited with status: {}", status);
                            if status.success() {
                                let _ = tx.send(PlayerEvent::EndOfFile);
                            } else {
                                let _ = tx.send(PlayerEvent::Error(format!(
                                    "VLC exited with status {}",
                                    status
                                )));
                            }
                            *child_guard = None;
                            break;
                        }
                        Ok(None) => {} // Still running
                        Err(e) => {
                            warn!("Error checking VLC process: {}", e);
                        }
                    }
                } else {
                    // No child process -- nothing to monitor
                    debug!("No VLC child process, exiting event loop");
                    break;
                }
            }

            // Poll position via socket
            if let Ok(stream) =
                timeout(Duration::from_millis(200), UnixStream::connect(&socket_path)).await
            {
                if stream.is_ok() {
                    // Get position
                    if let Ok(pos_response) = self.send_command("get_time").await {
                        let position_secs = parse_number_from_response(&pos_response).unwrap_or(0.0);
                        let position_ms = (position_secs * 1000.0) as i64;

                        // Get duration
                        if let Ok(dur_response) = self.send_command("get_length").await {
                            let duration_secs =
                                parse_number_from_response(&dur_response).unwrap_or(0.0);
                            let duration_ms = (duration_secs * 1000.0) as i64;

                            let _ = tx.send(PlayerEvent::PositionChanged {
                                position_ms,
                                duration_ms,
                            });
                        }
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        info!("VLC event loop exited");
    }
}

impl Drop for VlcPlayer {
    fn drop(&mut self) {
        info!("VlcPlayer dropped -- cleaning up");
        // Remove socket file synchronously (best-effort)
        let _ = std::fs::remove_file(&self.socket_path);

        // We cannot do async kill in Drop, but the child process will be
        // killed when the Child handle is dropped (tokio behavior).
    }
}

// ---------------------------------------------------------------------------
// Adaptive RAM cache
// ---------------------------------------------------------------------------

/// Read MemAvailable from /proc/meminfo (bytes). Returns 0 on failure.
fn read_mem_available() -> u64 {
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("MemAvailable:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<u64>() {
                        return kb * 1024;
                    }
                }
            }
        }
    }
    0
}

/// Calculate adaptive --prefetch-buffer-size based on available RAM.
/// Returns bytes. Reserves 2GB for system, uses 25% of remainder, caps at 512MB.
fn adaptive_prefetch_bytes() -> u64 {
    let available = read_mem_available();
    if available == 0 {
        return 16 * 1024 * 1024; // 16MB fallback
    }

    let reserve: u64 = 2 * 1024 * 1024 * 1024; // 2GB
    let usable = available.saturating_sub(reserve).max(256 * 1024 * 1024);
    let usable = usable.min((available as f64 * 0.75) as u64);

    let prefetch = (usable / 4).min(512 * 1024 * 1024).max(1024 * 1024);

    info!(
        "Adaptive cache: available={}MB usable={}MB prefetch={}MB",
        available / (1024 * 1024),
        usable / (1024 * 1024),
        prefetch / (1024 * 1024)
    );

    prefetch
}

// ---------------------------------------------------------------------------
// /dev/shm persistent stream cache (LRU eviction)
// ---------------------------------------------------------------------------

const CACHE_DIR: &str = "/dev/shm/jmp-cache";
const CACHE_PRESSURE_FLOOR_GB: f64 = 2.0;

/// Entry in the stream cache.
struct CacheEntry {
    path: String,
    size: u64,
    complete: bool,
    last_access: f64,
}

/// Persistent RAM cache — downloads streams to /dev/shm, serves from file on replay.
pub struct StreamCache {
    entries: std::sync::Mutex<std::collections::HashMap<String, CacheEntry>>,
    pinned: std::sync::Mutex<Option<String>>,
}

impl StreamCache {
    pub fn new() -> Self {
        let _ = std::fs::create_dir_all(CACHE_DIR);

        // Recover existing files from previous session
        let mut entries = std::collections::HashMap::new();
        if let Ok(dir) = std::fs::read_dir(CACHE_DIR) {
            for entry in dir.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let mtime = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0);
                        entries.insert(
                            name,
                            CacheEntry {
                                path: entry.path().to_string_lossy().to_string(),
                                size: meta.len(),
                                complete: true,
                                last_access: mtime,
                            },
                        );
                    }
                }
            }
        }

        if !entries.is_empty() {
            info!("StreamCache: recovered {} items from previous session", entries.len());
        }
        info!("StreamCache: dir={} floor={}GB", CACHE_DIR, CACHE_PRESSURE_FLOOR_GB);

        Self {
            entries: std::sync::Mutex::new(entries),
            pinned: std::sync::Mutex::new(None),
        }
    }

    /// Return local file path if item is fully cached.
    pub fn get(&self, item_id: &str) -> Option<String> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.get_mut(item_id) {
            if e.complete && std::path::Path::new(&e.path).exists() {
                e.last_access = now_secs();
                return Some(e.path.clone());
            }
        }
        None
    }

    /// Pin item as currently playing (immune to eviction).
    pub fn pin(&self, item_id: &str) {
        *self.pinned.lock().unwrap() = Some(item_id.to_string());
        if let Some(e) = self.entries.lock().unwrap().get_mut(item_id) {
            e.last_access = now_secs();
        }
    }

    /// Unpin (item stays cached, just evictable).
    pub fn unpin(&self) {
        let pinned = self.pinned.lock().unwrap().take();
        if let Some(id) = pinned {
            if let Some(e) = self.entries.lock().unwrap().get_mut(&id) {
                e.last_access = now_secs();
            }
        }
    }

    /// Start a background download of the stream to /dev/shm.
    pub fn start_download(&self, item_id: &str, url: &str) {
        {
            let entries = self.entries.lock().unwrap();
            if let Some(e) = entries.get(item_id) {
                if e.complete {
                    return; // already cached
                }
            }
        }

        let path = format!("{}/{}", CACHE_DIR, item_id);
        let item_id = item_id.to_string();
        let url = url.to_string();

        // Insert placeholder
        {
            let mut entries = self.entries.lock().unwrap();
            entries.insert(
                item_id.clone(),
                CacheEntry {
                    path: path.clone(),
                    size: 0,
                    complete: false,
                    last_access: now_secs(),
                },
            );
        }

        // Background download via curl (thread-safe, no async runtime needed)
        let entries_ref = unsafe {
            // SAFETY: StreamCache lives for the entire program lifetime (static-like).
            // The background thread only accesses entries via Mutex.
            &*((&self.entries) as *const _)
        };

        std::thread::spawn(move || {
            info!("StreamCache: downloading {}...", &item_id[..8.min(item_id.len())]);
            let status = std::process::Command::new("curl")
                .args(["-sS", "-L", "-o", &path, "--max-time", "600", &url])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .status();

            let mut entries = entries_ref.lock().unwrap();
            if let Some(e) = entries.get_mut(&item_id) {
                match status {
                    Ok(s) if s.success() => {
                        e.size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        e.complete = true;
                        info!(
                            "StreamCache: complete {} size={}MB",
                            &item_id[..8.min(item_id.len())],
                            e.size / (1024 * 1024)
                        );
                    }
                    _ => {
                        warn!("StreamCache: download failed {}", &item_id[..8.min(item_id.len())]);
                    }
                }
            }
        });
    }

    /// Evict oldest non-pinned entry if MemAvailable < floor.
    pub fn pressure_check(&self) {
        let available = read_mem_available();
        let floor = (CACHE_PRESSURE_FLOOR_GB * 1024.0 * 1024.0 * 1024.0) as u64;
        if available == 0 || available >= floor {
            return;
        }

        let pinned = self.pinned.lock().unwrap().clone();
        let mut entries = self.entries.lock().unwrap();

        // Find oldest non-pinned, complete entry
        let victim = entries
            .iter()
            .filter(|(k, e)| {
                e.complete && pinned.as_deref() != Some(k.as_str())
            })
            .min_by(|a, b| a.1.last_access.partial_cmp(&b.1.last_access).unwrap())
            .map(|(k, _)| k.clone());

        if let Some(id) = victim {
            if let Some(e) = entries.remove(&id) {
                let _ = std::fs::remove_file(&e.path);
                info!(
                    "StreamCache: evicted {} freed={}MB ({}GB free)",
                    &id[..8.min(id.len())],
                    e.size / (1024 * 1024),
                    available / (1024 * 1024 * 1024)
                );
            }
        }
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Extract a Jellyfin item ID from a stream URL (/Videos/<id>/stream).
pub fn extract_item_id(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    if let Some(pos) = lower.find("/videos/") {
        let after = &url[pos + 8..];
        if let Some(end) = after.find('/') {
            let id = &after[..end];
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a numeric value from a VLC RC response string.
/// VLC might return "42", "( audio volume: 256 )", or other formatted text.
fn parse_number_from_response(response: &str) -> Option<f64> {
    // Try to find any number in the response
    for word in response.split(|c: char| !c.is_ascii_digit() && c != '.' && c != '-') {
        if !word.is_empty() {
            if let Ok(n) = word.parse::<f64>() {
                return Some(n);
            }
        }
    }
    None
}
