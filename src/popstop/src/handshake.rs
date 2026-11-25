//! The report of a background copy to the parent that started it.
//!
//! `popstop --background` starts a copy of popstop and waits for one line on
//! the stdout of that copy. The line says that the copy plays, or why it did
//! not start. The parent then writes the answer for the user and ends, and the
//! copy sends its later output to the log.
//!
//! The writer and the reader of that line live in two processes, so the line
//! has one shape here and nowhere else. The shape is one line of JSON, as the
//! record of the lock file is. Thus a device name or a message that holds a
//! tab or a newline stays one line, and the reader gives back the text that
//! the writer sent.

use serde::{Deserialize, Serialize};

/// What a background copy tells the parent that started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "report", rename_all = "lowercase")]
pub enum Handshake {
    /// The copy holds the lock, and the signal plays on the device.
    Ready {
        /// The name of the device that stays awake.
        device_name: String,
        /// The process ID of the copy.
        pid: u32,
    },
    /// The copy did not start. The parent writes the message for the user and
    /// ends with the status.
    Failed {
        /// The exit status for the parent.
        status: u8,
        /// The whole text for the user. It carries the name of popstop
        /// already.
        message: String,
    },
}

impl Handshake {
    /// Gives the report as one line, with a newline at its end.
    ///
    /// JSON writes a tab and a newline of a text as two letters, thus the
    /// line holds one newline and that newline is its end.
    ///
    /// # Panics
    ///
    /// Panics when the report cannot be written as JSON. Every field of a
    /// report is a number or a string, and JSON holds both, thus this panic
    /// cannot happen.
    #[must_use]
    pub fn line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a report of popstop is JSON");
        line.push('\n');
        line
    }

    /// Reads a report from one line, with or without the newline at its end.
    ///
    /// Gives `None` for a line that is not a report of popstop.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::Handshake;

    #[test]
    fn a_report_that_a_copy_wrote_reads_back_as_the_same_report() {
        let ready = Handshake::Ready {
            device_name: "Klipsch R-51PM".to_owned(),
            pid: 4242,
        };
        let failed = Handshake::Failed {
            status: 3,
            message: "popstop: another copy runs".to_owned(),
        };

        for report in [&ready, &failed] {
            let line = report.line();
            assert!(
                line.ends_with('\n'),
                "a report ends the line that carries it: {line:?}"
            );
            assert_eq!(
                line.matches('\n').count(),
                1,
                "a report is one line: {line:?}"
            );
            assert_eq!(Handshake::parse(&line), Some(report.clone()));
            assert_eq!(
                Handshake::parse(line.trim_end_matches('\n')),
                Some(report.clone()),
                "a reader that took the newline away reads the same report"
            );
        }
    }

    #[test]
    fn a_line_that_is_not_a_report_of_popstop_gives_nothing() {
        for line in [
            "",
            "\n",
            "ready",
            "ready\tKlipsch R-51PM\t4242",
            "{}",
            "{\"report\":\"ready\"}",
            "{\"report\":\"maybe\"}",
        ] {
            assert_eq!(
                Handshake::parse(line),
                None,
                "the line {line:?} is not a report"
            );
        }
    }

    #[test]
    fn a_message_that_holds_a_tab_and_a_newline_stays_one_line() {
        let awkward = Handshake::Failed {
            status: 1,
            message: "popstop: a\tname\nand a second line\n".to_owned(),
        };
        let device = Handshake::Ready {
            device_name: "a\tdevice\nwith two lines".to_owned(),
            pid: 7,
        };

        for report in [&awkward, &device] {
            let line = report.line();
            assert_eq!(
                line.matches('\n').count(),
                1,
                "the text of the report makes no second line: {line:?}"
            );
            assert_eq!(
                Handshake::parse(&line),
                Some(report.clone()),
                "the reader gives back the text that the writer sent"
            );
        }
    }
}
