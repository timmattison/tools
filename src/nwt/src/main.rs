use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{exit, Command, ExitStatus, Stdio};
use std::thread;

use buildinfo::version_string;
use clap::Parser;
use names::Generator;
use repowalker::find_main_repo;
use serde::Deserialize;
use shellsetup::ShellIntegration;
use walkdir::WalkDir;

/// Shell code to be installed by --shell-setup.
///
/// This function wraps the nwt binary and automatically changes to the new worktree
/// directory after creation. When --tmux is specified, it skips the cd since the
/// worktree opens in a new tmux window.
const SHELL_CODE: &str = r#"
function nwt() {
    # If --tmux is specified, don't cd (worktree opens in new tmux window)
    case " $* " in
        *" --tmux "* | *" --tmux")
            command nwt "$@"
            return $?
            ;;
    esac

    # Capture the worktree path and cd to it
    local dir
    dir=$(command nwt "$@")
    local exit_code=$?
    if [ $exit_code -eq 0 ] && [ -n "$dir" ] && [ -d "$dir" ]; then
        echo "$dir"
        cd "$dir" || return 1
    else
        [ -n "$dir" ] && echo "$dir"
        return $exit_code
    fi
}
"#;

/// Returns the default value for copy_env (true).
fn default_copy_env() -> bool {
    true
}

/// Configuration file schema for nwt.
///
/// All fields are optional - only set what you need to override defaults.
/// The config file is loaded from `~/.nwt.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NwtConfig {
    /// Default branch name (conflicts with checkout if both set).
    branch: Option<String>,

    /// Default checkout ref (conflicts with branch if both set).
    checkout: Option<String>,

    /// Copy untracked .env files from main worktree to new worktree.
    /// Defaults to true if not specified.
    #[serde(default = "default_copy_env")]
    copy_env: bool,

    /// Enable quiet mode by default.
    #[serde(default)]
    quiet: bool,

    /// Default command to run after worktree creation.
    run: Option<String>,

    /// Open worktree in tmux by default.
    #[serde(default)]
    tmux: bool,
}

impl Default for NwtConfig {
    fn default() -> Self {
        Self {
            branch: None,
            checkout: None,
            copy_env: true, // Copy .env files by default
            quiet: false,
            run: None,
            tmux: false,
        }
    }
}

/// Errors that can occur when loading or validating the config file.
#[derive(Debug)]
enum ConfigError {
    /// Failed to read the config file.
    Io(std::io::Error),
    /// Failed to parse the TOML content.
    Parse(toml::de::Error),
    /// Config validation failed (e.g., conflicting options).
    Validation(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "Failed to read config file: {}", e),
            ConfigError::Parse(e) => write!(f, "Failed to parse config file: {}", e),
            ConfigError::Validation(msg) => write!(f, "Config validation error: {}", msg),
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Parse(e)
    }
}

/// Final merged configuration from CLI arguments and config file.
///
/// CLI arguments take precedence over config file values.
#[derive(Debug)]
struct MergedConfig {
    branch: Option<String>,
    checkout: Option<String>,
    copy_env: bool,
    quiet: bool,
    run: Option<String>,
    tmux: bool,
}

/// Returns the path to the config file.
///
/// The config file is located at `~/.nwt.toml`.
fn get_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".nwt.toml"))
}

/// Loads the config file from `~/.nwt.toml`.
///
/// Returns `Ok(None)` if the config file doesn't exist (not an error).
/// Returns `Err` for invalid TOML, IO errors (other than not found), or validation failures.
fn load_config() -> Result<Option<NwtConfig>, ConfigError> {
    let config_path = match get_config_path() {
        Some(path) => path,
        None => return Ok(None), // No home directory, no config
    };

    if !config_path.exists() {
        return Ok(None); // No config file is OK
    }

    let contents = fs::read_to_string(&config_path)?;
    let config: NwtConfig = toml::from_str(&contents)?;

    validate_config(&config)?;

    Ok(Some(config))
}

/// Validates the config file for conflicting options.
fn validate_config(config: &NwtConfig) -> Result<(), ConfigError> {
    if config.branch.is_some() && config.checkout.is_some() {
        return Err(ConfigError::Validation(
            "branch and checkout cannot both be set in config file".to_string(),
        ));
    }
    Ok(())
}

/// Merges CLI arguments with config file values.
///
/// CLI arguments always take precedence over config file values.
///
/// # Boolean Flag Merging Design Decision
///
/// Boolean flags (`quiet`, `tmux`) use OR logic: `cli.flag || config.flag`. This means:
/// - If CLI specifies `--quiet` or `--tmux`, the flag is enabled (CLI wins).
/// - If CLI doesn't specify the flag, the config file value is used.
/// - **Limitation**: Users cannot disable a config file's `true` value from CLI.
///
/// This is an intentional design choice, not a bug:
/// 1. **Standard CLI convention**: Most tools (git, docker, etc.) use this pattern.
///    Adding `--no-quiet`/`--no-tmux` flags adds CLI complexity for a rare use case.
/// 2. **Simple mental model**: "CLI flags enable features" is easier to understand
///    than "CLI flags toggle features based on config state".
/// 3. **Workaround exists**: Users who need to temporarily disable a config default
///    can use an empty/different config file, or simply edit their config.
/// 4. **Alternative rejected**: Using `Option<bool>` with `--flag`/`--no-flag` pairs
///    would require clap's `ArgGroup` or custom parsing, adding complexity for edge cases.
fn merge_config(cli: &Cli, config: Option<NwtConfig>) -> MergedConfig {
    let config = config.unwrap_or_default();

    MergedConfig {
        // CLI options override config file values
        branch: cli.branch.clone().or(config.branch),
        checkout: cli.checkout.clone().or(config.checkout),
        // copy_env: config default is true, CLI --no-copy-env disables it.
        // If CLI specifies --no-copy-env, we disable. Otherwise use config value.
        copy_env: !cli.no_copy_env && config.copy_env,
        // Boolean flags use OR: CLI can enable but not disable config defaults.
        // See function-level doc comment for rationale.
        quiet: cli.quiet || config.quiet,
        run: cli.run.clone().or(config.run),
        tmux: cli.tmux || config.tmux,
    }
}

/// Result of running a shell command.
#[derive(Debug)]
enum ShellCommandResult {
    /// Command completed successfully
    Success,
    /// Command failed with an exit code
    Failed(i32),
    /// Failed to execute the command
    ExecutionError(std::io::Error),
}

