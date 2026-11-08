//! The one entrance for every child process that this workspace starts.
//!
//! Git obeys the inherited `GIT_` variables before it obeys the directory that
//! a command points at, and a hook exports them. A child process that keeps
//! them aims at the repository of the hook. This crate holds the one rule that
//! removes them, so that no call site writes the rule again.
//!
//! Each child process sheds the inherited `GIT_` variables through this
//! entrance. Production code takes [`command`]. Test code takes
//! [`fixture_command`]. The doc of [`fixture_command`] names the one production
//! site that takes it. A value that a call site must pin goes on after the
//! constructor, and so it wins.
//!
//! The sweep has no effect on a program that does not run git. Thus the rule
//! needs no knowledge of what a program runs. `go`, `cargo`, `gh`, `claude`,
//! a shell, and `tmux` all run git later, and a rule about the literal `"git"`
//! does not see them.
//!
//! # The rule is the prefix
//!
//! The rule is the `GIT_` prefix, and never a list of names. A list removes
//! nothing new on the day that git adds a variable, and it then gives the same
//! clean answer as a list that works. This rule was a list of fifteen names
//! once, and `GIT_CONFIG_PARAMETERS` went through it. Git exports that variable
//! into each hook, and it sets any configuration key in the child. The sweep
//! reads [`std::env::vars_os`], so it also removes the variables that git adds
//! later. `tests/inherited-environment.rs` holds this rule.
//!
//! The prefix covers three families:
//!
//! - The location variables, for example `GIT_DIR` and `GIT_INDEX_FILE`, aim
//!   git at another repository. Git often exports `GIT_INDEX_FILE` as a relative
//!   path, so it silently changes with the directory of each command.
//! - The attribution variables, for example `GIT_AUTHOR_NAME`, outrank `-c` and
//!   `git config`. The two `DATE` variables give each commit of a run one
//!   timestamp.
//! - The configuration variables, for example `GIT_CONFIG_PARAMETERS` and
//!   `GIT_CONFIG_COUNT`, set any key.
//!
//! # Two rules, and the split is by intent
//!
//! The prefix cannot tell the families of git apart, so a call site cannot ask
//! for one family. The split that is safe is a split by whose intent a variable
//! carries. A hook exports `GIT_DIR` and `GIT_CONFIG_PARAMETERS`, so those carry
//! the intent of the hook, and they leave. A person exports `GIT_SSH_COMMAND`,
//! so it carries the intent of the user, and [`command`] keeps it. A fixture has
//! no user, so [`fixture_command`] keeps nothing.
//!
//! A keep-list is safe in a way that a strip-list is not. A variable that git
//! adds next year leaves by default, and the cost is one setting that a user
//! states again. `tests/user-intent-environment.rs` holds the two lists disjoint.
//!
//! # What holds the entrance
//!
//! `clippy.toml` bans `std::process::Command::new`,
//! `tokio::process::Command::new`, and `portable_pty::CommandBuilder::new`, so a
//! raw spawn fails the build. A spawn that must stay raw says so at its site,
//! with an `#[expect]` and a reason. `repo_guards::git_env_sweep` refuses the
//! other shape, a list of names written into `env_remove`.
//!
//! The ban does not reach a dependency that starts a child process inside its
//! own code. `.husky/pre-commit` and `test.sh` remove the whole prefix before
//! each cargo command for that reason.
//!
//! # The dependencies
//!
//! This crate uses the standard library only. The build script of `buildinfo`
//! takes it as a build-dependency, and every tool builds `buildinfo`, so a
//! dependency here would reach every build in the workspace.

use std::ffi::{OsStr, OsString};
use std::process::Command;

/// The prefix that makes a variable git's, and therefore one the sweep
/// removes.
pub const GIT_ENVIRONMENT_PREFIX: &str = "GIT_";

