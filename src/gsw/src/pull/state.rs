//! The state machine of the pull-request indicator.
//!
//! [`PullWatch`] decides when a lookup starts and what the indicator shows. It
//! reads no clock, because every instant is a parameter. It starts no thread
//! and runs no child. The watch loop runs the lookups that it asks for.

use std::collections::HashMap;
use std::time::Instant;

use ghrest::{EtagEntry, PullNumber};

use super::{from_numbers, LookupOutcome, PullIndicator, PullTarget};

/// One lookup that the loop must start: the target, and the cache entry to send its `ETag`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LookupRequest {
    pub(crate) target: PullTarget,
    pub(crate) cached: Option<EtagEntry>,
}

/// What the cache keeps for one target.
struct Entry {
    /// The last answer. `None` means that no answer is known.
    numbers: Option<Vec<PullNumber>>,
    /// The entry to send as `If-None-Match` with the next request.
    etag: Option<EtagEntry>,
}

/// The state machine. See the module documentation.
pub(crate) struct PullWatch {
    active: bool,
    entries: HashMap<PullTarget, Entry>,
    last_target: Option<PullTarget>,
    in_flight: bool,
    pending: bool,
    blocked_until: Option<Instant>,
}

impl PullWatch {
    /// A watch that looks up. The caller builds it only when the indicator applies
    /// (watch mode, no `--no-pr`, `gh` on PATH).
    pub(crate) fn enabled() -> Self {
        Self::with_active(true)
    }

    /// A watch that never looks up and never shows an indicator.
    pub(crate) fn disabled() -> Self {
        Self::with_active(false)
    }

    fn with_active(active: bool) -> Self {
        Self {
            active,
            entries: HashMap::new(),
            last_target: None,
            in_flight: false,
            pending: false,
            blocked_until: None,
        }
    }

    /// Tells the watch which target the frame shows now (`None`: no target, for
    /// example a detached HEAD or a remote that is not GitHub), and whether
    /// this wake is a timed refresh or `r` (`force`). Returns the lookup to start, if any.
    ///
    /// A lookup is due when the target differs from the target of the last call
    /// (the first call with `Some` counts as a change), when `force` is set, or when
    /// a pending lookup waited for a rate-limit block that has ended at `now`.
    /// A due lookup with a lookup in flight, or inside a rate-limit block, only
    /// marks one lookup as pending. A disabled watch, and a `None` target, start nothing.
    pub(crate) fn refresh(
        &mut self,
        target: Option<&PullTarget>,
        force: bool,
        now: Instant,
    ) -> Option<LookupRequest> {
        if !self.active {
            return None;
        }
        let changed = self.last_target.as_ref() != target;
        self.last_target = target.cloned();
        self.end_block_if_over(now);
        let ready = self.pending && !self.in_flight && self.blocked_until.is_none();
        if changed || force || ready {
            self.start(now)
        } else {
            None
        }
    }

    /// The lookup of `target` ended with `outcome` at `now`. Puts the outcome in the
    /// cache of `target`, whatever target the frame shows now, and returns the pending
    /// lookup to start, if any, for the target of the last `refresh`.
    pub(crate) fn answered(
        &mut self,
        target: PullTarget,
        outcome: LookupOutcome,
        now: Instant,
    ) -> Option<LookupRequest> {
        self.in_flight = false;
        match outcome {
            LookupOutcome::Answered { numbers, entry } => {
                self.entries.insert(
                    target,
                    Entry {
                        numbers: Some(numbers),
                        etag: entry,
                    },
                );
            }
            LookupOutcome::Failed(_) => self.forget_answer(target),
            LookupOutcome::RateLimited(wait) => {
                self.forget_answer(target);
                self.blocked_until = Some(now + wait);
            }
            LookupOutcome::NotInstalled => self.active = false,
        }
        if !self.active || !self.pending {
            return None;
        }
        self.pending = false;
        self.end_block_if_over(now);
        self.start(now)
    }

    /// What the header shows for `target` at `now`. `None` when the watch is disabled
    /// or `gh` went missing. `Unknown` inside a rate-limit block, for a target with no
    /// answer, and after a failed lookup. Otherwise `from_numbers` of the last answer.
    pub(crate) fn indicator(&self, target: &PullTarget, now: Instant) -> Option<PullIndicator> {
        if !self.active {
            return None;
        }
        let blocked = self.blocked_until.is_some_and(|until| now < until);
        let numbers = self
            .entries
            .get(target)
            .and_then(|entry| entry.numbers.as_deref());
        Some(match numbers {
            Some(numbers) if !blocked => from_numbers(numbers),
            _ => PullIndicator::Unknown,
        })
    }

