//! The gate that asks before gitnuke deletes a branch that no recent commit names.
//!
//! A squash merge names its branch on a `Branch: <branch>` line. So the recent
//! commits of the main worktree's branch name each branch that is safe to nuke,
//! and a typo gives a name they do not name. `confirm` is the entrance. It reads
//! the last commits of the main branch and of its upstream, and it asks when none
//! names the branch. `names_branch` finds a branch name in a message as a whole
//! token. `ask` puts a yes or no question to the person at the terminal.

use std::io::{BufRead, IsTerminal, Write};
use std::process::Output;

use colored::Colorize;

use crate::{BranchName, ExitCode, NukeError, NukeOptions, Worktree};

/// How many recent commits of each ref gitnuke reads to find a branch name.
pub(crate) const RECENT_COMMIT_COUNT: usize = 10;

/// A ref to read the recent commits of, and the name to show for it.
struct RecentRef {
    /// What git resolves, for example `main` or `main@{upstream}`.
    reference: String,
    /// What the messages show, for example `main` or `origin/main`.
    label: String,
}

/// Decide whether a nuke of `worktree` may go on, asking the person if needed.
///
/// A detached worktree has no branch to delete, so nothing can be mistyped and
/// this function returns `Ok`. So does `--yes`. Otherwise the branch must appear
/// in the last [`RECENT_COMMIT_COUNT`] commits of the branch of `main` or of its
/// upstream. The upstream matters because `gh pr merge` in a worktree does not
/// move the local main branch. gitnuke does not fetch and uses no network.
///
/// A dry run never asks. It reports the refusal that a real run could give, so
/// `gitnuke -n x` fails when `gitnuke x` can fail. A ref that git cannot read
/// adds nothing, which is the safe direction because it leads to the question.
pub(crate) fn confirm(
    main: &Worktree,
    worktree: &Worktree,
    options: NukeOptions,
) -> Result<(), NukeError> {
    let Some(branch) = &worktree.branch else {
        return Ok(());
    };
    if options.yes {
        return Ok(());
    }

    let refs = recent_refs(main);
    if refs
        .iter()
        .any(|recent| recent_commits_name(main, recent, branch))
    {
        return Ok(());
    }

    let labels = refs
        .iter()
        .map(|recent| recent.label.as_str())
        .collect::<Vec<_>>()
        .join(" or ");
    let path = &worktree.path;
    let not_named =
        format!("no commit in the last {RECENT_COMMIT_COUNT} of {labels} names branch '{branch}'");
    let fetch_advice = "If its PR merged moments ago, run git fetch and try again.";

    if options.dry_run {
        return Err(NukeError::new(
            ExitCode::BranchNotNamed,
            format!(
                "{not_named}, so a real run asks before it removes worktree {path}.\n  \
                 {fetch_advice} To skip the question, add --yes."
            ),
        ));
    }

    let question = format!(
        "{} {not_named}.\n  If you typed the wrong name, answer n. {fetch_advice}\n  \
         Remove worktree {path} and delete branch {branch}? [y/N] ",
        "gitnuke:".yellow().bold(),
    );
    let answer = ask(
        &question,
        std::io::stdin().is_terminal(),
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )
    .map_err(|error| {
        NukeError::new(
            ExitCode::BranchNotNamed,
            format!("could not ask about branch '{branch}': {error}"),
        )
    })?;

    match answer {
        Answer::Yes => Ok(()),
        Answer::No => Err(NukeError::new(
            ExitCode::BranchNotNamed,
            format!("kept worktree {path} and branch '{branch}', because the answer was not yes"),
        )),
        Answer::NoTerminal => Err(NukeError::new(
            ExitCode::BranchNotNamed,
            format!(
                "{not_named}, and stdin is not a terminal, so gitnuke cannot ask.\n  \
                 {fetch_advice} To nuke it anyway, add --yes."
            ),
        )),
    }
}

