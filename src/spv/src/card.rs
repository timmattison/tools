//! The detail card of one process.
//!
//! When the pattern of `spv` is exactly one PID, `spv` prints a card instead of
//! a table. The card is a two-column table of key and value. A long value wraps
//! at the width of the terminal, and no value is ever shortened. With `--raw`,
//! the card is `key: value` lines.
//!
//! This module holds the data model and the two renders. It reads nothing from
//! the machine. The caller fills a [`ProcessCard`] and prints it.

use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};

/// Key of the PID row.
const KEY_PID: &str = "PID";
/// Key of the name row.
const KEY_NAME: &str = "Name";
/// Key of the user row.
const KEY_USER: &str = "User";
/// Key of the status row.
const KEY_STATUS: &str = "Status";
/// Key of the CPU row.
const KEY_CPU: &str = "CPU";
/// Key of the memory row.
const KEY_MEMORY: &str = "Memory";
/// Key of the start time row.
const KEY_STARTED: &str = "Started";
/// Key of the executable row.
const KEY_EXE: &str = "Exe";
/// Key of the working directory row.
const KEY_CWD: &str = "CWD";
/// Key of the command line row.
const KEY_COMMAND: &str = "Command";
/// Key of the parents row.
const KEY_PARENTS: &str = "Parents";
/// Raw key of one ancestor line. The raw form has one line for each ancestor.
const RAW_KEY_PARENT: &str = "parent";
/// Raw key of the parents line, used when there is no chain to list.
const RAW_KEY_PARENTS: &str = "parents";

/// Text of the parents row when the process has no ancestor.
const TEXT_NONE: &str = "(none)";
/// Text of a row with `NeedsSudo`.
const TEXT_NEEDS_SUDO: &str = "(needs sudo)";
/// Text of a row with `Zombie`.
const TEXT_ZOMBIE: &str = "(cannot be read: the process is a zombie)";
/// Text of a row with `Unknown`. An ancestor with no command uses it too.
const TEXT_UNKNOWN: &str = "(cannot be read)";
/// Start of the text that says why the parents are not known.
const TEXT_PARENTS_UNREAD_PREFIX: &str = "(cannot be read: ";

/// Why the card shows no value in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnreadReason {
    /// The process belongs to another user, and this caller is not root.
    NeedsSudo,
    /// The process is a zombie. It holds no executable, directory, or command line.
    Zombie,
    /// The kernel gave no value, and this tool cannot say why.
    Unknown,
}

impl UnreadReason {
    /// Chooses the reason for a row with no value.
    ///
    /// A zombie wins over sudo, because sudo does not bring a zombie back.
    ///
    /// # Arguments
    ///
    /// * `is_zombie` - Whether the process is a zombie
    /// * `can_refuse` - Whether the kernel can refuse this caller, so that sudo can help
    ///
    /// # Returns
    ///
    /// The reason to show in the row.
    pub(crate) fn choose(is_zombie: bool, can_refuse: bool) -> Self {
        let _ = (is_zombie, can_refuse);
        Self::Unknown
    }

    /// The text the row shows.
    fn text(self) -> &'static str {
        let _ = (TEXT_NEEDS_SUDO, TEXT_ZOMBIE, TEXT_UNKNOWN);
        ""
    }
}

/// The value of one row that the kernel can withhold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fact {
    /// The value was read.
    Known(String),
    /// The value was not read, for the stated reason.
    Unread(UnreadReason),
}

impl Fact {
    /// Makes a fact from a value that can be missing.
    ///
    /// # Arguments
    ///
    /// * `value` - The value, if the kernel gave one
    /// * `reason` - The reason to use when `value` is `None` or empty
    ///
    /// # Returns
    ///
    /// `Known` when `value` is `Some` and not empty, otherwise `Unread(reason)`.
    pub(crate) fn from_value(value: Option<String>, reason: UnreadReason) -> Self {
        let _ = value;
        Self::Unread(reason)
    }

    /// The text of the row: the value, or the text of the reason.
    fn text(&self) -> &str {
        ""
    }
}

/// One process above the process of the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ancestor {
    /// The PID of the ancestor.
    pub(crate) pid: u32,
    /// The command of the ancestor.
    pub(crate) command: String,
}

