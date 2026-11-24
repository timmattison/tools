//! Runs git for the sidecar tool.
//!
//! Each git command sheds the git environment that a hook leaves behind. A CI token
//! reaches git as a configuration header and never as an argument.

use std::ffi::OsStr;
use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// The program that this module runs.
const GIT: &str = "git";

/// The name of the main branch of a sidecar.
pub const MAIN_BRANCH: &str = "main";

/// The git configuration key that holds the authorization header for GitHub over HTTPS.
pub const TOKEN_HEADER_KEY: &str = "http.https://github.com/.extraheader";

/// The user name that GitHub expects with an access token.
pub const TOKEN_USER: &str = "x-access-token";

/// The CI token. The value leaves this module only as the base64 header value.
/// `Debug` prints `Token(<redacted>)`. The type has no `Display`.
pub struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token(<redacted>)")
    }
}

impl Token {
    /// Reads the token from the environment variable `variable`.
    /// Returns `None` when the variable is not set or is empty.
    #[must_use]
    pub fn from_env(variable: &str) -> Option<Token> {
        Self::from_value(std::env::var(variable).ok())
    }

    /// Makes a token from the value of a variable. An absent or empty value gives `None`.
    fn from_value(value: Option<String>) -> Option<Token> {
        value.filter(|v| !v.is_empty()).map(Token)
    }
}

/// The result of one git command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitResult {
    /// True when git started and exited with status 0.
    pub ok: bool,
    /// The standard output, trimmed.
    pub stdout: String,
    /// The standard error, with its lines joined by one space and trimmed.
    /// When git cannot start, this holds the text of the spawn error.
    pub stderr: String,
}

/// Runs git commands, with a token when the caller has one.
#[derive(Debug)]
pub struct Git {
    token: Option<Token>,
    token_env: Option<String>,
}

impl Git {
    /// Makes a runner that sends no token.
    #[must_use]
    pub fn without_token() -> Git {
        Git {
            token: None,
            token_env: None,
        }
    }

    /// Makes a runner that reads the token from the variable `token_env` now.
    /// The variable stays out of the environment of each git command.
    #[must_use]
    pub fn with_token_from(token_env: &str) -> Git {
        Git {
            token: Token::from_env(token_env),
            token_env: Some(token_env.to_string()),
        }
    }

    /// Makes a git command for the directory `cwd`. The caller adds the subcommand.
    #[must_use]
    pub fn command(&self, cwd: &Path) -> Command {
        command_with(
            childenv::command(GIT),
            cwd,
            self.token.as_ref(),
            self.token_env.as_deref(),
        )
    }

    /// Runs git with `args` in `cwd` and collects the result. This function never panics.
    pub fn run<I, S>(&self, cwd: &Path, args: I) -> GitResult
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(cwd);
        command.args(args);
        match command.output() {
            Ok(output) => GitResult {
                ok: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                stderr: String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string(),
            },
            Err(error) => GitResult {
                ok: false,
                stdout: String::new(),
                stderr: error.to_string(),
            },
        }
    }
}

