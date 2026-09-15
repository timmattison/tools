//! Bringing the branch up to date with the base, from watch mode.
//!
//! The `R` key rebases the branch onto the base, and the `M` key merges the
//! base into the branch. Neither act is gsw's own. Each key runs one command
//! that the user supplies, in the user's own interactive shell, and that
//! command pushes the branch when it has finished. [`crate::shell`] holds the
//! shell, the type a command name becomes, and the probe that asks whether the
//! command exists.
//!
//! What is here is what belongs to these two keys alone: the variable that
//! names each command, the name each key falls back on, the line the shell
//! runs, and the run itself.

use shellquote::shell_quote;

use crate::shell::ShellCommand;

/// The variable that holds the command `R` runs.
///
/// The value is a whole command line, so it can carry arguments. See
/// [`ShellCommand`] for what gsw does with each part of it.
pub(crate) const REBASE_COMMAND_ENV: &str = "GSW_REBASE_COMMAND";

/// The variable that holds the command `M` runs.
pub(crate) const MERGE_COMMAND_ENV: &str = "GSW_MERGE_COMMAND";

/// The command `R` runs when the environment names none.
///
/// This repository ships no `grp`. It is a shell function that the user
/// supplies, and it is the default here because it is the name one person's rc
/// file gives it. [`REBASE_COMMAND_ENV`] names a different one, and a value of
/// nothing but space turns the key off.
pub(crate) const DEFAULT_REBASE_COMMAND: &str = "grp";

/// The command `M` runs when the environment names none. See
/// [`DEFAULT_REBASE_COMMAND`] for why this repository ships no such command.
pub(crate) const DEFAULT_MERGE_COMMAND: &str = "gmp";

/// Which act a key of watch mode asks the user's command to carry out.
///
/// **The value answers for its own key.** Two keys run the same code over two
/// sets of words, so every difference between them is a method here: the
/// variable that names the command, the command the key falls back on, the
/// letter the user presses, the verb every message about the act uses, and the
/// advice a refused run gives. A caller that matched on the act a second time
/// to pick one of those would be the place the two sets drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BaseUpdate {
    /// Put the commits of the branch on top of the base, then push. `R`.
    Rebase,
    /// Bring the base into the branch as a commit of its own, then push. `M`.
    Merge,
}

impl BaseUpdate {
    /// Both acts, for a caller that does the same work for each of them.
    ///
    /// The probe starts one shell for each key at startup, and the tests here
    /// state a rule once and hold it for both. An array rather than an
    /// iterator, so a variant added later fails to compile here rather than
    /// going quietly unprobed.
    pub(crate) const ALL: [Self; 2] = [Self::Rebase, Self::Merge];

    /// The variable that names the command this act runs.
    pub(crate) const fn env(self) -> &'static str {
        match self {
            Self::Rebase => REBASE_COMMAND_ENV,
            Self::Merge => MERGE_COMMAND_ENV,
        }
    }

    /// The command this act runs where its variable names none.
    pub(crate) const fn default_command(self) -> &'static str {
        match self {
            Self::Rebase => DEFAULT_REBASE_COMMAND,
            Self::Merge => DEFAULT_MERGE_COMMAND,
        }
    }

    /// The key the user presses for this act.
    ///
    /// Both letters are capitals. A rebase rewrites the commits of the branch
    /// and a merge writes a commit, so neither belongs on a key that a hand
    /// resting on the keyboard reaches by accident.
    pub(crate) const fn key(self) -> char {
        match self {
            Self::Rebase => 'R',
            Self::Merge => 'M',
        }
    }

    /// The word every message about this act uses for it.
    pub(crate) const fn verb(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Merge => "merge",
        }
    }

    /// What a refused run tells the user to do.
    ///
    /// The letter comes from [`BaseUpdate::key`] rather than from a string of
    /// its own, so the advice names the key that exists rather than the key
    /// that existed when somebody wrote the sentence. Pressing it again asks
    /// the question against the repository as it stands now, which is the whole
    /// remedy — the same remedy [`crate::push`] gives, in the same words.
    pub(crate) fn retry_advice(self) -> String {
        format!("press {} again", self.key())
    }
}

