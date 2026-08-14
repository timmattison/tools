//! Process facts, and the rules that decide which of them is a Claude Code
//! session.
//!
//! Three kinds of process run a Claude Code executable image, and only one of
//! them is a session:
//!
//! - a **session**, the thing a person started and is talking to;
//! - a **support** process — the background daemon, a pty host, a spare — which
//!   Claude Code starts for itself;
//! - a **spawned tool**, such as the `ugrep` a search runs, which can still be
//!   holding the Claude Code image at the moment it is sampled.
//!
//! The image path alone cannot separate them, because all three report a Claude
//! Code image. The argument vector alone cannot either, because a session
//! reports the bare name `claude` exactly as a support process does. The rules
//! below read both.

use crate::ClaudeVersion;
use std::path::{Path, PathBuf};

/// What one process reports about itself, gathered from the operating system.
///
/// This is the whole input to every rule in this crate. Keeping it a plain
/// value — rather than a handle onto a live process table — is what makes the
/// rules testable against process shapes that are awkward to create on demand.
#[derive(Debug, Clone)]
pub struct ProcessFact {
    /// The process identifier.
    pub pid: u32,
    /// The kernel accounting name, which is the basename of the executed file.
    ///
    /// For Claude Code this is the release number, because each release is
    /// installed as a file named for its version.
    pub accounting_name: String,
    /// The executable image the process runs, when readable.
    pub exe: Option<PathBuf>,
    /// The argument vector, when readable.
    pub argv: Vec<String>,
    /// The current working directory, when readable.
    pub cwd: Option<PathBuf>,
    /// Seconds the process has been running.
    pub uptime_secs: u64,
    /// The wall-clock time the process started, in seconds since the epoch.
    pub start_time_epoch_secs: u64,
}

/// What a process running a Claude Code image actually is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// A Claude Code session: what a person started and talks to.
    Session,
    /// A process Claude Code runs for its own account, named by its subcommand.
    Support(String),
    /// A process that holds a Claude Code image but is not Claude Code, such as
    /// a tool spawned to serve a session.
    SpawnedTool,
    /// A process unrelated to Claude Code.
    Unrelated,
}

/// Subcommands that name a support process rather than a session.
///
/// The list is a deny-list rather than an allow-list because the session forms
/// are open-ended: `claude`, `claude "a prompt"`, and `claude /some-command`
/// are all sessions, and a new one must not be misread as support.
const SUPPORT_SUBCOMMANDS: [&str; 5] = ["daemon", "bg-spare", "bg-pty-host", "mcp", "install"];

/// Flags that mark a process as a support process whatever else it carries.
///
/// A pty host reports the ordinary `claude` name and is separated from a session
/// only by this flag.
const SUPPORT_FLAGS: [&str; 2] = ["--bg-spare", "--bg-pty-host"];

/// Returns `true` when `path` names a Claude Code executable.
///
/// Two install shapes reach here: the versioned release file under a
/// `share/claude/versions` directory, and a `claude` entry point such as the
/// launcher on `PATH` or the executable inside the application bundle.
fn is_claude_image(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == "claude" {
        return true;
    }
    // A versioned release file counts only inside a `versions` directory, so an
    // unrelated file that happens to be named like a number does not qualify.
    ClaudeVersion::parse(name).is_some()
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|parent| parent == "versions")
}

/// Returns the first word of `argv[0]`.
///
/// Claude Code writes a descriptive `argv[0]` for its support processes, such as
/// `claude bg-spare`, so the announced program name is the first word only.
fn announced_program(argv: &[String]) -> Option<&str> {
    argv.first()?.split_whitespace().next()
}

/// Returns the subcommand a support process announces inside its `argv[0]`.
fn announced_subcommand(argv: &[String]) -> Option<&str> {
    let mut words = argv.first()?.split_whitespace();
    words.next()?;
    words.next()
}

