//! The timeline of the keystroke HUD: it groups the presses of keys into
//! boxes and gives the times of each box.

use super::keys::Key;

/// The number of fade steps for each second of fade.
pub(crate) const FADE_STEPS_PER_SECOND: f64 = 30.0;
/// The most fade steps of one fade, so that a long fade does not make a frame
/// list without end.
pub(crate) const MAX_FADE_STEPS: u32 = 300;
/// The label of the key that closes a box.
const CLOSING_KEY: &str = "Enter";
/// The label of a piece of text made only of spaces.
const SPACE_LABEL: &str = "Space";

/// One press of a key, at a time relative to the start of the region.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Press {
    /// The time of the press, in seconds.
    pub(crate) time: f64,
    /// The key that the user pressed.
    pub(crate) key: Key,
}

/// One piece of the label of a box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece {
    /// Typed text.
    Text(String),
    /// A special key, pressed `count` times in a row.
    Special {
        /// The label of the key.
        label: String,
        /// The number of presses in a row.
        count: u32,
    },
}

/// What the HUD shows at one time: the pieces of the label and the opacity,
/// above 0 and at most 1.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shown {
    /// The pieces of the label.
    pub(crate) pieces: Vec<Piece>,
    /// The opacity of the box.
    pub(crate) alpha: f64,
}

/// The timing of the HUD. `limit` is the length of the region: nothing shows
/// at or after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Timing {
    /// How long a box stays after its last press, in seconds.
    pub(crate) duration: f64,
    /// How long a box takes to fade in and to fade out, in seconds.
    pub(crate) fade: f64,
    /// The length of the region, in seconds.
    pub(crate) limit: f64,
}

/// One box: its presses and its window.
#[derive(Debug, Clone)]
struct Entry {
    /// The presses of the box, in time order.
    presses: Vec<Press>,
    /// The time at which the box starts.
    start: f64,
    /// The time at which the box ends.
    end: f64,
    /// True when the box ends before its natural end.
    cut: bool,
}

/// The boxes of the HUD and their times.
///
/// The rules:
///
/// 1. `new` puts the presses in time order with a stable sort.
/// 2. One entry is one box. A press joins the current entry when the entry is
///    open and the press is before `last + duration`. `last` is the time of
///    the last press of the entry. A press of `Enter` joins (or starts) an
///    entry and then closes it, so the next press starts a new entry. A press
///    at `last` joins the entry also when the entry is closed. One input event
///    gives all of its keys one time, so the keys of a paste such as
///    `ls\rpwd\r` stay in one box. Any other press starts a new entry.
/// 3. An entry starts at its first press. Its natural end is `last +
///    duration`. Its end is the least of the natural end, the start of the
///    next entry, and `limit`. The entry is cut when its end is before its
///    natural end. An entry that starts at or after `limit`, or that has no
///    time, never shows.
/// 4. `at` finds the entry with `start <= time < end`. It shows the presses
///    of the entry at or before `time`. Without a fade the alpha is 1. With a
///    fade, the alpha is the least of 1, the fade-in, and the fade-out. A cut
///    box has no fade-out, so it vanishes. At alpha 0 or below, `at` shows
///    nothing.
/// 5. The pieces come from a fold of the presses. Text joins the text before
///    it. A special key that repeats the last piece adds 1 to its count. Text
///    that holds only spaces becomes a `Space` piece with the number of
///    spaces as its count.
/// 6. `frames` gives the press times, the steps of each fade, and the end of
///    each entry.
pub(crate) struct Timeline {
    entries: Vec<Entry>,
    timing: Timing,
}