/// Which rule a sweep applies to a `GIT_`-prefixed variable it finds.
///
/// The two rules exist because the two kinds of caller differ in one way that
/// decides the answer: a fixture has no user whose intent to honor, and a
/// production tool has one. A call site names the rule it wants, so a reader of
/// that call site sees the decision instead of a bare boolean.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InheritedGitEnvironment {
    /// Remove every `GIT_`-prefixed key the source holds.
    ///
    /// The rule for a fixture. A fixture builds a throwaway repository and must
    /// reach nothing else, so it keeps nothing that a launching hook or a
    /// developer's shell left in the environment.
    ShedEverything,
    /// Remove every `GIT_`-prefixed key except a name in
    /// [`USER_INTENT_GIT_ENVIRONMENT`].
    ///
    /// The rule for a production tool. Such a tool acts for the person who
    /// started it, so the six names that person sets on purpose stay, and the
    /// rest of the prefix leaves.
    KeepUserIntent,
}

/// The `GIT_` variables a person sets on purpose, which
/// [`InheritedGitEnvironment::KeepUserIntent`] keeps.
///
/// **A keep-list is safe in the way a strip-list is not.** A strip-list goes
/// stale the day git adds a variable, and from then on it inherits the new
/// variable and reports the same clean-looking answer as a list that works. A
/// keep-list goes stale in the other direction: a variable git invents next
/// year is shed by default, and the cost of that staleness is one setting a
/// user has to state again rather than one repository a tool writes into by
/// mistake.
///
/// Each name below states what the keep buys and what a drop costs.
pub const USER_INTENT_GIT_ENVIRONMENT: &[&str] = &[
    // The file git reads in place of `~/.gitconfig`. Kept, a tool that predicts
    // what git does at commit time reads the configuration the user chose.
    // Dropped, the tool predicts against a file the user replaced, and the
    // worktree to repo-local to global to system precedence breaks at its third
    // step.
    "GIT_CONFIG_GLOBAL",
    // The file git reads in place of `/etc/gitconfig`. The same gain and the
    // same cost, at the fourth step of that precedence.
    "GIT_CONFIG_SYSTEM",
    // The program git runs in place of `ssh`. Kept, a fetch or a clone over
    // `git@` reaches the host the way the user's own shell reaches it. Dropped,
    // git runs plain `ssh` with the default key, and a user who holds a
    // non-default key gets an authentication failure.
    "GIT_SSH",
    // The same program, with the arguments git is to pass it. A user states one
    // of these two, and a tool that drops either one breaks the same fetch.
    "GIT_SSH_COMMAND",
    // The program git runs to ask for a password. Kept, a private dependency
    // over HTTPS gets its credential from the helper the user chose. Dropped,
    // git falls back to the terminal.
    "GIT_ASKPASS",
    // Whether git asks a person for a credential on the terminal. A user sets
    // it to `0` so that a missing credential fails at once. Dropped, git
    // prompts on `/dev/tty`, and a tool whose stdout a shell wrapper captures
    // shows the user a command that stalls with nothing on the screen.
    "GIT_TERMINAL_PROMPT",
];

/// The `GIT_` variables git exports into the environment of a hook.
///
/// It exists to be asserted disjoint from [`USER_INTENT_GIT_ENVIRONMENT`], and
/// the test that asserts it is `tests/user-intent-environment.rs`. A name git
/// hands every hook cannot be read as the user's intent: the value in the
/// environment came from git, not from the person, so a keep-list that held one
/// of these names would let a hook aim a tool at the hook's own repository.
///
/// This is a sample of what git exports and never its definition. A list of
/// names is the wrong shape for deciding what to shed, which is why the sweep
/// matches the prefix instead. A list is the right shape for the one question
/// asked here, because the question is about six names that are written down.
pub const HOOK_EXPORTED_GIT_ENVIRONMENT: &[&str] = &[
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_CONFIG_PARAMETERS",
    "GIT_EXEC_PATH",
    "GIT_PREFIX",
    "GIT_EDITOR",
];

