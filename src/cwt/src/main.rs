use std::process::exit;

use buildinfo::version_string;
use clap::Parser;
use repowalker::find_git_repo;
use shellsetup::ShellIntegration;

mod family;
mod worktree;

use family::{Family, WorktreeMatch, MAIN_BRANCH_NAMES};

/// Exit codes for different error conditions.
mod exit_codes {
    /// Not in a git repository.
    pub const NOT_IN_REPO: i32 = 1;
    /// Git command failed to execute or returned an error.
    pub const GIT_COMMAND_ERROR: i32 = 2;
    /// The specified worktree was not found.
    pub const WORKTREE_NOT_FOUND: i32 = 3;
    /// Could not determine current worktree.
    pub const CURRENT_UNKNOWN: i32 = 4;
    /// Shell setup failed.
    pub const SHELL_SETUP_ERROR: i32 = 5;
    /// Multiple worktrees matched the search term.
    pub const MULTIPLE_MATCHES: i32 = 6;
}

/// Macro for printing error messages that respects quiet mode.
macro_rules! error {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}

/// Change to a different git worktree.
///
/// Lists all worktrees in the current repository or navigates to a specific one.
///
/// # Families of repositories
///
/// A repository that holds other repositories one level below it — a workspace
/// that tracks the map, with the real repositories checked out inside it — is a
/// family. `cwt` lists every worktree of every repository in the family, and
/// navigates to any of them by name.
///
/// # Usage
///
/// ```sh
/// cwt           # Show list of worktrees with current highlighted
/// cwt -f        # Go to next worktree (wraps around)
/// cwt -p        # Go to previous worktree (wraps around)
/// cwt -m        # Go to the main worktree (branch main, or master)
/// cwt NAME      # Go to worktree by directory name or branch name
/// cwt TEXT      # Go to worktree by case-insensitive substring match on branch
/// cwt REPO:NAME # Go to a worktree of one repository in the family
/// ```
///
/// # Shell Integration
///
/// Add this to your ~/.bashrc or ~/.zshrc:
///
/// ```sh
/// function wt() {
///     if [ $# -eq 0 ]; then
///         cwt
///     else
///         local target=$(cwt "$@")
///         if [ $? -eq 0 ] && [ -n "$target" ]; then
///             cd "$target"
///         fi
///     fi
/// }
///
/// alias wtf='wt -f'  # Next worktree
/// alias wtb='wt -p'  # Previous worktree (back)
/// alias wtm='wt --main'  # Main worktree (branch main, or master)
/// ```
///
/// # Exit Codes
///
/// - 0: Success
/// - 1: Not in a git repository
/// - 2: Git command error
/// - 3: Worktree not found
/// - 4: Could not determine current worktree (for -f/-p)
/// - 5: Shell setup failed
/// - 6: Multiple worktrees matched (need more specific search term)
#[derive(Parser)]
#[command(name = "cwt")]
#[command(about = "Change to a different git worktree")]
#[command(version = version_string!())]
// Without this, clap folds the long help into one paragraph and the code blocks
// below come out as a single run-on line.
#[command(verbatim_doc_comment)]
#[allow(clippy::struct_excessive_bools)] // CLI flags are naturally bool-heavy
struct Cli {
    /// Go to the next worktree (wraps around).
    #[arg(short = 'f', long, conflicts_with_all = ["prev", "main", "target", "shell_setup"])]
    forward: bool,

    /// Go to the previous worktree (wraps around).
    #[arg(short = 'p', long, conflicts_with_all = ["forward", "main", "target", "shell_setup"])]
    prev: bool,

    /// Go to the main worktree.
    ///
    /// The main worktree is the one on branch `main`, or the one on branch `master` when
    /// no worktree is on `main`. The branch name must match exactly.
    #[arg(short = 'm', long, verbatim_doc_comment, conflicts_with_all = ["forward", "prev", "target", "shell_setup"])]
    main: bool,

    /// Worktree to switch to (directory name, branch name, or branch substring).
    ///
    /// Matches in order: exact directory name, exact branch name, then case-insensitive
    /// substring on branch names. If multiple branches match, lists them and exits.
    ///
    /// Prefix a repository name to search one repository of the family, for
    /// example `REPO:feature-x`. Part of the name is enough, and a bare `REPO:`
    /// selects that repository's main worktree.
    ///
    /// A repository whose directory name belongs to another repository of the
    /// family — a parent that holds a child named after itself — is named by
    /// the path that leads to it instead, the way the listing heads it:
    /// `PARENT/REPO:feature-x`.
    #[arg(conflicts_with_all = ["forward", "prev", "main", "shell_setup"], verbatim_doc_comment)]
    target: Option<String>,