impl Timeline {
    /// Make the timeline of `presses`.
    pub(crate) fn new(mut presses: Vec<Press>, timing: Timing) -> Self {
        presses.sort_by(|a, b| a.time.total_cmp(&b.time));
        let mut groups: Vec<Vec<Press>> = Vec::new();
        let mut open = false;
        for press in presses {
            let joins = groups
                .last()
                .and_then(|group| group.last())
                .is_some_and(|last| {
                    press.time == last.time || (open && press.time < last.time + timing.duration)
                });
            let closes = press.key == Key::Special(CLOSING_KEY.to_string());
            if joins {
                if let Some(group) = groups.last_mut() {
                    group.push(press);
                }
            } else {
                groups.push(vec![press]);
            }
            open = !closes;
        }
        let starts: Vec<f64> = groups
            .iter()
            .filter_map(|group| group.first().map(|press| press.time))
            .collect();
        let entries = groups
            .into_iter()
            .enumerate()
            .filter_map(|(index, presses)| {
                let start = presses.first()?.time;
                let last = presses.last()?.time;
                let natural_end = last + timing.duration;
                let next = starts.get(index + 1).copied().unwrap_or(f64::INFINITY);
                let end = natural_end.min(next).min(timing.limit);
                (start < timing.limit && start < end).then_some(Entry {
                    presses,
                    start,
                    end,
                    cut: end < natural_end,
                })
            })
            .collect();
        Self { entries, timing }
    }

    /// What the HUD shows at `time`, or `None`.
    pub(crate) fn at(&self, time: f64) -> Option<Shown> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.start <= time && time < entry.end)?;
        let alpha = self.alpha(entry, time);
        if alpha.is_nan() || alpha <= 0.0 {
            return None;
        }
        let shown: Vec<&Press> = entry
            .presses
            .iter()
            .filter(|press| press.time <= time)
            .collect();
        Some(Shown {
            pieces: fold(&shown),
            alpha,
        })
    }

    /// The opacity of `entry` at `time`.
    fn alpha(&self, entry: &Entry, time: f64) -> f64 {
        let fade = self.timing.fade;
        if fade <= 0.0 {
            return 1.0;
        }
        let fade_in = (time - entry.start) / fade;
        if entry.cut {
            fade_in.min(1.0)
        } else {
            fade_in.min((entry.end - time) / fade).min(1.0)
        }
    }

    /// The times at which what the HUD shows changes, in time order, with no
    /// two equal.
    pub(crate) fn frames(&self) -> Vec<f64> {
        let fade = self.timing.fade;
        let mut frames = Vec::new();
        for entry in &self.entries {
            frames.extend(
                entry
                    .presses
                    .iter()
                    .map(|press| press.time)
                    .filter(|time| *time < entry.end),
            );
            if fade > 0.0 {
                let steps = fade_steps(fade);
                let step = fade / f64::from(steps);
                let mut fades: Vec<f64> = (1..=steps)
                    .map(|k| entry.start + f64::from(k) * step)
                    .collect();
                if !entry.cut {
                    fades.extend((1..steps).map(|k| entry.end - fade + f64::from(k) * step));
                }
                frames.extend(
                    fades
                        .into_iter()
                        .filter(|time| *time > entry.start && *time < entry.end),
                );
            }
            frames.push(entry.end);
        }
        frames.sort_by(f64::total_cmp);
        frames.dedup();
        frames
    }
}

/// Fold presses into the pieces of a label.
fn fold(presses: &[&Press]) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    for press in presses {
        match &press.key {
            Key::Text(text) => {
                if let Some(Piece::Text(last)) = pieces.last_mut() {
                    last.push_str(text);
                } else {
                    pieces.push(Piece::Text(text.clone()));
                }
            }
            Key::Special(label) => match pieces.last_mut() {
                Some(Piece::Special { label: last, count }) if last == label => *count += 1,
                _ => pieces.push(Piece::Special {
                    label: label.clone(),
                    count: 1,
                }),
            },
        }
    }
    pieces
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(text) if !text.is_empty() && text.chars().all(|c| c == ' ') => {
                Piece::Special {
                    label: SPACE_LABEL.to_string(),
                    count: u32::try_from(text.chars().count()).unwrap_or(u32::MAX),
                }
            }
            other => other,
        })
        .collect()
}

/// The number of steps of a fade of `fade` seconds, from 1 to
/// `MAX_FADE_STEPS`.
fn fade_steps(fade: f64) -> u32 {
    let steps = (fade * FADE_STEPS_PER_SECOND)
        .ceil()
        .clamp(1.0, f64::from(MAX_FADE_STEPS));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is a whole number from 1 to MAX_FADE_STEPS"
    )]
    let count = steps as u32;
    count.max(1)
}

#[cfg(test)]
mod tests;
