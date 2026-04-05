//! Segment manager for intro/credits skip logic.
//!
//! Tracks which media segments (Intro, Outro, Recap, Preview) the playback
//! position is currently within and manages the skip-button lifecycle so
//! that each segment is only offered once per playback session.

use crate::api::models::MediaSegment;

/// Manages intro/credits skip logic using MediaSegment data from the
/// Jellyfin API.
pub struct SegmentManager {
    segments: Vec<MediaSegment>,
    /// Segment IDs that have already been skipped (or dismissed) this session.
    skipped_segments: Vec<String>,
}

impl SegmentManager {
    /// Create a new, empty segment manager.
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            skipped_segments: Vec::new(),
        }
    }

    /// Load segments for the current playback item.
    ///
    /// This replaces any previously loaded segments and resets the
    /// skipped-segments list for the new session.
    pub fn set_segments(&mut self, segments: Vec<MediaSegment>) {
        self.segments = segments;
        self.skipped_segments.clear();
    }

    /// Clear all segments and reset state (e.g. on playback stop).
    pub fn clear(&mut self) {
        self.segments.clear();
        self.skipped_segments.clear();
    }

    /// Check whether the current playback position falls within a skippable
    /// segment that has not yet been skipped.
    ///
    /// Returns `Some((segment_type, end_ticks))` if a skip button should be
    /// displayed, where `segment_type` is one of `"Intro"`, `"Outro"`,
    /// `"Recap"`, or `"Preview"`, and `end_ticks` is the position to seek to.
    ///
    /// Returns `None` if there is no active skippable segment at this
    /// position, or if the segment has already been skipped.
    pub fn check_position(&self, position_ticks: i64) -> Option<(&str, i64)> {
        for segment in &self.segments {
            // Skip segments that were already acted on.
            if self.skipped_segments.contains(&segment.id) {
                continue;
            }

            if position_ticks >= segment.start_ticks && position_ticks <= segment.end_ticks {
                return Some((&segment.segment_type, segment.end_ticks));
            }
        }
        None
    }

    /// Mark a segment as skipped so the skip button will not reappear if
    /// the user seeks back into the same segment.
    pub fn mark_skipped(&mut self, segment_id: &str) {
        if !self.skipped_segments.iter().any(|id| id == segment_id) {
            self.skipped_segments.push(segment_id.to_owned());
        }
    }

    /// Get the skip-to position (end_ticks) for the segment that the
    /// playback position is currently inside.
    ///
    /// This is a convenience wrapper around [`check_position`] that returns
    /// only the seek target.
    pub fn get_skip_target(&self, position_ticks: i64) -> Option<i64> {
        self.check_position(position_ticks).map(|(_, end)| end)
    }

    /// Return a human-readable skip-button label for the segment at the
    /// current position, e.g. "Skip Intro", "Skip Credits".
    ///
    /// Returns `None` when no skippable segment is active.
    pub fn skip_label(&self, position_ticks: i64) -> Option<String> {
        self.check_position(position_ticks).map(|(seg_type, _)| {
            match seg_type {
                "Intro" => "Skip Intro".to_owned(),
                "Outro" => "Skip Credits".to_owned(),
                "Recap" => "Skip Recap".to_owned(),
                "Preview" => "Skip Preview".to_owned(),
                other => format!("Skip {other}"),
            }
        })
    }

    /// Find the segment ID for the segment at the current position.
    ///
    /// Useful for calling [`mark_skipped`] after the user presses the skip
    /// button.
    pub fn active_segment_id(&self, position_ticks: i64) -> Option<&str> {
        for segment in &self.segments {
            if self.skipped_segments.contains(&segment.id) {
                continue;
            }
            if position_ticks >= segment.start_ticks && position_ticks <= segment.end_ticks {
                return Some(&segment.id);
            }
        }
        None
    }
}

