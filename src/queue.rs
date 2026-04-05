//! Playback queue module for managing sequential playback and auto-advance.
//!
//! This module provides a queue data structure that holds a list of items
//! to play, tracks the current position, and supports advancing to the next
//! item when playback ends. It handles repeat modes (none, one, all) and
//! shuffle toggling.
//!
//! # Integration with main.rs / playback loop
//!
//! When the VLC player emits an EndOfFile (or equivalent end-of-media) event,
//! the integration code should:
//!
//! 1. Call `queue.has_next()` to check if another item is available.
//! 2. If true, call `queue.advance()` to move to the next item and get it.
//! 3. Trigger playback of the next item via `play-item(next_item.item_id)`
//!    through the UI bridge (e.g., sending a command to the webview).
//! 4. If false (no next item), stop playback normally and navigate back.
//!
//! # Integration with series detail page
//!
//! When a user selects an episode from a series/season detail page:
//!
//! 1. Fetch all episodes in the season via `JellyfinClient::get_episodes()`.
//! 2. Convert each `BaseItemDto` to a `QueueItem` using `QueueItem::from()`.
//! 3. Call `queue.set_items(episodes, selected_index)` to populate the queue
//!    starting at the episode the user picked.
//! 4. This enables seamless auto-advance through the rest of the season.
//!
//! # Integration with movie playback
//!
//! For standalone movies, simply enqueue a single item:
//!
//! ```ignore
//! queue.clear();
//! queue.set_items(vec![QueueItem::from(&movie_dto)], 0);
//! ```
//!
//! Since there is only one item and repeat mode defaults to None, playback
//! will stop naturally when the movie ends.

use crate::api::models::BaseItemDto;

/// A single item in the playback queue, containing the minimal data needed
/// to identify and display the item without re-fetching from the server.
#[derive(Debug, Clone)]
pub struct QueueItem {
    /// The Jellyfin item ID, used to start playback via the API.
    pub item_id: String,

    /// Display title (episode name, movie title, etc.).
    pub title: String,

    /// Secondary display text. For episodes this is formatted as
    /// "Series Name - S01E03"; for movies it is the production year.
    pub subtitle: String,

    /// Duration in Jellyfin ticks (10,000,000 ticks = 1 second).
    /// None if the server did not provide a runtime.
    pub runtime_ticks: Option<i64>,

    /// Pre-built primary image URL for thumbnail display in the queue UI.
    /// None if the item has no primary image tag.
    pub image_url: Option<String>,
}

/// Controls what happens when the queue reaches the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepeatMode {
    /// Stop after the last item.
    None,
    /// Repeat the current item indefinitely.
    RepeatOne,
    /// Loop back to the first item after the last one finishes.
    RepeatAll,
}

/// The playback queue: an ordered list of items with a cursor, repeat mode,
/// and shuffle flag.
///
/// This struct is not thread-safe on its own. Wrap it in an
/// `Arc<tokio::sync::RwLock<PlaybackQueue>>` (or similar) if shared across
/// tasks, consistent with how `StateManager` wraps `AppState`.
pub struct PlaybackQueue {
    items: Vec<QueueItem>,
    current_index: Option<usize>,
    repeat_mode: RepeatMode,
    shuffle: bool,
}

impl PlaybackQueue {
    /// Create an empty queue with no repeat and shuffle off.
    pub fn new() -> Self {
        PlaybackQueue {
            items: Vec::new(),
            current_index: None,
            repeat_mode: RepeatMode::None,
            shuffle: false,
        }
    }

    /// Replace the entire queue with a new set of items and jump to
    /// `start_index`. This is the primary method for populating the queue
    /// when a user starts playback from a season episode list.
    ///
    /// If `start_index` is out of bounds it is clamped to the last item,
    /// or `current_index` is set to `None` if the list is empty.
    pub fn set_items(&mut self, items: Vec<QueueItem>, start_index: usize) {
        self.items = items;
        if self.items.is_empty() {
            self.current_index = None;
        } else {
            self.current_index = Some(start_index.min(self.items.len() - 1));
        }
    }

    /// Append an item to the end of the queue. If the queue was empty and
    /// had no current index, the new item becomes current.
    pub fn enqueue(&mut self, item: QueueItem) {
        self.items.push(item);
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
    }

    /// Insert an item immediately after the current item so it plays next.
    /// If no item is current (empty queue), it becomes the first and current item.
    pub fn play_next(&mut self, item: QueueItem) {
        match self.current_index {
            Some(idx) => {
                let insert_pos = idx + 1;
                self.items.insert(insert_pos, item);
            }
            None => {
                self.items.push(item);
                self.current_index = Some(0);
            }
        }
    }

