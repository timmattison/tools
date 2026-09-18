//! The exit statuses of popstop.
//!
//! A script reads the exit status, not the text, so each status has one name
//! and one meaning. `--help` lists them from [`help_section`], which builds
//! its text from these constants, so the numbers in the help never drift from
//! the numbers that popstop returns.

/// Success.
pub const SUCCESS: u8 = 0;

/// An error, for example an audio unit that did not start.
pub const ERROR: u8 = 1;

/// A usage error. clap returns it for a command line that it cannot parse.
pub const USAGE: u8 = 2;

/// Another copy runs, so this copy did not start.
pub const ANOTHER_COPY_RUNS: u8 = 3;

/// `--status` only: no copy runs.
pub const NO_COPY_RUNS: u8 = 4;

/// Gives the section of `--help` that lists the exit statuses.
#[must_use]
pub fn help_section() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::help_section;

    #[test]
    fn the_help_section_lists_each_status_with_its_meaning() {
        assert_eq!(
            help_section(),
            "Exit status:\n\
             \x20 0  success\n\
             \x20 1  an error, for example an audio unit that did not start\n\
             \x20 2  a usage error\n\
             \x20 3  another copy runs, so this copy did not start\n\
             \x20 4  --status only: no copy runs"
        );
    }
}
