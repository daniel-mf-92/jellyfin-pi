use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use image::GenericImageView;
use log::{debug, error, info, warn};
use slint::Image as SlintImage;
use slint::Rgba8Pixel;
use slint::SharedPixelBuffer;
use tokio::sync::RwLock;

/// Maximum number of images kept in memory before eviction kicks in.
const DEFAULT_MAX_MEMORY_ITEMS: usize = 50;

/// Maximum number of concurrent image downloads during preload.
const MAX_CONCURRENT_DOWNLOADS: usize = 10;
const IMAGE_CONNECT_TIMEOUT_SECS: u64 = 5;
const IMAGE_REQUEST_TIMEOUT_SECS: u64 = 15;

/// Thread-safe, async image cache that stores decoded Slint images in memory
/// and raw image bytes on disk for fast reloading across sessions.
pub struct ImageCache {
    cache_dir: PathBuf,
    memory_cache: Arc<RwLock<HashMap<String, SlintImage>>>,
    http: reqwest::Client,
    max_memory_items: usize,
}

impl ImageCache {
    /// Create a new `ImageCache`.
    ///
    /// The on-disk cache lives under the platform-appropriate cache directory
    /// provided by `directories::ProjectDirs` (e.g. `~/.cache/jellyfin-pi/images`
    /// on Linux, `~/Library/Caches/org.jellyfin.jellyfin-pi/images` on macOS).
    /// The directory is created if it does not already exist.
    pub fn new(http: reqwest::Client) -> Self {
        let cache_dir = directories::ProjectDirs::from("org", "jellyfin", "jellyfin-pi")
            .map(|dirs| dirs.cache_dir().join("images"))
            .unwrap_or_else(|| {
                warn!("Could not determine platform cache directory, falling back to /tmp");
                PathBuf::from("/tmp/jellyfin-pi-images")
            });

        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            error!("Failed to create image cache directory {:?}: {}", cache_dir, e);
        } else {
            info!("Image cache directory: {:?}", cache_dir);
        }

        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(IMAGE_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(IMAGE_REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or(http);

        Self {
            cache_dir,
            memory_cache: Arc::new(RwLock::new(HashMap::new())),
            http,
            max_memory_items: DEFAULT_MAX_MEMORY_ITEMS,
        }
    }

    /// Load an image from cache (memory -> disk -> network), returning a Slint
    /// `Image` ready for display. Returns `None` if the image could not be
    /// obtained from any source.
    pub async fn load_image(&self, url: &str) -> Option<SlintImage> {
        // 1. Fast path: in-memory cache.
        {
            let cache = self.memory_cache.read().await;
            if let Some(img) = cache.get(url) {
                debug!("Image cache hit (memory): {}", url);
                return Some(img.clone());
            }
        }

        // 2. Disk cache.
        let disk_path = self.url_to_cache_path(url);
        if disk_path.exists() {
            debug!("Image cache hit (disk): {}", url);
            if let Some(img) = self.load_from_disk(&disk_path).await {
                // Promote to memory cache.
                self.insert_memory_cache(url, &img).await;
                return Some(img);
            } else {
                warn!("Disk-cached image corrupt, will re-download: {}", url);
                let _ = tokio::fs::remove_file(&disk_path).await;
            }
        }

        // 3. Download from network.
        debug!("Image cache miss, downloading: {}", url);
        let bytes = self.download_bytes(url).await?;

        // Save raw bytes to disk (before decode, so we store the original).
        if let Err(e) = self.save_to_disk(&disk_path, &bytes).await {
            warn!("Failed to persist image to disk cache: {}", e);
        }

        // Decode into a Slint image.
        let img = Self::decode_to_slint(&bytes)?;
        self.insert_memory_cache(url, &img).await;
        Some(img)
    }

    /// Deterministically map a URL to a file path inside the cache directory.
    /// Uses the standard `DefaultHasher` for a stable, fast hash.
    fn url_to_cache_path(&self, url: &str) -> PathBuf {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut hasher);
        let hash = hasher.finish();
        self.cache_dir.join(format!("{:016x}.img", hash))
    }

