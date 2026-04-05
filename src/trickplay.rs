//! Trickplay thumbnail manager for Jellyfin.
//!
//! Fetches trickplay tile sprite sheets from the Jellyfin API and calculates
//! which sub-rectangle to crop for a given playback position. Tile sheets
//! are cached in memory to avoid re-fetching.
//!
//! URL pattern: `{server}/Videos/{itemId}/Trickplay/{width}/{tileIndex}.jpg?api_key={token}`

use std::collections::HashMap;

/// Trickplay metadata describing the sprite-sheet layout.
///
/// Mirrors the `TrickplayInfo` struct returned by the Jellyfin API, but kept
/// self-contained so this module can compile independently.
#[derive(Debug, Clone)]
pub struct TrickplayInfo {
    /// Width of each individual thumbnail in pixels.
    pub width: i32,
    /// Height of each individual thumbnail in pixels.
    pub height: i32,
    /// Number of thumbnail columns per tile sheet.
    pub tile_width: i32,
    /// Number of thumbnail rows per tile sheet.
    pub tile_height: i32,
    /// Total number of thumbnails across all tile sheets.
    pub thumbnail_count: i32,
    /// Milliseconds between consecutive thumbnails.
    pub interval: i32,
}

impl TrickplayInfo {
    /// How many thumbnails fit on a single tile sheet.
    fn thumbnails_per_tile(&self) -> i32 {
        self.tile_width * self.tile_height
    }
}

/// Manages trickplay tile fetching and position-to-crop mapping.
pub struct TrickplayManager {
    server_url: String,
    item_id: String,
    access_token: String,
    info: Option<TrickplayInfo>,
    tile_cache: HashMap<usize, Vec<u8>>,
    http: reqwest::Client,
}

impl TrickplayManager {
    /// Create a new manager for a specific media item.
    pub fn new(server_url: String, item_id: String, access_token: String) -> Self {
        Self {
            server_url,
            item_id,
            access_token,
            info: None,
            tile_cache: HashMap::new(),
            http: reqwest::Client::new(),
        }
    }

    /// Set (or replace) the trickplay metadata for the current item.
    pub fn set_info(&mut self, info: TrickplayInfo) {
        // Clear the cache when metadata changes (e.g. new item).
        self.tile_cache.clear();
        self.info = Some(info);
    }

    /// Return the current trickplay info, if set.
    pub fn info(&self) -> Option<&TrickplayInfo> {
        self.info.as_ref()
    }

    /// Build the URL for a specific tile sheet image.
    ///
    /// Jellyfin serves tile sheets at:
    ///   `{server}/Videos/{itemId}/Trickplay/{width}/{tileIndex}.jpg?api_key={token}`
    pub fn get_tile_url(&self, tile_index: usize) -> String {
        let width = self
            .info
            .as_ref()
            .map(|i| i.width)
            .unwrap_or(320);

        format!(
            "{}/Videos/{}/Trickplay/{}/{}.jpg?api_key={}",
            self.server_url, self.item_id, width, tile_index, self.access_token,
        )
    }

    /// Map a playback position (in milliseconds) to a tile-sheet index and
    /// the pixel coordinates of the thumbnail within that sheet.
    ///
    /// Returns `(tile_sheet_index, crop_x, crop_y)` where crop_x/crop_y are
    /// the top-left pixel of the thumbnail inside the tile sheet.
    ///
    /// Returns `None` if no trickplay info has been set.
    pub fn position_to_tile(&self, position_ms: i64) -> Option<(usize, i32, i32)> {
        let info = self.info.as_ref()?;

        if info.interval <= 0 || info.thumbnail_count <= 0 {
            return None;
        }

        // Which thumbnail index does this position correspond to?
        let thumb_index = (position_ms / info.interval as i64)
            .max(0)
            .min(info.thumbnail_count as i64 - 1) as i32;

        let per_tile = info.thumbnails_per_tile();
        if per_tile <= 0 {
            return None;
        }

        // Which tile sheet contains this thumbnail?
        let tile_sheet_index = (thumb_index / per_tile) as usize;

        // Position within the tile sheet (row, column).
        let index_in_tile = thumb_index % per_tile;
        let col = index_in_tile % info.tile_width;
        let row = index_in_tile / info.tile_width;

        let crop_x = col * info.width;
        let crop_y = row * info.height;

        Some((tile_sheet_index, crop_x, crop_y))
    }

    /// Convenience: get the full information needed to display a thumbnail
    /// for a given playback position.
    ///
    /// Returns `(image_url, crop_x, crop_y, crop_w, crop_h)`.
    pub fn get_thumbnail_for_position(
        &self,
        position_ms: i64,
    ) -> Option<(String, i32, i32, i32, i32)> {
        let info = self.info.as_ref()?;
        let (tile_index, crop_x, crop_y) = self.position_to_tile(position_ms)?;
        let url = self.get_tile_url(tile_index);

        Some((url, crop_x, crop_y, info.width, info.height))
    }

    /// Fetch a tile sheet image from the server, using the in-memory cache.
    ///
    /// Returns the raw JPEG bytes on success.
    pub async fn fetch_tile(&mut self, tile_index: usize) -> Result<Vec<u8>, reqwest::Error> {
        // Return cached copy if available.
        if let Some(cached) = self.tile_cache.get(&tile_index) {
            return Ok(cached.clone());
        }

        let url = self.get_tile_url(tile_index);
        let bytes = self.http.get(&url).send().await?.bytes().await?.to_vec();

        self.tile_cache.insert(tile_index, bytes.clone());
        Ok(bytes)
    }