/// List the refs whose recent commits can name a branch.
///
/// The branch of the main worktree comes first, then its upstream when it has
/// one. A main worktree with a detached HEAD gives `HEAD` alone.
fn recent_refs(main: &Worktree) -> Vec<RecentRef> {
    let Some(branch) = &main.branch else {
        return vec![RecentRef {
            reference: "HEAD".to_string(),
            label: "HEAD".to_string(),
        }];
    };
    let mut refs = vec![RecentRef {
        reference: branch.to_string(),
        label: branch.to_string(),
    }];
    if let Some(upstream) = upstream_label(main, branch) {
        refs.push(RecentRef {
            reference: format!("{branch}@{{upstream}}"),
            label: upstream,
        });
    }
    refs
}

/// The short name of the upstream of `branch`, such as `origin/main`, if it has one.
fn upstream_label(main: &Worktree, branch: &BranchName) -> Option<String> {
    let output = childenv::command("git")
        .args([
            "rev-parse",
            "--abbrev-ref",
            &format!("{branch}@{{upstream}}"),
        ])
        .current_dir(main.path.as_path())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let label = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!label.is_empty()).then_some(label)
}

/// Tell whether a message of the recent commits of `recent` names `branch`.
///
/// A ref that git cannot read names nothing.
fn recent_commits_name(main: &Worktree, recent: &RecentRef, branch: &BranchName) -> bool {
    recent_messages(main, &recent.reference)
        .is_some_and(|messages| names_branch(&messages, branch.as_str()))
}

