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

use std::collections::BTreeMap;

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
        #[arg(long, allow_hyphen_values = true, default_value_t = 0)]
        exit_code: i32,

        /// Verbatim text of the last command line.
        #[arg(long, default_value = "")]
        last_command: String,

        /// Hidden: synthetic subprocess probe for the watchdog acceptance test.
        ///
        /// When set, the recorder skips its normal append path and instead
        /// spawns the provided command (split on whitespace, no shell), wraps
        /// it in the watchdog timeout, and exits 0 regardless of outcome.
        #[arg(long, hide = true)]
        probe_subprocess: Option<String>,
    },
}

/// Stub returning false for every input. Implemented in the green commit.
fn is_redacted(_name: &str) -> bool {
    false
}

/// Stub: returns env unchanged with empty `redacted_keys`. Implemented in green.
fn redact_env(
    env: BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    (env, Vec::new())
}

/// Maximum number of consecutive `tsm record` failures before the precmd
/// hook self-disables for the current shell.
const FAILURE_THRESHOLD: u32 = 3;

/// Generate the zsh `eval`-able shell-init snippet.
///
/// The emitted snippet, when sourced via `eval "$(tsm shell-init zsh)"`:
///
/// 1. Bails out silently if `$TSM_DISABLE` is set in the environment.
/// 2. Runs `tsm --version` as a health check with a ~1 second timeout. On
///    failure it prints one warning to stderr and returns without installing
///    the hook.
/// 3. Exports `$TSM_SESSION_ID`. The id is inlined into the snippet at print
///    time (one fresh id per `tsm shell-init zsh` invocation). If the env var
///    is already set on entry, the existing value is preserved.
/// 4. Registers a `precmd` hook that backgrounds `tsm record &!`, passing
///    `--exit-code` and `--last-command`. Before backgrounding, the hook
///    reads `$XDG_STATE_HOME/tsm/fail-count.<shell-pid>`; if the count has
///    reached the failure threshold it unregisters itself and emits one
///    warning line pointing at the error log.
/// 5. Does NOT emit any OSC pane-title sequences; pane-title work is deferred
///    to a later slice.
fn generate_zsh_snippet(session_id: &SessionId) -> String {
    let id = session_id.as_hex();
    let threshold = FAILURE_THRESHOLD;
    format!(
        r#"# === tsm shell integration (begin) ===
if [[ -n "${{TSM_DISABLE-}}" ]]; then
    return 0 2>/dev/null || exit 0
fi

# Health check: tsm --version with a ~1 second timeout. macOS does not ship
# GNU timeout(1), so we implement the timeout inline by backgrounding the
# health check, racing it against a sleep, and killing the loser.
_tsm_health_check() {{
    local _pid _watchdog _rc
    ( tsm --version >/dev/null 2>&1 ) &
    _pid=$!
    ( sleep 1 && kill -TERM $_pid 2>/dev/null ) &!
    _watchdog=$!
    if wait $_pid 2>/dev/null; then
        _rc=0
    else
        _rc=$?
    fi
    kill $_watchdog 2>/dev/null
    return $_rc
}}
if ! _tsm_health_check; then
    unset -f _tsm_health_check
    print -u2 "tsm: health check failed; tsm functionality disabled for this shell"
    return 0 2>/dev/null || exit 0
fi
unset -f _tsm_health_check

# Export session id. The literal hex below is generated fresh by `tsm shell-init`
# at print time, so each `eval "$(tsm shell-init zsh)"` yields a new id. If
# TSM_SESSION_ID is already set in the environment (parent shell exported it),
# the existing value is preserved.
: ${{TSM_SESSION_ID:={id}}}
export TSM_SESSION_ID

typeset -g _tsm_state_dir="${{XDG_STATE_HOME:-$HOME/.local/state}}/tsm"
typeset -g _tsm_fail_file="${{_tsm_state_dir}}/fail-count.$$"
typeset -g _tsm_error_log="${{_tsm_state_dir}}/errors.log"

_tsm_precmd() {{
    local _last_exit=$?
    local _last_cmd
    _last_cmd=$(fc -ln -1 2>/dev/null) || _last_cmd=""

    # Self-disable after {threshold} consecutive failures. The recorder writes
    # this counter; we read it synchronously before dispatching the next call.
    if [[ -r "$_tsm_fail_file" ]]; then
        local _fc
        _fc=$(<"$_tsm_fail_file")
        if [[ "$_fc" -ge {threshold} ]]; then
            add-zsh-hook -d precmd _tsm_precmd
            rm -f "$_tsm_fail_file"
            print -u2 "tsm: precmd hook disabled after {threshold} consecutive failures; see ${{_tsm_error_log}}"
            return 0
        fi
    fi

    # Dispatch recorder in the background and disown so the parent shell never
    # waits and the prompt is not delayed.
    {{ tsm record --exit-code "$_last_exit" --last-command "$_last_cmd" }} &!
}}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _tsm_precmd
# === tsm shell integration (end) ===
"#
    )
}