    /// Fetch the tile sheet for a given playback position and return the raw
    /// bytes along with the crop rectangle.
    ///
    /// Returns `(bytes, crop_x, crop_y, crop_w, crop_h)`.
    pub async fn fetch_thumbnail_for_position(
        &mut self,
        position_ms: i64,
    ) -> Option<Result<(Vec<u8>, i32, i32, i32, i32), reqwest::Error>> {
        let info = self.info.as_ref()?;
        let (tile_index, crop_x, crop_y) = self.position_to_tile(position_ms)?;
        let crop_w = info.width;
        let crop_h = info.height;

        Some(
            self.fetch_tile(tile_index)
                .await
                .map(|bytes| (bytes, crop_x, crop_y, crop_w, crop_h)),
        )
    }

    /// Clear the tile cache (e.g. when switching to a different media item).
    pub fn clear_cache(&mut self) {
        self.tile_cache.clear();
    }

    /// Reset the manager for a new media item.
    pub fn reset(&mut self, item_id: String) {
        self.item_id = item_id;
        self.info = None;
        self.tile_cache.clear();
    }
}

// =============================================================================
// Conversion from the API model
// =============================================================================

impl From<&crate::api::models::TrickplayInfo> for TrickplayInfo {
    fn from(api: &crate::api::models::TrickplayInfo) -> Self {
        Self {
            width: api.width,
            height: api.height,
            tile_width: api.tile_width,
            tile_height: api.tile_height,
            thumbnail_count: api.thumbnail_count,
            interval: api.interval,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info() -> TrickplayInfo {
        TrickplayInfo {
            width: 320,
            height: 180,
            tile_width: 10,
            tile_height: 10,
            thumbnail_count: 250,
            interval: 10_000, // 10s
        }
    }

    #[test]
    fn position_to_tile_first_thumbnail() {
        let mut mgr = TrickplayManager::new(
            "http://localhost:8096".into(),
            "abc123".into(),
            "token".into(),
        );
        mgr.set_info(sample_info());

        let result = mgr.position_to_tile(0);
        assert_eq!(result, Some((0, 0, 0)));
    }

    #[test]
    fn position_to_tile_second_row() {
        let mut mgr = TrickplayManager::new(
            "http://localhost:8096".into(),
            "abc123".into(),
            "token".into(),
        );
        mgr.set_info(sample_info());

        // 10 thumbnails per row, interval=10s → position 100s = thumb index 10 = row 1, col 0
        let result = mgr.position_to_tile(100_000);
        assert_eq!(result, Some((0, 0, 180)));
    }

    #[test]
    fn position_to_tile_second_sheet() {
        let mut mgr = TrickplayManager::new(
            "http://localhost:8096".into(),
            "abc123".into(),
            "token".into(),
        );
        mgr.set_info(sample_info());

        // 100 thumbs per tile sheet (10x10), interval=10s
        // Position 1_000_000ms = 1000s → thumb index 100 → sheet 1, index 0
        let result = mgr.position_to_tile(1_000_000);
        assert_eq!(result, Some((1, 0, 0)));
    }

    #[test]
    fn position_to_tile_clamps_past_end() {
        let mut mgr = TrickplayManager::new(
            "http://localhost:8096".into(),
            "abc123".into(),
            "token".into(),
        );
        mgr.set_info(sample_info());

        // Way past the last thumbnail
        let result = mgr.position_to_tile(99_999_999);
        // Should clamp to last thumbnail (index 249)
        // 249 / 100 = sheet 2, index_in_tile = 49, col=9, row=4
        assert_eq!(result, Some((2, 9 * 320, 4 * 180)));
    }

    #[test]
    fn position_to_tile_none_without_info() {
        let mgr = TrickplayManager::new(
            "http://localhost:8096".into(),
            "abc123".into(),
            "token".into(),
        );
        assert_eq!(mgr.position_to_tile(5000), None);
    }

    #[test]
    fn get_tile_url_format() {
        let mut mgr = TrickplayManager::new(
            "http://jellyfin:8096".into(),
            "item-xyz".into(),
            "mytoken".into(),
        );
        mgr.set_info(sample_info());

        let url = mgr.get_tile_url(3);
        assert_eq!(
            url,
            "http://jellyfin:8096/Videos/item-xyz/Trickplay/320/3.jpg?api_key=mytoken"
        );
    }

    #[test]
    fn get_thumbnail_for_position_returns_crop() {
        let mut mgr = TrickplayManager::new(
            "http://localhost:8096".into(),
            "abc123".into(),
            "token".into(),
        );
        mgr.set_info(sample_info());

        // Position 55s → thumb index 5, col=5, row=0
        let result = mgr.get_thumbnail_for_position(55_000);
        assert!(result.is_some());
        let (url, cx, cy, cw, ch) = result.unwrap();
        assert!(url.contains("/Trickplay/320/0.jpg"));
        assert_eq!(cx, 5 * 320);
        assert_eq!(cy, 0);
        assert_eq!(cw, 320);
        assert_eq!(ch, 180);
    }
}
