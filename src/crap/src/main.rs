//! crap — "Claude, Resume Anywhere Please".
//!
//! Resume a Claude Code session from whatever directory it was originally
//! started in, no matter where you are now. Given a session id, `crap` looks
//! up that session under `~/.claude/projects`, recovers the directory it ran
//! in, changes into it, and re-launches Claude with `--resume <id>`.
//!
//! Because a binary cannot change its parent shell's working directory (nor see
//! shell aliases such as `clauded`), the user-facing `crap` command is a shell
//! function installed via `crap --shell-setup`. This binary's job is to resolve
//! a session id to its original directory and print that path to stdout; the
//! shell function performs the `cd` and runs `clauded`/`claude` from there.

use std::path::{Path, PathBuf};
use std::process::exit;

use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use shellsetup::ShellIntegration;

/// Exit codes for the different failure conditions.
mod exit_codes {
    /// No session file matched the given id.
    pub const SESSION_NOT_FOUND: i32 = 1;
    /// The session file had no recorded working directory.
    pub const NO_CWD_IN_SESSION: i32 = 2;
    /// The recorded working directory no longer exists.
    pub const DIRECTORY_MISSING: i32 = 3;
    /// The session id was not a valid filename component.
    pub const INVALID_SESSION_ID: i32 = 4;
    /// Shell setup failed.
    pub const SHELL_SETUP_ERROR: i32 = 5;
    /// The user's home directory could not be determined.
    pub const NO_HOME_DIR: i32 = 6;
}

/// Why a session id could not be resolved to an existing directory.
#[derive(Debug)]
enum ResolveError {
    /// The session id contained path separators or traversal sequences.
    InvalidSessionId,
    /// No `<session_id>.jsonl` file was found under any project directory.
    SessionNotFound,
    /// The session file exists but records no working directory.
    NoCwdInSession,
    /// The recorded working directory no longer exists on disk.
    DirectoryMissing(PathBuf),
}

/// Returns `true` if `id` is safe to use as a `.jsonl` filename component.
///
/// Rejects empty strings, anything containing a path separator (`/` or `\`),
/// and any traversal sequence (`..`) so a crafted id cannot escape the
/// projects directory.
fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
}

/// Extracts the first non-empty `cwd` value from Claude session JSONL contents.
///
/// Each line is an independent JSON object; the early bookkeeping lines often
/// carry `"cwd": null`, so the first line with a non-empty string `cwd` wins.
/// Non-JSON lines are skipped. Returns `None` if no usable `cwd` is present.
fn extract_cwd(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Locates `<session_id>.jsonl` inside any immediate subdirectory of
/// `projects_dir`, returning its full path if found.
fn find_session_file(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    for entry in std::fs::read_dir(projects_dir).ok()?.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let candidate = entry.path().join(&file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolves a session id to the existing directory the session ran in.
///
/// # Errors
///
/// Returns a [`ResolveError`] when the id is invalid, no session file matches,
/// the session records no working directory, or that directory no longer
/// exists.
fn resolve_session_dir(projects_dir: &Path, session_id: &str) -> Result<PathBuf, ResolveError> {
    if !is_valid_session_id(session_id) {
        return Err(ResolveError::InvalidSessionId);
    }
    let file = find_session_file(projects_dir, session_id).ok_or(ResolveError::SessionNotFound)?;
    let contents = std::fs::read_to_string(&file).map_err(|_| ResolveError::SessionNotFound)?;
    let cwd = extract_cwd(&contents).ok_or(ResolveError::NoCwdInSession)?;
    let path = PathBuf::from(cwd);
    if !path.is_dir() {
        return Err(ResolveError::DirectoryMissing(path));
    }
    Ok(path)
}

/// Returns the `~/.claude/projects` directory, or `None` if the home directory
/// cannot be determined.
fn claude_projects_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("projects"))
}

/// The shell function installed by `crap --shell-setup`.
///
/// `crap` shadows the binary, so the function reaches the binary explicitly via
/// `command crap` to resolve the session's directory. It then `cd`s there and
/// re-launches Claude. `clauded` is resolved through `eval` so that an alias of
/// that name is expanded at call time (shell aliases are otherwise not expanded
/// inside function bodies); if no `clauded` exists, plain `claude` is used.
const SHELL_CODE: &str = r#"
function crap() {
    if [ "$#" -eq 0 ]; then
        command crap
        return $?
    fi
    local __crap_session="$1"
    local __crap_dir
    __crap_dir=$(command crap "$__crap_session") || return $?
    cd "$__crap_dir" || return 1
    if command -v clauded >/dev/null 2>&1; then
        eval 'clauded --resume "$__crap_session"'
    else
        claude --resume "$__crap_session"
    fi
}
"#;

/// Installs the `crap` shell function into the user's shell config.
fn setup_shell_integration() -> Result<(), shellsetup::ShellSetupError> {
    let integration = ShellIntegration::new(
        "crap",
        "Claude, Resume Anywhere Please",
        SHELL_CODE,
    )
    .with_command("crap", "Resume a Claude session from its original directory");
    integration.setup()
}

/// Command-line interface for `crap`.
#[derive(Parser)]
#[command(name = "crap")]
#[command(
    about = "Claude, Resume Anywhere Please — resume a Claude session from its original directory"
)]
#[command(version = version_string!())]
struct Cli {
    /// The Claude session id to resume (the `.jsonl` filename under
    /// `~/.claude/projects`).
    #[arg(value_name = "SESSION_ID", required_unless_present = "shell_setup")]
    session_id: Option<String>,

    /// Install the `crap` shell function into your shell config, then exit.
    ///
    /// Run this once: `crap --shell-setup`. After re-sourcing your shell,
    /// `crap <session-id>` will cd into the session's directory and resume it.
    #[arg(long)]
    shell_setup: bool,
}

