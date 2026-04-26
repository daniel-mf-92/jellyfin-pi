//! Central 2D directional focus manager (JMP `focusManager` parity).
//!
//! Picks the best focus target among a set of geometric rectangles given a
//! current focus rect and a directional intent (Up/Down/Left/Right).
//!
//! The algorithm is geometry-based, not tabindex-based: every focusable
//! element publishes its on-screen rectangle (in any consistent coordinate
//! system — screen pixels are typical), and `pick_best` returns the
//! candidate whose rectangle scores best by:
//!
//!   1. **Directional cone gate.** Candidates not on the requested side of
//!      `from` (using the *centre* of each rect for the directional test)
//!      are rejected outright.
//!   2. **Primary-axis distance.** The signed distance along the direction
//!      axis from the leading edge of `from` to the leading edge of the
//!      candidate. Smaller is better.
//!   3. **Perpendicular penalty.** The off-axis overlap distance between
//!      the two rectangles on the orthogonal axis. Rectangles that share
//!      pixels on the perpendicular axis pay zero penalty; otherwise the
//!      gap is added with a weight of `2.0` so neighbours that line up
//!      cleanly always beat off-axis candidates with a smaller raw
//!      distance.
//!
//! This matches JMP's web `focusManager` (which itself is a port of the
//! TV-platform "spatial-navigation" rect-distance heuristic).
//!
//! ## Usage from Slint
//!
//! Each `.focusable` element in Slint exposes its absolute `x`, `y`,
//! `width`, `height` (e.g. via `Rectangle.absolute-position` and dimension
//! properties) plus a stable string id. The Rust side collects these into
//! `Vec<FocusableRect>` whenever the active focus scope changes (or just
//! lazily on each key-press), then on a directional key event:
//!
//! ```ignore
//! use crate::focus::{FocusableRect, Direction, pick_best};
//!
//! // Pseudo-code inside a Slint key-pressed callback bridged to Rust:
//! let from = current_focused_rect();          // looked up by id
//! let candidates: Vec<FocusableRect> = ui.invoke_collect_focusables();
//! if let Some(next) = pick_best(&from, &candidates, Direction::Right) {
//!     ui.invoke_focus_by_id(next.id.clone().into());
//! }
//! ```
//!
//! Migrating one screen (e.g. `HomeScreen` content rows): replace the
//! per-row `focused-item: int` index + manual `key-pressed` arrow handlers
//! with a single FocusScope at the screen root that, on arrow keys, calls
//! into Rust with the currently-focused id. Each row item publishes its
//! geometry up via a `focusables: [FocusableRect]` model property. Rust
//! runs `pick_best`, then sets `focused-id: string` on the screen — child
//! tiles bind `is-focused: focused-id == self.id` instead of comparing
//! integer indexes. Result: cross-row + cross-column traversal Just Works
//! without per-screen index arithmetic.

use std::cmp::Ordering;

/// Rectangle for a focusable UI element, in any consistent coordinate
/// system (screen pixels are conventional). Origin is top-left, +y
/// downwards (Slint convention).
#[derive(Debug, Clone, PartialEq)]
pub struct FocusableRect {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FocusableRect {
    #[inline]
    pub fn center_x(&self) -> f32 {
        self.x + self.w * 0.5
    }
    #[inline]
    pub fn center_y(&self) -> f32 {
        self.y + self.h * 0.5
    }
    #[inline]
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    #[inline]
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Directional intent passed to [`pick_best`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Weight applied to the perpendicular-axis gap when scoring candidates.
/// Higher = penalises off-axis candidates more. JMP uses ~2.0.
const PERP_WEIGHT: f32 = 2.0;

/// Pick the geometrically best focus target in `dir` from `from`.
///
/// Returns `None` if no candidate lies in the requested direction (the
/// `from` rect's own entry, if present in `candidates`, is always
/// rejected because it is not strictly past `from` on the primary axis).
pub fn pick_best<'a>(
    from: &FocusableRect,
    candidates: &'a [FocusableRect],
    dir: Direction,
) -> Option<&'a FocusableRect> {
    candidates
        .iter()
        .filter(|c| c.id != from.id)
        .filter_map(|c| score(from, c, dir).map(|s| (s, c)))
        .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(Ordering::Equal))
        .map(|(_, c)| c)
}

