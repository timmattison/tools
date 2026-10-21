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
        if is_zombie {
            Self::Zombie
        } else if can_refuse {
            Self::NeedsSudo
        } else {
            Self::Unknown
        }
    }

    /// The text the row shows.
    fn text(self) -> &'static str {
        match self {
            Self::NeedsSudo => TEXT_NEEDS_SUDO,
            Self::Zombie => TEXT_ZOMBIE,
            Self::Unknown => TEXT_UNKNOWN,
        }
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
        match value {
            Some(text) if !text.is_empty() => Self::Known(text),
            _ => Self::Unread(reason),
        }
    }

    /// The text of the row: the value, or the text of the reason.
    fn text(&self) -> &str {
        match self {
            Self::Known(value) => value,
            Self::Unread(reason) => reason.text(),
        }
    }
}

/// The CPU and memory use of the process, or why the kernel withheld them.
///
/// One read of the task info gives both values, so the kernel withholds both or neither.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Usage {
    /// The values were read.
    Known {
        /// The CPU use, in percent.
        cpu_usage: f32,
        /// The memory use, in bytes.
        memory: u64,
    },
    /// The values were not read, for the stated reason.
    Unread(UnreadReason),
}

impl Usage {
    /// Makes the usage from the values that `sysinfo` gave.
    ///
    /// The rule rests on the memory alone. A live process always holds memory,
    /// but 0.0% CPU is a normal value.
    ///
    /// # Arguments
    ///
    /// * `cpu_usage` - The CPU use, in percent, as `sysinfo` gave it
    /// * `memory` - The memory use, in bytes, as `sysinfo` gave it
    /// * `withheld` - The reason to use when a 0 means the kernel withheld the
    ///   values, or `None` when a 0 is a real value
    ///
    /// # Returns
    ///
    /// `Unread(reason)` when `memory` is 0 and `withheld` is `Some(reason)`,
    /// otherwise `Known` with both values.
    pub(crate) fn from_snapshot(
        cpu_usage: f32,
        memory: u64,
        withheld: Option<UnreadReason>,
    ) -> Self {
        match withheld {
            Some(reason) if memory == 0 => Self::Unread(reason),
            _ => Self::Known { cpu_usage, memory },
        }
    }