    /// The instant the loop must wake to send a lookup that waits for the end of a
    /// rate-limit block. `None` when no lookup waits.
    pub(crate) fn wake_at(&self) -> Option<Instant> {
        if self.active && self.pending && !self.in_flight {
            self.blocked_until
        } else {
            None
        }
    }

    /// Makes the answer of `target` unknown. Its `ETag` entry stays, so a later `304` still works.
    fn forget_answer(&mut self, target: PullTarget) {
        self.entries
            .entry(target)
            .and_modify(|entry| entry.numbers = None)
            .or_insert(Entry {
                numbers: None,
                etag: None,
            });
    }

    /// Clears the block when its end is not after `now`.
    fn end_block_if_over(&mut self, now: Instant) {
        if self.blocked_until.is_some_and(|until| until <= now) {
            self.blocked_until = None;
        }
    }

    /// Starts a lookup for the target of the last `refresh`. A lookup in flight or a
    /// block makes it pending instead. Nothing starts when no target is on the screen.
    fn start(&mut self, now: Instant) -> Option<LookupRequest> {
        let target = self.last_target.clone()?;
        if self.in_flight || self.blocked_until.is_some_and(|until| now < until) {
            self.pending = true;
            return None;
        }
        self.pending = false;
        self.in_flight = true;
        let cached = self
            .entries
            .get(&target)
            .and_then(|entry| entry.etag.clone());
        Some(LookupRequest { target, cached })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::pull::lookup::pulls_path;
    use ghrest::{parse_response, GithubRepo};

    fn target(branch: &str) -> PullTarget {
        PullTarget::new(
            GithubRepo::from_remote_url("git@github.com:owner/name.git", None).unwrap(),
            branch,
        )
    }

    fn numbers(raw: &[u64]) -> Vec<PullNumber> {
        raw.iter().map(|&n| PullNumber::new(n)).collect()
    }

    fn etag(target: &PullTarget, tag: &str) -> EtagEntry {
        let text = format!("HTTP/2.0 200 OK\nEtag: \"{tag}\"\r\n\r\n[]");
        EtagEntry::from_response(&pulls_path(target), parse_response(&text).unwrap()).unwrap()
    }

    fn answer(raw: &[u64], entry: Option<EtagEntry>) -> LookupOutcome {
        LookupOutcome::Answered {
            numbers: numbers(raw),
            entry,
        }
    }

    fn open(lowest: u64, others: usize) -> PullIndicator {
        PullIndicator::Open {
            lowest: PullNumber::new(lowest),
            others,
        }
    }

    #[test]
    fn the_first_refresh_starts_a_lookup_and_a_walk_after_it_starts_none() {
        let now = Instant::now();
        let a = target("a");
        let mut watch = PullWatch::enabled();
        let request = watch.refresh(Some(&a), false, now);
        assert_eq!(
            request,
            Some(LookupRequest {
                target: a.clone(),
                cached: None
            })
        );
        assert_eq!(watch.indicator(&a, now), Some(PullIndicator::Unknown));
        watch.answered(a.clone(), answer(&[], None), now);
        assert_eq!(watch.refresh(Some(&a), false, now), None);
    }

    #[test]
    fn two_triggers_during_a_lookup_start_exactly_one_more() {
        let now = Instant::now();
        let a = target("a");
        let mut watch = PullWatch::enabled();
        assert!(watch.refresh(Some(&a), false, now).is_some());
        assert_eq!(watch.refresh(Some(&a), true, now), None);
        assert_eq!(watch.refresh(Some(&a), true, now), None);
        let next = watch.answered(a.clone(), answer(&[], None), now);
        assert_eq!(next.map(|r| r.target), Some(a.clone()));
        assert_eq!(watch.answered(a, answer(&[], None), now), None);
    }

    #[test]
    fn an_answer_gives_the_indicator_and_its_etag_goes_in_the_next_request() {
        let now = Instant::now();
        let a = target("a");
        let mut watch = PullWatch::enabled();
        watch.refresh(Some(&a), false, now);
        let first = etag(&a, "v1");
        watch.answered(a.clone(), answer(&[700, 690], Some(first.clone())), now);
        assert_eq!(watch.indicator(&a, now), Some(open(690, 1)));
        let request = watch.refresh(Some(&a), true, now).expect("a request");
        assert_eq!(request.cached, Some(first));
        let second = etag(&a, "v2");
        watch.answered(a.clone(), answer(&[], Some(second.clone())), now);
        assert_eq!(watch.indicator(&a, now), Some(PullIndicator::NoPull));
        let request = watch.refresh(Some(&a), true, now).expect("a request");
        assert_eq!(request.cached, Some(second));
    }

    #[test]
    fn an_answer_for_a_target_that_left_the_screen_goes_into_the_cache_only() {
        let now = Instant::now();
        let (a, b) = (target("a"), target("b"));
        let mut watch = PullWatch::enabled();
        watch.refresh(Some(&a), false, now);
        assert_eq!(watch.refresh(Some(&b), false, now), None);
        let next = watch.answered(a.clone(), answer(&[100], None), now);
        assert_eq!(next.map(|r| r.target), Some(b.clone()));
        assert_eq!(watch.indicator(&b, now), Some(PullIndicator::Unknown));
        assert_eq!(watch.refresh(Some(&a), false, now), None);
        assert_eq!(watch.indicator(&a, now), Some(open(100, 0)));
        let back = watch.answered(b, answer(&[], None), now);
        assert_eq!(back.map(|r| r.target), Some(a));
    }

    #[test]
    fn a_failed_lookup_shows_unknown_and_keeps_the_etag() {
        let now = Instant::now();
        let a = target("a");
        let mut watch = PullWatch::enabled();
        watch.refresh(Some(&a), false, now);
        let kept = etag(&a, "v1");
        watch.answered(a.clone(), answer(&[5], Some(kept.clone())), now);
        watch.refresh(Some(&a), true, now);
        watch.answered(a.clone(), LookupOutcome::Failed("boom".into()), now);
        assert_eq!(watch.indicator(&a, now), Some(PullIndicator::Unknown));
        let request = watch.refresh(Some(&a), true, now).expect("a request");
        assert_eq!(request.cached, Some(kept));
    }

    #[test]
    fn a_rate_limit_stops_every_lookup_until_its_reset() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let (a, b) = (target("a"), target("b"));
        let mut watch = PullWatch::enabled();
        watch.refresh(Some(&a), false, t0);
        let kept = etag(&a, "v1");
        watch.answered(a.clone(), answer(&[5], Some(kept)), t0);
        watch.refresh(Some(&a), true, t0);
        let next = watch.answered(
            a.clone(),
            LookupOutcome::RateLimited(Duration::from_secs(60)),
            t0,
        );
        assert_eq!(next, None);
        assert_eq!(watch.indicator(&a, at(59)), Some(PullIndicator::Unknown));
        assert_eq!(watch.indicator(&b, at(59)), Some(PullIndicator::Unknown));
        assert_eq!(watch.wake_at(), None);
        assert_eq!(watch.refresh(Some(&a), true, at(59)), None);
        assert_eq!(watch.wake_at(), Some(at(60)));
        assert_eq!(watch.refresh(Some(&a), false, at(59)), None);
        let request = watch.refresh(Some(&a), false, at(60)).expect("a request");
        assert!(request.cached.is_some());
        assert_eq!(watch.wake_at(), None);
        assert_eq!(watch.refresh(Some(&a), true, at(61)), None);
        assert_eq!(watch.indicator(&a, at(61)), Some(PullIndicator::Unknown));
    }