/// Decides what a process is.
///
/// # Examples
///
/// ```
/// use occ::{classify, ProcessFact, Role};
/// use std::path::PathBuf;
///
/// let session = ProcessFact {
///     pid: 1,
///     accounting_name: "2.1.232".to_string(),
///     exe: Some(PathBuf::from("/home/u/.local/share/claude/versions/2.1.232")),
///     argv: vec!["claude".to_string()],
///     cwd: None,
///     uptime_secs: 0,
///     start_time_epoch_secs: 0,
/// };
/// assert_eq!(classify(&session), Role::Session);
/// ```
#[must_use]
pub fn classify(fact: &ProcessFact) -> Role {
    // The image is the first gate. Without a Claude Code image the process is
    // nothing to do with Claude Code, whatever it calls itself.
    if !fact.exe.as_deref().is_some_and(is_claude_image) {
        return Role::Unrelated;
    }

    // A support process may announce its job inside argv[0], as `claude
    // bg-spare` does, and this reading must come before the session test
    // because its first word is the ordinary program name.
    if let Some(subcommand) = announced_subcommand(&fact.argv) {
        return Role::Support(subcommand.to_string());
    }

    // A pty host announces the plain name and is separated from a session only
    // by a flag, so the whole argument vector has to be read.
    for argument in &fact.argv {
        if let Some(flag) = SUPPORT_FLAGS.iter().find(|f| *f == argument) {
            return Role::Support(flag.trim_start_matches('-').to_string());
        }
    }

    // The image is Claude Code's but the announced program is not, which is how
    // a tool spawned to serve a session looks while it holds that image.
    let announced = announced_program(&fact.argv);
    let announces_claude = announced.is_some_and(|program| {
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "claude" || ClaudeVersion::parse(name).is_some())
    });
    if !announces_claude {
        return Role::SpawnedTool;
    }

    // A subcommand in the first argument names support. Anything else there is
    // a prompt, a slash command, or a flag, and all of those are sessions.
    if let Some(first) = fact.argv.get(1) {
        if SUPPORT_SUBCOMMANDS.contains(&first.as_str()) {
            return Role::Support(first.clone());
        }
    }

    Role::Session
}

/// Returns the Claude Code release a process runs.
///
/// The accounting name is preferred over the image path because it is recorded
/// when the process starts and does not change afterwards. An upgrade that
/// deletes the old release file leaves the path unresolvable while the
/// accounting name still names the release the process is running — which is
/// exactly the stale session this tool exists to surface.
#[must_use]
pub fn version_of(fact: &ProcessFact) -> Option<ClaudeVersion> {
    ClaudeVersion::parse(&fact.accounting_name).or_else(|| {
        let name = fact.exe.as_deref()?.file_name()?.to_str()?;
        ClaudeVersion::parse(name)
    })
}

#[cfg(test)]
mod tests {
    use super::{classify, version_of, ProcessFact, Role};
    use std::path::PathBuf;

    /// Builds a fact with the given accounting name, image, and arguments.
    fn fact(accounting_name: &str, exe: &str, argv: &[&str]) -> ProcessFact {
        ProcessFact {
            pid: 4242,
            accounting_name: accounting_name.to_string(),
            exe: Some(PathBuf::from(exe)),
            argv: argv.iter().map(|a| (*a).to_string()).collect(),
            cwd: Some(PathBuf::from("/work")),
            uptime_secs: 60,
            start_time_epoch_secs: 1_700_000_000,
        }
    }

    const VERSIONED: &str = "/Users/u/.local/share/claude/versions/2.1.232";
    const BUNDLED: &str = "/Users/u/.local/share/claude/ClaudeCode.app/Contents/MacOS/claude";
    const LAUNCHER: &str = "/Users/u/.local/bin/claude";

    #[test]
    fn a_bare_session_is_a_session() {
        let observed = fact("2.1.220", BUNDLED, &["claude", "--dangerously-skip-permissions"]);
        assert_eq!(classify(&observed), Role::Session);
    }