    /// The text of the CPU row and the text of the memory row.
    fn texts(&self) -> (String, String) {
        match self {
            Self::Known { cpu_usage, memory } => {
                (format!("{cpu_usage:.1}%"), super::format_memory(*memory))
            }
            Self::Unread(reason) => (reason.text().to_string(), reason.text().to_string()),
        }
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
    /// The CPU and memory use.
    pub(crate) usage: Usage,
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
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic);
        for (key, value) in self.facts() {
            table.add_row([key, &value]);
        }
        table.add_row([KEY_PARENTS, &self.parents_table_text()]);
        table
    }

    /// Renders the card as `key: value` lines for `--raw`.
    ///
    /// # Returns
    ///
    /// One line for each fact, and one line for each ancestor. A control
    /// character in a value is escaped, so each fact stays on one line.
    pub(crate) fn raw_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .facts()
            .into_iter()
            .map(|(key, value)| raw_line(&key.to_lowercase(), &value))
            .collect();
        match &self.parents {
            Parents::Chain(chain) if chain.is_empty() => {
                lines.push(raw_line(RAW_KEY_PARENTS, TEXT_NONE));
            }
            Parents::Chain(chain) => {
                for ancestor in chain {
                    let value = format!("{} {}", ancestor.pid, ancestor.command);
                    lines.push(raw_line(RAW_KEY_PARENT, &value));
                }
            }
            Parents::Unread(reason) => {
                lines.push(raw_line(RAW_KEY_PARENTS, &unread_parents_text(reason)));
            }
        }
        lines
    }

    /// The rows that hold one value each, in the order of the card.
    ///
    /// # Returns
    ///
    /// The key and the text of each row, without the parents.
    fn facts(&self) -> Vec<(&'static str, String)> {
        let (cpu, memory) = self.usage.texts();
        vec![
            (KEY_PID, self.pid.to_string()),
            (KEY_NAME, self.name.clone()),
            (KEY_USER, self.user.clone()),
            (KEY_STATUS, self.status.clone()),
            (KEY_CPU, cpu),
            (KEY_MEMORY, memory),
            (KEY_STARTED, self.started.text().to_string()),
            (KEY_EXE, self.exe.text().to_string()),
            (KEY_CWD, self.cwd.text().to_string()),
            (KEY_COMMAND, self.command.text().to_string()),
        ]
    }

    /// The value of the parents row in the table.
    ///
    /// # Returns
    ///
    /// One line for each ancestor, with the PIDs padded to the widest PID.
    fn parents_table_text(&self) -> String {
        match &self.parents {
            Parents::Chain(chain) if chain.is_empty() => TEXT_NONE.to_string(),
            Parents::Chain(chain) => {
                let width = chain
                    .iter()
                    .map(|ancestor| ancestor.pid.to_string().len())
                    .max()
                    .unwrap_or(0);
                chain
                    .iter()
                    .map(|ancestor| format!("{:<width$}  {}", ancestor.pid, ancestor.command))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Parents::Unread(reason) => unread_parents_text(reason),
        }
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
pub(crate) fn ancestor_command(snapshot_command: Option<&str>, tree_comm: Option<&str>) -> String {
    snapshot_command
        .filter(|command| !command.is_empty())
        .or(tree_comm.filter(|comm| !comm.is_empty()))
        .unwrap_or(TEXT_UNKNOWN)
        .to_string()
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
    let (days, hours) = (seconds / 86_400, seconds % 86_400 / 3_600);
    let (minutes, rest) = (seconds % 3_600 / 60, seconds % 60);
    if seconds < 60 {
        format!("{rest}s")
    } else if seconds < 3_600 {
        format!("{minutes}m {rest}s")
    } else if seconds < 86_400 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{days}d {hours}h")
    }
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
    if start_epoch_seconds == 0 {
        return None;
    }
    let seconds = i64::try_from(start_epoch_seconds).ok()?;
    let time = chrono::DateTime::from_timestamp(seconds, 0)?.with_timezone(zone);
    Some(format!(
        "{} ({} ago)",
        time.format("%Y-%m-%d %H:%M:%S"),
        format_elapsed(run_seconds)
    ))
}

/// Makes the text that says why the parents are not known.
///
/// # Arguments
///
/// * `reason` - The reason, in plain words
fn unread_parents_text(reason: &str) -> String {
    format!("{TEXT_PARENTS_UNREAD_PREFIX}{reason})")
}

/// Makes one raw line. Each control character in the value is escaped.
///
/// # Arguments
///
/// * `key` - The raw key
/// * `value` - The value, which can hold control characters
fn raw_line(key: &str, value: &str) -> String {
    let mut line = format!("{key}: ");
    for c in value.chars() {
        if c.is_control() {
            line.extend(c.escape_default());
        } else {
            line.push(c);
        }
    }
    line
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
            usage: Usage::Known {
                cpu_usage: 0.0,
                memory: 41_523_200,
            },
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

        for width in [30_u16, 31] {
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
    fn the_table_escapes_a_control_character_in_the_command() {
        let mut card = sample_card();
        card.command = Fact::Known("x\u{1b}]0;PWNED\u{7}y".to_string());
        let out = render(&card, 200);
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert!(!out.contains('\u{7}'), "{out:?}");
        assert!(out.contains("x\\u{1b}]0;PWNED\\u{7}y"), "{out}");
    }

    #[test]
    fn the_table_escapes_a_control_character_in_an_ancestor() {
        let mut card = sample_card();
        card.parents = Parents::Chain(vec![Ancestor {
            pid: 7,
            command: "evil\u{1b}[2J".to_string(),
        }]);
        let out = render(&card, 200);
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert!(out.contains("evil\\u{1b}[2J"), "{out}");
    }

    #[test]
    fn a_newline_in_the_command_cannot_fake_a_row_of_the_table() {
        let mut card = sample_card();
        card.command = Fact::Known("sh -c a\nPID 1".to_string());
        let out = render(&card, 200);
        assert!(out.contains("sh -c a\\nPID 1"), "{out}");
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
                raw.contains(&format!("{key}: (cannot be read: the process is a zombie)")),
                "{key}"
            );
        }
    }

    /// The raw lines of a card with the given usage.
    fn raw_with_usage(usage: Usage) -> Vec<String> {
        let mut card = sample_card();
        card.usage = usage;
        card.raw_lines()
    }

    #[test]
    fn withheld_task_info_says_needs_sudo_for_cpu_and_memory() {
        let usage = Usage::from_snapshot(0.0, 0, Some(UnreadReason::NeedsSudo));
        let raw = raw_with_usage(usage);
        assert!(raw.contains(&"cpu: (needs sudo)".to_string()), "{raw:?}");
        assert!(raw.contains(&"memory: (needs sudo)".to_string()), "{raw:?}");
    }

    #[test]
    fn a_zero_is_a_value_when_the_kernel_withholds_nothing() {
        let raw = raw_with_usage(Usage::from_snapshot(0.0, 0, None));
        assert!(raw.contains(&"cpu: 0.0%".to_string()), "{raw:?}");
        let memory = format!("memory: {}", super::super::format_memory(0));
        assert!(raw.contains(&memory), "{raw:?}");
    }

    #[test]
    fn memory_that_is_not_zero_means_the_read_worked() {
        let usage = Usage::from_snapshot(1.5, 4096, Some(UnreadReason::NeedsSudo));
        assert_eq!(
            usage,
            Usage::Known {
                cpu_usage: 1.5,
                memory: 4096
            }
        );
    }

    #[test]
    fn a_zombie_says_it_cannot_be_read_for_cpu_and_memory() {
        let usage = Usage::from_snapshot(0.0, 0, Some(UnreadReason::Zombie));
        let raw = raw_with_usage(usage);
        for key in ["cpu", "memory"] {
            assert!(
                raw.contains(&format!("{key}: (cannot be read: the process is a zombie)")),
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
