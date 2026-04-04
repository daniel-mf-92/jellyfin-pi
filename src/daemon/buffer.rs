use std::sync::Arc;
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, RwLock};
use tokio::io::AsyncWriteExt;
use tokio::time::Duration;
use log::{info, warn, debug, error};

use crate::api::JellyfinClient;
use super::{DaemonEvent, DaemonShared};

const BUFFER_DIR: &str = "/tmp/jellyfin-buffer";
const CURRENT_ID_FILE: &str = "/tmp/jellyfin-buffer/.current_id";
const AUTOPLAY_THRESHOLD: u64 = 100 * 1024 * 1024; // 100MB

pub struct BufferManager {
    shared: Arc<DaemonShared>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    client: Arc<RwLock<JellyfinClient>>,
    interval_sec: u64,
    min_free_ram_mb: u64,
}

impl BufferManager {
    pub fn new(
        shared: Arc<DaemonShared>,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        client: Arc<RwLock<JellyfinClient>>,
        interval_sec: u64,
        min_free_ram_mb: u64,
    ) -> Self {
        Self {
            shared,
            event_tx,
            client,
            interval_sec,
            min_free_ram_mb,
        }
    }

    pub async fn run(self) {
        // Wait for initial bandwidth measurement
        tokio::time::sleep(Duration::from_secs(20)).await;

        // Ensure buffer directory exists
        let _ = tokio::fs::create_dir_all(BUFFER_DIR).await;

        let mut interval = tokio::time::interval(Duration::from_secs(self.interval_sec));
        loop {
            interval.tick().await;

            // RAM pressure cleanup
            if let Err(e) = self.evict_for_ram_pressure().await {
                warn!("Buffer eviction error: {}", e);
            }

            // Check if there's a current buffer target
            let current_id = match tokio::fs::read_to_string(CURRENT_ID_FILE).await {
                Ok(id) => {
                    let id = id.trim().to_string();
                    if id.is_empty() { continue; } else { id }
                }
                Err(_) => continue,
            };

            // Skip if download already in progress
            if super::system::get_pids_pattern(&format!("curl.*{}", current_id)).await.len() > 0 {
                // Log progress
                let buffer_file = PathBuf::from(BUFFER_DIR).join(format!("{}.mkv", current_id));
                if let Ok(meta) = tokio::fs::metadata(&buffer_file).await {
                    debug!("Buffer download in progress: {} at {}MB", current_id, meta.len() / 1_048_576);
                }
                // Check autoplay
                self.check_autoplay(&current_id).await;
                continue;
            }

            // Attempt download
            if let Err(e) = self.download_to_buffer(&current_id).await {
                warn!("Buffer download error for {}: {}", current_id, e);
            }

            // Check autoplay
            self.check_autoplay(&current_id).await;
        }
    }

    /// Read /proc/meminfo and return MemAvailable in MB.
    async fn available_ram_mb() -> u64 {
        let meminfo = match tokio::fs::read_to_string("/proc/meminfo").await {
            Ok(s) => s,
            Err(_) => return u64::MAX, // Can't read, assume enough
        };

        for line in meminfo.lines() {
            if line.starts_with("MemAvailable:") {
                let kb: u64 = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                return kb / 1024;
            }
        }

        u64::MAX
    }

    /// Evict oldest buffer files until free RAM >= min_free_ram_mb.
    async fn evict_for_ram_pressure(&self) -> anyhow::Result<()> {
        let avail = Self::available_ram_mb().await;
        if avail >= self.min_free_ram_mb {
            return Ok(());
        }

        info!("Buffer: RAM pressure {}MB < {}MB, evicting...", avail, self.min_free_ram_mb);

        let current_id = tokio::fs::read_to_string(CURRENT_ID_FILE)
            .await
            .unwrap_or_default()
            .trim()
            .to_string();

        // List .mkv files sorted by modification time (oldest first)
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(BUFFER_DIR).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("mkv") {
                if let Ok(meta) = entry.metadata().await {
                    if let Ok(modified) = meta.modified() {
                        entries.push((path, meta.len(), modified));
                    }
                }
            }
        }
        entries.sort_by_key(|(_, _, t)| *t);

        for (path, size, _) in entries {
            let basename = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            // Don't evict the currently playing item
            if basename == current_id {
                continue;
            }

            let size_mb = size / 1_048_576;
            tokio::fs::remove_file(&path).await?;
            info!("Buffer: evicted {} ({}MB)", basename, size_mb);

            // Re-check RAM
            if Self::available_ram_mb().await >= self.min_free_ram_mb {
                break;
            }
        }