    #[test]
    fn a_session_launched_from_the_versioned_file_is_a_session() {
        let observed = fact(
            "2.1.197",
            VERSIONED,
            &[VERSIONED, "--session-id", "ed84c8c7-0117-4670-936c-98e0f0d2c80b"],
        );
        assert_eq!(classify(&observed), Role::Session);
    }

    #[test]
    fn a_session_running_a_slash_command_is_a_session() {
        // The argument after `claude` is a prompt, not a subcommand.
        let observed = fact("2.1.220", BUNDLED, &["claude", "/start-issue", "109"]);
        assert_eq!(classify(&observed), Role::Session);
    }

    #[test]
    fn the_daemon_is_support() {
        let observed = fact("2.1.232", VERSIONED, &[LAUNCHER, "daemon", "run"]);
        assert_eq!(classify(&observed), Role::Support("daemon".to_string()));
    }

    #[test]
    fn a_spare_named_in_its_own_argv_zero_is_support() {
        // Claude Code writes a descriptive argv[0] for these.
        let observed = fact("2.1.232", VERSIONED, &["claude bg-spare", "--bg-spare", "/tmp/s.sock"]);
        assert_eq!(classify(&observed), Role::Support("bg-spare".to_string()));
    }

    #[test]
    fn a_pty_host_flagged_only_by_its_arguments_is_support() {
        // This one announces the plain name `claude`; only the flag separates it
        // from a session.
        let observed = fact("claude", BUNDLED, &["claude", "--bg-pty-host", "/tmp/p.sock"]);
        assert_eq!(classify(&observed), Role::Support("bg-pty-host".to_string()));
    }

    #[test]
    fn a_tool_spawned_by_a_session_is_not_a_session() {
        // Observed on a live machine: the image and the accounting name are both
        // Claude Code's, but the process is a search tool. Counting it as a
        // session would report a working directory and an uptime for something
        // that is not a session at all.
        let observed = fact("2.1.232", VERSIONED, &["ugrep", "-G", "--ignore-files"]);
        assert_eq!(classify(&observed), Role::SpawnedTool);
    }

    #[test]
    fn an_unrelated_process_is_unrelated() {
        let observed = fact("zsh", "/bin/zsh", &["/bin/zsh", "-c", "echo hi"]);
        assert_eq!(classify(&observed), Role::Unrelated);
    }

    #[test]
    fn a_numeric_file_outside_a_versions_directory_is_unrelated() {
        let observed = fact("2.1.232", "/opt/data/2.1.232", &["2.1.232"]);
        assert_eq!(classify(&observed), Role::Unrelated);
    }

    #[test]
    fn the_version_comes_from_the_accounting_name() {
        let observed = fact("2.1.220", BUNDLED, &["claude"]);
        assert_eq!(
            version_of(&observed).map(|v| v.as_str().to_string()),
            Some("2.1.220".to_string())
        );
    }

    #[test]
    fn the_version_survives_deletion_of_the_release_file() {
        // The release file for 2.1.220 is gone after an upgrade, so the image
        // path resolves to the bundle. The accounting name still names 2.1.220,
        // and a session stuck on a deleted release is the whole point of `occ`.
        let observed = fact("2.1.220", BUNDLED, &["claude"]);
        assert_eq!(
            version_of(&observed).map(|v| v.as_str().to_string()),
            Some("2.1.220".to_string())
        );
    }

    #[test]
    fn the_version_falls_back_to_the_image_path() {
        let observed = fact("claude", VERSIONED, &["claude"]);
        assert_eq!(
            version_of(&observed).map(|v| v.as_str().to_string()),
            Some("2.1.232".to_string())
        );
    }

    #[test]
    fn an_unknowable_version_is_reported_as_absent() {
        let observed = fact("claude", LAUNCHER, &["claude"]);
        assert_eq!(version_of(&observed), None);
    }
}