    #[test]
    fn a_missing_gh_turns_the_watch_off_for_good() {
        let now = Instant::now();
        let a = target("a");
        let mut watch = PullWatch::enabled();
        watch.refresh(Some(&a), false, now);
        watch.refresh(Some(&a), true, now);
        assert_eq!(
            watch.answered(a.clone(), LookupOutcome::NotInstalled, now),
            None
        );
        assert_eq!(watch.indicator(&a, now), None);
        assert_eq!(watch.refresh(Some(&a), true, now), None);
        assert_eq!(watch.refresh(Some(&target("b")), true, now), None);
        assert_eq!(watch.wake_at(), None);
    }

    #[test]
    fn a_disabled_watch_and_a_missing_target_start_nothing() {
        let now = Instant::now();
        let a = target("a");
        let mut off = PullWatch::disabled();
        assert_eq!(off.refresh(Some(&a), true, now), None);
        assert_eq!(off.indicator(&a, now), None);
        let mut on = PullWatch::enabled();
        assert_eq!(on.refresh(None, true, now), None);
        assert!(on.refresh(Some(&a), false, now).is_some());
    }

    #[test]
    fn a_target_that_goes_away_and_returns_starts_a_new_lookup() {
        let now = Instant::now();
        let a = target("a");
        let mut watch = PullWatch::enabled();
        watch.refresh(Some(&a), false, now);
        watch.answered(a.clone(), answer(&[], None), now);
        assert_eq!(watch.refresh(None, false, now), None);
        let request = watch.refresh(Some(&a), false, now);
        assert_eq!(request.map(|r| r.target), Some(a));
    }
}