/// A confirmed rebase or merge: the act, the branch the question named, the
/// base it named, and the command the user supplied.
///
/// The four are one value because they are one sentence — rebase *this branch*
/// onto *this base*, with *this command*. The command alone does not say which
/// branch: `grp` reads HEAD when the shell starts it, and that is not
/// necessarily what HEAD pointed at when the question went on the screen. The
/// answer arrives whenever the user presses `y`, and a checkout in another pane
/// fits in between. Carrying the branch beside the command is what lets [`run`]
/// refuse a repository that moved on.
///
/// Built only by the question of this module, so a command nobody confirmed
/// cannot be assembled somewhere else and handed to the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaseUpdateCommand {
    /// Which act the question asked about.
    update: BaseUpdate,
    /// The branch the question named, as [`crate::repo::branch_name`] reports
    /// it.
    branch: String,
    /// The base the question named, which is the last word of the script.
    base: String,
    /// The command the user supplied, arguments and all.
    command: ShellCommand,
}

impl BaseUpdateCommand {
    /// The command that runs `command` for `update` against `base`, on
    /// `branch`.
    ///
    /// Private on purpose. The question of this module is the only caller, so
    /// every value of this type describes a question that a user answered.
    fn new(
        update: BaseUpdate,
        branch: impl Into<String>,
        base: impl Into<String>,
        command: ShellCommand,
    ) -> Self {
        Self {
            update,
            branch: branch.into(),
            base: base.into(),
            command,
        }
    }

    /// Which act the question asked about.
    pub(crate) fn update(&self) -> BaseUpdate {
        self.update
    }

    /// The branch the question named.
    pub(crate) fn branch(&self) -> &str {
        &self.branch
    }

    /// The base the question named.
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// The command the user supplied, which is what every message names.
    pub(crate) fn command(&self) -> &ShellCommand {
        &self.command
    }

    /// The line the shell runs: the whole command line, then the base.
    ///
    /// **The base goes on the line, and it goes last.** `grp` and `gmp` fall
    /// back on `main`, so a bare `grp` fails in a repository whose base is
    /// `master`. The question names the base, and what the user confirms is
    /// what runs, so the name of the base belongs in the line rather than in
    /// the defaults of somebody's shell function. Last, because everything in
    /// front of it is the user's own arguments: a value of `grp --fork-point`
    /// runs `grp --fork-point 'main'`.
    ///
    /// **The base is quoted and the command line is not.** The two halves come
    /// from two places. The command line is what the user wrote into the
    /// variable, and the shell reads it here the way it reads it at an
    /// interactive prompt — see [`crate::shell`], which states that rule for
    /// every key that runs a command. The base is a branch name gsw read out of
    /// a repository, and git accepts a great deal in one: a name that carries a
    /// space, a quotation mark, or a semicolon would otherwise leave the line
    /// as several words, and a semicolon would leave it as several commands.
    /// [`shell_quote`] makes it one word whatever is in it.
    pub(crate) fn script(&self) -> String {
        format!("{} {}", self.command.name(), shell_quote(&self.base))
    }
}

#[cfg(test)]
mod script_tests {
    use super::*;

    /// The command a question about `update` on `issue-12` would have carried,
    /// for a variable holding `value` and a base of `base`.
    fn confirmed(update: BaseUpdate, value: &str, base: &str) -> BaseUpdateCommand {
        BaseUpdateCommand::new(
            update,
            "issue-12",
            base,
            ShellCommand::new(Some(value), update.default_command()).expect("a name"),
        )
    }

    #[test]
    fn the_script_ends_with_the_base_as_a_quoted_word() {
        // `grp` falls back on `main`, so a repository whose base is `master`
        // gets `no such branch or commit: 'main'` from a bare `grp`.
        assert_eq!(
            confirmed(BaseUpdate::Rebase, "grp", "master").script(),
            "grp 'master'",
        );
        assert_eq!(
            confirmed(BaseUpdate::Merge, "gmp", "main").script(),
            "gmp 'main'",
        );
    }

    #[test]
    fn the_arguments_of_the_command_stay_in_front_of_the_base() {
        // Everything the user wrote after the first word is an argument of
        // their own command, and the base is an argument gsw adds after them.
        assert_eq!(
            confirmed(BaseUpdate::Rebase, "grp --fork-point", "main").script(),
            "grp --fork-point 'main'",
        );
    }

    #[test]
    fn a_base_that_carries_a_semicolon_is_still_one_word() {
        // git takes a branch name with a semicolon in it, and the shell reads
        // an unquoted semicolon as the end of one command and the start of the
        // next. The quotation is what keeps the name a name.
        assert_eq!(
            confirmed(BaseUpdate::Rebase, "grp", "main;touch pwned").script(),
            "grp 'main;touch pwned'",
        );
    }
}