fn main() {
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
        Commands::Record {
            exit_code: _,
            last_command: _,
            probe_subprocess: _,
        } => {
            // Stub for this slice. The green commit replaces this body with the
            // real recorder. We intentionally do not write anything to stderr
            // (per the safety policy that `tsm record` never writes to stderr
            // from the hot path).
        }
    }
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

    // ----- redaction unit tests (red commit will see these fail against stubs) -----

    #[test]
    fn is_redacted_matches_suffix_patterns() {
        assert!(is_redacted("AWS_SESSION_TOKEN"));
        assert!(is_redacted("MY_API_KEY"));
        assert!(is_redacted("DB_PASSWORD"));
        assert!(is_redacted("OPENAI_API_KEY"));
    }

    #[test]
    fn is_redacted_case_insensitive() {
        assert!(is_redacted("aws_session_token"));
        assert!(is_redacted("github_token"));
        assert!(is_redacted("my_secret"));
    }

    #[test]
    fn is_redacted_does_not_match_plain_names() {
        assert!(!is_redacted("HOME"));
        assert!(!is_redacted("PATH"));
        assert!(!is_redacted("USER"));
        assert!(!is_redacted("SHELL"));
    }

    #[test]
    fn is_redacted_prefix_aws() {
        assert!(is_redacted("AWS_REGION"));
        assert!(is_redacted("AWS_PROFILE"));
        assert!(is_redacted("AWS_DEFAULT_REGION"));
    }

    #[test]
    fn is_redacted_prefix_op() {
        assert!(is_redacted("OP_SESSION"));
        assert!(is_redacted("OP_DEVICE"));
    }

    #[test]
    fn is_redacted_anthropic_exact() {
        assert!(is_redacted("ANTHROPIC_API_KEY"));
        assert!(is_redacted("GH_TOKEN"));
        assert!(is_redacted("GITHUB_TOKEN"));
    }

    #[test]
    fn is_redacted_claude_prefix() {
        assert!(is_redacted("CLAUDE_CODE_THING"));
        assert!(is_redacted("CLAUDE_API_KEY"));
    }

    #[test]
    fn redact_env_filters_keys_and_keeps_redacted_keys_list() {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/x".to_string());
        env.insert("AWS_SESSION_TOKEN".to_string(), "secret1".to_string());
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        env.insert("MY_API_KEY".to_string(), "secret2".to_string());

        let (filtered, redacted_keys) = redact_env(env);

        assert!(filtered.contains_key("HOME"));
        assert!(filtered.contains_key("PATH"));
        assert!(!filtered.contains_key("AWS_SESSION_TOKEN"));
        assert!(!filtered.contains_key("MY_API_KEY"));
        assert!(redacted_keys.contains(&"AWS_SESSION_TOKEN".to_string()));
        assert!(redacted_keys.contains(&"MY_API_KEY".to_string()));
        assert_eq!(redacted_keys.len(), 2);
    }

    #[test]
    fn redact_env_redacted_keys_is_sorted() {
        let mut env = BTreeMap::new();
        env.insert("Z_TOKEN".to_string(), "v".to_string());
        env.insert("A_SECRET".to_string(), "v".to_string());
        env.insert("M_PASSWORD".to_string(), "v".to_string());
        env.insert("HOME".to_string(), "v".to_string());

        let (_filtered, redacted_keys) = redact_env(env);
        let mut sorted = redacted_keys.clone();
        sorted.sort();
        assert_eq!(redacted_keys, sorted, "redacted_keys must be sorted");
        assert_eq!(redacted_keys.len(), 3);
    }
}