    /// Add shell integration to your shell config. Adds these commands:
    ///
    ///   wt [target]  - List worktrees or change to one
    ///   wtf          - Next worktree (forward)
    ///   wtb          - Previous worktree (back)
    ///   wtm          - Main worktree (branch main, or master)
    #[arg(long, verbatim_doc_comment, conflicts_with_all = ["forward", "prev", "main", "target"])]
    shell_setup: bool,

    /// List only the repository you are standing in, not the whole family.
    ///
    /// Set `CWT_NO_FAMILY` to any value other than 0 to make this the default.
    #[arg(long, verbatim_doc_comment, conflicts_with = "shell_setup")]
    no_family: bool,

    /// Suppress error messages.
    #[arg(short, long)]
    quiet: bool,
}

/// Lists every worktree of the family to stderr, one `  name [branch]` a line.
///
/// Both the "not found" and the "no main worktree" errors end with this list,
/// so the format lives here and not at either call site.
///
/// The list follows an error, so it obeys quiet mode.
fn print_available(family: &Family, quiet: bool) {
    error!(quiet, "Available worktrees:");
    for label in family.labels() {
        error!(quiet, "  {}", label);
    }
}

/// Formats `names` as a quoted alternation: `'main' or 'master'`.
///
/// The missing-main-worktree error is built from [`MAIN_BRANCH_NAMES`] through
/// this function, so a name added to the constant cannot leave the message
/// naming a shorter list than the search actually tried.
fn quoted_branch_alternatives(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// The environment variable that makes `--no-family` the default.
const NO_FAMILY_ENV: &str = "CWT_NO_FAMILY";

/// True when the environment asks `cwt` to stay inside one repository.
///
/// An unset or empty variable is not a choice, and `0` is the choice not to.
fn no_family_in_env() -> bool {
    match std::env::var(NO_FAMILY_ENV) {
        Ok(value) => !value.is_empty() && value != "0",
        Err(_) => false,
    }
}

/// The shell code to add to shell config files.
const SHELL_CODE: &str = r#"
function wt() {
    if [ $# -eq 0 ]; then
        # No args: show list interactively
        cwt
    else
        local target
        target=$(cwt "$@")
        local exit_code=$?
        if [ $exit_code -eq 0 ] && [ -n "$target" ] && [ -d "$target" ]; then
            cd "$target"
        else
            [ -n "$target" ] && echo "$target"
            return $exit_code
        fi
    fi
}

# Quick navigation aliases
alias wtf='wt -f'  # Next worktree
alias wtb='wt -p'  # Previous worktree (back)
alias wtm='wt --main'  # Main worktree (branch main, or master)
"#;

/// Sets up shell integration by adding the wt function to the user's shell config.
fn setup_shell_integration() -> Result<(), shellsetup::ShellSetupError> {
    let integration = ShellIntegration::new("cwt", "Change Worktree", SHELL_CODE)
        .with_command("wt", "List worktrees or change to one")
        .with_command("wtf", "Next worktree")
        .with_command("wtb", "Previous worktree (back)")
        .with_command("wtm", "Main worktree (branch main, or master)")
        // Old installations ended with this alias (before end marker was added)
        .with_old_end_marker("alias wtb='wt -p'");

    integration.setup()
}

fn main() {
    let cli = Cli::parse();

    // Handle shell setup (doesn't require being in a git repo)
    if cli.shell_setup {
        match setup_shell_integration() {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("Error: {e}");
                exit(exit_codes::SHELL_SETUP_ERROR);
            }
        }
    }

    // Find git repo root
    let Some(repo_root) = find_git_repo() else {
        error!(cli.quiet, "Error: Not in a git repository");
        exit(exit_codes::NOT_IN_REPO);
    };

    // Collect the family: this repository, and any repository beside it
    let scan_children = !cli.no_family && !no_family_in_env();
    let family = match Family::discover(&repo_root, scan_children) {
        Ok(family) => family,
        Err(e) => {
            error!(cli.quiet, "Error getting worktrees: {}", e);
            exit(exit_codes::GIT_COMMAND_ERROR);
        }
    };

    for warning in family.warnings() {
        error!(cli.quiet, "Warning: skipped {}", warning);
    }

    if family.is_empty() {
        error!(cli.quiet, "No worktrees found");
        exit(exit_codes::GIT_COMMAND_ERROR);
    }

    // Handle different modes
    if cli.forward {
        let Some(index) = family.next() else {
            error!(cli.quiet, "Error: Could not determine current worktree");
            exit(exit_codes::CURRENT_UNKNOWN);
        };
        println!("{}", family.path(index).display());
    } else if cli.prev {
        let Some(index) = family.previous() else {
            error!(cli.quiet, "Error: Could not determine current worktree");
            exit(exit_codes::CURRENT_UNKNOWN);
        };
        println!("{}", family.path(index).display());
    } else if cli.main {
        // Main worktree: branch main, or branch master when there is no main.
        let Some(index) = family.main_worktree() else {
            error!(
                cli.quiet,
                "Error: No worktree is on branch {}",
                quoted_branch_alternatives(&MAIN_BRANCH_NAMES)
            );
            print_available(&family, cli.quiet);
            exit(exit_codes::WORKTREE_NOT_FOUND);
        };
        println!("{}", family.path(index).display());
    } else if let Some(name) = &cli.target {
        match family.find(name) {
            WorktreeMatch::Single(index) => {
                println!("{}", family.path(index).display());
            }
            WorktreeMatch::Multiple(indices) => {
                error!(
                    cli.quiet,
                    "Error: Multiple worktrees match '{}'. Be more specific:", name
                );
                for index in indices {
                    error!(cli.quiet, "  {}", family.label(index));
                }
                exit(exit_codes::MULTIPLE_MATCHES);
            }
            WorktreeMatch::None => {
                error!(cli.quiet, "Error: Worktree '{}' not found", name);
                print_available(&family, cli.quiet);
                exit(exit_codes::WORKTREE_NOT_FOUND);
            }
        }
    } else {
        // No args: display list
        print!("{}", family.render());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tests of the main-worktree search live beside the search itself, in
    // the family module.

    #[test]
    fn test_quoted_branch_alternatives_quotes_a_single_name() {
        assert_eq!(quoted_branch_alternatives(&["main"]), "'main'");
    }

    #[test]
    fn test_quoted_branch_alternatives_joins_two_names() {
        assert_eq!(
            quoted_branch_alternatives(&["main", "master"]),
            "'main' or 'master'"
        );
    }

    #[test]
    fn test_quoted_branch_alternatives_joins_three_names() {
        // This is the assertion that proves the arity coupling is gone. The old
        // message read MAIN_BRANCH_NAMES[0] and [1] by index, so a third name
        // would be searched for and never named. The formatter must render
        // every name it is given, whatever the length of the slice.
        assert_eq!(
            quoted_branch_alternatives(&["main", "master", "trunk"]),
            "'main' or 'master' or 'trunk'"
        );
    }

    #[test]
    fn test_quoted_branch_alternatives_renders_the_branch_constant() {
        // Ties the formatter to the constant the not-found message is built
        // from, so the user-facing text stays what it has always been.
        assert_eq!(
            quoted_branch_alternatives(&MAIN_BRANCH_NAMES),
            "'main' or 'master'"
        );
    }

    #[test]
    fn test_main_flag_parses() {
        let long = Cli::try_parse_from(["cwt", "--main"]).expect("--main must parse");
        assert!(long.main);

        let short = Cli::try_parse_from(["cwt", "-m"]).expect("-m must parse");
        assert!(short.main);

        let absent = Cli::try_parse_from(["cwt"]).expect("no arguments must parse");
        assert!(!absent.main);
    }

    #[test]
    fn test_main_flag_conflicts_with_other_modes() {
        // --main selects a worktree, so it cannot combine with the other selectors.
        for args in [
            vec!["cwt", "--main", "feature"],
            vec!["cwt", "--main", "-f"],
            vec!["cwt", "--main", "-p"],
            vec!["cwt", "--main", "--shell-setup"],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_err(),
                "{args:?} must be rejected"
            );
        }
    }

    #[test]
    fn test_shell_code_contains_wtm() {
        // wtm goes through --main so that it finds master in a repository
        // that has no main branch.
        assert!(SHELL_CODE.contains("alias wtm='wt --main'"));
    }
}