/// The parents of the process, or why the card cannot show them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Parents {
    /// The ancestors, nearest first. The chain is empty when the process has none.
    Chain(Vec<Ancestor>),
    /// The reason the chain is not known.
    Unread(String),
}

/// The facts of one process, read once, with no live process behind them.
pub(crate) struct ProcessCard {
    /// The PID of the process.
    pub(crate) pid: u32,
    /// The name of the process.
    pub(crate) name: String,
    /// The user that owns the process.
    pub(crate) user: String,
    /// The status of the process, for example `Run` or `Sleep`.
    pub(crate) status: String,
    /// The CPU use, in percent.
    pub(crate) cpu_usage: f32,
    /// The memory use, in bytes.
    pub(crate) memory: u64,
    /// The start time with the elapsed time.
    pub(crate) started: Fact,
    /// The path of the executable.
    pub(crate) exe: Fact,
    /// The working directory.
    pub(crate) cwd: Fact,
    /// The full command line.
    pub(crate) command: Fact,
    /// The ancestors of the process.
    pub(crate) parents: Parents,
}

impl ProcessCard {
    /// Renders the card as a two-column table.
    ///
    /// The caller prints the table. A test sets the width of the table first.
    ///
    /// # Returns
    ///
    /// A table with one `[key, value]` row for each key and no header.
    pub(crate) fn table(&self) -> Table {
        let _ = (UTF8_FULL, ContentArrangement::Dynamic);
        Table::new()
    }

    /// Renders the card as `key: value` lines for `--raw`.
    ///
    /// # Returns
    ///
    /// One line for each fact, and one line for each ancestor. A control
    /// character in a value is escaped, so each fact stays on one line.
    pub(crate) fn raw_lines(&self) -> Vec<String> {
        let _ = (
            KEY_PID,
            KEY_NAME,
            KEY_USER,
            KEY_STATUS,
            KEY_CPU,
            KEY_MEMORY,
            KEY_STARTED,
            KEY_EXE,
            KEY_CWD,
            KEY_COMMAND,
            KEY_PARENTS,
            RAW_KEY_PARENT,
            RAW_KEY_PARENTS,
            TEXT_NONE,
            TEXT_PARENTS_UNREAD_PREFIX,
        );
        Vec::new()
    }
}

/// Chooses the command to show for an ancestor.
///
/// # Arguments
///
/// * `snapshot_command` - The command line from the sysinfo snapshot
/// * `tree_comm` - The comm from the process tree
///
/// # Returns
///
/// The command line when it is not empty, else the comm, else "(cannot be read)".
pub(crate) fn ancestor_command(
    snapshot_command: Option<&str>,
    tree_comm: Option<&str>,
) -> String {
    let _ = (snapshot_command, tree_comm);
    String::new()
}

/// Formats an elapsed time in its two largest units.
///
/// # Arguments
///
/// * `seconds` - The elapsed time in seconds
///
/// # Returns
///
/// For example `59s`, `9m 23s`, `2h 5m`, or `3d 4h`.
pub(crate) fn format_elapsed(seconds: u64) -> String {
    let _ = seconds;
    String::new()
}