fn main() {
    let cli = Cli::parse();

    if cli.shell_setup {
        match setup_shell_integration() {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
                exit(exit_codes::SHELL_SETUP_ERROR);
            }
        }
    }

    // `required_unless_present = "shell_setup"` guarantees this is present here.
    let session_id = cli.session_id.expect("session id is required without --shell-setup");

    let Some(projects_dir) = claude_projects_dir() else {
        eprintln!(
            "{} could not determine your home directory",
            "Error:".red().bold()
        );
        exit(exit_codes::NO_HOME_DIR);
    };

    match resolve_session_dir(&projects_dir, &session_id) {
        Ok(dir) => println!("{}", dir.display()),
        Err(ResolveError::InvalidSessionId) => {
            eprintln!(
                "{} '{session_id}' is not a valid session id",
                "Error:".red().bold()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
        Err(ResolveError::SessionNotFound) => {
            eprintln!(
                "{} no Claude session found with id '{session_id}'",
                "Error:".red().bold()
            );
            eprintln!("       looked under {}", projects_dir.display());
            exit(exit_codes::SESSION_NOT_FOUND);
        }
        Err(ResolveError::NoCwdInSession) => {
            eprintln!(
                "{} session '{session_id}' has no recorded working directory",
                "Error:".red().bold()
            );
            exit(exit_codes::NO_CWD_IN_SESSION);
        }
        Err(ResolveError::DirectoryMissing(path)) => {
            eprintln!(
                "{} the directory for session '{session_id}' no longer exists:",
                "Error:".red().bold()
            );
            eprintln!("       {}", path.display());
            exit(exit_codes::DIRECTORY_MISSING);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// A representative session id used across tests.
    const SAMPLE_ID: &str = "11111111-2222-3333-4444-555555555555";

    /// Builds a single JSONL line recording `cwd`.
    fn cwd_line(cwd: &str) -> String {
        format!("{}\n", serde_json::json!({ "cwd": cwd }))
    }

    #[test]
    fn extract_cwd_returns_first_non_null_cwd() {
        let contents = format!(
            "{}{}{}{}",
            "{\"type\":\"summary\"}\n",
            "{\"cwd\":null}\n",
            cwd_line("/Users/tim/code/foo"),
            cwd_line("/Users/tim/code/bar"),
        );
        assert_eq!(
            extract_cwd(&contents).as_deref(),
            Some("/Users/tim/code/foo")
        );
    }

    #[test]
    fn extract_cwd_skips_non_json_and_missing_or_empty() {
        let contents = "not json at all\n{\"foo\":1}\n{\"cwd\":\"\"}\n";
        assert_eq!(extract_cwd(contents), None);
    }

    #[test]
    fn extract_cwd_empty_input_is_none() {
        assert_eq!(extract_cwd(""), None);
    }

    #[test]
    fn extract_cwd_handles_multibyte_paths() {
        let contents = cwd_line("/Users/tim/コード/café");
        assert_eq!(
            extract_cwd(&contents).as_deref(),
            Some("/Users/tim/コード/café")
        );
    }

    #[test]
    fn valid_session_id_accepts_uuid() {
        assert!(is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e1901"));
    }

    #[test]
    fn valid_session_id_rejects_traversal_and_separators() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id(".."));
        assert!(!is_valid_session_id("../etc/passwd"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("a\\b"));
    }

    #[test]
    fn find_session_file_locates_jsonl_in_project_subdir() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let proj = projects.join("-Users-tim-code-foo");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{}\n").unwrap();

        assert_eq!(find_session_file(projects, SAMPLE_ID), Some(file));
    }

    #[test]
    fn find_session_file_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        assert_eq!(find_session_file(dir.path(), SAMPLE_ID), None);
    }

    #[test]
    fn resolve_rejects_invalid_id() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_session_dir(dir.path(), "../escape"),
            Err(ResolveError::InvalidSessionId)
        ));
    }

    #[test]
    fn resolve_errors_when_session_missing() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_session_dir(dir.path(), SAMPLE_ID),
            Err(ResolveError::SessionNotFound)
        ));
    }

    #[test]
    fn resolve_errors_when_no_cwd_in_session() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join(format!("{SAMPLE_ID}.jsonl")), "{\"cwd\":null}\n").unwrap();

        assert!(matches!(
            resolve_session_dir(dir.path(), SAMPLE_ID),
            Err(ResolveError::NoCwdInSession)
        ));
    }

    #[test]
    fn resolve_errors_when_directory_missing() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let missing = dir.path().join("gone");
        fs::write(
            proj.join(format!("{SAMPLE_ID}.jsonl")),
            cwd_line(missing.to_str().unwrap()),
        )
        .unwrap();

        match resolve_session_dir(dir.path(), SAMPLE_ID) {
            Err(ResolveError::DirectoryMissing(path)) => assert_eq!(path, missing),
            other => panic!("expected DirectoryMissing, got {other:?}"),
        }
    }

    #[test]
    fn resolve_returns_existing_directory() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let cwd = dir.path().join("real-cwd");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            proj.join(format!("{SAMPLE_ID}.jsonl")),
            cwd_line(cwd.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(resolve_session_dir(dir.path(), SAMPLE_ID).unwrap(), cwd);
    }

    #[test]
    fn shell_code_defines_function_and_dispatches_to_claude() {
        assert!(SHELL_CODE.contains("function crap()"));
        assert!(SHELL_CODE.contains("command crap"));
        assert!(SHELL_CODE.contains("clauded --resume"));
        assert!(SHELL_CODE.contains("claude --resume"));
    }
}