/// Runs a shell command in the specified directory.
///
/// On Unix systems, uses `sh -c` to execute the command.
/// On Windows systems, uses `cmd /C` to execute the command.
///
/// Returns the appropriate exit code, using the bash convention of 128 + signal
/// when a process is killed by a signal on Unix.
///
/// # Platform Support
///
/// This function only supports Unix and Windows platforms. Compilation will fail
/// on other platforms (e.g., wasm32).
fn run_shell_command(cmd: &str, working_dir: &Path) -> ShellCommandResult {
    #[cfg(unix)]
    let result = Command::new("sh")
        .args(["-c", cmd])
        .current_dir(working_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    #[cfg(windows)]
    let result = Command::new("cmd")
        .args(["/C", cmd])
        .current_dir(working_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    #[cfg(not(any(unix, windows)))]
    compile_error!("run_shell_command is only supported on Unix and Windows platforms");

    match result {
        Ok(status) => {
            if status.success() {
                ShellCommandResult::Success
            } else {
                ShellCommandResult::Failed(get_exit_code(status))
            }
        }
        Err(e) => ShellCommandResult::ExecutionError(e),
    }
}

/// Extracts the exit code from an ExitStatus, using bash conventions.
///
/// On Unix, if the process was killed by a signal, returns 128 + signal number.
/// Otherwise returns the exit code, or a default of 1 if neither is available.
fn get_exit_code(status: ExitStatus) -> i32 {
    // First try to get the regular exit code
    if let Some(code) = status.code() {
        return code;
    }

    // On Unix, check if killed by signal (exit code = 128 + signal)
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    // Fallback
    1
}

/// Escapes a string for safe use as a Unix shell argument.
///
/// Wraps the string in single quotes and escapes any embedded single quotes
/// using the `'\''` technique (end quote, escaped quote, start quote).
///
/// # Platform Note
///
/// This function uses Unix single-quote escaping conventions and is only
/// compiled on Unix platforms. It is used by the `--tmux` code path, which
/// is itself Unix-only (tmux is not typically available on Windows).
#[cfg(unix)]
fn shell_escape(s: &str) -> String {
    // Wrap in single quotes and escape any embedded single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Checks if a string contains any ASCII control characters.
///
/// Control characters (0x00-0x1F and 0x7F) can cause unexpected behavior
/// in terminal applications like tmux when used in window names.
///
/// # Why ASCII-only, not full Unicode control characters (U+0080-U+009F)?
///
/// We only check ASCII control characters because:
/// 1. The `names` crate generates only lowercase ASCII letters and hyphens,
///    so Unicode control characters cannot appear in generated names.
/// 2. Even if a future version allowed Unicode, the C1 control characters
///    (U+0080-U+009F) are extremely rare in practice and tmux handles them
///    by displaying replacement characters rather than causing terminal issues.
/// 3. Using `char::is_control()` would add overhead for a theoretical edge case
///    that cannot occur with the current name generator.
fn contains_control_chars(s: &str) -> bool {
    s.bytes().any(|b| b < 0x20 || b == 0x7F)
}

/// Checks if the current process is running inside a tmux session.
///
/// This is determined by the presence of the `TMUX` environment variable,
/// which tmux sets automatically when a shell is spawned inside it.
fn is_running_in_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Exit codes for different failure modes.
///
/// # Exit Code Design Decision: Why --run passes through the command's exit code
///
/// When `--run` is used without `--tmux`, we pass through the command's exit code
/// directly. This means exit codes 1-8 from the user's command will shadow nwt's
/// own error codes. This is intentional:
///
/// 1. **Composability**: Users often chain commands like `nwt --run "make test" && deploy`.
///    Passing through the exit code allows the shell to correctly detect test failures.
///
/// 2. **Precedent**: Tools like `xargs`, `find -exec`, and `docker run` all pass through
///    the child process exit code rather than masking it.
///
/// 3. **Workaround**: Users who need to distinguish nwt errors from command errors can
///    use `--quiet` mode (nwt won't print errors, but the command might) or check stderr.
///
/// 4. **Alternative rejected**: Using exit codes 128+ (like git does for signals) would
///    conflict with the bash convention where 128+N means "killed by signal N". Starting
///    at a higher base (e.g., 200+) would be non-standard and confusing.
///
/// The `RUN_COMMAND_FAILED` (9) exit code is only used when the command fails to *execute*
/// (e.g., shell not found), not when the command runs but returns a non-zero exit code.
mod exit_codes {
    /// Not running inside a git repository
    pub const NOT_IN_REPO: i32 = 1;
    /// Invalid or missing repository name
    pub const INVALID_REPO_NAME: i32 = 2;
    /// Repository has no parent directory
    pub const NO_PARENT_DIR: i32 = 3;
    /// Failed to create worktrees directory
    pub const DIR_CREATE_FAILED: i32 = 4;
    /// Could not find available directory name after max attempts
    pub const NAME_COLLISION: i32 = 5;
    /// Git command failed to execute
    pub const GIT_COMMAND_ERROR: i32 = 6;
    /// Git worktree creation failed
    pub const WORKTREE_FAILED: i32 = 7;
    /// Path contains non-UTF8 characters
    pub const INVALID_PATH_ENCODING: i32 = 8;
    /// Command specified with --run failed to execute (not the command's own exit code)
    pub const RUN_COMMAND_FAILED: i32 = 9;
    /// Tmux command failed to execute
    pub const TMUX_FAILED: i32 = 10;
    // Note: Exit code 11 is reserved (previously INVALID_WINDOW_NAME, now a debug assertion)
    /// Config file error (invalid TOML, validation failed, etc.)
    pub const CONFIG_ERROR: i32 = 12;
    /// Tmux option specified but not running inside tmux
    pub const TMUX_NOT_RUNNING: i32 = 13;
    /// Shell setup failed
    pub const SHELL_SETUP_ERROR: i32 = 14;
}

/// Maximum attempts to find an available directory name before giving up.
/// The `names` crate has ~100 adjectives and ~200 nouns, giving ~20,000 combinations.
/// With 10 attempts, the probability of failure when <2000 worktrees exist is negligible.
const MAX_ATTEMPTS: u32 = 10;

/// Create a new git worktree with a Docker-style random name.
///
/// This tool simplifies creating git worktrees by automatically generating
/// unique directory names and managing the worktree directory structure.
///
/// # Examples
///
/// Create a worktree with a random name and branch:
///     nwt
///
/// Create a worktree with a specific branch name:
///     nwt --branch feature/my-feature
///
/// Checkout an existing branch in a new worktree:
///     nwt --checkout main
#[derive(Parser)]
#[command(name = "nwt")]
#[command(version = version_string!())]
#[command(about = "Create a new git worktree with a Docker-style random name")]
#[command(long_about = "Creates a git worktree in a '{repo-name}-worktrees' directory alongside \
the repository. Generates Docker-style random names (adjective-noun) for both the directory \
and branch unless overridden. Automatically copies untracked .env files from the main \
worktree to preserve development settings.

CONFIGURATION:
    Default values can be set in ~/.nwt.toml. CLI arguments override config values.

    Example config file:
        branch = \"feature/default\"
        copy_env = false  # disable .env file copying
        quiet = false
        tmux = true
        run = \"pnpm install\"

ENV FILE COPYING:
    By default, nwt copies untracked .env files (e.g., .env, .env.local, .env.development)
    from the main worktree to the new worktree, preserving their relative paths. This is
    useful for development settings that shouldn't be committed to git.

    Use --no-copy-env to disable this for a single invocation, or set copy_env = false
    in ~/.nwt.toml to disable it by default.

EXAMPLES:
    nwt                              # Random name for both directory and branch
    nwt -b feature/login             # Custom branch name, random directory
    nwt -c main                      # Checkout existing 'main' branch
    nwt -c v1.0.0                    # Checkout a tag
    nwt --run \"npm install\"          # Run a command after creation
    nwt --tmux                       # Open worktree in a new tmux window
    nwt --tmux --run \"npm install\"   # Run command in a new tmux window
    nwt --no-copy-env                # Skip copying .env files
    nwt --shell-setup                # Install shell integration for auto-cd

SHELL INTEGRATION:
    Run 'nwt --shell-setup' to install a shell function that automatically
    changes to the new worktree directory after creation. The shell function
    skips the cd when --tmux is used (since the worktree opens in a new window).

EXIT CODES:
    0  Success
    1  Not in a git repository
    2  Invalid repository name
    3  Repository has no parent directory
    4  Failed to create worktrees directory
    5  Could not find available directory name
    6  Git command failed to execute
    7  Git worktree creation failed
    8  Path contains non-UTF8 characters
    9  Command specified with --run failed
    10 Tmux command failed
    12 Config file error (invalid TOML, validation failed)
    13 Not running inside tmux (--tmux specified)
    14 Shell setup failed")]
struct Cli {
    /// Specify branch name instead of generating a random one.
    #[arg(short, long, conflicts_with = "checkout")]
    branch: Option<String>,

    /// Checkout a specific ref/commit instead of creating a new branch.
    #[arg(short, long, conflicts_with = "branch")]
    checkout: Option<String>,

    /// Suppress error messages (only output worktree path on success).
    #[arg(short, long)]
    quiet: bool,

    /// Run a command in the worktree directory after creation.
    ///
    /// When used alone, executes via `sh -c` on Unix or `cmd /C` on Windows.
    /// Shell aliases are NOT available in this mode.
    ///
    /// When combined with --tmux, the command runs in an interactive shell
    /// (`$SHELL -ic`), so aliases and shell functions ARE available.
    ///
    /// Exit codes: When --run is used without --tmux, the command's exit code is
    /// passed through directly. This means exit codes 1-8 may shadow nwt's own
    /// error codes. Use --quiet if you need to distinguish command failures from
    /// nwt errors (nwt won't print errors in quiet mode, but the command might).
    /// If the command is killed by a signal, exits with 128 + signal number (bash convention).
    ///
    /// Security: Should only contain trusted input as commands are executed directly.
    #[arg(long)]
    run: Option<String>,

    /// Create a new tmux window for the worktree.
    ///
    /// Opens a new tmux window with the working directory set to the worktree.
    /// The window is named after the worktree directory (e.g., "adjective-noun").
    ///
    /// When combined with --run, the command runs in an interactive shell
    /// (`$SHELL -ic`), so aliases and shell functions are available. This assumes
    /// your shell supports `-i` (interactive) and `-c` (command) flags, which is
    /// true for bash, zsh, fish, and most POSIX-compatible shells.
    ///
    /// Note: tmux is typically only available on Unix systems (Linux, macOS).
    /// This option will fail on Windows unless tmux is installed via WSL or similar.
    #[arg(long)]
    tmux: bool,

    /// Disable copying untracked .env files to the new worktree.
    ///
    /// By default, nwt copies untracked .env files (e.g., .env, .env.local,
    /// .env.development) from the main worktree to the new worktree, preserving
    /// their relative paths. This is useful for development settings that shouldn't
    /// be committed to git.
    ///
    /// Use this flag to disable this behavior for a single invocation, or set
    /// `copy_env = false` in ~/.nwt.toml to disable it by default.
    #[arg(long)]
    no_copy_env: bool,

    /// Install shell integration to automatically cd into new worktrees.
    ///
    /// Adds a shell function to your ~/.zshrc or ~/.bashrc that wraps nwt
    /// and automatically changes directory to the new worktree after creation.
    ///
    /// When --tmux is used, the shell function skips the cd (since the worktree
    /// opens in a new tmux window).
    ///
    /// To activate after installation, run `source ~/.zshrc` (or `~/.bashrc`)
    /// or open a new terminal.
    #[arg(long, conflicts_with_all = ["branch", "checkout", "quiet", "run", "tmux", "no_copy_env"])]
    shell_setup: bool,
}

/// Prints an error message to stderr unless quiet mode is enabled.
macro_rules! error {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}

/// Generates a Docker-style random name in the format "adjective-noun".
///
/// Returns `None` if the generator fails to produce a name (should not happen
/// with the default generator configuration, but handled gracefully).
fn generate_docker_name(generator: &mut Generator) -> Option<String> {
    generator.next()
}

/// Sanitizes a repository name to only allow safe characters.
/// Uses an allowlist approach: only alphanumeric, hyphen, underscore, and dot are permitted.
/// All other characters are replaced with underscores.
fn sanitize_repo_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }

    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Reject names that are just dots (path traversal)
    if sanitized.chars().all(|c| c == '.') {
        return None;
    }

    // Reject names that start with a dot followed by only dots and underscores
    // (could be attempts to create hidden or traversal paths)
    if sanitized.starts_with('.') && sanitized[1..].chars().all(|c| c == '.' || c == '_') {
        return None;
    }

    Some(sanitized)
}

/// Determines the branch name to use based on merged config and generated directory name.
fn get_branch_name<'a>(config: &'a MergedConfig, dir_name: &'a str) -> &'a str {
    config.branch.as_deref().unwrap_or(dir_name)
}

/// Result of attempting to create a worktree.
enum WorktreeResult {
    /// Worktree created successfully
    Success,
    /// Directory path collision (TOCTOU race or pre-existing) - should retry with new name
    PathCollision,
    /// Branch already exists - user should use --checkout
    BranchExists(String),
    /// Ref is already checked out in another worktree
    RefInUse(String),
    /// Other git error
    GitError(String),
    /// Failed to execute git command
    CommandError(std::io::Error),
}

/// Attempts to create a git worktree at the given path.
///
/// Returns a `WorktreeResult` indicating success or the type of failure.
///
/// This function displays git's progress output (e.g., "Updating files: X%") in real-time
/// while also capturing stderr for error classification. This is done by spawning a thread
/// that reads stderr and both echoes it to the terminal and captures it for later analysis.
fn try_create_worktree(
    repo_root: &std::path::Path,
    worktree_path: &str,
    branch_name: &str,
    checkout_ref: Option<&str>,
) -> WorktreeResult {
    let mut cmd = Command::new("git");
    if let Some(ref_name) = checkout_ref {
        cmd.args(["worktree", "add", worktree_path, ref_name]);
    } else {
        cmd.args(["worktree", "add", worktree_path, "-b", branch_name]);
    }

    // Spawn the process with piped stderr so we can both display progress and capture errors.
    // stdin is explicitly closed to prevent hangs if git ever prompts for input.
    let child = cmd
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => return WorktreeResult::CommandError(e),
    };

    // Take ownership of stderr and spawn a thread to read/display/capture it
    let stderr = child.stderr.take().expect("stderr was piped");
    let stderr_thread = thread::spawn(move || {
        let mut stderr = stderr;
        let mut captured = Vec::new();
        let mut buf = [0_u8; 1024];

        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    // Echo to terminal so user sees progress
                    eprint!("{}", String::from_utf8_lossy(&buf[..n]));
                    // Capture for error analysis
                    captured.extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }

        String::from_utf8_lossy(&captured).into_owned()
    });

    // Wait for the process to complete
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => return WorktreeResult::CommandError(e),
    };

    // Get the captured stderr. If the thread panicked, we still want meaningful error handling.
    let stderr = stderr_thread
        .join()
        .unwrap_or_else(|_| "Error: Failed to capture stderr output from git command".to_string());

    if status.success() {
        WorktreeResult::Success
    } else {
        // Check for branch already exists first using git's specific error format.
        // Git says: "fatal: A branch named '<branch>' already exists."
        // We check for "A branch named" specifically to avoid false positives when
        // the path contains the word "branch" (e.g., "/path/to/branch-test/").
        if stderr.contains("A branch named") {
            return WorktreeResult::BranchExists(branch_name.to_string());
        }

        // Check for path/directory collision (TOCTOU race condition)
        // Git says: "fatal: '<path>' already exists"
        if stderr.contains("already exists") {
            return WorktreeResult::PathCollision;
        }

        // Check for ref already checked out
        if stderr.contains("is already used by worktree")
            || stderr.contains("is already checked out")
        {
            let ref_name = checkout_ref.unwrap_or(branch_name);
            return WorktreeResult::RefInUse(ref_name.to_string());
        }

        WorktreeResult::GitError(stderr)
    }
}

