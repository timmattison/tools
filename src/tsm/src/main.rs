//! `tsm` - terminal session manager.
//!
//! This binary is the entry point for the tsm tool. It currently provides
//! three subcommands:
//!
//! - `tsm --version` prints `tsm <version> (<short-hash>, clean|dirty)`.
//! - `tsm shell-init <shell>` emits an eval-able shell snippet that wires up
//!   a per-command precmd hook calling `tsm record` in the background.
//! - `tsm record` is the precmd-time recorder; in this slice it is a stub
//!   that accepts the same args the snippet will pass.

use buildinfo::version_string;
use clap::{Parser, Subcommand};
use tsm_id::SessionId;

/// Exit code returned when `tsm shell-init` receives an unsupported shell.
const EXIT_UNSUPPORTED_SHELL: i32 = 2;

/// Top-level CLI for `tsm`.
#[derive(Parser)]
#[command(name = "tsm")]
#[command(about = "Terminal session manager")]
#[command(version = version_string!())]
struct Cli {
    /// The subcommand to invoke.
    #[command(subcommand)]
    command: Commands,
}

/// All `tsm` subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Emit an eval-able shell snippet that wires up the precmd hook.
    ShellInit {
        /// The shell to emit integration for. Only `zsh` is supported in v1.
        shell: String,
    },
    /// Record one command's metadata. Invoked by the shell precmd hook.
    Record {
        /// Exit status of the last command in the shell.
        #[arg(long, allow_hyphen_values = true)]
        exit_code: i32,

        /// Verbatim text of the last command line.
        #[arg(long, default_value = "")]
        last_command: String,
    },
}

/// Generate the zsh `eval`-able shell-init snippet.
///
/// In the red commit this returns an obviously-broken placeholder so the
/// behavioral test suite fails. The green commit replaces the body with the
/// actual snippet.
fn generate_zsh_snippet(_session_id: &SessionId) -> String {
    String::from("# tsm placeholder\n")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::ShellInit { shell } => {
            if shell != "zsh" {
                eprintln!(
                    "tsm: shell-init: only \"zsh\" is supported in v1, got: {shell}"
                );
                std::process::exit(EXIT_UNSUPPORTED_SHELL);
            }
            let session_id = SessionId::random();
            print!("{}", generate_zsh_snippet(&session_id));
        }
        Commands::Record { exit_code: _, last_command: _ } => {
            // Stub for this slice. Subagent 4 will replace this body with the
            // real recorder. We intentionally do not write anything to stderr
            // (per the safety policy that `tsm record` never writes to stderr
            // from the hot path).
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fixed session ID for deterministic snippet inspection.
    fn fixed_session_id() -> SessionId {
        SessionId::from_hex("0123456789abcdef0123456789abcdef")
            .expect("fixed hex is valid")
    }

    #[test]
    fn zsh_snippet_starts_with_tsm_disable_guard() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("TSM_DISABLE"),
            "snippet should guard on $TSM_DISABLE near the top, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_inlines_session_id() {
        let id = fixed_session_id();
        let s = generate_zsh_snippet(&id);
        let hex = id.as_hex();
        let occurrences = s.matches(hex).count();
        assert_eq!(
            occurrences, 1,
            "expected session id {hex} to appear exactly once in snippet, got {occurrences}:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_uses_random_id_when_called_via_main() {
        let a = generate_zsh_snippet(&SessionId::random());
        let b = generate_zsh_snippet(&SessionId::random());
        assert_ne!(
            a, b,
            "two snippets generated with random ids should differ"
        );
    }

    #[test]
    fn zsh_snippet_uses_disowned_call() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("&!"),
            "snippet should background-disown the tsm record call with &!, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_passes_exit_code_and_last_command() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("--exit-code"),
            "snippet should pass --exit-code to tsm record, got:\n{s}"
        );
        assert!(
            s.contains("--last-command"),
            "snippet should pass --last-command to tsm record, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_registers_precmd_hook() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("add-zsh-hook precmd"),
            "snippet should register the precmd hook via add-zsh-hook, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_health_check_present() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("tsm --version"),
            "snippet should perform a health check by calling tsm --version, got:\n{s}"
        );
        assert!(
            s.contains("sleep 1") || s.contains("sleep 1\n") || s.contains("sleep 1 "),
            "snippet should bound the health check with a ~1 second timeout, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_self_disables_after_3_failures() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("add-zsh-hook -d precmd"),
            "snippet should unregister the precmd hook on repeated failure, got:\n{s}"
        );
        assert!(
            s.contains('3'),
            "snippet should reference the failure threshold of 3, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_does_not_emit_osc_sequences() {
        let s = generate_zsh_snippet(&fixed_session_id());
        for needle in ["\\033]", "\\x1b]", "\\e]"] {
            assert!(
                !s.contains(needle),
                "snippet should not emit OSC sequences (found {needle}), got:\n{s}"
            );
        }
    }

    #[test]
    fn zsh_snippet_uses_xdg_state_home_fallback() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("${XDG_STATE_HOME:-$HOME/.local/state}"),
            "snippet should compute state dir via XDG_STATE_HOME fallback, got:\n{s}"
        );
    }
}
