//! create — `swt create <name>`: a worktree that only exists if HEAD was green.
//!
//! The order of operations is the whole design, and it is not the obvious one.
//! The worktree is built *first* and the check runs *inside* it, because a fresh
//! worktree is a clean checkout of HEAD: uncommitted work in the parent — the
//! half-finished edit that is the normal state of the tree a subagent is being
//! branched from — cannot reach it, and so cannot fake a green. Checking the
//! parent instead would verify a commit nobody is about to branch from.
//!
//! Building first is what creates the window this module then has to close: for
//! as long as the check runs, a worktree and a branch exist that nobody has
//! agreed to keep. [`crate::teardown`] owns that window; `create` hands the
//! worktree over the moment `git worktree add` returns and takes it back only
//! once the check has passed.
//!
//! The names are the other half. A worktree directory and a branch are both
//! shared resources, and `swt` is for running several subagents at once, so
//! every run keys *both* of them on one [`UniqueToken`] built from this
//! process's id and the clock. Keying only the branch — which is what this
//! started as — leaves two concurrent `swt create <same-name>` calls fighting
//! over one directory, and a token spelled from the clock alone collides in the
//! millisecond an orchestrator fans them out in.
//!
//! What survives a failed check is reported, never assumed. Teardown is
//! best-effort — git refuses to remove a working tree whose `.git` link has gone
//! missing — and claiming a cleanup that did not happen would strand the user
//! with an orphaned worktree *and* branch they were told did not exist.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::{git_must, validate_worktree_name, WorktreeName, WORKTREE_NAME_RULE};
use crate::green_check::{is_green, shell_quote};
use crate::teardown::{hold_unverified_worktree, remove_unverified_worktree};

/// The git query that names the root of the worktree `swt` was invoked in.
const TOPLEVEL_ARGS: [&str; 2] = ["rev-parse", "--show-toplevel"];

/// Suffix every subagent worktree directory carries, so a stray directory beside
/// a repository is recognizable as `swt`'s at a glance.
const WORKTREE_SUFFIX: &str = ".swt";

/// Namespace every branch `swt` creates lives under.
const BRANCH_PREFIX: &str = "swt";

/// Radix the uniqueness token is spelled in — the Rust spelling of the
/// original's `Date.now().toString(36)`. Base 36 is the largest radix `char`
/// digits cover, and keeps the token to a handful of compact characters that are
/// all legal in both a branch name and a path component.
const TOKEN_RADIX: u32 = 36;

/// Spells a number in lowercase base 36.
///
/// `value` is the number to spell. Returns its digits, most significant first;
/// zero is `"0"` rather than the empty string.
fn base36(mut value: u128) -> String {
    let radix = u128::from(TOKEN_RADIX);
    let mut digits = String::new();
    // Division peels the digits off least-significant first, so they are pushed
    // in reverse and the string is flipped once at the end — cheaper and clearer
    // than repeatedly inserting at the front.
    loop {
        let remainder = value % radix;
        value /= radix;
        // Both conversions are total: a remainder below the radix always fits in
        // a `u32`, and `char::from_digit` covers every digit below 36. The
        // fallback exists only so an impossible case cannot panic.
        digits.push(
            u32::try_from(remainder)
                .ok()
                .and_then(|digit| char::from_digit(digit, TOKEN_RADIX))
                .unwrap_or('0'),
        );
        if value == 0 {
            break;
        }
    }
    digits.chars().rev().collect()
}

/// Milliseconds since the UNIX epoch, half of what the uniqueness token spells.
///
/// A clock somehow set before the epoch yields `0` rather than killing the
/// command: the token exists to distinguish two runs, and even a useless reading
/// is a better outcome than refusing to create a worktree over one.
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_millis())
}

/// Bits the packed uniqueness value reserves for the process id, low-order.
///
/// A `u32` pid is *exactly* 32 bits wide, which is what makes the packing
/// injective: no pid can reach into the timestamp's bits, so distinct
/// `(millis, pid)` pairs always pack to distinct numbers and therefore spell
/// distinct tokens. Two readings interleaved into one number rather than
/// concatenated as two strings for the same reason — `"1" + "23"` and
/// `"12" + "3"` are the same six characters, and a separator between them
/// would put a character in the token that is not a base-36 digit.
const PID_BITS: u32 = 32;

/// A token that distinguishes one `swt create` invocation from every other,
/// minted once per run and spelled in base 36.
///
/// It is built from *both* the process id and the clock, and needs both. The
/// clock alone — which is all the original spelled — collides whenever an
/// orchestrator fans two runs out inside the same millisecond, which is exactly
/// the situation `swt` exists to serve. The pid alone repeats as soon as the
/// operating system recycles it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UniqueToken(String);

