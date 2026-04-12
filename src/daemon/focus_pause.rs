use std::time::Duration;
use log::{info, debug, warn};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::player::vlc::PlayerEvent;

const VLC_SOCKET_PATH: &str = "/tmp/jellyfin-pi-vlc.sock";
const FOREGROUND_APP_PATH: &str = "/tmp/foreground-app";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// Apps that count as "us" — if foreground-app contains one of these, do NOT pause.
const ALLOWED_FOREGROUND_APPS: &[&str] = &[
    "jellyfinmediaplayer",
    "jellyfin-pi",
    "jellyfin",
    "vlc",
    "mpv",
];

pub struct FocusPauseWatcher {
    player_rx: mpsc::UnboundedReceiver<PlayerEvent>,
}

impl FocusPauseWatcher {
    pub fn new(player_rx: mpsc::UnboundedReceiver<PlayerEvent>) -> Self {
        Self { player_rx }
    }

    pub async fn run(mut self) {
        info!("FocusPause: watcher started");

        let mut playing = false;
        // Track whether we already paused due to defocus, so we don't spam pause commands
        let mut paused_by_defocus = false;

        loop {
            tokio::select! {
                event = self.player_rx.recv() => {
                    match event {
                        Some(PlayerEvent::Playing) => {
                            playing = true;
                            paused_by_defocus = false;
                            debug!("FocusPause: playback started");
                        }
                        Some(PlayerEvent::Paused) => {
                            // If we didn't cause this pause, reset our flag
                            if !paused_by_defocus {
                                playing = false;
                            }
                            debug!("FocusPause: playback paused");
                        }
                        Some(PlayerEvent::Stopped) | Some(PlayerEvent::EndOfFile) => {
                            playing = false;
                            paused_by_defocus = false;
                            debug!("FocusPause: playback stopped");
                        }
                        None => {
                            info!("FocusPause: player event channel closed, exiting");
                            break;
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if !playing || paused_by_defocus {
                        continue;
                    }

                    // Read current foreground app
                    let foreground = match tokio::fs::read_to_string(FOREGROUND_APP_PATH).await {
                        Ok(content) => content.trim().to_string(),
                        Err(_) => {
                            // File doesn't exist or unreadable — assume we're in focus
                            continue;
                        }
                    };

                    if foreground.is_empty() {
                        continue;
                    }

                    // Check if the foreground app is one of ours
                    let in_focus = ALLOWED_FOREGROUND_APPS
                        .iter()
                        .any(|&app| foreground == app);

                    if !in_focus {
                        info!(
                            "FocusPause: pausing — app lost focus (foreground: {})",
                            foreground
                        );
                        if let Err(e) = send_vlc_pause().await {
                            warn!("FocusPause: failed to send pause to VLC: {}", e);
                        } else {
                            paused_by_defocus = true;
                        }
                    }
                }
            }
        }
    }
}

/// Send a "pause" command directly to the VLC RC Unix socket.
async fn send_vlc_pause() -> Result<(), String> {
    let stream = match timeout(SOCKET_TIMEOUT, UnixStream::connect(VLC_SOCKET_PATH)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("socket connect failed: {}", e)),
        Err(_) => return Err("socket connect timeout".to_string()),
    };

    let (_reader, mut writer) = stream.into_split();

    match timeout(SOCKET_TIMEOUT, writer.write_all(b"pause\n")).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("socket write failed: {}", e)),
        Err(_) => Err("socket write timeout".to_string()),
    }
}
