//! The values that the status item shows, and the rules that change them.
//!
//! A [`View`] holds the samples of the charts and the text of the last error.
//! A [`KernelInput`] (local mode) and a [`RemoteInput`] (remote mode) apply
//! what they get to a view. These rules need no AppKit, so a test can drive
//! them on every platform.

use std::num::NonZeroUsize;

use crate::connection::{Event, HostNaming};
use crate::history::History;
use crate::host::HostName;
use crate::sample::{Reading, Sample};
use crate::text::Origin;

/// The samples of the charts and the text of the last error.
#[derive(Debug)]
pub struct View {
    history: History<Sample>,
    error: Option<String>,
}

impl View {
    /// A view with an empty history of `capacity` samples, and no error.
    #[must_use]
    pub fn new(capacity: NonZeroUsize) -> View {
        View {
            history: History::new(capacity),
            error: None,
        }
    }

    /// The samples that the charts show.
    #[must_use]
    pub fn history(&self) -> &History<Sample> {
        &self.history
    }

    /// The text of the last error, or `None` when the last sample arrived.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn push_sample(&mut self, sample: Sample) {
        self.history.push(sample);
        self.error = None;
    }

    fn fail(&mut self, reason: String) {
        self.error = Some(reason);
    }
}

/// The rules of local mode.
#[derive(Debug, Default)]
pub struct KernelInput {
    previous: Option<Reading>,
}

impl KernelInput {
    /// An input that has seen no reading.
    #[must_use]
    pub fn new() -> KernelInput {
        KernelInput::default()
    }

    /// Applies the result of one read of the kernel to `view`.
    ///
    /// A good reading pushes the sample between it and the last good reading,
    /// and clears the error. The first good reading pushes nothing. A failed
    /// read sets the error and changes nothing else.
    pub fn apply<E: std::fmt::Display>(&mut self, result: Result<Reading, E>, view: &mut View) {
        match result {
            Ok(reading) => {
                if let Some(earlier) = &self.previous {
                    view.history.push(Sample::between(earlier, &reading));
                }
                self.previous = Some(reading);
                view.error = None;
            }
            Err(error) => view.fail(error.to_string()),
        }
    }
}

/// The rules of remote mode.
///
/// The [`HostNaming`] of the input decides the name that [`origin`] gives.
///
/// [`origin`]: RemoteInput::origin
#[derive(Debug)]
pub struct RemoteInput {
    destination: String,
    naming: HostNaming,
    host: Option<HostName>,
}

impl RemoteInput {
    /// An input for the SSH `destination`, as the user typed it.
    ///
    /// With [`HostNaming::Destination`], the name is the destination for the
    /// full life of the input. With [`HostNaming::Reported`], the name is the
    /// host that the newest line reports.
    #[must_use]
    pub fn new(destination: String, naming: HostNaming) -> RemoteInput {
        RemoteInput {
            destination,
            naming,
            host: None,
        }
    }

    /// Applies one event of the connection to `view`.
    ///
    /// A sample is pushed and clears the error. A read error and an end set
    /// the error. With [`HostNaming::Reported`], a sample and a read error
    /// that has a host also keep that host as the name. With
    /// [`HostNaming::Destination`], no event changes the name.
    pub fn apply(&mut self, event: Event, view: &mut View) {
        match event {
            Event::Sample { host, sample } => {
                if self.naming == HostNaming::Reported {
                    self.host = Some(host);
                }
                view.push_sample(sample);
            }
            Event::ReadError { host, reason } => {
                if let (HostNaming::Reported, Some(name)) = (self.naming, host) {
                    self.host = Some(name);
                }
                view.fail(reason);
            }
            Event::Ended { reason } => view.fail(reason),
        }
    }