impl UniqueToken {
    /// Mints the token for *this* invocation, from this process's id and the
    /// current time.
    fn mint() -> Self {
        Self::from_parts(std::process::id(), now_millis())
    }

    /// Mints a token from an explicit process id and millisecond timestamp.
    ///
    /// The pure seam behind [`UniqueToken::mint`]: it takes both readings as
    /// arguments so the property that actually matters — two processes minting
    /// in the same millisecond still get different tokens — can be pinned
    /// without racing two real clocks.
    fn from_parts(pid: u32, millis: u128) -> Self {
        // The shift cannot overflow a `u128` at any timestamp a clock can
        // produce: 32 bits of headroom leaves 96 for the milliseconds, and
        // 2^96 ms is some 10^18 times the age of the universe.
        Self(base36((millis << PID_BITS) | u128::from(pid)))
    }
}

impl fmt::Display for UniqueToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The two names one `swt create` invocation brings into existence: the worktree
/// directory and the branch checked out in it.
///
/// They are minted together, from one [`UniqueToken`], because that is the whole
/// guarantee — a path and a branch keyed on *different* tokens would be two
/// worktrees wearing one name, and two callers picking their own tokens is
/// exactly how that happens. There is no way to build one of these with a token
/// in only one of the two names.
struct WorktreeNaming {
    /// The worktree directory, beside the repository root.
    path: PathBuf,
    /// The branch checked out in it.
    branch: String,
}

impl WorktreeNaming {
    /// Names the worktree and branch for this invocation, minting the token they
    /// share.
    ///
    /// `root` is the repository root and `name` the validated worktree name.
    fn mint(root: &Path, name: &WorktreeName) -> Self {
        Self::with_token(root, name, &UniqueToken::mint())
    }

    /// Names both from a token supplied by the caller — a pure function of
    /// `(root, name, token)`, so the naming can be pinned without a clock, a
    /// repository or a subprocess.
    fn with_token(root: &Path, name: &WorktreeName, token: &UniqueToken) -> Self {
        Self {
            // The lexical parent, which is what resolving
            // `<root>/../<name>-<token>.swt` comes to: git answers
            // `--show-toplevel` with an absolute, already-normalized path, so
            // there is no `..` component left for a lexical step to get wrong. A
            // root with nothing above it stands in for itself, exactly as path
            // resolution treats `/..`.
            path: root
                .parent()
                .unwrap_or(root)
                .join(format!("{name}-{token}{WORKTREE_SUFFIX}")),
            branch: format!("{BRANCH_PREFIX}/{name}-{token}"),
        }
    }

    /// The worktree directory: a sibling of the repository root, so worktrees
    /// sit beside the repo rather than inside it, where git would have to be told
    /// to ignore them.
    fn path(&self) -> &Path {
        &self.path
    }

    /// The branch the worktree is created on.
    fn branch(&self) -> &str {
        &self.branch
    }
}

/// Creates a subagent worktree named `raw_name`, branched from a green HEAD.
///
/// The name is checked before any git runs, because it becomes both a branch and
/// a directory: `..` or a leading `-` would otherwise have git create the wrong
/// thing somewhere else entirely, and the cheapest place to stop that is before
/// anything exists to clean up. A rejected name is reported on stderr together
/// with the rule it broke, and the command fails.
///
/// On success the worktree's path — and nothing else — goes to stdout, so a
/// caller can capture it cleanly. On a red check the worktree and its branch are
/// torn down again, what that teardown actually did is reported on stderr, and
/// the command fails.
pub fn create(raw_name: &str) -> ExitCode {
    let Some(name): Option<WorktreeName> = validate_worktree_name(raw_name) else {
        eprintln!("Invalid worktree name {raw_name:?} — {WORKTREE_NAME_RULE}.");
        return ExitCode::FAILURE;
    };

    let root = PathBuf::from(git_must(TOPLEVEL_ARGS, None));
    // One token, both names: minted here and nowhere else, so the directory and
    // the branch a run leaves behind visibly belong to each other.
    let naming = WorktreeNaming::mint(&root, &name);
    let branch = naming.branch();
    let path = naming.path();

    // The worktree comes first and the check runs inside it. `git worktree add`
    // populates it from HEAD, so what gets verified is the commit a subagent
    // would actually branch from rather than the parent's working tree.
    git_must(
        [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-b"),
            OsStr::new(branch),
            path.as_os_str(),
            OsStr::new("HEAD"),
        ],
        Some(&root),
    );
    // Nothing has verified this worktree yet, and the check about to run can take
    // minutes. Until it passes, `swt` owns removing it — including when the user
    // gives up and hits Ctrl-C, and including when the check panics: the hold is
    // a guard, so an unwind past this point takes the worktree with it.
    let hold = hold_unverified_worktree(&root, path, branch);

    // Checked in the new worktree, configured from the parent: the `.swt-check`
    // override is an uncommitted per-developer file, so it exists only in `root`,
    // while the tree worth verifying is the fresh one. Swapping these two
    // directories is the difference between verifying HEAD and verifying
    // whatever the user happens to have half-written.
    let green = is_green(path, Some(&root));
    if !green.ok {
        // The verdict before the cleanup: why the worktree is going away matters
        // more than the fact that it did. The check's output already ends in a
        // newline of its own.
        eprint!("HEAD not green: {}", green.out);
        // Asked for explicitly rather than left to `hold`'s destructor, because
        // this is the one path with something to *say* about the teardown. The
        // drop that follows finds nothing left to do — the removal is latched.
        report_teardown(path, branch);
        return ExitCode::FAILURE;
    }

    // Verified: it is the caller's worktree now, not `swt`'s to tear down.
    hold.keep();

    // Only the path, and only on stdout — a caller captures this.
    println!("{}", path.display());
    ExitCode::SUCCESS
}