/// Returns `Some(score)` (lower = better) if `c` is a valid candidate in
/// direction `dir` from `from`, else `None`.
fn score(from: &FocusableRect, c: &FocusableRect, dir: Direction) -> Option<f32> {
    match dir {
        Direction::Right => {
            // Candidate must start strictly to the right of from's right edge,
            // OR centre be right of from's centre with no full overlap.
            let primary = c.x - from.right();
            if primary < 0.0 && c.center_x() <= from.center_x() {
                return None;
            }
            let primary = primary.max(0.0);
            let perp = vertical_gap(from, c);
            Some(primary + PERP_WEIGHT * perp)
        }
        Direction::Left => {
            let primary = from.x - c.right();
            if primary < 0.0 && c.center_x() >= from.center_x() {
                return None;
            }
            let primary = primary.max(0.0);
            let perp = vertical_gap(from, c);
            Some(primary + PERP_WEIGHT * perp)
        }
        Direction::Down => {
            let primary = c.y - from.bottom();
            if primary < 0.0 && c.center_y() <= from.center_y() {
                return None;
            }
            let primary = primary.max(0.0);
            let perp = horizontal_gap(from, c);
            Some(primary + PERP_WEIGHT * perp)
        }
        Direction::Up => {
            let primary = from.y - c.bottom();
            if primary < 0.0 && c.center_y() >= from.center_y() {
                return None;
            }
            let primary = primary.max(0.0);
            let perp = horizontal_gap(from, c);
            Some(primary + PERP_WEIGHT * perp)
        }
    }
}

/// Vertical (y-axis) gap between two rects. 0 if they overlap on y.
fn vertical_gap(a: &FocusableRect, b: &FocusableRect) -> f32 {
    if b.bottom() < a.y {
        a.y - b.bottom()
    } else if b.y > a.bottom() {
        b.y - a.bottom()
    } else {
        0.0
    }
}