    /// Return a reference to the currently active queue item, if any.
    pub fn current(&self) -> Option<&QueueItem> {
        self.current_index.and_then(|i| self.items.get(i))
    }

    /// Advance to the next item in the queue, respecting the current repeat
    /// mode. Returns a reference to the new current item, or `None` if
    /// playback should stop.
    ///
    /// Behavior by repeat mode:
    /// - `None`: advances to the next index; returns `None` at the end.
    /// - `RepeatOne`: stays on the same item (returns it again).
    /// - `RepeatAll`: wraps around to index 0 after the last item.
    pub fn advance(&mut self) -> Option<&QueueItem> {
        let idx = self.current_index?;

        match self.repeat_mode {
            RepeatMode::RepeatOne => {
                // Stay on the same item.
                self.items.get(idx)
            }
            RepeatMode::RepeatAll => {
                let next = if idx + 1 < self.items.len() {
                    idx + 1
                } else {
                    0 // wrap around
                };
                self.current_index = Some(next);
                self.items.get(next)
            }
            RepeatMode::None => {
                if idx + 1 < self.items.len() {
                    self.current_index = Some(idx + 1);
                    self.items.get(idx + 1)
                } else {
                    // End of queue, no more items.
                    None
                }
            }
        }
    }

    /// Go back to the previous item. Returns `None` if already at the
    /// beginning (index 0) or the queue is empty.
    ///
    /// In RepeatAll mode, wraps from the first item to the last.
    pub fn previous(&mut self) -> Option<&QueueItem> {
        let idx = self.current_index?;

        if idx > 0 {
            self.current_index = Some(idx - 1);
            self.items.get(idx - 1)
        } else if self.repeat_mode == RepeatMode::RepeatAll && !self.items.is_empty() {
            let last = self.items.len() - 1;
            self.current_index = Some(last);
            self.items.get(last)
        } else {
            None
        }
    }

    /// Jump to a specific index in the queue. Returns the item at that
    /// index, or `None` if the index is out of bounds.
    pub fn skip_to(&mut self, index: usize) -> Option<&QueueItem> {
        if index < self.items.len() {
            self.current_index = Some(index);
            self.items.get(index)
        } else {
            None
        }
    }