    /// The Mac that the values come from.
    ///
    /// With [`HostNaming::Destination`], the host is the destination. With
    /// [`HostNaming::Reported`], the host is the host of the newest line, or
    /// the destination before the first line. The destination is always
    /// present.
    #[must_use]
    pub fn origin(&self) -> Origin<'_> {
        Origin {
            host: self
                .host
                .as_ref()
                .map_or(self.destination.as_str(), HostName::as_str),
            destination: Some(self.destination.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Ticks;

    const DESTINATION: &str = "me@studio.example.com";
    const FULL_HOST: &[u8] = b"studio.example.com";
    const SHORT_HOST: &str = "studio";
    const OTHER_HOST: &[u8] = b"laptop.example.com";
    const OTHER_SHORT_HOST: &str = "laptop";
    const READ_FAILED: &str = "the kernel refused";
    const REASON_ONE: &str = "reason one";
    const REASON_TWO: &str = "reason two";
    const REASON_ENDED: &str = "ssh exited with status 255";
    /// The number of samples that the views of the tests hold.
    const CAPACITY: NonZeroUsize = NonZeroUsize::MIN.saturating_add(3);

    fn reading(user: u32, idle: u32) -> Reading {
        Reading {
            ticks: Ticks {
                user,
                idle,
                ..Ticks::default()
            },
            pressure_level: 1,
            free_percent: 60,
        }
    }

    fn sample_of(first: u32, second: u32) -> Sample {
        Sample::between(&reading(0, 0), &reading(first, second))
    }

    fn ok(value: Reading) -> Result<Reading, String> {
        Ok(value)
    }

    fn failed() -> Result<Reading, String> {
        Err(READ_FAILED.to_owned())
    }

    fn line(raw: &[u8], sample: Sample) -> Event {
        Event::Sample {
            host: HostName::from_raw(raw),
            sample,
        }
    }

    fn read_error(raw: Option<&[u8]>, reason: &str) -> Event {
        Event::ReadError {
            host: raw.map(HostName::from_raw),
            reason: reason.to_owned(),
        }
    }

    fn remote(naming: HostNaming) -> (RemoteInput, View) {
        (
            RemoteInput::new(DESTINATION.to_owned(), naming),
            View::new(CAPACITY),
        )
    }

    #[test]
    fn a_view_keeps_the_number_of_samples_that_it_gets() {
        let (mut input, mut view) = remote(HostNaming::Reported);
        for share in 0..10 {
            input.apply(line(FULL_HOST, sample_of(share, 100 - share)), &mut view);
        }
        assert_eq!(view.history().capacity(), CAPACITY.get());
        assert_eq!(view.history().len(), CAPACITY.get());
        assert_eq!(view.history().newest(), Some(sample_of(9, 91)));
    }

    #[test]
    fn the_first_kernel_reading_pushes_nothing_and_clears_an_error() {
        let mut input = KernelInput::new();
        let mut view = View::new(CAPACITY);
        input.apply(failed(), &mut view);
        input.apply(ok(reading(0, 0)), &mut view);
        assert!(view.history().is_empty());
        assert_eq!(view.error(), None);
    }

    #[test]
    fn the_second_kernel_reading_pushes_the_sample_between_the_two() {
        let mut input = KernelInput::new();
        let mut view = View::new(CAPACITY);
        input.apply(ok(reading(0, 0)), &mut view);
        input.apply(ok(reading(30, 70)), &mut view);
        assert_eq!(view.history().len(), 1);
        assert_eq!(view.history().newest(), Some(sample_of(30, 70)));
    }

    #[test]
    fn a_kernel_error_sets_the_text_and_keeps_the_history_and_the_previous_reading() {
        let mut input = KernelInput::new();
        let mut view = View::new(CAPACITY);
        input.apply(ok(reading(0, 0)), &mut view);
        input.apply(failed(), &mut view);
        assert_eq!(view.error(), Some(READ_FAILED));
        assert!(view.history().is_empty());
        input.apply(ok(reading(50, 50)), &mut view);
        assert_eq!(view.error(), None);
        assert_eq!(view.history().newest(), Some(sample_of(50, 50)));
    }

    #[test]
    fn a_remote_sample_sets_the_host_pushes_and_clears_the_error() {
        let (mut input, mut view) = remote(HostNaming::Reported);
        input.apply(read_error(None, REASON_ONE), &mut view);
        input.apply(line(FULL_HOST, sample_of(10, 90)), &mut view);
        assert_eq!(input.origin().host, SHORT_HOST);
        assert_eq!(view.history().newest(), Some(sample_of(10, 90)));
        assert_eq!(view.error(), None);
    }

    #[test]
    fn a_remote_read_error_with_a_host_changes_the_host_and_sets_the_error() {
        let (mut input, mut view) = remote(HostNaming::Reported);
        input.apply(line(FULL_HOST, sample_of(10, 90)), &mut view);
        input.apply(read_error(Some(OTHER_HOST), REASON_ONE), &mut view);
        assert_eq!(input.origin().host, OTHER_SHORT_HOST);
        assert_eq!(view.error(), Some(REASON_ONE));
        assert_eq!(view.history().len(), 1);
    }

    #[test]
    fn a_remote_read_error_with_no_host_keeps_the_host() {
        let (mut input, mut view) = remote(HostNaming::Reported);
        input.apply(line(FULL_HOST, sample_of(10, 90)), &mut view);
        input.apply(read_error(None, REASON_TWO), &mut view);
        assert_eq!(input.origin().host, SHORT_HOST);
        assert_eq!(view.error(), Some(REASON_TWO));
    }

    #[test]
    fn a_remote_end_sets_the_error_and_keeps_the_host_and_the_history() {
        let (mut input, mut view) = remote(HostNaming::Reported);
        input.apply(line(FULL_HOST, sample_of(10, 90)), &mut view);
        input.apply(
            Event::Ended {
                reason: REASON_ENDED.to_owned(),
            },
            &mut view,
        );
        assert_eq!(view.error(), Some(REASON_ENDED));
        assert_eq!(input.origin().host, SHORT_HOST);
        assert_eq!(view.history().len(), 1);
    }

    #[test]
    fn before_the_first_line_the_origin_names_the_destination() {
        let (input, _view) = remote(HostNaming::Reported);
        let origin = input.origin();
        assert_eq!(origin.host, DESTINATION);
        assert_eq!(origin.destination, Some(DESTINATION));
    }

    #[test]
    fn after_a_sample_the_origin_keeps_the_destination_and_shows_the_short_host() {
        let (mut input, mut view) = remote(HostNaming::Reported);
        input.apply(line(FULL_HOST, sample_of(10, 90)), &mut view);
        let origin = input.origin();
        assert_eq!(origin.host, SHORT_HOST);
        assert_eq!(origin.destination, Some(DESTINATION));
    }

    #[test]
    fn without_resolve_a_sample_keeps_the_destination_as_the_name() {
        let (mut input, mut view) = remote(HostNaming::Destination);
        input.apply(line(FULL_HOST, sample_of(10, 90)), &mut view);
        assert_eq!(input.origin().host, DESTINATION);
        assert_eq!(input.origin().destination, Some(DESTINATION));
        assert_eq!(view.history().newest(), Some(sample_of(10, 90)));
    }

    #[test]
    fn without_resolve_a_read_error_with_a_host_keeps_the_destination_as_the_name() {
        let (mut input, mut view) = remote(HostNaming::Destination);
        input.apply(read_error(Some(OTHER_HOST), REASON_ONE), &mut view);
        assert_eq!(input.origin().host, DESTINATION);
        assert_eq!(view.error(), Some(REASON_ONE));
    }
}
