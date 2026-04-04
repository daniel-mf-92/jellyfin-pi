use std::sync::Arc;
use std::path::PathBuf;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Duration;
use log::{info, warn, debug};

use crate::api::JellyfinClient;
use super::{BandwidthProfile, DaemonEvent, DaemonShared};

const BW_FILE_PATH: &str = "/tmp/pi-home-wg-bandwidth.json";
const DOWNLOAD_SIZE: u64 = 2_097_152; // 2MB

pub struct BandwidthMonitor {
    shared: Arc<DaemonShared>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    client: Arc<RwLock<JellyfinClient>>,
    test_item_id: String,
    interval_sec: u64,
}

impl BandwidthMonitor {
    pub fn new(
        shared: Arc<DaemonShared>,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        client: Arc<RwLock<JellyfinClient>>,
        test_item_id: String,
        interval_sec: u64,
    ) -> Self {
        Self {
            shared,
            event_tx,
            client,
            test_item_id,
            interval_sec,
        }
    }

    pub async fn run(self) {
        // Initial measurement after 10s (let things settle)
        tokio::time::sleep(Duration::from_secs(10)).await;

        let mut interval = tokio::time::interval(Duration::from_secs(self.interval_sec));
        loop {
            interval.tick().await;

            match self.measure_once().await {
                Some(profile) => {
                    // Update shared state
                    {
                        let mut bw = self.shared.bandwidth.write().await;
                        *bw = Some(profile.clone());
                    }

                    // Write backward-compatible JSON file
                    if let Err(e) = self.write_compat_json(&profile).await {
                        warn!("Failed to write bandwidth JSON: {}", e);
                    }

                    info!(
                        "Bandwidth: {:.0} B/s, video: {} bps, audio: {} bps",
                        profile.raw_bytes_per_sec, profile.video_bitrate, profile.audio_bitrate
                    );

                    let _ = self.event_tx.send(DaemonEvent::BandwidthUpdated(profile));
                }
                None => {
                    debug!("Bandwidth measurement failed, keeping previous config");
                }
            }
        }
    }

    /// Single measurement: download 2MB from Jellyfin, measure throughput.
    async fn measure_once(&self) -> Option<BandwidthProfile> {
        let (server_url, token) = {
            let c = self.client.read().await;
            (
                c.server_url.clone(),
                c.access_token.clone().unwrap_or_default(),
            )
        };

        if token.is_empty() {
            debug!("No auth token, skipping bandwidth test");
            return None;
        }

        let url = format!(
            "{}/Videos/{}/stream?Static=true&api_key={}",
            server_url, self.test_item_id, token
        );

        let http_client = reqwest::Client::new();
        let start = tokio::time::Instant::now();

        let response = http_client
            .get(&url)
            .header("Range", format!("bytes=0-{}", DOWNLOAD_SIZE - 1))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?;

        if !response.status().is_success() && response.status().as_u16() != 206 {
            debug!("Bandwidth test HTTP {}", response.status());
            return None;
        }

        let bytes = response.bytes().await.ok()?;
        let elapsed = start.elapsed();

        if elapsed.as_millis() == 0 || bytes.is_empty() {
            return None;
        }

        let bytes_per_sec = bytes.len() as f64 / elapsed.as_secs_f64();
        let total_bps = (bytes_per_sec * 8.0) as u64;

        // 55% of total for video (rest for audio + overhead)
        let mut video_bps = (total_bps as f64 * 0.55) as u64;
        // Clamp 150kbps - 8Mbps
        video_bps = video_bps.clamp(150_000, 8_000_000);
        // Round to nearest 50kbps
        video_bps = (video_bps / 50_000) * 50_000;
        if video_bps < 150_000 {
            video_bps = 150_000;
        }

        // Adaptive audio
        let audio_bps = if video_bps < 300_000 { 64_000 } else { 128_000 };

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        Some(BandwidthProfile {
            timestamp,
            raw_bytes_per_sec: bytes_per_sec,
            total_bps,
            video_bitrate: video_bps,
            audio_bitrate: audio_bps,
            max_width: 1280,
            max_height: 720,
        })
    }

    /// Write profile to /tmp/pi-home-wg-bandwidth.json for backward compat.
    async fn write_compat_json(&self, profile: &BandwidthProfile) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(profile)?;
        tokio::fs::write(BW_FILE_PATH, json).await?;
        Ok(())
    }
}
