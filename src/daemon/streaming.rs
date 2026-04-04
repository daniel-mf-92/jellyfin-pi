use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};
use log::{info, warn, debug};

use crate::player::vlc::PlayerEvent;
use super::{DaemonEvent, DaemonShared};

/// Internal streaming health monitor.
/// Reacts to VLC PlayerEvent::Buffering to adapt bitrate — no external log parsing.
pub struct StreamingHealth {
    shared: Arc<DaemonShared>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    player_event_rx: mpsc::UnboundedReceiver<PlayerEvent>,
}

impl StreamingHealth {
    pub fn new(
        shared: Arc<DaemonShared>,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        player_event_rx: mpsc::UnboundedReceiver<PlayerEvent>,
    ) -> Self {
        Self {
            shared,
            event_tx,
            player_event_rx,
        }
    }

    pub async fn run(mut self) {
        let mut buffering_count: u32 = 0;
        let mut launched_bitrate: Option<u64> = None;
        let mut last_buffering_at: Option<Instant> = None;
        let mut playing = false;

        // Reset buffering count if no buffering for 2 minutes
        let buffering_decay = Duration::from_secs(120);

        loop {
            tokio::select! {
                event = self.player_event_rx.recv() => {
                    match event {
                        Some(PlayerEvent::Playing) => {
                            playing = true;
                            // Record the launched bitrate from shared state
                            let bw = self.shared.bandwidth.read().await;
                            launched_bitrate = bw.as_ref().map(|p| p.video_bitrate);
                            buffering_count = 0;
                            last_buffering_at = None;
                            debug!("StreamingHealth: playback started, bitrate={:?}", launched_bitrate);
                        }
                        Some(PlayerEvent::Stopped) | Some(PlayerEvent::EndOfFile) => {
                            playing = false;
                            buffering_count = 0;
                            launched_bitrate = None;
                            last_buffering_at = None;
                        }
                        Some(PlayerEvent::Buffering(percent)) => {
                            if !playing {
                                continue;
                            }
                            buffering_count += 1;
                            last_buffering_at = Some(Instant::now());
                            debug!("StreamingHealth: buffering event #{} ({}%)", buffering_count, percent);

                            // If >3 buffering events, reduce bitrate
                            if buffering_count > 3 {
                                self.reduce_bitrate().await;
                                buffering_count = 0;
                            }
                        }
                        None => break,
                        _ => {}
                    }
                }
                // Periodic check: upscale if bandwidth improved
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    if !playing {
                        continue;
                    }

                    // Decay buffering count if no recent buffering
                    if let Some(last) = last_buffering_at {
                        if last.elapsed() > buffering_decay {
                            buffering_count = 0;
                            last_buffering_at = None;
                        }
                    }

                    // Check for upscale opportunity
                    if buffering_count == 0 {
                        if let Some(lb) = launched_bitrate {
                            let bw = self.shared.bandwidth.read().await;
                            if let Some(ref profile) = *bw {
                                if lb > 0 && profile.video_bitrate > lb * 3 / 2 {
                                    info!(
                                        "StreamingHealth: bandwidth improved ({} -> {}), upscaling",
                                        lb, profile.video_bitrate
                                    );
                                    let _ = self.event_tx.send(DaemonEvent::BitrateAdapted {
                                        video_bitrate: profile.video_bitrate,
                                        audio_bitrate: profile.audio_bitrate,
                                    });
                                    launched_bitrate = Some(profile.video_bitrate);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Reduce video bitrate by 40%, emit BitrateAdapted event.
    async fn reduce_bitrate(&self) {
        let bw = self.shared.bandwidth.read().await;
        let current_video = bw.as_ref().map(|p| p.video_bitrate).unwrap_or(2_000_000);
        drop(bw);

        let new_video = (current_video as f64 * 0.60) as u64;
        let new_video = new_video.max(150_000);

        let new_audio = if new_video < 300_000 { 64_000 } else { 128_000 };

        // Update shared bandwidth profile with reduced values
        {
            let mut bw = self.shared.bandwidth.write().await;
            if let Some(ref mut profile) = *bw {
                profile.video_bitrate = new_video;
                profile.audio_bitrate = new_audio;
                if new_video < 400_000 {
                    profile.max_width = 640;
                    profile.max_height = 360;
                } else if new_video < 700_000 {
                    profile.max_width = 854;
                    profile.max_height = 480;
                } else {
                    profile.max_width = 1280;
                    profile.max_height = 720;
                }
            }
        }

        info!(
            "StreamingHealth: reduced bitrate {} -> {} (audio: {})",
            current_video, new_video, new_audio
        );

        let _ = self.event_tx.send(DaemonEvent::BitrateAdapted {
            video_bitrate: new_video,
            audio_bitrate: new_audio,
        });
    }
}
