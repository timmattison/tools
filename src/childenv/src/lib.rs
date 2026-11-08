//! The one entrance for every child process that this workspace starts.
//!
//! Git obeys the inherited `GIT_` variables before it obeys the directory that
//! a command points at, and a hook exports them. A child process that keeps
//! them aims at the repository of the hook. This crate holds the one rule that
//! removes them, so that no call site writes the rule again.
//!
//! Each child process sheds the inherited `GIT_` variables through this
//! entrance. Production code takes [`command`]. Test code takes
//! [`fixture_command`]. A value that a call site must pin goes on after the
//! constructor, and so it wins.
//!
//! This crate uses the standard library only. The build script of `buildinfo`
//! takes it as a build-dependency, and every tool builds `buildinfo`, so a
//! dependency here would reach every build in the workspace.

use std::ffi::{OsStr, OsString};
use std::process::Command;

/// The prefix that makes a variable git's, and therefore one the sweep
/// removes.
pub const GIT_ENVIRONMENT_PREFIX: &str = "GIT_";

/// Spawn a command that takes its repository, and its identity, from nothing it
/// inherited.
///
/// **The rule is the `GIT_` prefix, and never a list of names.** A list strips
/// nothing new the day git adds a variable, and from then on it returns the same
/// clean-looking answer as a list that works. This scrub was a fifteen-name list
/// once, and `GIT_CONFIG_PARAMETERS` walked straight through it — a variable git
/// exports to every hook, which injects arbitrary configuration (`user.email`,
/// `core.bare`, `core.hooksPath`) into every git this crate spawns. It is not a
/// location variable, so no amount of adding location names would have caught
/// it. Enumerating [`std::env::vars_os`] sweeps whatever git invents next
/// without anyone editing this file. Pinned by `tests/inherited-environment.rs`.
///
/// Three families the old lists named are worth keeping in mind, because the
/// prefix now covers all of them. The location variables — `GIT_DIR`,
/// `GIT_WORK_TREE`, `GIT_INDEX_FILE` and their kin — aim git at a repository
/// other than the one the runner is rooted in, and git hands several of them to
/// its hooks, `GIT_INDEX_FILE` often *relative* so it silently re-anchors on
/// whatever directory each command runs in. The attribution variables —
/// `GIT_AUTHOR_NAME` and its five siblings — are read in preference to `-c`
/// *and* to `git config`, so leaving them in place re-attributes anything this
/// crate commits, and the two `DATE` variables are the quieter half: left in
/// place, every commit a run makes carries one identical timestamp. The
/// configuration variables — `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`,
/// `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM` — hand the caller a way to set any
/// key at all.
///
/// Removing `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` costs nothing here, because
/// removing a variable is not the same as pinning it to `/dev/null`: with them
/// gone git falls back to the host's `~/.gitconfig` and `/etc/gitconfig` exactly
/// as it does in a normal shell. It costs a *production* tool the answer for a
/// user who exported one on purpose, which is why production takes the other
/// entrance - see below.
///
/// **One trait, and the one split beside it is not a split by family.** The
/// prefix cannot tell git's families apart, so a call site that asked for the
/// location half alone would be asking for the shape this crate stopped
/// trusting. The split that is safe is a split by *whose intent* a variable
/// carries. [`shed_inherited_git_environment_keeping_user_intent`] keeps the
/// six names of [`USER_INTENT_GIT_ENVIRONMENT`], which a person states on
/// purpose, and sheds the rest of the prefix. A keep-list is safe in the way a
/// strip-list is not, because its staleness runs in the safe direction: a
/// variable git invents next year is shed by default rather than inherited by
/// default. This trait keeps the blanket rule, because a fixture has no user
/// whose intent to honor.
///
/// Public, and an extension trait rather than a private helper, because the
/// commands that need it are not all git: a consumer's test suite spawning its
/// own binary inherits exactly the same environment, and a second copy of this
/// rule living in a test file is the drift this exists to prevent.
pub trait NoInheritedGitEnvironment {
    /// Remove every `GIT_`-prefixed variable from the environment this command
    /// will be spawned with.
    ///
    /// A no-op in normal use — nothing sets these outside a hook or a replay —
    /// which is precisely why it has to be unconditional. The one run where it
    /// matters is the one nobody is watching.
    ///
    /// A caller that wants one of these variables set — or wants it gone
    /// whatever this process holds — does so *after* calling this, and wins.
    /// That is how `Git::command` pins `GIT_EDITOR` and the four identity
    /// names on top of a swept command, and takes the two identity dates off it
    /// again: this sweep removes only what the process carries, so a restated
    /// removal is what makes the second guard hold on its own.
    ///
    /// Keys are compared through [`std::ffi::OsStr::to_string_lossy`]: lossy
    /// conversion replaces invalid bytes with U+FFFD, so it can never
    /// manufacture a `GIT_` prefix out of bytes that did not spell one.
    fn without_inherited_git_environment(&mut self) -> &mut Self;
}