/// Tears the unverified worktree down and says what actually happened to it.
///
/// `path` is the worktree directory and `branch` the branch checked out in it,
/// both named in the message so a failed teardown leaves a copy-pasteable
/// recovery command behind.
fn report_teardown(path: &Path, branch: &str) {
    // A teardown nobody had left to do is reported as a cleanup, because the
    // state it claims is the state that holds: somebody else already got there.
    let failure = remove_unverified_worktree().filter(|torn| !torn.ok);
    let Some(failed) = failure else {
        eprintln!(
            "Cleaned up worktree {} and branch {branch}.",
            path.display()
        );
        return;
    };

    // Teardown is best-effort, so claiming it worked would strand the user with
    // an orphaned worktree *and* branch they were told did not exist. Report
    // git's own account, then the command that finishes the job by hand.
    eprint!("{}", failed.out);
    eprintln!(
        "Could not clean up {}. Remove it by hand:\n  git worktree remove --force {} && git branch -D {branch}",
        path.display(),
        shell_quote(&path.to_string_lossy())
    );
}

#[cfg(test)]
mod tests {
    use super::{base36, UniqueToken, WorktreeNaming};
    use crate::git::validate_worktree_name;
    use std::path::Path;

    /// A repository root for the naming tests.
    const ROOT: &str = "/repos/tools";

    /// A stand-in token, so the naming tests read as the pure functions of
    /// `(root, name, token)` that they are.
    const TOKEN: &str = "abc123";

    /// A validated name for the helpers under test.
    fn name(raw: &str) -> crate::git::WorktreeName {
        validate_worktree_name(raw).expect("fixture name should validate")
    }

    /// A token spelled literally, for tests about what is done *with* a token
    /// rather than about how one is minted.
    fn token(raw: &str) -> UniqueToken {
        UniqueToken(raw.to_string())
    }

    /// Names both halves of one invocation from a literal token.
    fn naming(raw_name: &str, raw_token: &str) -> WorktreeNaming {
        WorktreeNaming::with_token(Path::new(ROOT), &name(raw_name), &token(raw_token))
    }

    // The branch suffix has to be spelled exactly the way the TypeScript
    // original spelled it, digit for digit: `Date.now().toString(36)`. These are
    // that function's answers.
    #[test]
    fn base36_spells_the_same_digits_as_the_original() {
        let cases: [(u128, &str); 8] = [
            (0, "0"),
            (1, "1"),
            (35, "z"),
            (36, "10"),
            (37, "11"),
            (1295, "zz"),
            (1296, "100"),
            (1_706_651_234_567, "ls0w2whz"),
        ];
        for (value, expected) in cases {
            assert_eq!(base36(value), expected, "base36({value})");
        }
    }

    #[test]
    fn the_worktree_is_a_sibling_of_the_repository_root() {
        assert_eq!(
            naming("fix-parser", TOKEN).path(),
            Path::new("/repos/fix-parser-abc123.swt"),
            "the worktree belongs beside the repository, not inside it"
        );
    }