        Ok(())
    }

    /// Download a transcode stream to the buffer directory.
    async fn download_to_buffer(&self, item_id: &str) -> anyhow::Result<()> {
        let buffer_file = PathBuf::from(BUFFER_DIR).join(format!("{}.mkv", item_id));

        // Get bandwidth profile
        let (video_br, audio_br) = {
            let bw = self.shared.bandwidth.read().await;
            match bw.as_ref() {
                Some(p) => (p.video_bitrate, p.audio_bitrate),
                None => (500_000, 128_000),
            }
        };

        // Get server URL and token
        let (server_url, token) = {
            let c = self.client.read().await;
            (
                c.server_url.clone(),
                c.access_token.clone().unwrap_or_default(),
            )
        };

        if token.is_empty() {
            return Ok(());
        }

        // Check existing file size for resume
        let existing_size = tokio::fs::metadata(&buffer_file)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        // Get item info to estimate expected size
        let item_url = format!(
            "{}/Items/{}?Fields=MediaSources&api_key={}",
            server_url, item_id, token
        );
        let http_client = reqwest::Client::new();
        let item_info = http_client
            .get(&item_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()
            .and_then(|r| futures::executor::block_on(r.json::<serde_json::Value>()).ok());

        let expected_size = item_info.as_ref().and_then(|info| {
            let ticks = info["RunTimeTicks"].as_u64().unwrap_or(0);
            let dur_secs = if ticks > 0 { ticks / 10_000_000 } else { 7200 };
            Some((video_br + audio_br) * dur_secs / 8)
        });

        // Check if already complete (>90% of expected)
        if let Some(expected) = expected_size {
            if existing_size > 0 && existing_size >= expected * 90 / 100 {
                debug!("Buffer complete: {} at {}MB", item_id, existing_size / 1_048_576);
                return Ok(());
            }
        }

        // Check RAM before downloading
        if Self::available_ram_mb().await <= self.min_free_ram_mb {
            debug!("Buffer: skipping download, RAM too low");
            return Ok(());
        }

        // Check for subtitle stream
        let sub_params = item_info.as_ref().and_then(|info| {
            let streams = info["MediaSources"][0]["MediaStreams"].as_array()?;
            let sub = streams.iter().find(|s| s["Type"].as_str() == Some("Subtitle"))?;
            let idx = sub["Index"].as_u64()?;
            Some(format!("&SubtitleStreamIndex={}&SubtitleMethod=Encode", idx))
        }).unwrap_or_default();

        // Build transcode URL
        let stream_url = format!(
            "{}/Videos/{}/stream.mkv?Static=false&VideoCodec=h264&AudioCodec=aac&MaxVideoBitDepth=8&VideoBitRate={}&AudioBitRate={}&MaxWidth=1280&MaxHeight=720{}&api_key={}",
            server_url, item_id, video_br, audio_br, sub_params, token
        );

        // Download using reqwest streaming
        let mut request = http_client.get(&stream_url).timeout(Duration::from_secs(7200));

        // Resume from existing size
        if existing_size > 0 {
            request = request.header("Range", format!("bytes={}-", existing_size));
            info!("Buffer: resuming {} from {}MB", item_id, existing_size / 1_048_576);
        } else {
            info!("Buffer: starting download for {} (target ~{}MB) at {}bps",
                item_id,
                expected_size.unwrap_or(0) / 1_048_576,
                video_br
            );
        }

        let response = request.send().await?;
        if !response.status().is_success() && response.status().as_u16() != 206 {
            warn!("Buffer: HTTP {} for {}", response.status(), item_id);
            return Ok(());
        }

        // Stream to file
        let mut file = if existing_size > 0 {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(&buffer_file)
                .await?
        } else {
            tokio::fs::File::create(&buffer_file).await?
        };

        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    file.write_all(&bytes).await?;

                    // Check RAM pressure during download
                    if Self::available_ram_mb().await <= self.min_free_ram_mb {
                        warn!("Buffer: stopping download due to RAM pressure");
                        break;
                    }
                }
                Err(e) => {
                    warn!("Buffer: download error: {}", e);
                    break;
                }
            }
        }

        file.flush().await?;
        let final_size = tokio::fs::metadata(&buffer_file)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        info!("Buffer: {} at {}MB", item_id, final_size / 1_048_576);

        Ok(())
    }

    /// Check if buffer is large enough for auto-play. Emit BufferReady if so.
    async fn check_autoplay(&self, item_id: &str) {
        let play_when_ready = PathBuf::from(BUFFER_DIR).join(".play_when_ready");
        if !play_when_ready.exists() {
            return;
        }

        let play_id = match tokio::fs::read_to_string(&play_when_ready).await {
            Ok(id) => id.trim().to_string(),
            Err(_) => return,
        };

        if play_id != item_id {
            return;
        }

        let buffer_file = PathBuf::from(BUFFER_DIR).join(format!("{}.mkv", item_id));
        let size = tokio::fs::metadata(&buffer_file)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        if size > AUTOPLAY_THRESHOLD {
            // Check no player is running
            if !super::system::is_process_running("vlc").await
                && !super::system::is_process_running("mpv").await
            {
                info!("Buffer: auto-play ready for {} ({}MB)", item_id, size / 1_048_576);
                let _ = tokio::fs::remove_file(&play_when_ready).await;
                let _ = self.event_tx.send(DaemonEvent::BufferReady {
                    item_id: item_id.to_string(),
                    path: buffer_file.to_string_lossy().to_string(),
                });
            }
        }
    }
}