/// Read the messages of the last [`RECENT_COMMIT_COUNT`] commits of `reference`.
///
/// `-z` ends each message with NUL, and `names_branch` takes NUL as a boundary,
/// so one string holds all of them. Returns `None` when git fails.
fn recent_messages(main: &Worktree, reference: &str) -> Option<String> {
    let output: Output = childenv::command("git")
        .args([
            "log",
            "-n",
            &RECENT_COMMIT_COUNT.to_string(),
            "-z",
            "--format=%B",
        ])
        .arg(reference)
        .arg("--")
        .current_dir(main.path.as_path())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Tell whether `message` names `branch` as a whole token.
///
/// The character before the match and the character after it must not be a
/// name character. The start and the end of the message count as boundaries.
/// An empty `branch` names nothing.
///
/// A substring match is the defect this check exists to prevent. `issue-70`
/// is a substring of `Branch: issue-704`, so a substring match lets that typo
/// through.
pub(crate) fn names_branch(message: &str, branch: &str) -> bool {
    if branch.is_empty() {
        return false;
    }
    // `match_indices` gives offsets on char boundaries. `get` returns `None`
    // instead of panicking if an offset is ever not on one.
    message.match_indices(branch).any(|(start, _)| {
        let before = message
            .get(..start)
            .and_then(|head| head.chars().next_back());
        let after = message
            .get(start + branch.len()..)
            .and_then(|tail| tail.chars().next());
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

/// Tell whether `c` can be part of a branch name token.
///
/// The set is ASCII letters, ASCII digits, `.`, `_`, `/` and `-`. Any other
/// character, such as a space, a bracket, `\r`, NUL or a non-ASCII character,
/// ends a token.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-')
}

/// What the person answered, or that nobody could answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// `y` or `yes`, in any case.
    Yes,
    /// Any other line, an empty line, or end of input.
    No,
    /// Standard input is not a terminal, so gitnuke asked nothing.
    NoTerminal,
}

/// Tell whether `answer` is `y` or `yes`, in any case, after trimming.
fn is_yes(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// Put `question` to the person and read one line of answer.
///
/// With no terminal, this function writes nothing, reads nothing, and returns
/// `Answer::NoTerminal`. A caller that runs without a person, such as an agent,
/// can type the wrong target too, so the caller must not continue silently.
///
/// With a terminal, it writes `question` to `output` as given, flushes, and
/// reads one line. Only `y` or `yes` gives `Answer::Yes`. End of input gives
/// `Answer::No`.
pub(crate) fn ask(
    question: &str,
    terminal: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> std::io::Result<Answer> {
    if !terminal {
        return Ok(Answer::NoTerminal);
    }
    output.write_all(question.as_bytes())?;
    output.flush()?;
    let mut line = String::new();
    // Zero bytes read is end of input, and `is_yes("")` is false.
    input.read_line(&mut line)?;
    Ok(if is_yes(&line) {
        Answer::Yes
    } else {
        Answer::No
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: &str = "Branch not found in recent commits. Delete it? [y/N] ";

    fn run(input: &str) -> (Answer, Vec<u8>) {
        let mut out = Vec::new();
        let answer = ask(Q, true, &mut input.as_bytes(), &mut out).unwrap();
        (answer, out)
    }

    #[test]
    fn match_at_end_of_message() {
        assert!(names_branch("Squash\n\nBranch: issue-42", "issue-42"));
        assert!(!names_branch("Squash\n\nBranch: issue-704", "issue-70"));
    }

    #[test]
    fn shorter_number_is_not_a_match() {
        assert!(!names_branch("Branch: issue-70", "issue-7"));
        assert!(names_branch("Branch: issue-7", "issue-7"));
    }

    #[test]
    fn brackets_are_boundaries() {
        assert!(names_branch("fix: thing (issue-42)", "issue-42"));
        assert!(!names_branch("fix: thing (xissue-42)", "issue-42"));
    }

    #[test]
    fn match_at_start_of_message() {
        assert!(names_branch("issue-42 merged", "issue-42"));
        assert!(!names_branch("issue-421 merged", "issue-42"));
    }

    #[test]
    fn crlf_is_a_boundary() {
        assert!(names_branch("Branch: issue-42\r\n", "issue-42"));
        assert!(!names_branch("Branch: issue-42x\r\n", "issue-42"));
    }

    #[test]
    fn multi_byte_neighbours_do_not_panic() {
        assert!(names_branch("日本語 issue-42 🎉", "issue-42"));
        assert!(names_branch("日本語issue-42🎉", "issue-42"));
        assert!(!names_branch("日本語 issue-42.🎉", "issue-42"));
        assert!(!names_branch("café issue-42-é", "issue-42"));
    }

    #[test]
    fn slash_names_match_whole() {
        assert!(names_branch("Branch: feature/login", "feature/login"));
        assert!(!names_branch("Branch: feature/login/v2", "feature/login"));
        assert!(!names_branch("Branch: feature/login", "login"));
    }

    #[test]
    fn a_later_occurrence_counts() {
        assert!(names_branch("Branch: issue-421 and issue-42", "issue-42"));
        assert!(!names_branch("Branch: issue-421 and issue-420", "issue-42"));
        assert!(!names_branch("anything", ""));
    }

    #[test]
    fn nul_separated_messages() {
        assert!(names_branch("first\0Branch: issue-42\0", "issue-42"));
        assert!(!names_branch("first\0Branch: issue-420\0", "issue-42"));
    }

    #[test]
    fn y_continues_and_n_refuses() {
        assert_eq!(run("y\n").0, Answer::Yes);
        assert_eq!(run("n\n").0, Answer::No);
    }

    #[test]
    fn yes_in_any_case_continues() {
        for line in ["Y\n", "yes\n", "YES\n", "Yes\r\n"] {
            assert_eq!(run(line).0, Answer::Yes, "{line:?}");
        }
        assert_eq!(run("yep\n").0, Answer::No);
    }

    #[test]
    fn empty_line_and_end_of_input_refuse() {
        assert_eq!(run("\n").0, Answer::No);
        assert_eq!(run("").0, Answer::No);
    }

    #[test]
    fn only_the_first_line_is_read() {
        assert_eq!(run("yes please\n").0, Answer::No);
        assert_eq!(run("n\ny\n").0, Answer::No);
    }

    #[test]
    fn question_goes_to_output_verbatim() {
        let (_, out) = run("y\n");
        assert_eq!(out, Q.as_bytes());
    }

    #[test]
    fn no_terminal_asks_nothing_and_reads_nothing() {
        let mut out = Vec::new();
        let mut input: &[u8] = b"y\n";
        let answer = ask(Q, false, &mut input, &mut out).unwrap();
        assert_eq!(answer, Answer::NoTerminal);
        assert!(out.is_empty());
        assert_eq!(input, b"y\n");
    }
}
