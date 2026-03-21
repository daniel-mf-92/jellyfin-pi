use libmpv2::events::{Event, PropertyData};
use libmpv2::{FileState, Format, Mpv};
use log::{debug, error, info, warn};
use std::fmt;
use std::sync::Arc;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PlayerError {
    Mpv(String),
    NotInitialized,
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::Mpv(msg) => write!(f, "mpv error: {}", msg),
            PlayerError::NotInitialized => write!(f, "player not initialized"),
        }
    }
}

impl std::error::Error for PlayerError {}

impl From<libmpv2::Error> for PlayerError {
    fn from(e: libmpv2::Error) -> Self {
        PlayerError::Mpv(e.to_string())
    }
}

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
// Reply‐userdata constants for observed properties
// ---------------------------------------------------------------------------

const REPLY_TIME_POS: u64 = 1;
const REPLY_PAUSE: u64 = 2;
const REPLY_VOLUME: u64 = 3;
const REPLY_MUTE: u64 = 4;
const REPLY_AID: u64 = 5;
const REPLY_SID: u64 = 6;

// ---------------------------------------------------------------------------
// MpvPlayer
// ---------------------------------------------------------------------------

pub struct MpvPlayer {
    mpv: Arc<Mpv>,
    event_tx: mpsc::UnboundedSender<PlayerEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<PlayerEvent>>,
}

impl MpvPlayer {
    /// Create a new MpvPlayer configured for Raspberry Pi 5 hardware decoding.
    pub fn new() -> PlayerResult<Self> {
        let mpv = Mpv::new().map_err(|e| PlayerError::Mpv(e.to_string()))?;

        // ---- Hardware decoding (Pi 5 V4L2 M2M) ----
        mpv.set_property("hwdec", "v4l2m2m-copy")?;
        mpv.set_property("vo", "gpu")?;
        mpv.set_property("gpu-context", "drm")?;

        // ---- Audio ----
        mpv.set_property("audio-device", "alsa/default")?;
        mpv.set_property("alsa-buffer-time", 100_000i64)?;

        // ---- Sync / rendering ----
        mpv.set_property("video-sync", "audio")?;
        mpv.set_property("interpolation", "no")?;

        // ---- Cache / demuxer ----
        mpv.set_property("cache", "yes")?;
        mpv.set_property("demuxer-max-bytes", "500M")?;
        mpv.set_property("demuxer-readahead-secs", 60i64)?;

        // ---- Subtitles / OSD ----
        mpv.set_property("sub-auto", "fuzzy")?;
        mpv.set_property("sub-font-size", 48i64)?;
        mpv.set_property("osd-font-size", 32i64)?;

        // ---- Input / terminal ----
        mpv.set_property("input-default-bindings", "no")?;
        mpv.set_property("input-vo-keyboard", "no")?;
        mpv.set_property("terminal", "no")?;

        // ---- Window / display ----
        mpv.set_property("keep-open", "yes")?;
        mpv.set_property("idle", "yes")?;
        mpv.set_property("force-window", "yes")?;
        mpv.set_property("fullscreen", "yes")?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        info!("MpvPlayer initialised (hwdec=v4l2m2m-copy, vo=gpu, gpu-context=drm)");

        Ok(Self {
            mpv: Arc::new(mpv),
            event_tx,
            event_rx: Some(event_rx),
        })
    }

    /// Take the event receiver (can only be called once).
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<PlayerEvent>> {
        self.event_rx.take()
    }

    // -----------------------------------------------------------------------
    // Playback control
    // -----------------------------------------------------------------------

    /// Load and play a URL (direct-stream or transcode URL from Jellyfin).
    pub fn play_url(&self, url: &str, start_position_ms: Option<i64>) -> PlayerResult<()> {
        info!("play_url: {}", url);

        // If a start position is requested, set it before loading the file.
        if let Some(ms) = start_position_ms {
            let secs = ms as f64 / 1000.0;
            let start_str = format!("{:.3}", secs);
            self.mpv.set_property("start", start_str.as_str())?;
            debug!("start position set to {}s", secs);
        } else {
            self.mpv.set_property("start", "0")?;
        }

        self.mpv
            .command("loadfile", &[url, "replace"])
            .map_err(|e| PlayerError::Mpv(format!("loadfile failed: {}", e)))?;

        Ok(())
    }

