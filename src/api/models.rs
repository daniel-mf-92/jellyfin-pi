use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Authentication
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    pub user: UserDto,
    pub access_token: String,
    pub server_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserDto {
    pub id: String,
    pub name: String,
    pub server_id: Option<String>,
    pub has_password: bool,
    pub has_configured_password: bool,
    pub primary_image_tag: Option<String>,
    pub last_login_date: Option<String>,
    pub last_activity_date: Option<String>,
}

// =============================================================================
// Query Results
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct QueryResult {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: i32,
}

// =============================================================================
// Core Item
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct BaseItemDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub original_title: Option<String>,
    pub sort_name: Option<String>,
    pub overview: Option<String>,
    pub taglines: Option<Vec<String>>,
    pub genres: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub community_rating: Option<f64>,
    pub critic_rating: Option<f64>,
    pub official_rating: Option<String>,
    pub production_year: Option<i32>,
    pub premiere_date: Option<String>,
    pub date_created: Option<String>,
    pub run_time_ticks: Option<i64>,
    pub collection_type: Option<String>,
    pub media_type: Option<String>,
    pub primary_image_tag: Option<String>,
    pub image_tags: Option<HashMap<String, String>>,
    pub backdrop_image_tags: Option<Vec<String>>,
    pub image_blur_hashes: Option<HashMap<String, HashMap<String, String>>>,
    pub user_data: Option<UserItemDataDto>,
    pub primary_image_aspect_ratio: Option<f64>,

    // Series/Episode fields
    pub series_name: Option<String>,
    pub series_id: Option<String>,
    pub season_id: Option<String>,
    pub season_name: Option<String>,
    pub parent_index_number: Option<i32>,
    pub index_number: Option<i32>,
    pub parent_backdrop_image_tags: Option<Vec<String>>,
    pub parent_thumb_item_id: Option<String>,
    pub series_primary_image_tag: Option<String>,

    // Media info
    pub media_sources: Option<Vec<MediaSourceInfo>>,
    pub media_streams: Option<Vec<MediaStream>>,

    // People & Studios
    pub people: Option<Vec<BaseItemPerson>>,
    pub studios: Option<Vec<NameGuidPair>>,

    // Chapters
    pub chapters: Option<Vec<ChapterInfo>>,

    // Child counts
    pub child_count: Option<i32>,
    pub recursive_item_count: Option<i32>,

    // Display preferences
    pub display_preferences_id: Option<String>,

    // Trickplay
    pub trickplay: Option<HashMap<String, HashMap<String, TrickplayInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserItemDataDto {
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub played: bool,
    pub played_percentage: Option<f64>,
    pub last_played_date: Option<String>,
    pub unplayed_item_count: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct MediaSourceInfo {
    pub id: String,
    pub name: Option<String>,
    pub path: Option<String>,
    pub protocol: Option<String>,
    pub container: Option<String>,
    pub size: Option<i64>,
    pub bitrate: Option<i64>,
    pub supports_direct_play: Option<bool>,
    pub supports_direct_stream: Option<bool>,
    pub supports_transcoding: Option<bool>,
    pub transcoding_url: Option<String>,
    pub media_streams: Option<Vec<MediaStream>>,
    pub direct_stream_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct MediaStream {
    #[serde(rename = "Type")]
    pub stream_type: String,
    pub index: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub display_title: Option<String>,
    pub title: Option<String>,
    pub is_default: Option<bool>,
    pub is_forced: Option<bool>,
    pub is_external: Option<bool>,
    pub is_text_subtitle_stream: Option<bool>,
    pub is_hearing_impaired: Option<bool>,
    pub channels: Option<i32>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_rate: Option<i64>,
    pub bit_depth: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub average_frame_rate: Option<f64>,
    pub video_range: Option<String>,
    pub video_range_type: Option<String>,
    pub color_space: Option<String>,
    pub profile: Option<String>,
    pub level: Option<f64>,
    pub aspect_ratio: Option<String>,
    pub delivery_method: Option<String>,
    pub delivery_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct BaseItemPerson {
    pub id: Option<String>,
    pub name: String,
    pub role: Option<String>,
    #[serde(rename = "Type")]
    pub person_type: Option<String>,
    pub primary_image_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct NameGuidPair {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct ChapterInfo {
    pub start_position_ticks: i64,
    pub name: Option<String>,
    pub image_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct TrickplayInfo {
    pub width: i32,
    pub height: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub thumbnail_count: i32,
    pub interval: i32,
    pub bandwidth: i32,
}

// =============================================================================
// Search
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct SearchHintResult {
    pub search_hints: Vec<SearchHint>,
    pub total_record_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct SearchHint {
    pub item_id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub media_type: Option<String>,
    pub primary_image_tag: Option<String>,
    pub thumb_image_tag: Option<String>,
    pub production_year: Option<i32>,
    pub series: Option<String>,
    pub run_time_ticks: Option<i64>,
    pub album: Option<String>,
}

// =============================================================================
// Playback
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlaybackInfoResponse {
    pub media_sources: Vec<MediaSourceInfo>,
    pub play_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlaybackStartInfo {
    pub item_id: String,
    pub media_source_id: Option<String>,
    pub play_session_id: Option<String>,
    pub play_method: String,
    pub position_ticks: i64,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlaybackProgressInfo {
    pub item_id: String,
    pub media_source_id: Option<String>,
    pub play_session_id: Option<String>,
    pub play_method: String,
    pub position_ticks: i64,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlaybackStopInfo {
    pub item_id: String,
    pub media_source_id: Option<String>,
    pub play_session_id: Option<String>,
    pub position_ticks: i64,
}

// =============================================================================
// Media Segments
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct MediaSegmentResult {
    pub items: Vec<MediaSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct MediaSegment {
    pub id: String,
    pub item_id: String,
    #[serde(rename = "Type")]
    pub segment_type: String,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

// =============================================================================
// System
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct PublicSystemInfo {
    pub server_name: String,
    pub version: String,
    pub id: String,
    #[serde(alias = "startupWizardCompleted")]
    pub startup_wizard_completed: Option<bool>,
}

// =============================================================================
// Helper Implementations
// =============================================================================

impl BaseItemDto {
    /// Get primary image URL for this item.
    pub fn primary_image_url(&self, server_url: &str, max_height: i32) -> Option<String> {
        let tag = self
            .image_tags
            .as_ref()
            .and_then(|tags| tags.get("Primary"))
            .map(|value| value.as_str())
            .or(self.primary_image_tag.as_deref())?;
        Some(format!(
            "{}/Items/{}/Images/Primary?maxHeight={}&quality=90&tag={}",
            server_url, self.id, max_height, tag
        ))
    }

    /// Get backdrop image URL for this item (first backdrop).
    pub fn backdrop_image_url(&self, server_url: &str, max_width: i32) -> Option<String> {
        let tags = self.backdrop_image_tags.as_ref()?;
        let tag = tags.first()?;
        Some(format!(
            "{}/Items/{}/Images/Backdrop/0?maxWidth={}&quality=80&tag={}",
            server_url, self.id, max_width, tag
        ))
    }

    /// Convert `run_time_ticks` to a human-readable duration string (e.g. "2h 15m").
    pub fn runtime_string(&self) -> Option<String> {
        let ticks = self.run_time_ticks?;
        let total_minutes = ticks / 600_000_000;
        let hours = total_minutes / 60;
        let minutes = total_minutes % 60;
        if hours > 0 {
            Some(format!("{}h {}m", hours, minutes))
        } else {
            Some(format!("{}m", minutes))
        }
    }

    /// Get playback progress as a value between 0.0 and 1.0.
    pub fn progress(&self) -> f32 {
        if let (Some(user_data), Some(runtime)) = (&self.user_data, self.run_time_ticks) {
            if runtime > 0 {
                return (user_data.playback_position_ticks as f32) / (runtime as f32);
            }
        }
        0.0
    }
}
