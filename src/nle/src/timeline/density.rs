//! The event density of the clips on the timeline.
//!
//! Each clip is a row of tiles of `TILE_WIDTH` pixels, and only the tiles in
//! the window and in `OVERSCAN` pixels on each side of it render. Each tile
//! draws the count of events in each pixel column as one SVG path. Never one
//! DOM node for each event or for each column: the measurements of
//! `specs/2026-11-26-nle-timeline-research.md` show that such nodes miss
//! frames of a zoom.

use std::fmt::Write as _;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use crate::types::{Clip, Recording, Seconds};

/// The pixel columns of one tile.
const TILE_COLUMNS: usize = 512;
/// The width of one tile, in pixels: one pixel for each column.
pub const TILE_WIDTH: f64 = TILE_COLUMNS as f64;
/// The pixels on each side of the window where the tiles also render, so a
/// short scroll shows no empty tile.
pub const OVERSCAN: f64 = 600.0;
/// The count of events in one column that fills the height of a tile.
const FULL_COUNT: f64 = 64.0;

/// The times of the events that one clip holds, from the start of the clip,
/// in order, and the length of the clip in seconds.
///
/// Two values are equal only when they share one list of times, so a
/// comparison costs no walk of the events. A tile thus renders again only
/// when its clip gets a new list.
#[derive(Debug, Clone)]
pub struct ClipEvents {
    times: Rc<[f64]>,
    length: f64,
}

impl PartialEq for ClipEvents {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.times, &other.times) && self.length.to_bits() == other.length.to_bits()
    }
}

impl ClipEvents {
    /// The events of `clip`: the time of each event that `Clip::holds`
    /// accepts, less the in point, in order.
    #[must_use]
    pub fn of(clip: &Clip) -> Self {
        let in_point = clip.in_point.get();
        let mut times: Vec<f64> = clip
            .source
            .events
            .iter()
            .filter(|event| clip.holds(event.time))
            .map(|event| event.time - in_point)
            .collect();
        // A recording holds its events in order, and the sort costs one pass
        // over a list that is in order already.
        times.sort_by(f64::total_cmp);
        Self {
            times: Rc::from(times),
            length: clip.out_point.get() - in_point,
        }
    }

    /// The length of the clip in seconds.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.length
    }

    /// The times of the events, from the start of the clip, in order.
    #[must_use]
    pub fn times(&self) -> &[f64] {
        &self.times
    }
}

/// The key of one entry of an `EventCache`: the source and the range of a
/// clip. The place of the clip on the timeline is not part of the key, so a
/// move of a clip keeps its events.
#[derive(Debug)]
struct CacheKey {
    source: Arc<Recording>,
    in_point: Seconds,
    out_point: Seconds,
}

impl CacheKey {
    /// The key of `clip`.
    fn of(clip: &Clip) -> Self {
        Self {
            source: Arc::clone(&clip.source),
            in_point: clip.in_point,
            out_point: clip.out_point,
        }
    }

    /// True when `self` and `other` name one source and one range.
    fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.in_point == other.in_point
            && self.out_point == other.out_point
    }
}

/// The events of the clips of the last call of `EventCache::events`.
#[derive(Debug, Default)]
pub struct EventCache {
    entries: Vec<(CacheKey, ClipEvents)>,
}

impl EventCache {
    /// The events of each clip of `clips`, in the same order.
    ///
    /// A clip with the source and the range of a clip of the last call gets
    /// the same list as that call gave, so its tiles do not render again. The
    /// cache then keeps only the clips of this call.
    pub fn events(&mut self, clips: &[Clip]) -> Vec<ClipEvents> {
        let mut entries = Vec::with_capacity(clips.len());
        let lists = clips
            .iter()
            .map(|clip| {
                let key = CacheKey::of(clip);
                let events = self
                    .entries
                    .iter()
                    .find(|(old, _)| old.matches(&key))
                    .map_or_else(|| ClipEvents::of(clip), |(_, events)| events.clone());
                entries.push((key, events.clone()));
                events
            })
            .collect();
        self.entries = entries;
        lists
    }
}

/// The tiles of a clip that render: the tiles in the window, or in `OVERSCAN`
/// pixels on each side of it. The clip starts at `clip_x` pixels from the
/// left edge of a window of `window_width` pixels, and it is `clip_width`
/// pixels wide.
#[must_use]
pub fn visible_tiles(clip_x: f64, clip_width: f64, window_width: f64) -> Range<usize> {
    let count = to_index((clip_width / TILE_WIDTH).ceil());
    let first = to_index(((-OVERSCAN - clip_x) / TILE_WIDTH).floor());
    let end = to_index(((window_width + OVERSCAN - clip_x) / TILE_WIDTH).ceil()).min(count);
    first.min(end)..end
}

/// The width in pixels of the tile at `index` of a clip of `clip_width`
/// pixels. Each tile is `TILE_WIDTH` wide, but the last tile can be narrower.
#[must_use]
pub fn tile_width(clip_width: f64, index: usize) -> f64 {
    (clip_width - index as f64 * TILE_WIDTH).clamp(0.0, TILE_WIDTH)
}

/// The SVG path of the event density of the tile at `index` of the clip of
/// `events`, at `pixels_per_second`, in a tile `height` pixels high.
///
/// The path has one vertical line for each pixel column that holds an event,
/// up from the bottom edge. Its height grows with the logarithm of the count
/// of events in the column, and 64 events or more fill the tile. An event at
/// the end of the clip counts in the last column. The answer is empty for a
/// tile with no event, for a tile past the end of the clip, and for a zoom
/// that is not finite and above zero.
#[must_use]
pub fn density_path(
    events: &ClipEvents,
    index: usize,
    pixels_per_second: f64,
    height: f64,
) -> String {
    let mut path = String::new();
    if !pixels_per_second.is_finite() || pixels_per_second <= 0.0 {
        return path;
    }
    let columns = to_index((events.length * pixels_per_second).ceil()).max(1);
    let Some(first) = index.checked_mul(TILE_COLUMNS) else {
        return path;
    };
    if first >= columns {
        return path;
    }
    let end = first.saturating_add(TILE_COLUMNS).min(columns);
    let column_of = |time: f64| to_index((time * pixels_per_second).floor()).min(columns - 1);
    // The columns grow with the times, so a binary search finds the first
    // event of the tile, and one pass counts the events up to its end.
    let start = events
        .times
        .partition_point(|&time| column_of(time) < first);
    let mut counts = [0_u32; TILE_COLUMNS];
    for &time in events.times.get(start..).unwrap_or_default() {
        let column = column_of(time);
        if column >= end {
            break;
        }
        if let Some(count) = column
            .checked_sub(first)
            .and_then(|offset| counts.get_mut(offset))
        {
            *count = count.saturating_add(1);
        }
    }
    let full = (1.0 + FULL_COUNT).ln();
    for (column, &count) in counts.iter().enumerate().filter(|(_, count)| **count > 0) {
        let share = ((1.0 + f64::from(count)).ln() / full).min(1.0);
        let bar = (height * share * 10.0).round() / 10.0;
        // A write to a `String` cannot fail.
        let _ = write!(path, "M{column}.5 {height}v-{bar}");
    }
    path
}

/// `value` as an index: zero for NaN and for a value that is not above zero,
/// and `usize::MAX` for a value above that.
fn to_index(value: f64) -> usize {
    if value.is_nan() || value <= 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is above zero, and the cast saturates at usize::MAX"
    )]
    let index = value as usize;
    index
}

#[cfg(test)]
mod tests;