    /// Pause playback.
    pub fn pause(&self) -> PlayerResult<()> {
        self.mpv.set_property("pause", true)?;
        Ok(())
    }

    /// Resume playback.
    pub fn resume(&self) -> PlayerResult<()> {
        self.mpv.set_property("pause", false)?;
        Ok(())
    }

    /// Toggle pause/resume.
    pub fn toggle_pause(&self) -> PlayerResult<()> {
        let paused: bool = self.mpv.get_property("pause")?;
        self.mpv.set_property("pause", !paused)?;
        Ok(())
    }

    /// Stop playback and clear the playlist.
    pub fn stop(&self) -> PlayerResult<()> {
        self.mpv
            .command("stop", &[])
            .map_err(|e| PlayerError::Mpv(format!("stop failed: {}", e)))?;
        let _ = self.event_tx.send(PlayerEvent::Stopped);
        Ok(())
    }

    /// Seek to an absolute position (milliseconds).
    pub fn seek_to(&self, position_ms: i64) -> PlayerResult<()> {
        let secs = position_ms as f64 / 1000.0;
        self.mpv.set_property("time-pos", secs)?;
        Ok(())
    }

    /// Seek relative to the current position (seconds, positive = forward).
    pub fn seek_relative(&self, offset_seconds: f64) -> PlayerResult<()> {
        let offset_str = format!("{:.3}", offset_seconds);
        self.mpv
            .command("seek", &[&offset_str, "relative"])
            .map_err(|e| PlayerError::Mpv(format!("seek failed: {}", e)))?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Volume / mute
    // -----------------------------------------------------------------------

    /// Set volume (0.0 – 100.0).
    pub fn set_volume(&self, volume: f64) -> PlayerResult<()> {
        self.mpv.set_property("volume", volume)?;
        Ok(())
    }

    /// Get current volume.
    pub fn get_volume(&self) -> PlayerResult<f64> {
        let vol: f64 = self.mpv.get_property("volume")?;
        Ok(vol)
    }

    /// Set mute state.
    pub fn set_mute(&self, muted: bool) -> PlayerResult<()> {
        self.mpv.set_property("mute", muted)?;
        Ok(())
    }

    /// Toggle mute.
    pub fn toggle_mute(&self) -> PlayerResult<()> {
        let muted: bool = self.mpv.get_property("mute")?;
        self.mpv.set_property("mute", !muted)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Track selection
    // -----------------------------------------------------------------------

    /// Select an audio track by its mpv track ID.
    pub fn set_audio_track(&self, track_id: i32) -> PlayerResult<()> {
        self.mpv.set_property("aid", track_id as i64)?;
        Ok(())
    }

    /// Select a subtitle track by its mpv track ID. Pass 0 to disable subtitles.
    pub fn set_subtitle_track(&self, track_id: i32) -> PlayerResult<()> {
        if track_id == 0 {
            self.mpv.set_property("sid", "no")?;
        } else {
            self.mpv.set_property("sid", track_id as i64)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Position / duration queries
    // -----------------------------------------------------------------------

    /// Get the current playback position in milliseconds.
    pub fn get_position_ms(&self) -> PlayerResult<i64> {
        let secs: f64 = self.mpv.get_property("time-pos")?;
        Ok((secs * 1000.0) as i64)
    }

    /// Get the total duration in milliseconds.
    pub fn get_duration_ms(&self) -> PlayerResult<i64> {
        let secs: f64 = self.mpv.get_property("duration")?;
        Ok((secs * 1000.0) as i64)
    }

    // -----------------------------------------------------------------------
    // Track information
    // -----------------------------------------------------------------------

    /// Retrieve audio and subtitle tracks from mpv's `track-list` property.
    pub fn get_tracks(&self) -> PlayerResult<(Vec<TrackInfo>, Vec<TrackInfo>)> {
        let count: i64 = self.mpv.get_property("track-list/count")?;
        let mut audio_tracks = Vec::new();
        let mut sub_tracks = Vec::new();

        for i in 0..count {
            let prefix = format!("track-list/{}", i);

            let track_type: String =
                self.mpv.get_property(&format!("{}/type", prefix))?;
            let id: i64 =
                self.mpv.get_property(&format!("{}/id", prefix))?;
            let title: String = self
                .mpv
                .get_property(&format!("{}/title", prefix))
                .unwrap_or_default();
            let lang: Option<String> = self
                .mpv
                .get_property(&format!("{}/lang", prefix))
                .ok();
            let codec: Option<String> = self
                .mpv
                .get_property(&format!("{}/codec", prefix))
                .ok();
            let is_default: bool = self
                .mpv
                .get_property(&format!("{}/default", prefix))
                .unwrap_or(false);

            let info = TrackInfo {
                id: id as i32,
                title,
                language: lang,
                codec,
                is_default,
            };

            match track_type.as_str() {
                "audio" => audio_tracks.push(info),
                "sub" => sub_tracks.push(info),
                _ => {} // ignore video tracks etc.
            }
        }

        Ok((audio_tracks, sub_tracks))
    }

    /// Returns `true` if mpv is actively playing (not paused, not idle).
    pub fn is_playing(&self) -> bool {
        let paused: bool = self.mpv.get_property("pause").unwrap_or(true);
        let idle: bool = self.mpv.get_property("idle-active").unwrap_or(true);
        !paused && !idle
    }

    // -----------------------------------------------------------------------
    // Event loop
    // -----------------------------------------------------------------------

    /// Run the mpv event loop. This spawns a blocking task that polls mpv
    /// events and translates them into `PlayerEvent` messages.
    ///
    /// Call this once after creating the player; it runs until mpv shuts down.
    pub async fn run_event_loop(&self) {
        let mpv = Arc::clone(&self.mpv);
        let tx = self.event_tx.clone();

        tokio::task::spawn_blocking(move || {
            // Observe properties we care about.
            let ev_ctx = mpv.event_context();

            if let Err(e) = ev_ctx.observe_property("time-pos", Format::Double, REPLY_TIME_POS) {
                error!("failed to observe time-pos: {}", e);
            }
            if let Err(e) = ev_ctx.observe_property("pause", Format::Flag, REPLY_PAUSE) {
                error!("failed to observe pause: {}", e);
            }
            if let Err(e) = ev_ctx.observe_property("volume", Format::Double, REPLY_VOLUME) {
                error!("failed to observe volume: {}", e);
            }
            if let Err(e) = ev_ctx.observe_property("mute", Format::Flag, REPLY_MUTE) {
                error!("failed to observe mute: {}", e);
            }
            if let Err(e) = ev_ctx.observe_property("aid", Format::Int64, REPLY_AID) {
                error!("failed to observe aid: {}", e);
            }
            if let Err(e) = ev_ctx.observe_property("sid", Format::Int64, REPLY_SID) {
                error!("failed to observe sid: {}", e);
            }

            // Enable property-change events to be delivered.
            ev_ctx.disable_deprecated_events().ok();

            info!("mpv event loop started");

            loop {
                // Wait up to 500ms for the next event (gives ~2 Hz position updates).
                match ev_ctx.wait_event(0.5) {
                    Some(Ok(event)) => {
                        Self::handle_event(&mpv, &tx, event);
                    }
                    Some(Err(e)) => {
                        warn!("mpv event error: {}", e);
                    }
                    None => {
                        // Timeout — no event within 500ms; that's fine.
                    }
                }

                // If the channel is closed the receiver has been dropped;
                // there's nobody listening so we can exit the loop.
                if tx.is_closed() {
                    info!("event channel closed, stopping event loop");
                    break;
                }
            }

            info!("mpv event loop exited");
        })
        .await
        .ok();
    }

    /// Translate a single mpv event into zero or more `PlayerEvent` messages.
    fn handle_event(mpv: &Mpv, tx: &mpsc::UnboundedSender<PlayerEvent>, event: Event<'_>) {
        match event {
            Event::PropertyChange {
                name,
                change,
                reply_userdata,
            } => {
                match reply_userdata {
                    REPLY_TIME_POS => {
                        if let PropertyData::Double(pos) = change {
                            let position_ms = (pos * 1000.0) as i64;
                            let duration_ms = mpv
                                .get_property::<f64>("duration")
                                .map(|d| (d * 1000.0) as i64)
                                .unwrap_or(0);
                            let _ = tx.send(PlayerEvent::PositionChanged {
                                position_ms,
                                duration_ms,
                            });
                        }
                    }
                    REPLY_PAUSE => {
                        if let PropertyData::Flag(paused) = change {
                            let ev = if paused {
                                PlayerEvent::Paused
                            } else {
                                PlayerEvent::Playing
                            };
                            let _ = tx.send(ev);
                        }
                    }
                    REPLY_VOLUME => {
                        if let PropertyData::Double(vol) = change {
                            let _ = tx.send(PlayerEvent::VolumeChanged(vol));
                        }
                    }
                    REPLY_MUTE => {
                        if let PropertyData::Flag(muted) = change {
                            let _ = tx.send(PlayerEvent::MuteChanged(muted));
                        }
                    }
                    REPLY_AID => {
                        if let PropertyData::Int64(id) = change {
                            let _ = tx.send(PlayerEvent::AudioTrackChanged(id as i32));
                        }
                    }
                    REPLY_SID => {
                        if let PropertyData::Int64(id) = change {
                            let _ = tx.send(PlayerEvent::SubtitleTrackChanged(id as i32));
                        }
                    }
                    _ => {
                        debug!("unhandled property change: {} (ud={})", name, reply_userdata);
                    }
                }
            }

            Event::EndFile(reason) => {
                // EndFile reason 0 = EOF, others indicate error/redirect/etc.
                match reason {
                    0 => {
                        info!("end of file reached");
                        let _ = tx.send(PlayerEvent::EndOfFile);
                    }
                    2 => {
                        // error
                        error!("mpv end-file with error (reason=2)");
                        let _ = tx.send(PlayerEvent::Error(
                            "playback ended with error".to_string(),
                        ));
                    }
                    _ => {
                        debug!("end-file reason={}", reason);
                        let _ = tx.send(PlayerEvent::EndOfFile);
                    }
                }
            }

            Event::FileLoaded => {
                info!("file loaded, querying tracks");
                // Query track list and send TracksAvailable event.
                let count: i64 = mpv.get_property("track-list/count").unwrap_or(0);
                let mut audio = Vec::new();
                let mut subtitles = Vec::new();

                for i in 0..count {
                    let prefix = format!("track-list/{}", i);
                    let track_type: String = mpv
                        .get_property(&format!("{}/type", prefix))
                        .unwrap_or_default();
                    let id: i64 = mpv
                        .get_property(&format!("{}/id", prefix))
                        .unwrap_or(0);
                    let title: String = mpv
                        .get_property(&format!("{}/title", prefix))
                        .unwrap_or_default();
                    let lang: Option<String> = mpv
                        .get_property(&format!("{}/lang", prefix))
                        .ok();
                    let codec: Option<String> = mpv
                        .get_property(&format!("{}/codec", prefix))
                        .ok();
                    let is_default: bool = mpv
                        .get_property(&format!("{}/default", prefix))
                        .unwrap_or(false);

                    let info = TrackInfo {
                        id: id as i32,
                        title,
                        language: lang,
                        codec,
                        is_default,
                    };

                    match track_type.as_str() {
                        "audio" => audio.push(info),
                        "sub" => subtitles.push(info),
                        _ => {}
                    }
                }

                let _ = tx.send(PlayerEvent::TracksAvailable { audio, subtitles });
            }

            Event::StartFile => {
                debug!("start-file");
            }

            Event::Shutdown => {
                info!("mpv shutdown event received");
                let _ = tx.send(PlayerEvent::Stopped);
            }

            _ => {
                debug!("unhandled mpv event: {:?}", event);
            }
        }
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) {
        info!("MpvPlayer dropped");
    }
}