/// Installs shell integration for nwt.
///
/// Adds a shell function to ~/.zshrc or ~/.bashrc that wraps the nwt binary
/// and automatically changes to the new worktree directory after creation.
fn setup_shell_integration() -> Result<(), shellsetup::ShellSetupError> {
    let integration = ShellIntegration::new("nwt", "New Worktree", SHELL_CODE)
        .with_command("nwt", "Create a new worktree and cd into it");

    integration.setup()
}

/// Gets the set of all files tracked by git in the repository.
///
/// Returns a HashSet of absolute paths for efficient membership checking.
/// Returns an empty set on any error (treats errors as "no tracked files found").
fn get_tracked_files(repo_root: &Path) -> std::collections::HashSet<PathBuf> {
    let output = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|line| repo_root.join(line))
            .collect(),
        _ => std::collections::HashSet::new(),
    }
}

/// Copies untracked .env files from the main worktree to the new worktree.
///
/// This function:
/// 1. Gets all tracked files from git in a single call (for performance)
/// 2. Walks the main repo looking for files starting with `.env`
/// 3. Skips the `.git` directory and any already-tracked files
/// 4. Copies untracked .env files to the same relative path in the new worktree
/// 5. Creates parent directories as needed
/// 6. Reports copied files unless quiet mode is enabled
///
/// Errors copying individual files are reported but don't stop the process.
fn copy_untracked_env_files(main_repo: &Path, worktree: &Path, quiet: bool) {
    // Get all tracked files in a single git call for performance
    let tracked_files = get_tracked_files(main_repo);
    let mut copied_count = 0;

    for entry in WalkDir::new(main_repo)
        .follow_links(false) // Don't follow symlinks
        .into_iter()
        .filter_entry(|e| {
            // Skip .git directory
            e.file_name() != ".git"
        })
        .filter_map(|e| e.ok())
    {
        // Only process files (not directories)
        if !entry.file_type().is_file() {
            continue;
        }

        // Check if the filename starts with .env
        let file_name = entry.file_name().to_string_lossy();
        if !file_name.starts_with(".env") {
            continue;
        }

        let file_path = entry.path();

        // Skip if tracked by git (use the pre-fetched set for O(1) lookup)
        if tracked_files.contains(file_path) {
            continue;
        }

        // Calculate relative path and destination
        let relative_path = match file_path.strip_prefix(main_repo) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let dest_path = worktree.join(relative_path);

        // Create parent directories if needed (create_dir_all is idempotent)
        if let Some(parent) = dest_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                if !quiet {
                    eprintln!(
                        "Warning: Failed to create directory '{}': {}",
                        parent.display(),
                        e
                    );
                }
                continue;
            }
        }

        // Copy the file
        match fs::copy(file_path, &dest_path) {
            Ok(_) => {
                copied_count += 1;
                if !quiet {
                    eprintln!("Copied: {}", relative_path.display());
                }
            }
            Err(e) => {
                if !quiet {
                    eprintln!(
                        "Warning: Failed to copy '{}': {}",
                        relative_path.display(),
                        e
                    );
                }
            }
        }
    }

    if copied_count > 0 && !quiet {
        eprintln!(
            "Copied {} untracked .env file{} to new worktree",
            copied_count,
            if copied_count == 1 { "" } else { "s" }
        );
    }
}