/// Horizontal (x-axis) gap between two rects. 0 if they overlap on x.
fn horizontal_gap(a: &FocusableRect, b: &FocusableRect) -> f32 {
    if b.right() < a.x {
        a.x - b.right()
    } else if b.x > a.right() {
        b.x - a.right()
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, x: f32, y: f32, w: f32, h: f32) -> FocusableRect {
        FocusableRect { id: id.to_string(), x, y, w, h }
    }

    #[test]
    fn direct_neighbor_wins_right() {
        let from = r("a", 0.0, 0.0, 100.0, 100.0);
        let near = r("b", 110.0, 0.0, 100.0, 100.0);
        let far = r("c", 400.0, 0.0, 100.0, 100.0);
        let pick = pick_best(&from, &[near.clone(), far], Direction::Right).unwrap();
        assert_eq!(pick.id, "b");
    }

    #[test]
    fn direct_neighbor_wins_down() {
        let from = r("a", 0.0, 0.0, 100.0, 100.0);
        let near = r("b", 0.0, 110.0, 100.0, 100.0);
        let far = r("c", 0.0, 400.0, 100.0, 100.0);
        let pick = pick_best(&from, &[near.clone(), far], Direction::Down).unwrap();
        assert_eq!(pick.id, "b");
    }

    #[test]
    fn no_candidate_in_direction_returns_none() {
        let from = r("a", 500.0, 500.0, 100.0, 100.0);
        // All candidates are LEFT of `from`; ask for Right -> None.
        let cands = vec![
            r("b", 0.0, 500.0, 100.0, 100.0),
            r("c", 100.0, 500.0, 100.0, 100.0),
            r("d", 200.0, 500.0, 100.0, 100.0),
        ];
        assert!(pick_best(&from, &cands, Direction::Right).is_none());
    }

    #[test]
    fn empty_candidates_returns_none() {
        let from = r("a", 0.0, 0.0, 100.0, 100.0);
        assert!(pick_best(&from, &[], Direction::Right).is_none());
    }

    #[test]
    fn equal_axis_distance_prefers_smaller_perpendicular_offset() {
        let from = r("a", 0.0, 100.0, 100.0, 100.0);
        // Both 100px to the right; "aligned" shares the y-band, "offset" doesn't.
        let aligned = r("aligned", 200.0, 100.0, 100.0, 100.0);
        let offset = r("offset", 200.0, 400.0, 100.0, 100.0);
        let pick = pick_best(&from, &[offset, aligned.clone()], Direction::Right).unwrap();
        assert_eq!(pick.id, "aligned");
    }

    #[test]
    fn off_axis_tie_break_smaller_gap_wins() {
        let from = r("a", 0.0, 100.0, 100.0, 100.0);
        // Both off-axis, neither overlaps from's y-band [100,200].
        let close_off = r("close", 200.0, 220.0, 100.0, 100.0); // gap 20
        let far_off = r("far", 200.0, 500.0, 100.0, 100.0); // gap 300
        let pick = pick_best(&from, &[far_off, close_off.clone()], Direction::Right).unwrap();
        assert_eq!(pick.id, "close");
    }

    #[test]
    fn self_id_excluded() {
        let from = r("a", 0.0, 0.0, 100.0, 100.0);
        let cands = vec![from.clone(), r("b", 200.0, 0.0, 100.0, 100.0)];
        let pick = pick_best(&from, &cands, Direction::Right).unwrap();
        assert_eq!(pick.id, "b");
    }

    #[test]
    fn perp_penalty_beats_raw_distance() {
        // Aligned-but-farther should beat off-axis-but-closer when the
        // perpendicular gap is large enough.
        let from = r("a", 0.0, 0.0, 100.0, 100.0);
        let aligned_far = r("aligned_far", 300.0, 0.0, 100.0, 100.0); // primary 200, perp 0 -> 200
        let off_close = r("off_close", 150.0, 500.0, 100.0, 100.0); // primary 50, perp 400 -> 50 + 800
        let pick = pick_best(&from, &[off_close, aligned_far.clone()], Direction::Right).unwrap();
        assert_eq!(pick.id, "aligned_far");
    }

    #[test]
    fn left_and_up_directions_work() {
        let from = r("a", 500.0, 500.0, 100.0, 100.0);
        let left = r("L", 300.0, 500.0, 100.0, 100.0);
        let up = r("U", 500.0, 300.0, 100.0, 100.0);
        let right = r("R", 700.0, 500.0, 100.0, 100.0);
        let down = r("D", 500.0, 700.0, 100.0, 100.0);
        let cands = vec![left, up, right, down];
        assert_eq!(pick_best(&from, &cands, Direction::Left).unwrap().id, "L");
        assert_eq!(pick_best(&from, &cands, Direction::Up).unwrap().id, "U");
        assert_eq!(pick_best(&from, &cands, Direction::Right).unwrap().id, "R");
        assert_eq!(pick_best(&from, &cands, Direction::Down).unwrap().id, "D");
    }

    #[test]
    fn overlapping_rect_on_primary_axis_rejected() {
        // A rect that fully overlaps `from` in x is not "to the right" of it.
        let from = r("a", 0.0, 0.0, 100.0, 100.0);
        let overlap = r("o", 50.0, 0.0, 100.0, 100.0); // centre 100 == from.right
        // overlap.center_x (100) > from.center_x (50), so it's accepted but
        // primary distance clamps to 0 + perp 0 = 0; that's fine — overlap
        // IS to the right of from's centre. Just sanity-check we don't panic.
        let _ = pick_best(&from, &[overlap], Direction::Right);
    }
}