/// Formats the start time as a time in `zone`, with the elapsed time.
///
/// # Arguments
///
/// * `start_epoch_seconds` - The start time in seconds since the Unix epoch
/// * `run_seconds` - The time the process has run, in seconds
/// * `zone` - The time zone to show the start time in
///
/// # Returns
///
/// For example "2026-10-20 13:24:50 (9m 23s ago)". `None` when
/// `start_epoch_seconds` is 0 (sysinfo gives 0 when it could not read the
/// start) or out of the range of chrono.
pub(crate) fn started<Tz: chrono::TimeZone>(
    start_epoch_seconds: u64,
    run_seconds: u64,
    zone: &Tz,
) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let _ = (start_epoch_seconds, run_seconds, zone);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// The border characters of the `UTF8_FULL` preset.
    const BORDERS: &[char] = &[
        '┌', '┬', '┐', '├', '┼', '┤', '└', '┴', '┘', '─', '│', '┆', '╞', '╪', '╡', '═',
    ];

    /// Drops every space and every border character.
    fn flatten(text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_whitespace() && !BORDERS.contains(c))
            .collect()
    }

    /// A card with a value in every row and two ancestors.
    fn sample_card() -> ProcessCard {
        ProcessCard {
            pid: 75803,
            name: "node".to_string(),
            user: "tim".to_string(),
            status: "Run".to_string(),
            cpu_usage: 0.0,
            memory: 41_523_200,
            started: Fact::Known("2026-10-20 13:24:50 (9m 23s ago)".to_string()),
            exe: Fact::Known("/usr/local/bin/node".to_string()),
            cwd: Fact::Known("/x".to_string()),
            command: Fact::Known("node cli.mjs --flag".to_string()),
            parents: Parents::Chain(vec![
                Ancestor {
                    pid: 75757,
                    command: "zsh -l".to_string(),
                },
                Ancestor {
                    pid: 1,
                    command: "/sbin/launchd".to_string(),
                },
            ]),
        }
    }

    /// Renders the table of a card at a width.
    fn render(card: &ProcessCard, width: u16) -> String {
        let mut table = card.table();
        table.set_width(width);
        table.to_string()
    }

    #[test]
    fn a_long_command_appears_whole_and_is_never_shortened() {
        let cmd = (0..40)
            .map(|i| format!("/usr/local/lib/package{i}/bin/tool"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(cmd.chars().count() > 300);
        let mut card = sample_card();
        card.command = Fact::Known(cmd.clone());

        let out = render(&card, 80);
        assert!(flatten(&out).contains(&flatten(&cmd)));
        assert!(!out.contains("..."));
        assert!(!out.contains('…'));
        for line in out.lines() {
            assert!(line.width() <= 80, "line too wide: {line}");
        }

        let raw = card.raw_lines();
        let want = format!("command: {cmd}");
        assert_eq!(raw.iter().filter(|l| **l == want).count(), 1);
    }

    #[test]
    fn format_elapsed_gives_the_two_largest_units() {
        let cases: [(u64, &str); 10] = [
            (0, "0s"),
            (59, "59s"),
            (60, "1m 0s"),
            (563, "9m 23s"),
            (3599, "59m 59s"),
            (3600, "1h 0m"),
            (7500, "2h 5m"),
            (86399, "23h 59m"),
            (86400, "1d 0h"),
            (273_600, "3d 4h"),
        ];
        for (seconds, want) in cases {
            assert_eq!(format_elapsed(seconds), want, "for {seconds}");
        }
    }

    #[test]
    fn multi_byte_values_wrap_on_a_character_boundary() {
        let cwd = "/日本語/🎉/café ".repeat(12).trim_end().to_string();
        let command = "日本語 🎉 café 日本語日本語日本語 🎉🎉🎉🎉 café "
            .repeat(8)
            .trim_end()
            .to_string();
        let mut card = sample_card();
        card.cwd = Fact::Known(cwd.clone());
        card.command = Fact::Known(command.clone());

        for width in [30u16, 31] {
            let out = render(&card, width);
            assert!(!out.contains('\u{FFFD}'));
            for line in out.lines() {
                assert!(
                    line.width() <= usize::from(width),
                    "width {width}, line too wide: {line}"
                );
            }
            assert!(flatten(&out).contains(&flatten(&cwd)), "cwd at {width}");
            assert!(
                flatten(&out).contains(&flatten(&command)),
                "command at {width}"
            );
        }
    }

    #[test]
    fn raw_gives_one_key_value_line_for_each_fact() {
        let card = sample_card();
        let memory = super::super::format_memory(41_523_200);
        let want = vec![
            "pid: 75803".to_string(),
            "name: node".to_string(),
            "user: tim".to_string(),
            "status: Run".to_string(),
            "cpu: 0.0%".to_string(),
            format!("memory: {memory}"),
            "started: 2026-10-20 13:24:50 (9m 23s ago)".to_string(),
            "exe: /usr/local/bin/node".to_string(),
            "cwd: /x".to_string(),
            "command: node cli.mjs --flag".to_string(),
            "parent: 75757 zsh -l".to_string(),
            "parent: 1 /sbin/launchd".to_string(),
        ];
        assert_eq!(card.raw_lines(), want);
    }

    #[test]
    fn raw_escapes_a_control_character_so_the_command_stays_on_one_line() {
        let mut card = sample_card();
        card.command = Fact::Known("sh -c echo a\necho b".to_string());
        let raw = card.raw_lines();
        assert!(raw.contains(&"command: sh -c echo a\\necho b".to_string()));
        assert!(!raw.contains(&"echo b".to_string()));
    }

    #[test]
    fn the_parents_line_up_by_the_widest_pid() {
        let mut card = sample_card();
        card.parents = Parents::Chain(vec![
            Ancestor {
                pid: 75757,
                command: "node cli.mjs".to_string(),
            },
            Ancestor {
                pid: 1,
                command: "launchd".to_string(),
            },
        ]);
        let out = render(&card, 200);
        assert!(out.contains("75757  node cli.mjs"), "{out}");
        assert!(out.contains("1      launchd"), "{out}");
    }

    #[test]
    fn an_empty_chain_says_none() {
        let mut card = sample_card();
        card.parents = Parents::Chain(Vec::new());
        assert!(render(&card, 200).contains("(none)"));
        assert!(card.raw_lines().contains(&"parents: (none)".to_string()));
    }

    #[test]
    fn a_chain_that_cannot_be_read_says_why() {
        let mut card = sample_card();
        card.parents = Parents::Unread("ps did not run".to_string());
        assert!(render(&card, 200).contains("(cannot be read: ps did not run)"));
        assert!(card
            .raw_lines()
            .contains(&"parents: (cannot be read: ps did not run)".to_string()));
    }

    /// A card whose exe, cwd, and command all carry one reason.
    fn card_with_reason(reason: UnreadReason) -> ProcessCard {
        let mut card = sample_card();
        card.exe = Fact::Unread(reason);
        card.cwd = Fact::Unread(reason);
        card.command = Fact::Unread(reason);
        card
    }

    #[test]
    fn a_row_the_kernel_refused_says_needs_sudo() {
        let raw = card_with_reason(UnreadReason::NeedsSudo).raw_lines();
        for key in ["exe", "cwd", "command"] {
            assert!(raw.contains(&format!("{key}: (needs sudo)")), "{key}");
        }
    }

    #[test]
    fn a_zombie_row_says_it_cannot_be_read() {
        let raw = card_with_reason(UnreadReason::Zombie).raw_lines();
        for key in ["exe", "cwd", "command"] {
            assert!(
                raw.contains(&format!(
                    "{key}: (cannot be read: the process is a zombie)"
                )),
                "{key}"
            );
        }
    }

    #[test]
    fn a_zombie_wins_over_sudo() {
        assert_eq!(UnreadReason::choose(true, true), UnreadReason::Zombie);
        assert_eq!(UnreadReason::choose(true, false), UnreadReason::Zombie);
        assert_eq!(UnreadReason::choose(false, true), UnreadReason::NeedsSudo);
        assert_eq!(UnreadReason::choose(false, false), UnreadReason::Unknown);
    }

    #[test]
    fn an_empty_value_is_not_a_fact() {
        let r = UnreadReason::NeedsSudo;
        assert_eq!(Fact::from_value(None, r), Fact::Unread(r));
        assert_eq!(Fact::from_value(Some(String::new()), r), Fact::Unread(r));
        assert_eq!(
            Fact::from_value(Some("x".into()), r),
            Fact::Known("x".to_string())
        );
    }

    #[test]
    fn an_ancestor_takes_the_command_line_then_the_comm() {
        assert_eq!(
            ancestor_command(Some("node a b"), Some("/x/node")),
            "node a b"
        );
        assert_eq!(
            ancestor_command(Some(""), Some("/sbin/launchd")),
            "/sbin/launchd"
        );
        assert_eq!(
            ancestor_command(None, Some("/sbin/launchd")),
            "/sbin/launchd"
        );
        assert_eq!(ancestor_command(None, None), "(cannot be read)");
    }

    #[test]
    fn started_gives_the_local_time_and_the_elapsed_time() {
        assert_eq!(
            started(1_000_000_000, 563, &chrono::Utc),
            Some("2001-09-09 01:46:40 (9m 23s ago)".to_string())
        );
        let east = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(
            started(1_000_000_000, 563, &east),
            Some("2001-09-09 03:46:40 (9m 23s ago)".to_string())
        );
        assert_eq!(started(0, 5, &chrono::Utc), None);
    }
}