    #[test]
    fn a_root_with_no_parent_still_names_a_worktree() {
        let no_parent = WorktreeNaming::with_token(Path::new("/"), &name("x"), &token(TOKEN));
        assert_eq!(
            no_parent.path(),
            Path::new("/x-abc123.swt"),
            "a root with nothing above it resolves '..' to itself, as path resolution does"
        );
    }

    // The branch is namespaced under `swt/` so a repository's own branches are
    // never confused for a subagent's.
    #[test]
    fn a_branch_is_the_name_under_the_swt_namespace_with_the_token_suffixed() {
        assert_eq!(
            naming("fix-parser", TOKEN).branch(),
            "swt/fix-parser-abc123"
        );
    }

    // The heart of issue #284. The branch was already keyed for uniqueness and
    // the path was not, so the one resource two concurrent runs of the same name
    // actually shared — the directory — was the one nothing distinguished. Both
    // names carry the token, and it is the *same* token: a path and a branch
    // keyed differently would be two worktrees wearing one name.
    #[test]
    fn the_worktree_path_and_the_branch_carry_the_same_token() {
        let naming = naming("fix-parser", TOKEN);
        let file_name = naming
            .path()
            .file_name()
            .expect("a worktree path names a directory")
            .to_string_lossy()
            .into_owned();
        let path_token = file_name
            .strip_prefix("fix-parser-")
            .and_then(|rest| rest.strip_suffix(".swt"))
            .unwrap_or_else(|| {
                panic!("the worktree path must embed a uniqueness token, got {file_name:?}")
            });
        let branch_token = naming
            .branch()
            .strip_prefix("swt/fix-parser-")
            .unwrap_or_else(|| panic!("branch should be namespaced, got {:?}", naming.branch()));
        assert_eq!(
            path_token, branch_token,
            "the directory and the branch of one run must be keyed on one token"
        );
        assert_eq!(path_token, TOKEN, "and it must be the token supplied");
    }

    // Two `swt create <same-name>` runs differ only in their token, so the token
    // is the only thing that can keep them apart — in *both* names. Before the
    // fix the paths were equal, which is precisely the collision.
    #[test]
    fn one_name_with_two_tokens_names_two_worktrees_and_two_branches() {
        let first = naming("fix-parser", "aaaaaa");
        let second = naming("fix-parser", "bbbbbb");
        assert_ne!(
            first.path(),
            second.path(),
            "two runs of one name must not resolve to one directory"
        );
        assert_ne!(
            first.branch(),
            second.branch(),
            "two runs of one name must not resolve to one branch"
        );
    }

    // The secondary issue #284 raises: a token spelled from the clock alone
    // collides whenever an orchestrator fans two runs out inside the same
    // millisecond, which is exactly when it is fanning them out at all. The pid
    // is what makes them distinct, and it is taken as an argument here so the
    // property is a fact about the function rather than a race the test hopes to
    // win.
    #[test]
    fn two_processes_minting_in_the_same_millisecond_get_different_tokens() {
        let millis: u128 = 1_706_651_234_567;
        assert_ne!(
            UniqueToken::from_parts(4242, millis),
            UniqueToken::from_parts(4243, millis),
            "two pids in one millisecond must not mint one token"
        );
        assert_ne!(
            UniqueToken::from_parts(4242, millis),
            UniqueToken::from_parts(4242, millis + 1),
            "one pid across two milliseconds must not mint one token"
        );
        assert_eq!(
            UniqueToken::from_parts(4242, millis),
            UniqueToken::from_parts(4242, millis),
            "the same readings must mint the same token, or nothing is a function"
        );
    }

    // The token is spliced into a branch name and a path component without any
    // further escaping, so every character it can produce has to be legal in
    // both — which is the whole reason it is spelled in base 36.
    #[test]
    fn a_token_is_base36_and_legal_in_both_a_branch_name_and_a_path_component() {
        let readings: [(u32, u128); 4] = [
            (1, 0),
            (u32::from(u16::MAX), 1_706_651_234_567),
            (u32::MAX, 1_706_651_234_567),
            (99_999, u128::from(u64::MAX)),
        ];
        for (pid, millis) in readings {
            let token = UniqueToken::from_parts(pid, millis).to_string();
            assert!(
                !token.is_empty(),
                "an empty token distinguishes nothing: pid {pid}, millis {millis}"
            );
            assert!(
                token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "a token must be base 36: {token:?}"
            );
            assert!(
                validate_worktree_name(&format!("fix-parser-{token}")).is_some(),
                "a name with the token spliced in must stay inside the worktree name rule: \
                 {token:?}"
            );
        }
    }
}