fn main() {
    let cli = Cli::parse();

    // Handle --shell-setup before anything else (doesn't require git repo)
    if cli.shell_setup {
        match setup_shell_integration() {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("Error: {e}");
                exit(exit_codes::SHELL_SETUP_ERROR);
            }
        }
    }

    // Load config file (missing file is OK, invalid file is error)
    let file_config = match load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            // # Why config errors bypass quiet mode
            //
            // Config errors are printed even before we know if quiet mode is enabled
            // (since quiet mode itself might be set in the broken config file). This is
            // intentional: if the user's config file is malformed, they need to know
            // immediately so they can fix it. Silent failure here would be confusing
            // since nwt would appear to ignore their config settings entirely.
            //
            // This differs from runtime errors (git failures, etc.) which respect quiet
            // mode because those don't prevent the tool from understanding user intent.
            eprintln!("Error: {}", e);
            if let Some(path) = get_config_path() {
                eprintln!("Config file location: {}", path.display());
            }
            exit(exit_codes::CONFIG_ERROR);
        }
    };

    // Merge CLI args with config file - CLI takes precedence
    let config = merge_config(&cli, file_config);

    // Early check: if tmux option is specified but we're not running in tmux, refuse to proceed
    if config.tmux && !is_running_in_tmux() {
        error!(
            config.quiet,
            "Error: --tmux option specified but not running inside tmux"
        );
        error!(
            config.quiet,
            "Please run this command from within a tmux session, or remove the --tmux option."
        );
        exit(exit_codes::TMUX_NOT_RUNNING);
    }

    // Find the main git repo root (resolves to main repo even from worktree)
    let repo_root = match find_main_repo() {
        Some(root) => root,
        None => {
            error!(config.quiet, "Error: Not in a git repository");
            error!(
                config.quiet,
                "Please run this command from within a git repository or worktree."
            );
            exit(exit_codes::NOT_IN_REPO);
        }
    };

    // Get repo name from path with sanitization (fail-fast on non-UTF8)
    let repo_name = match repo_root.file_name() {
        Some(name) => {
            let name_str = match name.to_str() {
                Some(s) => s,
                None => {
                    error!(
                        config.quiet,
                        "Error: Repository name contains invalid UTF-8 characters"
                    );
                    exit(exit_codes::INVALID_PATH_ENCODING);
                }
            };
            match sanitize_repo_name(name_str) {
                Some(sanitized) => sanitized,
                None => {
                    error!(config.quiet, "Error: Invalid repository name");
                    exit(exit_codes::INVALID_REPO_NAME);
                }
            }
        }
        None => {
            error!(config.quiet, "Error: Could not determine repository name");
            exit(exit_codes::INVALID_REPO_NAME);
        }
    };

    // Build worktrees directory path
    let parent = match repo_root.parent() {
        Some(p) => p,
        None => {
            error!(config.quiet, "Error: Repository has no parent directory");
            exit(exit_codes::NO_PARENT_DIR);
        }
    };
    let worktrees_dir = parent.join(format!("{}-worktrees", repo_name));

    // Create worktrees directory if needed
    if let Err(e) = fs::create_dir_all(&worktrees_dir) {
        error!(
            config.quiet,
            "Error: Could not create worktrees directory '{}': {}",
            worktrees_dir.display(),
            e
        );
        exit(exit_codes::DIR_CREATE_FAILED);
    }

    // Generate random Docker-style name and create worktree with retry on collision.
    // This handles TOCTOU race conditions by retrying with a new name if another
    // process creates a directory between our existence check and git's creation.
    let mut generator = Generator::default();
    let mut attempts = 0;

    loop {
        let dir_name = match generate_docker_name(&mut generator) {
            Some(n) => n,
            None => {
                error!(
                    config.quiet,
                    "Error: Name generator failed to produce a name"
                );
                exit(exit_codes::NAME_COLLISION);
            }
        };
        let worktree_path = worktrees_dir.join(&dir_name);

        // Quick check to avoid unnecessary git calls (optimization only, not relied upon)
        if worktree_path.exists() {
            attempts += 1;
            if attempts >= MAX_ATTEMPTS {
                error!(
                    config.quiet,
                    "Error: Could not find an available directory name after {} attempts",
                    MAX_ATTEMPTS
                );
                error!(config.quiet, "Please try again or clean up unused worktrees.");
                exit(exit_codes::NAME_COLLISION);
            }
            continue;
        }

        // Convert path to string, checking for valid UTF-8
        let worktree_path_str = match worktree_path.to_str() {
            Some(s) => s,
            None => {
                error!(
                    config.quiet,
                    "Error: Worktree path contains invalid UTF-8 characters: {}",
                    worktree_path.display()
                );
                error!(
                    config.quiet,
                    "Please ensure the repository path contains only valid UTF-8 characters."
                );
                exit(exit_codes::INVALID_PATH_ENCODING);
            }
        };

        // Determine branch name for this attempt
        let branch_name = get_branch_name(&config, &dir_name);

        // Attempt to create the worktree
        match try_create_worktree(
            &repo_root,
            worktree_path_str,
            branch_name,
            config.checkout.as_deref(),
        ) {
            WorktreeResult::Success => {
                // Handle tmux and/or run options
                if config.tmux {
                    // The names crate generates only lowercase ASCII letters and hyphens,
                    // so control characters cannot appear. This is a debug assertion to
                    // catch any future regression, not a runtime check.
                    debug_assert!(
                        !contains_control_chars(&dir_name),
                        "Generated directory name contains control characters: {:?}",
                        dir_name
                    );
                }

                println!("{}", worktree_path.display());

                // Copy untracked .env files from main worktree to new worktree
                if config.copy_env {
                    copy_untracked_env_files(&repo_root, &worktree_path, config.quiet);
                }

                // Execute tmux and/or run commands
                if config.tmux {
                    #[cfg(unix)]
                    {
                        // Create a new tmux window.
                        // Note: dir_name is passed directly to tmux as an argument (not through
                        // a shell), so it doesn't need shell escaping. Control characters are
                        // already validated above. The names crate generates simple adjective-noun
                        // combinations with only lowercase letters and hyphens.
                        //
                        // We move dir_name here (no .clone()) since it's not used after this point
                        // in the success path - we either run tmux and break, or exit on error.
                        let mut tmux_args: Vec<String> = vec![
                            "new-window".into(),
                            "-c".into(),
                            worktree_path_str.into(),
                            "-n".into(),
                            dir_name,
                        ];

                        // If --run is specified, wrap the command in an interactive shell
                        // so that aliases and shell functions are available.
                        if let Some(ref cmd) = config.run {
                            // Get the user's shell, defaulting to /bin/sh if SHELL is not set.
                            // We escape the shell path to prevent injection attacks from
                            // malicious SHELL environment variables.
                            //
                            // # Why /bin/sh fallback is acceptable
                            //
                            // When SHELL is unset, we fall back to /bin/sh. While /bin/sh with
                            // -ic won't load user aliases (since POSIX sh has no ~/.shrc), this
                            // is acceptable because:
                            // 1. SHELL is almost always set on Unix systems - it's required by
                            //    POSIX and set by login(1), sshd, and terminal emulators.
                            // 2. If SHELL is unset, the user likely doesn't have shell aliases
                            //    configured anyway, so there's nothing to load.
                            // 3. The command itself will still execute correctly; only aliases
                            //    and shell functions won't be available.
                            // 4. This matches the behavior of tools like `tmux` itself, which
                            //    also falls back to /bin/sh when SHELL is unset.
                            let shell =
                                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
                            // Use -ic to start an interactive shell that loads rc files.
                            // This ensures aliases and shell functions are available.
                            // Note: This assumes $SHELL supports -i (interactive) and -c (command)
                            // flags, which is true for bash, zsh, fish, and most POSIX shells.
                            // Exotic shells that don't support these flags will fail with a clear
                            // error message from the shell itself.
                            // Both the shell path and command are escaped for safety.
                            tmux_args.push(format!(
                                "{} -ic {}",
                                shell_escape(&shell),
                                shell_escape(cmd)
                            ));
                        }

                        match Command::new("tmux")
                            .args(&tmux_args)
                            .stdout(Stdio::inherit())
                            .stderr(Stdio::inherit())
                            .status()
                        {
                            Ok(status) => {
                                if !status.success() {
                                    // Use TMUX_FAILED for consistency. The worktree was created
                                    // successfully; this exit code indicates tmux itself failed,
                                    // not the user's --run command (which runs inside tmux).
                                    error!(
                                        config.quiet,
                                        "tmux exited with code {}",
                                        get_exit_code(status)
                                    );
                                    exit(exit_codes::TMUX_FAILED);
                                }
                            }
                            Err(e) => {
                                error!(config.quiet, "Error running tmux: {}", e);
                                exit(exit_codes::TMUX_FAILED);
                            }
                        }
                    }

                    // On Windows, --tmux is not supported (tmux is Unix-only)
                    #[cfg(windows)]
                    {
                        error!(
                            config.quiet,
                            "Error: --tmux is not supported on Windows. tmux is a Unix-only terminal multiplexer."
                        );
                        exit(exit_codes::TMUX_FAILED);
                    }
                } else if let Some(ref cmd) = config.run {
                    // Run command directly (no tmux)
                    match run_shell_command(cmd, &worktree_path) {
                        ShellCommandResult::Success => {}
                        ShellCommandResult::Failed(code) => {
                            // Print error message so users know the exit code is from
                            // the command, not nwt itself (since codes 1-8 overlap).
                            error!(config.quiet, "Command exited with code {}", code);
                            exit(code);
                        }
                        ShellCommandResult::ExecutionError(e) => {
                            error!(config.quiet, "Error running command: {}", e);
                            exit(exit_codes::RUN_COMMAND_FAILED);
                        }
                    }
                }

                break;
            }
            WorktreeResult::PathCollision => {
                // TOCTOU race or unexpected collision - retry with a new name
                attempts += 1;
                if attempts >= MAX_ATTEMPTS {
                    error!(
                        config.quiet,
                        "Error: Could not find an available directory name after {} attempts",
                        MAX_ATTEMPTS
                    );
                    error!(config.quiet, "Please try again or clean up unused worktrees.");
                    exit(exit_codes::NAME_COLLISION);
                }
                continue;
            }
            WorktreeResult::BranchExists(branch) => {
                error!(config.quiet, "Error: Branch '{}' already exists.", branch);
                error!(
                    config.quiet,
                    "Use --checkout to check out an existing branch instead."
                );
                exit(exit_codes::WORKTREE_FAILED);
            }
            WorktreeResult::RefInUse(ref_name) => {
                error!(
                    config.quiet,
                    "Error: The ref '{}' is already checked out in another worktree.",
                    ref_name
                );
                exit(exit_codes::WORKTREE_FAILED);
            }
            WorktreeResult::GitError(stderr) => {
                error!(config.quiet, "Failed to create worktree: {}", stderr);
                exit(exit_codes::WORKTREE_FAILED);
            }
            WorktreeResult::CommandError(e) => {
                error!(config.quiet, "Error running git command: {}", e);
                exit(exit_codes::GIT_COMMAND_ERROR);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_docker_name_format() {
        let mut generator = Generator::default();
        let name = generate_docker_name(&mut generator).expect("Generator should produce a name");
        assert!(name.contains('-'), "Name should contain a hyphen");

        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 2, "Name should have exactly two parts");

        // Verify both parts are non-empty lowercase strings
        assert!(!parts[0].is_empty(), "Adjective should not be empty");
        assert!(!parts[1].is_empty(), "Noun should not be empty");
        assert!(
            parts[0].chars().all(|c| c.is_ascii_lowercase()),
            "Adjective should be lowercase"
        );
        assert!(
            parts[1].chars().all(|c| c.is_ascii_lowercase()),
            "Noun should be lowercase"
        );
    }

    #[test]
    fn test_generate_docker_name_returns_some() {
        let mut generator = Generator::default();
        let result = generate_docker_name(&mut generator);
        assert!(result.is_some(), "Generator should return Some");
    }

    #[test]
    fn test_generate_docker_name_randomness() {
        let mut generator = Generator::default();
        let name1 = generate_docker_name(&mut generator).expect("Generator should produce a name");
        let mut found_different = false;

        // Generate several names to check randomness (very unlikely all same)
        for _ in 0..10 {
            let name2 =
                generate_docker_name(&mut generator).expect("Generator should produce a name");
            if name1 != name2 {
                found_different = true;
                break;
            }
        }

        assert!(found_different, "Names should be randomly generated");
    }

    #[test]
    fn test_generator_reuse_produces_different_names() {
        let mut generator = Generator::default();
        let mut names = Vec::new();

        for _ in 0..5 {
            names.push(
                generate_docker_name(&mut generator).expect("Generator should produce a name"),
            );
        }

        // Check that we got at least some different names
        let unique_count = {
            let mut sorted = names.clone();
            sorted.sort();
            sorted.dedup();
            sorted.len()
        };

        assert!(
            unique_count > 1,
            "Reusing generator should produce different names"
        );
    }

    #[test]
    fn test_sanitize_repo_name_valid() {
        assert_eq!(
            sanitize_repo_name("my-repo"),
            Some("my-repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my_repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("MyRepo123"),
            Some("MyRepo123".to_string())
        );
        assert_eq!(
            sanitize_repo_name("repo.name"),
            Some("repo.name".to_string())
        );
    }

    #[test]
    fn test_sanitize_repo_name_replaces_invalid_chars() {
        assert_eq!(
            sanitize_repo_name("my/repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my\\repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my repo"),
            Some("my_repo".to_string())
        );
        assert_eq!(
            sanitize_repo_name("my:repo"),
            Some("my_repo".to_string())
        );
    }

    #[test]
    fn test_sanitize_repo_name_rejects_invalid() {
        assert_eq!(sanitize_repo_name(""), None);
        assert_eq!(sanitize_repo_name("."), None);
        assert_eq!(sanitize_repo_name(".."), None);
        assert_eq!(sanitize_repo_name("..."), None);
        assert_eq!(sanitize_repo_name("._"), None);
        assert_eq!(sanitize_repo_name(".._"), None);
    }

    #[test]
    fn test_sanitize_repo_name_allows_dotfiles() {
        // .gitignore style names should be allowed
        assert_eq!(
            sanitize_repo_name(".gitignore"),
            Some(".gitignore".to_string())
        );
        assert_eq!(
            sanitize_repo_name(".hidden-repo"),
            Some(".hidden-repo".to_string())
        );
    }

    #[test]
    fn test_get_branch_name_with_explicit_branch() {
        let config = MergedConfig {
            branch: Some("feature/test".to_string()),
            checkout: None,
            copy_env: true,
            quiet: false,
            run: None,
            tmux: false,
        };
        assert_eq!(get_branch_name(&config, "random-name"), "feature/test");
    }

    #[test]
    fn test_get_branch_name_with_generated_name() {
        let config = MergedConfig {
            branch: None,
            checkout: None,
            copy_env: true,
            quiet: false,
            run: None,
            tmux: false,
        };
        assert_eq!(get_branch_name(&config, "random-name"), "random-name");
    }

    #[test]
    fn test_cli_branch_and_checkout_conflict() {
        // This tests that clap correctly rejects conflicting options
        use clap::CommandFactory;
        let cmd = Cli::command();

        // Try to parse with both --branch and --checkout - should fail
        let result = cmd.try_get_matches_from(["nwt", "--branch", "foo", "--checkout", "bar"]);

        assert!(
            result.is_err(),
            "Should fail when both --branch and --checkout are provided"
        );

        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "Error should be an argument conflict"
        );
    }

    // shell_escape tests are Unix-only since the function is Unix-only
    #[cfg(unix)]
    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("npm install"), "'npm install'");
    }

    #[cfg(unix)]
    #[test]
    fn test_shell_escape_with_single_quotes() {
        // Single quotes are escaped using the '\'' technique
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
        assert_eq!(shell_escape("echo 'hello'"), "'echo '\\''hello'\\'''");
    }

    #[cfg(unix)]
    #[test]
    fn test_shell_escape_with_special_chars() {
        // These should be safely wrapped in single quotes
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
        assert_eq!(shell_escape("foo && bar"), "'foo && bar'");
        assert_eq!(shell_escape("a;b"), "'a;b'");
    }

    #[cfg(unix)]
    #[test]
    fn test_shell_escape_empty_string() {
        // Empty string should still be safely quoted
        assert_eq!(shell_escape(""), "''");
    }

    #[cfg(unix)]
    #[test]
    fn test_shell_escape_shell_path() {
        // Verify that typical shell paths are properly escaped
        assert_eq!(shell_escape("/bin/bash"), "'/bin/bash'");
        assert_eq!(shell_escape("/usr/local/bin/zsh"), "'/usr/local/bin/zsh'");
    }

    #[test]
    fn test_exit_codes_are_unique() {
        let codes = [
            exit_codes::NOT_IN_REPO,
            exit_codes::INVALID_REPO_NAME,
            exit_codes::NO_PARENT_DIR,
            exit_codes::DIR_CREATE_FAILED,
            exit_codes::NAME_COLLISION,
            exit_codes::GIT_COMMAND_ERROR,
            exit_codes::WORKTREE_FAILED,
            exit_codes::INVALID_PATH_ENCODING,
            exit_codes::RUN_COMMAND_FAILED,
            exit_codes::TMUX_FAILED,
            exit_codes::CONFIG_ERROR,
            exit_codes::TMUX_NOT_RUNNING,
            exit_codes::SHELL_SETUP_ERROR,
        ];

        let mut sorted = codes.to_vec();
        sorted.sort();
        sorted.dedup();

        assert_eq!(
            sorted.len(),
            codes.len(),
            "All exit codes should be unique"
        );
    }

    #[test]
    fn test_contains_control_chars_normal_strings() {
        // Normal strings should not contain control characters
        assert!(!contains_control_chars("hello"));
        assert!(!contains_control_chars("hello-world"));
        assert!(!contains_control_chars("adjective-noun"));
        assert!(!contains_control_chars("my_worktree_123"));
    }

    #[test]
    fn test_contains_control_chars_with_newline() {
        // Newlines are control characters
        assert!(contains_control_chars("hello\nworld"));
        assert!(contains_control_chars("\n"));
    }

    #[test]
    fn test_contains_control_chars_with_tab() {
        // Tabs are control characters
        assert!(contains_control_chars("hello\tworld"));
        assert!(contains_control_chars("\t"));
    }

    #[test]
    fn test_contains_control_chars_with_null() {
        // Null byte is a control character
        assert!(contains_control_chars("hello\0world"));
        assert!(contains_control_chars("\0"));
    }

    #[test]
    fn test_contains_control_chars_with_escape() {
        // ESC (0x1B) is a control character
        assert!(contains_control_chars("hello\x1bworld"));
        assert!(contains_control_chars("\x1b[31m")); // ANSI escape sequence
    }

    #[test]
    fn test_contains_control_chars_with_delete() {
        // DEL (0x7F) is a control character
        assert!(contains_control_chars("hello\x7fworld"));
    }

    #[test]
    fn test_contains_control_chars_empty_string() {
        // Empty string should not contain control characters
        assert!(!contains_control_chars(""));
    }

    #[test]
    fn test_cli_run_option_parses() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // Parsing with --run should succeed
        let result = cmd.try_get_matches_from(["nwt", "--run", "npm install"]);
        assert!(result.is_ok(), "Should accept --run option");

        let matches = result.unwrap();
        assert_eq!(
            matches.get_one::<String>("run").map(|s| s.as_str()),
            Some("npm install"),
            "Should capture the run command"
        );
    }

    #[test]
    fn test_cli_run_with_branch() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --run can be combined with --branch
        let result =
            cmd.try_get_matches_from(["nwt", "--branch", "feature/test", "--run", "make build"]);
        assert!(result.is_ok(), "Should accept --run with --branch");
    }

    #[test]
    fn test_cli_run_with_checkout() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --run can be combined with --checkout
        let result = cmd.try_get_matches_from(["nwt", "--checkout", "main", "--run", "npm ci"]);
        assert!(result.is_ok(), "Should accept --run with --checkout");
    }

    #[test]
    fn test_cli_tmux_option_parses() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // Parsing with --tmux should succeed
        let result = cmd.try_get_matches_from(["nwt", "--tmux"]);
        assert!(result.is_ok(), "Should accept --tmux option");

        let matches = result.unwrap();
        assert!(
            matches.get_flag("tmux"),
            "Should set tmux flag"
        );
    }

    #[test]
    fn test_cli_tmux_with_run() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --tmux can be combined with --run
        let result = cmd.try_get_matches_from(["nwt", "--tmux", "--run", "npm install"]);
        assert!(result.is_ok(), "Should accept --tmux with --run");
    }

    #[test]
    fn test_cli_tmux_with_branch() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --tmux can be combined with --branch
        let result = cmd.try_get_matches_from(["nwt", "--tmux", "--branch", "feature/test"]);
        assert!(result.is_ok(), "Should accept --tmux with --branch");
    }

    #[test]
    fn test_cli_no_copy_env_parses() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // Parsing with --no-copy-env should succeed
        let result = cmd.try_get_matches_from(["nwt", "--no-copy-env"]);
        assert!(result.is_ok(), "Should accept --no-copy-env option");

        let matches = result.unwrap();
        assert!(
            matches.get_flag("no_copy_env"),
            "Should set no_copy_env flag"
        );
    }

    #[test]
    fn test_cli_no_copy_env_with_branch() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --no-copy-env can be combined with --branch
        let result =
            cmd.try_get_matches_from(["nwt", "--no-copy-env", "--branch", "feature/test"]);
        assert!(result.is_ok(), "Should accept --no-copy-env with --branch");
    }

    #[test]
    fn test_cli_shell_setup_conflicts_with_no_copy_env() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --shell-setup conflicts with --no-copy-env
        let result = cmd.try_get_matches_from(["nwt", "--shell-setup", "--no-copy-env"]);
        assert!(
            result.is_err(),
            "Should fail when both --shell-setup and --no-copy-env are provided"
        );
    }

    #[test]
    fn test_cli_shell_setup_parses() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // Parsing with --shell-setup should succeed
        let result = cmd.try_get_matches_from(["nwt", "--shell-setup"]);
        assert!(result.is_ok(), "Should accept --shell-setup option");

        let matches = result.unwrap();
        assert!(
            matches.get_flag("shell_setup"),
            "Should set shell_setup flag"
        );
    }

    #[test]
    fn test_cli_shell_setup_conflicts_with_branch() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --shell-setup conflicts with --branch
        let result = cmd.try_get_matches_from(["nwt", "--shell-setup", "--branch", "feature/test"]);
        assert!(
            result.is_err(),
            "Should fail when both --shell-setup and --branch are provided"
        );
    }

    #[test]
    fn test_cli_shell_setup_conflicts_with_tmux() {
        use clap::CommandFactory;
        let cmd = Cli::command();

        // --shell-setup conflicts with --tmux
        let result = cmd.try_get_matches_from(["nwt", "--shell-setup", "--tmux"]);
        assert!(
            result.is_err(),
            "Should fail when both --shell-setup and --tmux are provided"
        );
    }

    // Unix-specific tests for shell command execution.
    // These tests use Unix commands like `true`, `false`, `pwd`, and `sh`.
    #[cfg(unix)]
    mod unix_shell_tests {
        use super::*;
        use std::env;
        use std::process::Command;

        #[test]
        fn test_run_shell_command_success() {
            let temp_dir = env::temp_dir();

            // Test a simple command that should succeed
            let result = run_shell_command("true", &temp_dir);
            assert!(
                matches!(result, ShellCommandResult::Success),
                "true command should succeed"
            );
        }

        #[test]
        fn test_run_shell_command_failure() {
            let temp_dir = env::temp_dir();

            // Test a command that should fail with exit code 1
            let result = run_shell_command("false", &temp_dir);
            assert!(
                matches!(result, ShellCommandResult::Failed(1)),
                "false command should fail with exit code 1"
            );
        }

        #[test]
        fn test_run_shell_command_custom_exit_code() {
            let temp_dir = env::temp_dir();

            // Test a command with a specific exit code
            let result = run_shell_command("exit 42", &temp_dir);
            assert!(
                matches!(result, ShellCommandResult::Failed(42)),
                "Should capture custom exit code 42"
            );
        }

        #[test]
        fn test_run_shell_command_working_directory() {
            let temp_dir = env::temp_dir();

            // Test that the command runs in the correct directory
            // This command should succeed if we're in a valid directory
            let result = run_shell_command("pwd", &temp_dir);
            assert!(
                matches!(result, ShellCommandResult::Success),
                "pwd should succeed in temp directory"
            );
        }

        #[test]
        fn test_get_exit_code_with_success() {
            // Run a command that succeeds
            let status = Command::new("true").status().expect("Failed to run true");
            let code = get_exit_code(status);
            assert_eq!(code, 0, "Successful command should have exit code 0");
        }

        #[test]
        fn test_get_exit_code_with_failure() {
            // Run a command that fails
            let status = Command::new("false").status().expect("Failed to run false");
            let code = get_exit_code(status);
            assert_eq!(code, 1, "false command should have exit code 1");
        }

        #[test]
        fn test_get_exit_code_custom_code() {
            // Run a command with custom exit code
            let status = Command::new("sh")
                .args(["-c", "exit 123"])
                .status()
                .expect("Failed to run command");
            let code = get_exit_code(status);
            assert_eq!(code, 123, "Should capture custom exit code");
        }
    }

    // Config file tests
    mod config_tests {
        use super::*;

        #[test]
        fn test_parse_valid_config() {
            let toml = r#"
                branch = "feature/test"
                quiet = true
                tmux = false
            "#;
            let config: NwtConfig = toml::from_str(toml).expect("Should parse valid config");
            assert_eq!(config.branch, Some("feature/test".to_string()));
            assert!(config.quiet);
            assert!(!config.tmux);
            assert!(config.run.is_none());
            assert!(config.checkout.is_none());
        }

        #[test]
        fn test_parse_config_all_fields() {
            let toml = r#"
                branch = "my-branch"
                quiet = true
                tmux = true
                run = "npm install"
            "#;
            let config: NwtConfig = toml::from_str(toml).expect("Should parse valid config");
            assert_eq!(config.branch, Some("my-branch".to_string()));
            assert!(config.quiet);
            assert!(config.tmux);
            assert_eq!(config.run, Some("npm install".to_string()));
        }

        #[test]
        fn test_parse_empty_config() {
            let toml = "";
            let config: NwtConfig = toml::from_str(toml).expect("Should parse empty config");
            assert!(config.branch.is_none());
            assert!(config.checkout.is_none());
            assert!(config.copy_env); // defaults to true
            assert!(!config.quiet);
            assert!(config.run.is_none());
            assert!(!config.tmux);
        }

        #[test]
        fn test_parse_config_unknown_field_fails() {
            // Unknown fields should be rejected (catches typos like "banch")
            let toml = r#"banch = "typo""#;
            let result: Result<NwtConfig, _> = toml::from_str(toml);
            assert!(result.is_err(), "Should reject unknown field 'banch'");
        }

        #[test]
        fn test_validate_config_branch_checkout_conflict() {
            let config = NwtConfig {
                branch: Some("feat".to_string()),
                checkout: Some("main".to_string()),
                copy_env: true,
                quiet: false,
                run: None,
                tmux: false,
            };
            let result = validate_config(&config);
            assert!(result.is_err(), "Should reject branch+checkout conflict");
        }

        #[test]
        fn test_validate_config_branch_only() {
            let config = NwtConfig {
                branch: Some("feat".to_string()),
                checkout: None,
                copy_env: true,
                quiet: false,
                run: None,
                tmux: false,
            };
            let result = validate_config(&config);
            assert!(result.is_ok(), "Should accept config with only branch");
        }

        #[test]
        fn test_validate_config_checkout_only() {
            let config = NwtConfig {
                branch: None,
                checkout: Some("main".to_string()),
                copy_env: true,
                quiet: false,
                run: None,
                tmux: false,
            };
            let result = validate_config(&config);
            assert!(result.is_ok(), "Should accept config with only checkout");
        }

        #[test]
        fn test_merge_cli_overrides_config() {
            let cli = Cli {
                branch: Some("cli-branch".to_string()),
                checkout: None,
                no_copy_env: false,
                quiet: true,
                run: None,
                tmux: false,
                shell_setup: false,
            };
            let config = NwtConfig {
                branch: Some("config-branch".to_string()),
                checkout: None,
                copy_env: true,
                quiet: false,
                run: Some("npm install".to_string()),
                tmux: true,
            };
            let merged = merge_config(&cli, Some(config));

            // CLI values should override config for Option<String> fields
            assert_eq!(merged.branch, Some("cli-branch".to_string()));
            assert!(merged.quiet); // CLI --quiet flag was set, so quiet=true
            assert!(merged.copy_env); // config has true, CLI didn't disable

            // Boolean flags use OR logic: cli.tmux || config.tmux
            // Since CLI didn't specify --tmux (so cli.tmux=false, the default),
            // the config value (tmux=true) is used via the OR. This is the expected
            // behavior documented in merge_config(). See that function's doc comment
            // for the full rationale on why we don't support --no-tmux to override.
            assert!(merged.tmux);

            assert_eq!(merged.run, Some("npm install".to_string())); // Config provides default
        }

        #[test]
        fn test_merge_config_provides_defaults() {
            let cli = Cli {
                branch: None,
                checkout: None,
                no_copy_env: false,
                quiet: false,
                run: None,
                tmux: false,
                shell_setup: false,
            };
            let config = NwtConfig {
                branch: Some("config-branch".to_string()),
                checkout: None,
                copy_env: true,
                quiet: true,
                run: Some("make build".to_string()),
                tmux: true,
            };
            let merged = merge_config(&cli, Some(config));

            // Config values should be used when CLI doesn't specify
            assert_eq!(merged.branch, Some("config-branch".to_string()));
            assert!(merged.copy_env);
            assert!(merged.quiet);
            assert!(merged.tmux);
            assert_eq!(merged.run, Some("make build".to_string()));
        }

        #[test]
        fn test_merge_no_config() {
            let cli = Cli {
                branch: Some("my-branch".to_string()),
                checkout: None,
                no_copy_env: false,
                quiet: true,
                run: None,
                tmux: false,
                shell_setup: false,
            };
            let merged = merge_config(&cli, None);

            // Should use CLI values with defaults for missing
            assert_eq!(merged.branch, Some("my-branch".to_string()));
            assert!(merged.copy_env); // default is true
            assert!(merged.quiet);
            assert!(!merged.tmux);
            assert!(merged.run.is_none());
        }

        #[test]
        fn test_merge_no_copy_env_disables() {
            let cli = Cli {
                branch: None,
                checkout: None,
                no_copy_env: true, // CLI disables
                quiet: false,
                run: None,
                tmux: false,
                shell_setup: false,
            };
            let config = NwtConfig {
                branch: None,
                checkout: None,
                copy_env: true, // config enables
                quiet: false,
                run: None,
                tmux: false,
            };
            let merged = merge_config(&cli, Some(config));

            // CLI --no-copy-env should override config copy_env = true
            assert!(!merged.copy_env);
        }

        #[test]
        fn test_merge_config_copy_env_false() {
            let cli = Cli {
                branch: None,
                checkout: None,
                no_copy_env: false,
                quiet: false,
                run: None,
                tmux: false,
                shell_setup: false,
            };
            let config = NwtConfig {
                branch: None,
                checkout: None,
                copy_env: false, // config disables
                quiet: false,
                run: None,
                tmux: false,
            };
            let merged = merge_config(&cli, Some(config));

            // Config copy_env = false should be respected
            assert!(!merged.copy_env);
        }

        #[test]
        fn test_exit_code_config_error_is_unique() {
            // Ensure CONFIG_ERROR, TMUX_NOT_RUNNING, and SHELL_SETUP_ERROR don't conflict with other exit codes
            let codes = [
                exit_codes::NOT_IN_REPO,
                exit_codes::INVALID_REPO_NAME,
                exit_codes::NO_PARENT_DIR,
                exit_codes::DIR_CREATE_FAILED,
                exit_codes::NAME_COLLISION,
                exit_codes::GIT_COMMAND_ERROR,
                exit_codes::WORKTREE_FAILED,
                exit_codes::INVALID_PATH_ENCODING,
                exit_codes::RUN_COMMAND_FAILED,
                exit_codes::TMUX_FAILED,
                exit_codes::CONFIG_ERROR,
                exit_codes::TMUX_NOT_RUNNING,
                exit_codes::SHELL_SETUP_ERROR,
            ];

            let mut sorted = codes.to_vec();
            sorted.sort();
            sorted.dedup();

            assert_eq!(
                sorted.len(),
                codes.len(),
                "All exit codes including CONFIG_ERROR, TMUX_NOT_RUNNING, and SHELL_SETUP_ERROR should be unique"
            );
        }
    }
}
