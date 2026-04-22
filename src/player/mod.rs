pub mod vlc;
pub use vlc::VlcPlayer;
pub mod mpv;
pub use mpv::MpvPlayer;
pub mod controls;
pub use controls::PlaybackControls;

use vlc::{PlayerError, PlayerEvent, PlayerResult, TrackInfo};
use tokio::sync::mpsc;

use log::info;
use std::process::Command as StdCommand;

/// Kill ALL running media player processes system-wide.
/// Ensures only one media player can ever be active at a time.
/// Called before launching any new player instance.
pub fn kill_all_media_players() {
    let targets = [
        "vlc", "cvlc", "mpv", "ffplay", "mplayer",
        "jellyfinmediaplayer",
        "totem", "celluloid", "parole",
    ];
    for target in &targets {
        // Use pkill to kill by exact process name
        let _ = StdCommand::new("pkill")
            .args(["-x", target])
            .output();
    }
    // Also kill by broader pattern for VLC variants
    let _ = StdCommand::new("pkill")
        .args(["-f", "vlc --fullscreen"])
        .output();
    // Small grace period for processes to die
    std::thread::sleep(std::time::Duration::from_millis(200));
    info!("kill_all_media_players: cleared all competing players");
}

/// Unified player wrapper dispatching to VLC or MPV.
pub enum PlayerWrapper {
    Vlc(VlcPlayer),
    Mpv(MpvPlayer),
}

impl PlayerWrapper {
    pub fn new_vlc() -> PlayerResult<Self> { Ok(Self::Vlc(VlcPlayer::new()?)) }
    pub fn new_mpv() -> PlayerResult<Self> { Ok(Self::Mpv(MpvPlayer::new()?)) }

    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<PlayerEvent>> {
        match self { Self::Vlc(p) => p.take_event_receiver(), Self::Mpv(p) => p.take_event_receiver() }
    }
    pub async fn play_url(&self, url: &str, start_position_ms: Option<i64>) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.play_url(url, start_position_ms).await, Self::Mpv(p) => p.play_url(url, start_position_ms).await }
    }
    pub async fn send_command(&self, cmd: &str) -> PlayerResult<String> {
        match self { Self::Vlc(p) => p.send_command(cmd).await, Self::Mpv(p) => p.send_command(cmd).await }
    }
    pub async fn toggle_pause(&self) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.toggle_pause().await, Self::Mpv(p) => p.toggle_pause().await }
    }
    pub async fn stop(&self) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.stop().await, Self::Mpv(p) => p.stop().await }
    }
    pub async fn seek_to(&self, position_ms: i64) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.seek_to(position_ms).await, Self::Mpv(p) => p.seek_to(position_ms).await }
    }
    pub async fn seek_relative(&self, offset_seconds: f64) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.seek_relative(offset_seconds).await, Self::Mpv(p) => p.seek_relative(offset_seconds).await }
    }
    pub async fn set_volume(&self, volume: f64) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.set_volume(volume).await, Self::Mpv(p) => p.set_volume(volume).await }
    }
    pub async fn get_volume(&self) -> PlayerResult<f64> {
        match self { Self::Vlc(p) => p.get_volume().await, Self::Mpv(p) => p.get_volume().await }
    }
    pub async fn toggle_mute(&self) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.toggle_mute().await, Self::Mpv(p) => p.toggle_mute().await }
    }
    pub async fn set_mute(&self, muted: bool) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.set_mute(muted).await, Self::Mpv(p) => p.set_mute(muted).await }
    }
    pub async fn set_audio_track(&self, track_id: i32) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.set_audio_track(track_id).await, Self::Mpv(p) => p.set_audio_track(track_id).await }
    }
    pub async fn set_subtitle_track(&self, track_id: i32) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.set_subtitle_track(track_id).await, Self::Mpv(p) => p.set_subtitle_track(track_id).await }
    }
    pub async fn get_tracks(&self) -> PlayerResult<(Vec<TrackInfo>, Vec<TrackInfo>)> {
        match self { Self::Vlc(p) => p.get_tracks().await, Self::Mpv(p) => p.get_tracks().await }
    }
    pub async fn get_chapter_count(&self) -> PlayerResult<i32> {
        match self { Self::Vlc(p) => p.get_chapter_count().await, Self::Mpv(p) => p.get_chapter_count().await }
    }
    pub async fn set_chapter(&self, index: i32) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.set_chapter(index).await, Self::Mpv(p) => p.set_chapter(index).await }
    }
    pub async fn queue_url(&self, url: &str) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.queue_url(url).await, Self::Mpv(p) => p.queue_url(url).await }
    }
    pub async fn get_position_ms(&self) -> PlayerResult<i64> {
        match self { Self::Vlc(p) => p.get_position_ms().await, Self::Mpv(p) => p.get_position_ms().await }
    }
    pub async fn get_duration_ms(&self) -> PlayerResult<i64> {
        match self { Self::Vlc(p) => p.get_duration_ms().await, Self::Mpv(p) => p.get_duration_ms().await }
    }
    pub async fn pause(&self) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.pause().await, Self::Mpv(p) => p.pause().await }
    }
    pub async fn resume(&self) -> PlayerResult<()> {
        match self { Self::Vlc(p) => p.resume().await, Self::Mpv(p) => p.resume().await }
    }
    pub async fn is_playing(&self) -> bool {
        match self { Self::Vlc(p) => p.is_playing().await, Self::Mpv(p) => p.is_playing().await }
    }
    pub async fn run_event_loop(&self) {
        match self { Self::Vlc(p) => p.run_event_loop().await, Self::Mpv(p) => p.run_event_loop().await }
    }
}