    /// Remove the item at `index` from the queue. Adjusts `current_index`
    /// so that the currently playing item is not disrupted:
    ///
    /// - If removing before the current item, current_index decrements.
    /// - If removing the current item, current_index stays (now pointing at
    ///   the next item) or becomes None if the queue is now empty.
    /// - If removing after the current item, current_index is unchanged.
    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }

        self.items.remove(index);

        if self.items.is_empty() {
            self.current_index = None;
            return;
        }

        if let Some(current) = self.current_index {
            if index < current {
                // Removed before current: shift current back by one.
                self.current_index = Some(current - 1);
            } else if index == current {
                // Removed the current item. If the index is now past the end,
                // clamp to the last item.
                if current >= self.items.len() {
                    self.current_index = Some(self.items.len() - 1);
                }
                // Otherwise current_index stays, pointing at the item that
                // shifted into this position.
            }
            // If index > current, no adjustment needed.
        }
    }

    /// Move an item from position `from` to position `to`. Adjusts
    /// `current_index` so the currently playing item remains correct.
    pub fn move_item(&mut self, from: usize, to: usize) {
        if from >= self.items.len() || to >= self.items.len() || from == to {
            return;
        }

        let item = self.items.remove(from);
        self.items.insert(to, item);

        // Adjust current_index to follow the currently playing item.
        if let Some(current) = self.current_index {
            if current == from {
                // The current item was the one being moved.
                self.current_index = Some(to);
            } else {
                // The current item was not moved, but its position may have
                // shifted due to the remove+insert.
                let mut adjusted = current;
                if from < current {
                    adjusted -= 1; // remove shifted it left
                }
                if to <= adjusted {
                    adjusted += 1; // insert shifted it right
                }
                self.current_index = Some(adjusted);
            }
        }
    }

    /// Remove all items and reset the cursor.
    pub fn clear(&mut self) {
        self.items.clear();
        self.current_index = None;
    }

    /// Return a slice of all items in the queue, suitable for rendering
    /// a queue list in the UI.
    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    /// Return the current playback position in the queue.
    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    /// Return the total number of items in the queue.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Return true if the queue has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Cycle through repeat modes: None -> RepeatAll -> RepeatOne -> None.
    /// Returns the new mode so the UI can update its icon.
    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat_mode = match self.repeat_mode {
            RepeatMode::None => RepeatMode::RepeatAll,
            RepeatMode::RepeatAll => RepeatMode::RepeatOne,
            RepeatMode::RepeatOne => RepeatMode::None,
        };
        self.repeat_mode
    }

    /// Toggle shuffle on or off. Returns the new shuffle state.
    ///
    /// Note: actual shuffle logic (randomizing order) is left to the caller
    /// since it may involve re-fetching or reordering the items vector.
    /// This flag is stored so the UI can reflect the current state.
    pub fn toggle_shuffle(&mut self) -> bool {
        self.shuffle = !self.shuffle;
        self.shuffle
    }

    /// Return the current repeat mode.
    pub fn repeat_mode(&self) -> RepeatMode {
        self.repeat_mode
    }

    /// Return whether shuffle is enabled.
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// Check if there is a next item available, respecting the repeat mode.
    /// This is useful for the UI to show/hide a "next" button and for the
    /// playback loop to decide whether to auto-advance.
    pub fn has_next(&self) -> bool {
        match self.current_index {
            None => false,
            Some(idx) => match self.repeat_mode {
                RepeatMode::RepeatOne => true,
                RepeatMode::RepeatAll => !self.items.is_empty(),
                RepeatMode::None => idx + 1 < self.items.len(),
            },
        }
    }

    /// Check if there is a previous item available, respecting the repeat mode.
    pub fn has_previous(&self) -> bool {
        match self.current_index {
            None => false,
            Some(idx) => match self.repeat_mode {
                RepeatMode::RepeatOne => true,
                RepeatMode::RepeatAll => !self.items.is_empty(),
                RepeatMode::None => idx > 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion from Jellyfin API types
// ---------------------------------------------------------------------------

impl From<&BaseItemDto> for QueueItem {
    /// Convert a Jellyfin BaseItemDto into a QueueItem.
    ///
    /// Handles two primary item types:
    ///
    /// - **Episode**: subtitle is formatted as "Series Name - S01E03".
    ///   Uses `series_name`, `parent_index_number` (season), and
    ///   `index_number` (episode) from the DTO.
    ///
    /// - **Movie** (or any other type): subtitle is the production year,
    ///   or an empty string if unavailable.
    ///
    /// The `image_url` field is left as `None` here because building
    /// the full URL requires the server base URL, which is not available
    /// in a `From` impl. Callers should populate it separately via
    /// `BaseItemDto::primary_image_url()` or `JellyfinClient::image_url()`.
    fn from(item: &BaseItemDto) -> Self {
        let title = item.name.clone();

        let subtitle = if item.item_type == "Episode" {
            // Build "Series Name - S01E03" format.
            let series = item.series_name.as_deref().unwrap_or("");
            let season_num = item.parent_index_number.unwrap_or(0);
            let episode_num = item.index_number.unwrap_or(0);

            if series.is_empty() {
                format!("S{:02}E{:02}", season_num, episode_num)
            } else {
                format!("{} - S{:02}E{:02}", series, season_num, episode_num)
            }
        } else {
            // Movie or other type: use production year.
            item.production_year
                .map(|y| y.to_string())
                .unwrap_or_default()
        };

        QueueItem {
            item_id: item.id.clone(),
            title,
            subtitle,
            runtime_ticks: item.run_time_ticks,
            image_url: None, // Must be set by caller with server_url context.
        }
    }
}

/// Convenience constructor that also sets the image URL using the server URL.
/// This is the recommended way to create QueueItems from API responses.
///
/// # Example (in integration code)
///
/// ```ignore
/// let episodes = client.get_episodes(series_id, season_id).await?;
/// let queue_items: Vec<QueueItem> = episodes.iter()
///     .map(|ep| QueueItem::from_dto(ep, &client.server_url))
///     .collect();
/// queue.set_items(queue_items, selected_index);
/// ```
impl QueueItem {
    pub fn from_dto(item: &BaseItemDto, server_url: &str) -> Self {
        let mut queue_item = QueueItem::from(item);
        queue_item.image_url = item.primary_image_url(server_url, 200);
        queue_item
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(id: &str, title: &str) -> QueueItem {
        QueueItem {
            item_id: id.to_string(),
            title: title.to_string(),
            subtitle: String::new(),
            runtime_ticks: None,
            image_url: None,
        }
    }

    #[test]
    fn test_new_queue_is_empty() {
        let q = PlaybackQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert!(q.current().is_none());
        assert!(!q.has_next());
        assert!(!q.has_previous());
    }

    #[test]
    fn test_set_items_and_current() {
        let mut q = PlaybackQueue::new();
        let items = vec![
            make_item("1", "Episode 1"),
            make_item("2", "Episode 2"),
            make_item("3", "Episode 3"),
        ];
        q.set_items(items, 1);

        assert_eq!(q.len(), 3);
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(q.current().unwrap().item_id, "2");
    }

    #[test]
    fn test_set_items_clamps_index() {
        let mut q = PlaybackQueue::new();
        let items = vec![make_item("1", "Only One")];
        q.set_items(items, 100);
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn test_advance_no_repeat() {
        let mut q = PlaybackQueue::new();
        let items = vec![
            make_item("1", "Ep 1"),
            make_item("2", "Ep 2"),
            make_item("3", "Ep 3"),
        ];
        q.set_items(items, 0);

        // Advance through all items.
        let next = q.advance().unwrap();
        assert_eq!(next.item_id, "2");

        let next = q.advance().unwrap();
        assert_eq!(next.item_id, "3");

        // Past the end: should return None.
        assert!(q.advance().is_none());
    }

    #[test]
    fn test_advance_repeat_one() {
        let mut q = PlaybackQueue::new();
        q.set_items(vec![make_item("1", "Ep 1"), make_item("2", "Ep 2")], 0);
        q.cycle_repeat(); // None -> RepeatAll
        q.cycle_repeat(); // RepeatAll -> RepeatOne

        let next = q.advance().unwrap();
        assert_eq!(next.item_id, "1"); // stays on same item
        let next = q.advance().unwrap();
        assert_eq!(next.item_id, "1"); // still the same
    }

    #[test]
    fn test_advance_repeat_all() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![make_item("1", "Ep 1"), make_item("2", "Ep 2")],
            0,
        );
        q.cycle_repeat(); // None -> RepeatAll

        q.advance(); // -> Ep 2
        let next = q.advance().unwrap(); // -> wraps to Ep 1
        assert_eq!(next.item_id, "1");
    }

    #[test]
    fn test_previous() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![
                make_item("1", "Ep 1"),
                make_item("2", "Ep 2"),
                make_item("3", "Ep 3"),
            ],
            2,
        );

        let prev = q.previous().unwrap();
        assert_eq!(prev.item_id, "2");

        let prev = q.previous().unwrap();
        assert_eq!(prev.item_id, "1");

        // At the beginning, should return None.
        assert!(q.previous().is_none());
    }

    #[test]
    fn test_previous_repeat_all_wraps() {
        let mut q = PlaybackQueue::new();
        q.set_items(vec![make_item("1", "Ep 1"), make_item("2", "Ep 2")], 0);
        q.cycle_repeat(); // None -> RepeatAll

        let prev = q.previous().unwrap();
        assert_eq!(prev.item_id, "2"); // wraps to last
    }

    #[test]
    fn test_enqueue() {
        let mut q = PlaybackQueue::new();
        assert!(q.current().is_none());

        q.enqueue(make_item("1", "First"));
        assert_eq!(q.current_index(), Some(0));
        assert_eq!(q.current().unwrap().item_id, "1");

        q.enqueue(make_item("2", "Second"));
        assert_eq!(q.len(), 2);
        // Current should still be the first item.
        assert_eq!(q.current().unwrap().item_id, "1");
    }

    #[test]
    fn test_play_next() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![make_item("1", "Ep 1"), make_item("3", "Ep 3")],
            0,
        );

        q.play_next(make_item("2", "Ep 2"));

        assert_eq!(q.len(), 3);
        // "Ep 2" should be at index 1 (right after current index 0).
        assert_eq!(q.items()[1].item_id, "2");
    }

    #[test]
    fn test_skip_to() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![
                make_item("1", "Ep 1"),
                make_item("2", "Ep 2"),
                make_item("3", "Ep 3"),
            ],
            0,
        );

        let item = q.skip_to(2).unwrap();
        assert_eq!(item.item_id, "3");
        assert_eq!(q.current_index(), Some(2));

        // Out of bounds.
        assert!(q.skip_to(10).is_none());
    }

    #[test]
    fn test_remove_before_current() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![
                make_item("1", "Ep 1"),
                make_item("2", "Ep 2"),
                make_item("3", "Ep 3"),
            ],
            2,
        );

        q.remove(0);
        // Current was at 2, after removing index 0 it should be at 1.
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(q.current().unwrap().item_id, "3");
    }

    #[test]
    fn test_remove_current() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![
                make_item("1", "Ep 1"),
                make_item("2", "Ep 2"),
                make_item("3", "Ep 3"),
            ],
            1,
        );

        q.remove(1);
        // After removing index 1 ("Ep 2"), index 1 now holds "Ep 3".
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(q.current().unwrap().item_id, "3");
    }

    #[test]
    fn test_remove_last_item() {
        let mut q = PlaybackQueue::new();
        q.set_items(vec![make_item("1", "Only")], 0);
        q.remove(0);
        assert!(q.is_empty());
        assert!(q.current_index().is_none());
    }

    #[test]
    fn test_move_item() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![
                make_item("1", "Ep 1"),
                make_item("2", "Ep 2"),
                make_item("3", "Ep 3"),
            ],
            0, // current = "Ep 1"
        );

        // Move "Ep 1" from index 0 to index 2.
        q.move_item(0, 2);
        assert_eq!(q.items()[0].item_id, "2");
        assert_eq!(q.items()[1].item_id, "3");
        assert_eq!(q.items()[2].item_id, "1");
        // Current item ("Ep 1") should have followed to index 2.
        assert_eq!(q.current_index(), Some(2));
    }

    #[test]
    fn test_clear() {
        let mut q = PlaybackQueue::new();
        q.set_items(vec![make_item("1", "Ep 1")], 0);
        q.clear();
        assert!(q.is_empty());
        assert!(q.current().is_none());
    }

    #[test]
    fn test_cycle_repeat() {
        let mut q = PlaybackQueue::new();
        assert_eq!(q.repeat_mode(), RepeatMode::None);

        assert_eq!(q.cycle_repeat(), RepeatMode::RepeatAll);
        assert_eq!(q.cycle_repeat(), RepeatMode::RepeatOne);
        assert_eq!(q.cycle_repeat(), RepeatMode::None);
    }

    #[test]
    fn test_toggle_shuffle() {
        let mut q = PlaybackQueue::new();
        assert!(!q.shuffle());

        assert!(q.toggle_shuffle());
        assert!(q.shuffle());

        assert!(!q.toggle_shuffle());
        assert!(!q.shuffle());
    }

    #[test]
    fn test_has_next_and_has_previous() {
        let mut q = PlaybackQueue::new();
        q.set_items(
            vec![make_item("1", "Ep 1"), make_item("2", "Ep 2")],
            0,
        );

        assert!(q.has_next());
        assert!(!q.has_previous()); // at index 0, no previous in None mode

        q.advance();
        assert!(!q.has_next()); // at last item
        assert!(q.has_previous());
    }

    #[test]
    fn test_from_base_item_dto_episode() {
        let mut dto = BaseItemDto::default();
        dto.id = "ep-id-123".to_string();
        dto.name = "The One Where They Play".to_string();
        dto.item_type = "Episode".to_string();
        dto.series_name = Some("Friends".to_string());
        dto.parent_index_number = Some(3);
        dto.index_number = Some(7);
        dto.run_time_ticks = Some(15_000_000_000);

        let qi = QueueItem::from(&dto);

        assert_eq!(qi.item_id, "ep-id-123");
        assert_eq!(qi.title, "The One Where They Play");
        assert_eq!(qi.subtitle, "Friends - S03E07");
        assert_eq!(qi.runtime_ticks, Some(15_000_000_000));
        assert!(qi.image_url.is_none()); // no server_url context
    }

    #[test]
    fn test_from_base_item_dto_movie() {
        let mut dto = BaseItemDto::default();
        dto.id = "movie-456".to_string();
        dto.name = "Inception".to_string();
        dto.item_type = "Movie".to_string();
        dto.production_year = Some(2010);
        dto.run_time_ticks = Some(88_200_000_000);

        let qi = QueueItem::from(&dto);

        assert_eq!(qi.item_id, "movie-456");
        assert_eq!(qi.title, "Inception");
        assert_eq!(qi.subtitle, "2010");
        assert_eq!(qi.runtime_ticks, Some(88_200_000_000));
    }

    #[test]
    fn test_from_base_item_dto_episode_no_series_name() {
        let mut dto = BaseItemDto::default();
        dto.id = "ep-no-series".to_string();
        dto.name = "Pilot".to_string();
        dto.item_type = "Episode".to_string();
        dto.series_name = None;
        dto.parent_index_number = Some(1);
        dto.index_number = Some(1);

        let qi = QueueItem::from(&dto);
        assert_eq!(qi.subtitle, "S01E01");
    }
}