/// Schedule the removal of every `GIT_`-prefixed key of `keys` that `rule` does
/// not keep.
///
/// **The key source is a parameter rather than [`std::env::vars_os`], and that
/// is load-bearing.** Cargo runs the tests of one binary on parallel threads,
/// so a test that set a real `GIT_*` variable to prove this rule would redirect
/// the git children of every sibling thread while it did so. A synthetic key
/// list keeps such a test parallel-safe. The two constructors read the real
/// environment and delegate here, so the rule has one implementation and the
/// test measures the one that a spawn uses.
///
/// Keys are compared through [`std::ffi::OsStr::to_string_lossy`]: lossy
/// conversion replaces an invalid byte with U+FFFD, so it can never manufacture
/// a `GIT_` prefix out of bytes that did not spell one.
///
/// A value the call site wants pinned is set *after* this call, and so wins.
///
/// ```no_run
/// use childenv::InheritedGitEnvironment;
///
/// let mut command = std::process::Command::new("git");
/// childenv::shed_git_environment_from(
///     &mut command,
///     ["GIT_DIR", "GIT_SSH_COMMAND", "PATH"],
///     InheritedGitEnvironment::KeepUserIntent,
/// );
/// ```
pub fn shed_git_environment_from<I, K>(
    command: &mut Command,
    keys: I,
    rule: InheritedGitEnvironment,
) where
    I: IntoIterator<Item = K>,
    K: AsRef<OsStr>,
{
    for key in keys {
        if rule_sheds(key.as_ref(), rule) {
            command.env_remove(key.as_ref());
        }
    }
}

/// Decide whether `rule` sheds the variable `name`.
///
/// This is the one implementation of the rule. [`shed_git_environment_from`]
/// and [`inherited_git_variables`] both call it.
fn rule_sheds(name: &OsStr, rule: InheritedGitEnvironment) -> bool {
    let name = name.to_string_lossy();

    if !name.starts_with(GIT_ENVIRONMENT_PREFIX) {
        return false;
    }

    let kept = matches!(rule, InheritedGitEnvironment::KeepUserIntent)
        && USER_INTENT_GIT_ENVIRONMENT.contains(&&*name);

    !kept
}

/// Start a child process for production code.
///
/// The child sheds every inherited `GIT_` variable except the names in
/// [`USER_INTENT_GIT_ENVIRONMENT`], so a tool aims at the repository that it
/// stands in and authenticates as the shell of the user does. A value that the
/// call site must pin goes on after this call, and so it wins.
///
/// ```no_run
/// let mut command = childenv::command("git");
/// command.arg("status");
/// ```
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    swept(program, InheritedGitEnvironment::KeepUserIntent)
}

/// Start a child process for test code.
///
/// The child sheds every inherited `GIT_` variable and keeps none, because a
/// fixture builds a throwaway repository and has no user whose intent to honor.
/// A value that the call site must pin goes on after this call, and so it wins.
///
/// One production site takes this constructor on purpose:
/// `gitscratch::Git::command`. A replay works on local objects and needs no
/// credential, and a `GIT_CONFIG_GLOBAL` that one shell states must not change
/// what the replay reports.
///
/// The removal of `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` costs a fixture
/// nothing. A removal is not a redirect to `/dev/null`, so git reads the
/// `~/.gitconfig` and `/etc/gitconfig` of the host, as it does in a plain shell.
///
/// ```no_run
/// let mut command = childenv::fixture_command("git");
/// command.arg("init");
/// ```
pub fn fixture_command<S: AsRef<OsStr>>(program: S) -> Command {
    swept(program, InheritedGitEnvironment::ShedEverything)
}

/// List the names in the environment of this process that `rule` sheds.
///
/// A `std::process::Command` takes [`command`] or [`fixture_command`]. This
/// function is for a builder of another type, such as the command builder of a
/// pseudo-terminal crate. This crate cannot depend on such a type, so the
/// caller removes each listed name itself.
///
/// ```no_run
/// for name in childenv::inherited_git_variables(childenv::InheritedGitEnvironment::KeepUserIntent) {
///     println!("remove {}", name.to_string_lossy());
/// }
/// ```
pub fn inherited_git_variables(rule: InheritedGitEnvironment) -> Vec<OsString> {
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| rule_sheds(name, rule))
        .collect()
}

/// Build a command and sweep it with `rule`. This is the only place in this
/// crate that starts a command.
fn swept<S: AsRef<OsStr>>(program: S, rule: InheritedGitEnvironment) -> Command {
    #[expect(
        clippy::disallowed_methods,
        reason = "this function is the one entrance, so it holds the one raw spawn"
    )]
    let mut command = Command::new(program);
    shed_git_environment_from(&mut command, std::env::vars_os().map(|(key, _)| key), rule);
    command
}