impl Default for SegmentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(id: &str, seg_type: &str, start: i64, end: i64) -> MediaSegment {
        MediaSegment {
            id: id.to_owned(),
            item_id: "test-item".to_owned(),
            segment_type: seg_type.to_owned(),
            start_ticks: start,
            end_ticks: end,
        }
    }

    #[test]
    fn test_no_segments() {
        let mgr = SegmentManager::new();
        assert!(mgr.check_position(1000).is_none());
        assert!(mgr.get_skip_target(1000).is_none());
    }

    #[test]
    fn test_within_intro() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000), // 0-30s in ticks
        ]);

        // Inside the intro
        let result = mgr.check_position(150_000_000);
        assert!(result.is_some());
        let (seg_type, end) = result.unwrap();
        assert_eq!(seg_type, "Intro");
        assert_eq!(end, 300_000_000);

        // Skip label
        assert_eq!(mgr.skip_label(150_000_000).unwrap(), "Skip Intro");

        // Skip target
        assert_eq!(mgr.get_skip_target(150_000_000).unwrap(), 300_000_000);
    }

    #[test]
    fn test_outside_segment() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000),
        ]);

        // Past the intro
        assert!(mgr.check_position(400_000_000).is_none());
    }

    #[test]
    fn test_mark_skipped() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000),
        ]);

        // Verify it shows initially
        assert!(mgr.check_position(100_000_000).is_some());

        // Mark as skipped
        mgr.mark_skipped("seg1");

        // Should no longer show
        assert!(mgr.check_position(100_000_000).is_none());
    }

    #[test]
    fn test_multiple_segments() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("intro", "Intro", 0, 300_000_000),
            make_segment("recap", "Recap", 300_000_000, 600_000_000),
            make_segment("outro", "Outro", 25_000_000_000, 26_000_000_000),
        ]);

        // In recap zone
        let (seg_type, _) = mgr.check_position(450_000_000).unwrap();
        assert_eq!(seg_type, "Recap");
        assert_eq!(mgr.skip_label(450_000_000).unwrap(), "Skip Recap");

        // In outro zone
        let (seg_type, _) = mgr.check_position(25_500_000_000).unwrap();
        assert_eq!(seg_type, "Outro");
        assert_eq!(mgr.skip_label(25_500_000_000).unwrap(), "Skip Credits");
    }

    #[test]
    fn test_clear_resets_skipped() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000),
        ]);

        mgr.mark_skipped("seg1");
        assert!(mgr.check_position(100_000_000).is_none());

        // Clear and reload same segments (simulates new playback session)
        mgr.clear();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000),
        ]);

        // Should show again
        assert!(mgr.check_position(100_000_000).is_some());
    }

    #[test]
    fn test_set_segments_resets_skipped() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000),
        ]);

        mgr.mark_skipped("seg1");

        // Loading new segments should clear the skipped list
        mgr.set_segments(vec![
            make_segment("seg2", "Intro", 0, 250_000_000),
        ]);

        assert!(mgr.check_position(100_000_000).is_some());
    }

    #[test]
    fn test_active_segment_id() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("intro1", "Intro", 0, 300_000_000),
            make_segment("outro1", "Outro", 25_000_000_000, 26_000_000_000),
        ]);

        assert_eq!(mgr.active_segment_id(100_000_000), Some("intro1"));
        assert_eq!(mgr.active_segment_id(25_500_000_000), Some("outro1"));
        assert_eq!(mgr.active_segment_id(1_000_000_000), None);
    }

    #[test]
    fn test_boundary_values() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 100, 200),
        ]);

        // Exactly at start
        assert!(mgr.check_position(100).is_some());
        // Exactly at end
        assert!(mgr.check_position(200).is_some());
        // Just before start
        assert!(mgr.check_position(99).is_none());
        // Just after end
        assert!(mgr.check_position(201).is_none());
    }

    #[test]
    fn test_duplicate_skip_idempotent() {
        let mut mgr = SegmentManager::new();
        mgr.set_segments(vec![
            make_segment("seg1", "Intro", 0, 300_000_000),
        ]);

        mgr.mark_skipped("seg1");
        mgr.mark_skipped("seg1");
        mgr.mark_skipped("seg1");

        // Should only have one entry, not three
        assert_eq!(mgr.skipped_segments.len(), 1);
    }
}
