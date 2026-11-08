//! The parser of an endless run of `top`.
//!
//! `top -l 0` prints one sample after the other until the caller stops it.
//! [`StreamParser`] turns the bytes of that run into [`ParsedEvent`] values. It
//! holds no clock and no process, so a test feeds it bytes and reads events.
//!
//! A sample ends when the `Processes:` line of the next sample arrives. A timer
//! on an idle pipe would end it sooner, but it would also cut a sample that
//! `top` is slow to write.

use crate::top::TopSample;

/// What the parser found in the bytes that it has read so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedEvent {
    /// The header of a sample arrived. `finished` is the sample before it, when
    /// that sample is complete and usable. The first sample of a child never
    /// comes out here, because it holds the counter since each process started.
    Header {
        /// The sample that this header ended.
        finished: Option<TopSample>,
    },
    /// Output that is not a sample. `reason` is the text of a
    /// `MachineError::TopOutput`.
    Refused {
        /// Why the parser refused the output.
        reason: String,
    },
}

/// Reads the bytes of an endless run of `top` and gives events.
#[derive(Debug, Default)]
pub struct StreamParser {}

impl StreamParser {
    /// Makes a parser that has read no byte.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }

    /// Reads `bytes` and gives the events that they complete.
    pub fn feed(&mut self, _bytes: &[u8]) -> Vec<ParsedEvent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pid::Pid;
    use crate::top::FaultCount;
    use std::time::Duration;

    /// The header row that `top` prints for `-stats pid,faults`.
    const HEADER_ROW: &str = "PID    FAULTS    ";

    /// Gives one sample as `top` prints it, with the clock line `clock`.
    fn sample_at(clock: &str, rows: &str) -> String {
        format!(
            "Processes: 3 total, 1 running, 2 sleeping, 9 threads \n\
             {clock}\n\
             Load Avg: 1.00, 1.00, 1.00 \n\
             \n\
             {HEADER_ROW}\n\
             {rows}"
        )
    }

    /// Gives one sample whose clock line is `2026/09/18 <time>`.
    fn sample(time: &str, rows: &str) -> String {
        sample_at(&format!("2026/09/18 {time}"), rows)
    }

    /// Gives the row of `pid` with `faults`.
    fn row(pid: u32, faults: u64) -> FaultCount {
        FaultCount {
            pid: Pid::new(pid),
            faults,
        }
    }

    /// Feeds `text` to a new parser in one read.
    fn events_of(text: &str) -> Vec<ParsedEvent> {
        StreamParser::new().feed(text.as_bytes())
    }

    /// Three samples give three headers. The first sample holds the counter
    /// since each process started, so it comes out as no sample. The second
    /// sample comes out when the third header arrives, and the third waits.
    #[test]
    fn the_first_sample_is_discarded_and_the_second_comes_out() {
        let text = sample("12:00:00", "10     9000000   \n")
            + &sample("12:00:03", "10     3         \n20     4000      \n")
            + &sample("12:00:06", "10     5         \n");

        let events = events_of(&text);

        assert_eq!(
            events,
            [
                ParsedEvent::Header { finished: None },
                ParsedEvent::Header { finished: None },
                ParsedEvent::Header {
                    finished: Some(TopSample {
                        rows: vec![row(10, 3), row(20, 4_000)],
                        elapsed: Some(Duration::from_secs(3)),
                    })
                },
            ]
        );
    }
}