    /// Download image bytes and decode them into a `SlintImage`.
    pub async fn download_and_decode(&self, url: &str) -> Option<SlintImage> {
        let bytes = self.download_bytes(url).await?;
        Self::decode_to_slint(&bytes)
    }

    /// Write raw bytes to the given path on disk.
    async fn save_to_disk(&self, path: &PathBuf, data: &[u8]) -> Result<(), std::io::Error> {
        tokio::fs::write(path, data).await
    }

    /// Read an image from disk, decode it, and return as a `SlintImage`.
    async fn load_from_disk(&self, path: &PathBuf) -> Option<SlintImage> {
        let bytes = tokio::fs::read(path).await.ok()?;
        Self::decode_to_slint(&bytes)
    }

    /// If the memory cache exceeds `max_memory_items`, evict roughly half of
    /// the entries. This is a simple strategy that avoids tracking access order.
    async fn evict_if_needed(&self) {
        let mut cache = self.memory_cache.write().await;
        if cache.len() <= self.max_memory_items {
            return;
        }
        let target = self.max_memory_items / 2;
        let keys_to_remove: Vec<String> = cache
            .keys()
            .take(cache.len() - target)
            .cloned()
            .collect();
        for key in &keys_to_remove {
            cache.remove(key);
        }
        info!(
            "Evicted {} images from memory cache (was {}, now {})",
            keys_to_remove.len(),
            keys_to_remove.len() + cache.len(),
            cache.len()
        );
    }

    /// Download and cache multiple images sequentially.
    ///
    /// Note: runs on the Slint event loop thread (not spawned to tokio)
    /// because `SlintImage` is `!Send`.
    pub async fn preload_images(&self, urls: Vec<String>) {
        for url in urls {
            if self.load_image(&url).await.is_none() {
                warn!("Preload failed for: {}", url);
            }
        }
        debug!("Preload batch complete");
    }

    /// Drop all entries from the in-memory cache. The on-disk cache is
    /// preserved so images can still be loaded without a network request.
    pub async fn clear_memory_cache(&self) {
        let mut cache = self.memory_cache.write().await;
        let count = cache.len();
        cache.clear();
        info!("Cleared {} images from memory cache", count);
    }

    /// Remove every file in the on-disk cache directory.
    pub async fn clear_disk_cache(&self) -> Result<(), std::io::Error> {
        let mut dir = tokio::fs::read_dir(&self.cache_dir).await?;
        let mut removed = 0u64;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_file() {
                tokio::fs::remove_file(&path).await?;
                removed += 1;
            }
        }
        info!("Cleared {} files from disk cache", removed);
        Ok(())
    }

    // ---- private helpers ----

    /// Download raw bytes from `url`.
    async fn download_bytes(&self, url: &str) -> Option<Vec<u8>> {
        match self.http.get(url).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!("Image download failed (HTTP {}): {}", resp.status(), url);
                    return None;
                }
                match resp.bytes().await {
                    Ok(b) => Some(b.to_vec()),
                    Err(e) => {
                        error!("Failed to read image response body: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                error!("Image download request failed: {}", e);
                None
            }
        }
    }

    /// Decode raw image bytes (JPEG, PNG, WebP, etc.) into a Slint `Image`.
    fn decode_to_slint(bytes: &[u8]) -> Option<SlintImage> {
        let img = match image::load_from_memory(bytes) {
            Ok(i) => i,
            Err(e) => {
                error!("Failed to decode image: {}", e);
                return None;
            }
        };

        let rgba = img.to_rgba8();
        let (width, height) = img.dimensions();

        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            rgba.as_raw(),
            width,
            height,
        );

        Some(SlintImage::from_rgba8(buffer))
    }

    /// Insert an image into the memory cache, evicting if necessary.
    async fn insert_memory_cache(&self, url: &str, img: &SlintImage) {
        {
            let mut cache = self.memory_cache.write().await;
            cache.insert(url.to_string(), img.clone());
        }
        self.evict_if_needed().await;
    }
}