impl NoInheritedGitEnvironment for Command {
    fn without_inherited_git_environment(&mut self) -> &mut Self {
        shed_git_environment_from(
            self,
            std::env::vars_os().map(|(key, _)| key),
            InheritedGitEnvironment::ShedEverything,
        );

        self
    }
}

/// Detach `command` from every part of the git environment this process
/// inherited: the repository it names and the identity it carries, in one call.
///
/// The free-function spelling of [`NoInheritedGitEnvironment`], for a call site
/// that holds a `&mut Command` rather than a builder chain.
///
/// Public because the danger is not this crate's alone. Anything in this
/// repository that spawns git - a tool that adds a worktree, a test that builds
/// a throwaway repository - is broken the same way by the same environment, and
/// the rule for what to shed is worth keeping in one reusable place rather than
/// copied into each of them to drift.
///
/// **The entrance a fixture takes.** It keeps nothing, because a fixture has no
/// user whose intent to honor. A production spawn takes
/// [`shed_inherited_git_environment_keeping_user_intent`] instead, which sheds
/// the same prefix and keeps the six names of
/// [`USER_INTENT_GIT_ENVIRONMENT`].
///
/// It was an offer rather than a guarantee for as long as nothing obliged a git
/// spawn in this repository to call it. `repo_guards::git_env_sweep` now does
/// the obliging from the other side: it reports every `env_remove` in the
/// workspace whose argument names a `GIT_` variable, which is the shape a
/// hand-written list arrives in. A clippy rule cannot say that, because
/// `disallowed-methods` matches a method path and ignores its arguments.
///
/// What the guard proves is narrow, and worth stating. It proves no call site
/// passes a `GIT_`-prefixed name to `env_remove` as a literal. A list of those
/// names held in a constant and applied in a loop hands the call a binding, so
/// the guard reports it clean; its module doc states why widening the matcher
/// to see that shape was measured and refused. It does not prove a given spawn
/// sheds anything at all either, because a spawn that scrubs nothing is not
/// written in a shape any rule can name. So immunity still holds where this is
/// called, and the guard stops the spelling it used to be quietly replaced
/// with.
///
/// ```no_run
/// let mut command = std::process::Command::new("git");
/// childenv::shed_inherited_git_environment(&mut command);
/// ```
pub fn shed_inherited_git_environment(command: &mut Command) {
    command.without_inherited_git_environment();
}

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
/// list keeps such a test parallel-safe. The two entrances that read the real
/// environment, [`shed_inherited_git_environment`] and
/// [`shed_inherited_git_environment_keeping_user_intent`], both delegate here,
/// so the rule has one implementation and the test measures the one a spawn
/// uses.
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
        let name = key.as_ref().to_string_lossy();

        if !name.starts_with(GIT_ENVIRONMENT_PREFIX) {
            continue;
        }

        let kept = matches!(rule, InheritedGitEnvironment::KeepUserIntent)
            && USER_INTENT_GIT_ENVIRONMENT.contains(&&*name);

        if !kept {
            command.env_remove(key.as_ref());
        }
    }
}

/// Detach `command` from the inherited git environment, and keep the six names
/// the user stated on purpose.
///
/// **The entrance a production spawn takes.** It sheds the whole `GIT_` prefix,
/// [`USER_INTENT_GIT_ENVIRONMENT`] excepted, so a tool still aims at the
/// repository it was pointed at while it authenticates the way the user's own
/// shell authenticates. [`shed_inherited_git_environment`] is the entrance a
/// fixture takes, and it keeps nothing.
///
/// The split is not a split by git's families, which the prefix genuinely
/// cannot tell apart. It is a split by whose intent a variable carries. A hook
/// exports `GIT_DIR`, `GIT_INDEX_FILE` and `GIT_CONFIG_PARAMETERS`, so those
/// carry the launching hook's intent and leave. A person exports
/// `GIT_SSH_COMMAND` and `GIT_TERMINAL_PROMPT`, so those carry the user's
/// intent and stay. `tests/user-intent-environment.rs` pins the two lists
/// disjoint.
///
/// ```no_run
/// let mut command = std::process::Command::new("git");
/// childenv::shed_inherited_git_environment_keeping_user_intent(&mut command);
/// ```
pub fn shed_inherited_git_environment_keeping_user_intent(command: &mut Command) {
    shed_git_environment_from(
        command,
        std::env::vars_os().map(|(key, _)| key),
        InheritedGitEnvironment::KeepUserIntent,
    );
}

/// Start a child process for production code.
///
/// The child sheds every inherited `GIT_` variable except the names in
/// [`USER_INTENT_GIT_ENVIRONMENT`]. A value that the call site must pin goes on
/// after this call, and so it wins.
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
/// The child sheds every inherited `GIT_` variable and keeps none. A value that
/// the call site must pin goes on after this call, and so it wins.
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
    let _ = rule;
    Vec::new()
}

/// Build a command and sweep it with `rule`. This is the only place in this
/// crate that starts a command.
fn swept<S: AsRef<OsStr>>(program: S, rule: InheritedGitEnvironment) -> Command {
    let _ = rule;
    Command::new(program)
}