/// Builds the git command. `command` arrives with the sweep of the inherited
/// git environment done. The steps are in this order:
///
/// 1. The directory and a closed standard input.
/// 2. The removal of the token variable.
/// 3. The token as `GIT_CONFIG_*` variables, set after the sweep.
/// 4. The arguments `-c color.ui=never`.
pub(crate) fn command_with(
    mut command: Command,
    cwd: &Path,
    token: Option<&Token>,
    token_env: Option<&str>,
) -> Command {
    command.current_dir(cwd).stdin(Stdio::null());
    if let Some(variable) = token_env {
        command.env_remove(variable);
    }
    if let Some(token) = token {
        let encoded = STANDARD.encode(format!("{TOKEN_USER}:{}", token.0));
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", TOKEN_HEADER_KEY)
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("AUTHORIZATION: basic {encoded}"),
            );
    }
    command.args(["-c", "color.ui=never"]);
    command
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const SECRET: &str = "s3cr3t";
    const TOKEN_VARIABLE: &str = "SIDECAR_TEST_TOKEN_VARIABLE";
    const HEADER_VALUE: &str = "AUTHORIZATION: basic eC1hY2Nlc3MtdG9rZW46czNjcjN0";

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A command that has the sweep of a fixed list of keys, so that a test does
    /// not depend on the environment of this process.
    fn synthetic_sweep() -> Command {
        #[expect(
            clippy::disallowed_methods,
            reason = "this helper measures a fixed list of keys, so the command must not hold the sweep of the real environment"
        )]
        let mut command = Command::new(GIT);
        childenv::shed_git_environment_from(
            &mut command,
            [
                "GIT_DIR",
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_KEY_0",
                "GIT_CONFIG_VALUE_0",
                "GIT_SSH_COMMAND",
                "PATH",
            ],
            childenv::InheritedGitEnvironment::KeepUserIntent,
        );
        command
    }

    /// What a command does with one environment variable.
    #[derive(Debug, PartialEq, Eq)]
    enum EnvState {
        /// The command does not mention the variable.
        Absent,
        /// The command removes the variable.
        Removed,
        /// The command sets the variable to this value.
        Set(String),
    }

    fn set(value: &str) -> EnvState {
        EnvState::Set(value.to_string())
    }

    fn env_of(command: &Command, key: &str) -> EnvState {
        command
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(key))
            .map_or(EnvState::Absent, |(_, value)| {
                value.map_or(EnvState::Removed, |v| {
                    EnvState::Set(v.to_string_lossy().into_owned())
                })
            })
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the epoch")
            .as_nanos();
        let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "sidecar-git-{label}-{}-{nanos}-{counter}",
            std::process::id()
        ))
    }

    fn init_repository(dir: &Path) {
        std::fs::create_dir_all(dir).expect("make the fixture directory");
        let mut command = childenv::fixture_command("git");
        let status = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .status()
            .expect("git starts");
        assert!(status.success());
    }

    #[test]
    fn constants_have_the_design_values() {
        assert_eq!(MAIN_BRANCH, "main");
        assert_eq!(TOKEN_HEADER_KEY, "http.https://github.com/.extraheader");
        assert_eq!(TOKEN_USER, "x-access-token");
    }

    #[test]
    fn builder_sets_the_token_header_after_the_sweep() {
        let token = Token(SECRET.to_string());
        let command = command_with(
            synthetic_sweep(),
            Path::new("."),
            Some(&token),
            Some(TOKEN_VARIABLE),
        );

        assert_eq!(env_of(&command, "GIT_DIR"), EnvState::Removed);
        assert_eq!(env_of(&command, "GIT_SSH_COMMAND"), EnvState::Absent);
        assert_eq!(env_of(&command, "GIT_CONFIG_COUNT"), set("1"));
        assert_eq!(env_of(&command, "GIT_CONFIG_KEY_0"), set(TOKEN_HEADER_KEY));
        let encoded = STANDARD.encode(format!("{TOKEN_USER}:{SECRET}"));
        let expected = format!("AUTHORIZATION: basic {encoded}");
        assert_eq!(expected, HEADER_VALUE);
        assert_eq!(env_of(&command, "GIT_CONFIG_VALUE_0"), set(HEADER_VALUE));
        assert_eq!(env_of(&command, TOKEN_VARIABLE), EnvState::Removed);
    }

    #[test]
    fn builder_puts_no_token_in_the_arguments() {
        let token = Token(SECRET.to_string());
        let command = command_with(synthetic_sweep(), Path::new("."), Some(&token), None);
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args[..2], ["-c", "color.ui=never"]);
        assert!(args.iter().all(|a| !a.contains(SECRET)));
    }

    #[test]
    fn builder_without_a_token_sets_no_config_count() {
        let command = command_with(synthetic_sweep(), Path::new("."), None, None);

        // The sweep removes the variable, and the builder does not set it again.
        assert_eq!(env_of(&command, "GIT_CONFIG_COUNT"), EnvState::Removed);
        assert_eq!(env_of(&command, "GIT_CONFIG_KEY_0"), EnvState::Removed);
        assert_eq!(env_of(&command, "GIT_CONFIG_VALUE_0"), EnvState::Removed);
    }

    #[test]
    fn token_debug_does_not_show_the_value() {
        let token = Token(SECRET.to_string());
        let shown = format!("{token:?}");

        assert!(!shown.contains(SECRET));
        assert_eq!(shown, "Token(<redacted>)");
    }

    #[test]
    fn token_from_absent_value_is_none() {
        assert!(Token::from_value(None).is_none());
    }

    #[test]
    fn token_from_empty_value_is_none() {
        assert!(Token::from_value(Some(String::new())).is_none());
    }

    #[test]
    fn token_from_set_value_is_some() {
        let token = Token::from_value(Some(SECRET.to_string())).expect("a token");
        assert_eq!(token.0, SECRET);
    }

    #[test]
    fn git_with_a_token_variable_removes_it_from_the_command() {
        let git = Git {
            token: Some(Token(SECRET.to_string())),
            token_env: Some(TOKEN_VARIABLE.to_string()),
        };
        let command = git.command(Path::new("."));

        assert_eq!(env_of(&command, TOKEN_VARIABLE), EnvState::Removed);
        assert_eq!(env_of(&command, "GIT_CONFIG_VALUE_0"), set(HEADER_VALUE));
    }

    #[test]
    fn run_finds_the_top_level_of_a_repository() {
        let dir = temp_dir("top");
        init_repository(&dir);

        let result = Git::without_token().run(&dir, ["rev-parse", "--show-toplevel"]);
        let top = std::fs::canonicalize(&result.stdout).expect("the top level exists");
        let expected = std::fs::canonicalize(&dir).expect("the fixture exists");
        std::fs::remove_dir_all(&dir).expect("remove the fixture");

        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(top, expected);
        assert_eq!(result.stdout, result.stdout.trim());
    }

    #[test]
    fn run_reports_a_failing_command() {
        let dir = temp_dir("fail");
        init_repository(&dir);

        let result =
            Git::without_token().run(&dir, ["rev-parse", "--verify", "--quiet", "nosuchref"]);
        std::fs::remove_dir_all(&dir).expect("remove the fixture");

        assert!(!result.ok);
    }

    #[test]
    fn run_reports_the_spawn_error_when_the_directory_is_missing() {
        let dir = temp_dir("missing");

        let result = Git::without_token().run(&dir, ["status"]);

        assert!(!result.ok);
        assert!(!result.stderr.is_empty());
    }

    #[test]
    fn run_joins_the_lines_of_standard_error_with_one_space() {
        let dir = temp_dir("lines");
        init_repository(&dir);

        let result = Git::without_token().run(&dir, ["rev-parse", "--verify", "nosuchref"]);
        std::fs::remove_dir_all(&dir).expect("remove the fixture");

        assert!(!result.ok);
        assert!(!result.stderr.is_empty());
        assert!(!result.stderr.contains('\n'));
    }
}
