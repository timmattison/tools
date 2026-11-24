//! `Repo` is the pre-flight a consumer runs before deciding a replay is worth
//! starting, so what it must get right is the *cheap rejection*: a directory
//! that is not a repository and a revision that does not resolve have to fail
//! here, clearly and by name, rather than surfacing later as a simulation that
//! mysteriously failed.
//!
//! Every fixture lives in its own `TempDir`, so concurrent `cargo test` runs
//! never share a path.

use gitscratch::testing::{
    conflicting_repo, default_branch_choice_repo, nested_conflict_repo, not_a_repository,
    numbered_lines, TestRepo,
};
use gitscratch::{Repo, Uncommitted, DEFAULT_BRANCHES};

/// Every porcelain record plain git reports, byte for byte.
///
/// Through [`TestRepo::try_git`] rather than [`TestRepo::git`], because the
/// second one trims its answer and the first character of the first record is
/// the index column - a space wherever the record belongs to the working tree,
/// which is exactly what the controls below read.
fn porcelain_records(fixture: &TestRepo) -> String {
    let output = fixture.try_git(
        &["status", "--porcelain", "-z", "--untracked-files=all"],
        &[],
    );

    assert!(
        output.status.success(),
        "a control has to be able to read the status it asserts about: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The whole point of opening a repository up front is that "you pointed me at
/// somewhere that is not a repository" is a different, cheaper answer than "the
/// simulation failed" - so it has to be said in those words.
///
/// The premise arrives through [`not_a_repository`] rather than through a bare
/// `TempDir`, because a bare one only *assumes* the premise: a developer whose
/// `TMPDIR` sits inside a git repository would see this test fail on the
/// `expect_err` below, blaming the pre-flight for accepting a directory that was
/// a repository all along. The fixture probes instead, and names the offending
/// path where the mistake actually is.
#[test]
fn open_rejects_a_directory_that_is_not_a_git_repository() {
    let outside = not_a_repository();

    let error = Repo::open(outside.path()).expect_err("a bare temp dir is not a git repository");

    let message = format!("{error:#}");
    assert!(
        message.contains("not inside a git repository"),
        "the error should say the directory is not a repository: {message}"
    );
    assert!(
        message.contains(&outside.path().display().to_string()),
        "the error should name the directory it was given: {message}"
    );
}

/// Opening a repository has to *lead somewhere*, and the somewhere is a scratch
/// worktree of that same repository.
///
/// This used to assert that `Repo::path()` handed back the directory `open` was
/// given, which was true and worth nothing: the checked path and an unchecked
/// one were the same `&Path`, so the pre-flight validated something and then
/// published a value that carried no trace of having been validated. Every
/// consumer was free to skip it, and `grist` did. `Repo::scratch` is the
/// replacement, so what is worth pinning is that the worktree it builds really
/// is a worktree of the repository that was opened - a door that leads to the
/// wrong room is worse than no door.
///
/// `main` is the fixture's branch, so its commit is the discriminator: a scratch
/// checked out anywhere else, or of anything else, cannot be sitting on it.
#[test]
fn scratch_builds_a_worktree_of_the_repository_that_was_opened() {
    let fixture = conflicting_repo();

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");
    let scratch = repo
        .scratch("main")
        .expect("create a scratch worktree of the fixture");

    assert_eq!(
        scratch
            .testing_git()
            .rev_parse("HEAD")
            .expect("read the scratch worktree's HEAD"),
        fixture.rev_parse("main"),
        "the scratch should be checked out at the opened repository's own 'main'"
    );
    assert!(
        scratch.path().is_dir(),
        "the scratch worktree should exist on disk at {}",
        scratch.path().display()
    );
}

/// The revision a scratch is built at is a revision, even when it starts with a
/// dash, and `git worktree add` has to read it as one.
///
/// `git worktree add -q --detach <path> --force` is a complete and valid
/// command. Git reads `--force` in the commit-ish slot as its own `--force`
/// flag, finds no commit-ish left, and builds the worktree at HEAD - exit 0,
/// no complaint. So a caller who asked for a scratch of one revision silently
/// got one of another, and every number measured in it describes work nobody
/// asked about. That is the cheap answer this crate exists never to give, and
/// it costs a whole simulation to produce.
///
/// `--force` rather than a name nobody would type, because it is the shape that
/// succeeds. A dash-leading name git does not know fails either way; this one
/// is the name that used to be obeyed.
#[test]
fn scratch_refuses_a_revision_that_starts_with_a_dash_rather_than_building_one_at_head() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let error = repo.scratch("--force").map(|_| ()).expect_err(
        "a revision that names no commit has to be refused, or the scratch is checked out \
         somewhere the caller never asked about and every measurement taken in it is about \
         another branch",
    );

    let message = format!("{error:#}");
    assert!(
        message.contains("--force"),
        "the refusal has to name the revision it could not use: {message}"
    );
}

/// A developer is hardly ever standing in the repository root, so the directory
/// a tool hands to [`Repo::open`] is usually a subdirectory of one.
///
/// `Repo::open` says in as many words that this works, and until now nothing
/// checked it: every fixture was opened at its own root, so the claim was carried
/// by a comment. It is the kind of claim that breaks quietly, too - the path the
/// pre-flight validated is private now, so no test can inspect it, and every
/// consequence of getting it wrong arrives as an answer that merely looks
/// smaller or fails somewhere else entirely.
///
/// So the assertions are the three things a subdirectory must not change, and
/// none of them is about the path itself:
///
/// - **Where a revision points**, which has nothing to do with the directory the
///   question was asked from.
/// - **What counts as uncommitted**, asserted over an edit made *outside* the
///   subdirectory: a status scoped to the cwd would report a clean tree and the
///   caveat about work a replay cannot see would go unsaid.
/// - **That a worktree still comes out**, since [`Repo::scratch`] is the only
///   route to one and it is the stored subdirectory that git is asked from.
///
/// Verified by mutation: making `Repo::open` refuse a non-empty
/// `rev-parse --show-prefix` fails this test, and scoping `uncommitted_files` to
/// the cwd with a `-- .` pathspec fails it too, while the rest of the suite stays
/// green in both cases.
#[test]
fn open_from_a_subdirectory_answers_for_the_whole_repository() {
    let fixture = nested_conflict_repo();
    let root = Repo::open(fixture.path()).expect("open the fixture repository at its root");

    let nested = fixture.path().join("sub").join("nested");
    let repo = Repo::open(&nested).expect("a subdirectory of a repository is inside one");

    assert_eq!(
        repo.resolve("left")
            .expect("resolve a branch from the subdirectory"),
        root.resolve("left")
            .expect("resolve the same branch from the root"),
        "a branch points where it points; which directory the question was asked \
         from is no part of the answer"
    );

    // At the repository root, so the edit sits outside the subdirectory the
    // question is being asked from.
    fixture.write_file("shared.txt", "locally edited, never committed\n");

    assert_eq!(
        repo.uncommitted_files()
            .expect("count uncommitted files from the subdirectory"),
        Uncommitted::new(1),
        "uncommitted work is uncommitted wherever it sits, so a count taken from \
         a subdirectory has to cover the whole repository"
    );

    let scratch = repo
        .scratch("main")
        .expect("create a scratch worktree from the subdirectory-opened repository");

    assert_eq!(
        scratch
            .testing_git()
            .rev_parse("HEAD")
            .expect("read the scratch worktree's HEAD"),
        fixture.rev_parse("main"),
        "the scratch should be checked out at the opened repository's own 'main', \
         not at anything the subdirectory implies"
    );
    assert!(
        scratch.path().is_dir(),
        "the scratch worktree should exist on disk at {}",
        scratch.path().display()
    );
}

/// The defect this whole type exists to kill: a mistyped branch name used to
/// come back as "you have conflicts" because a failed rebase and a bad argument
/// were indistinguishable. Resolving up front turns that into an error, and the
/// error is only actionable if it repeats the name that did not resolve.
#[test]
fn resolve_rejects_an_unresolvable_revision_and_names_it() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let error = repo
        .resolve("mian")
        .expect_err("'mian' is not a branch in the fixture");

    let message = format!("{error:#}");
    assert!(
        message.contains("mian"),
        "the error should name the revision that did not resolve: {message}"
    );
}

/// Resolving is what lets a caller compare candidates and detect a no-op before
/// building anything, so it has to agree with git about where a branch points.
#[test]
fn resolve_returns_the_commit_a_branch_points_at() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let resolved = repo.resolve("left").expect("resolve an existing branch");

    assert_eq!(
        resolved,
        fixture.rev_parse("left"),
        "resolve should agree with git about where 'left' points"
    );
}

/// `main` is the name almost every run means, so a repository that holds one
/// must not make the developer type it.
#[test]
fn branch_or_default_picks_main_when_the_caller_named_none() {
    let fixture = default_branch_choice_repo(&["main"]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let branch = repo
        .branch_or_default(None)
        .expect("a repository holding main has a default to pick");

    assert_eq!(branch, "main", "main is the first candidate");
}

/// The older name is still the default branch of plenty of repositories, and a
/// tool that only knew the newer one would refuse every one of them.
#[test]
fn branch_or_default_falls_back_to_master_when_there_is_no_main() {
    let fixture = default_branch_choice_repo(&["master"]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let branch = repo
        .branch_or_default(None)
        .expect("a repository holding master has a default to pick");

    assert_eq!(
        branch, "master",
        "master answers for a repository with no main"
    );
}

/// A repository that carries both names is the ordinary shape of one that was
/// renamed, and the new name is the one it is actually developed on. Order is
/// the whole content of that decision, so it gets a test rather than a comment.
#[test]
fn branch_or_default_prefers_main_over_master_when_both_resolve() {
    let fixture = default_branch_choice_repo(&["main", "master"]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let branch = repo
        .branch_or_default(None)
        .expect("a repository holding both has a default to pick");

    assert_eq!(branch, "main", "main outranks master");
}

/// The refusal is the point of this function, and it is only actionable if it
/// says which names were tried - the developer's own default branch is
/// whichever third name this repository actually uses.
///
/// Deliberately not a fallback to `HEAD`: a replay of HEAD onto HEAD answers
/// "clean" for every repository on earth, which is a wrong answer standing
/// where a refusal belongs.
#[test]
fn branch_or_default_refuses_a_repository_holding_neither_candidate_and_names_both() {
    let fixture = default_branch_choice_repo(&[]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let error = repo
        .branch_or_default(None)
        .expect_err("the fixture holds neither candidate");

    let message = format!("{error:#}");
    for candidate in DEFAULT_BRANCHES {
        assert!(
            message.contains(candidate),
            "the refusal should name every candidate it tried, and it does not \
             name {candidate}: {message}"
        );
    }
}

/// The refusal has to carry the reason the first candidate failed, not only the
/// fact that it did.
///
/// Every candidate is asked through [`Repo::resolve`], and that answer fails for
/// two different reasons: the branch is not there, which is the ordinary case,
/// and git could not read a branch that is - a corrupt object, a broken symref,
/// a locked ref. A refusal that drops the answer reports the second as the
/// first, and the developer reads "no default branch resolves here" while
/// looking at a `main` that `git branch` lists.
#[test]
fn branch_or_default_carries_the_first_candidates_failure_into_the_refusal() {
    let fixture = default_branch_choice_repo(&[]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let error = repo
        .branch_or_default(None)
        .expect_err("the fixture holds neither candidate");

    // Alternate formatting, because that is what a caller prints - `grind`
    // writes `{err:#}` for exactly this reason - and the cause chain is the
    // half of the message this test is about.
    let message = format!("{error:#}");
    let first = DEFAULT_BRANCHES[0];
    assert!(
        message.contains(&format!("could not resolve '{first}' to a commit")),
        "the refusal has to carry why the first candidate failed, and it \
         carries nothing: {message}"
    );
}

/// The default exists to spare the developer a name they always type, and it
/// must never overrule the one they did type.
#[test]
fn branch_or_default_keeps_the_branch_the_caller_named() {
    let fixture = default_branch_choice_repo(&["main", "master"]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let branch = repo
        .branch_or_default(Some("master"))
        .expect("a named branch needs no default");

    assert_eq!(
        branch, "master",
        "the named branch wins over an existing main"
    );
}

/// A named branch is handed straight back, unresolved, so the caller's own
/// resolution keeps saying which name failed. Folding the check in here would
/// answer a typo with this function's words instead, and those words are about
/// a default the caller never asked for.
#[test]
fn branch_or_default_hands_back_a_named_branch_that_does_not_resolve() {
    let fixture = default_branch_choice_repo(&["main"]);
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    let branch = repo
        .branch_or_default(Some("mian"))
        .expect("a named branch is not this function's to reject");

    assert_eq!(branch, "mian", "the name is handed back as it was given");
}

/// A clean tree has to read as clean, or every caller that warns about
/// uncommitted work would cry wolf on every run.
#[test]
fn uncommitted_files_is_zero_on_a_clean_tree() {
    let fixture = conflicting_repo();
    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(0),
        "a freshly committed fixture should have nothing uncommitted"
    );
}

/// "Uncommitted" means everything a replay would not carry with it, so all
/// three flavours count: what is staged, what is only in the working tree, and
/// what git is not tracking at all.
#[test]
fn uncommitted_files_counts_staged_unstaged_and_untracked_work() {
    let fixture = TestRepo::init();
    fixture.commit_files(
        &[
            ("staged.txt", "committed\n"),
            ("unstaged.txt", "committed\n"),
        ],
        "base",
    );

    std::fs::write(fixture.path().join("staged.txt"), "staged edit\n").expect("edit a file");
    fixture.git(&["add", "staged.txt"]);
    std::fs::write(fixture.path().join("unstaged.txt"), "unstaged edit\n").expect("edit a file");
    std::fs::write(fixture.path().join("untracked.txt"), "brand new\n").expect("write a new file");

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(3),
        "staged, unstaged and untracked work should each count"
    );
}

/// A renamed file is one uncommitted file, and the format the count is read
/// from is the one that makes that hard to see.
///
/// `git status --porcelain` writes a rename as a single `R  old -> new`, so
/// counting records was counting lines. Its NUL-separated form cannot do that -
/// a path may itself contain ` -> ` - and spends *two* fields on the one record
/// instead, the new name and then the old. Counting fields would call a moved
/// file two uncommitted files, and inflate every warning about uncovered work
/// in precisely the situation a developer is most likely to be in: mid-refactor,
/// with a pile of renames staged.
///
/// The two plain files beside the rename are what make this fail from both
/// directions, and both directions are reachable. Pair nothing and the answer is
/// 4, one field per name. Pair unconditionally - swallow whatever follows every
/// record rather than only what follows a rename - and it is 2. Only a count
/// that pairs exactly the rename gives 3.
#[test]
fn uncommitted_files_counts_a_rename_as_the_one_file_it_is() {
    let fixture = TestRepo::init();
    fixture.commit_file("before.txt", "committed\n", "base");

    // `git mv` stages the rename, which is what lets git's rename detection
    // report it as one `R` record rather than a delete beside an addition.
    fixture.git(&["mv", "before.txt", "after.txt"]);
    for name in ["one-more.txt", "two-more.txt"] {
        std::fs::write(fixture.path().join(name), "brand new\n").expect("write a new file");
    }

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(3),
        "a rename is one uncommitted file, not one per name it has had"
    );
}

/// A copied file is one uncommitted file as well, and a copy is the record no
/// `git mv` can produce.
///
/// Git spends the same two fields on a copy that it spends on a rename - `C  new`,
/// NUL, `old` - so the count has to pair that record too. It reports a copy only
/// where copy detection is on, `status.renames` is the key that turns it on, and
/// this crate pins nothing about that key: the harness reads the developer's own
/// configuration for it, and `copies` is a value a developer really carries. The
/// fixture arms the key in its own repository, which settles the question
/// whatever the developer's global configuration holds.
///
/// Git pairs a copy with a source the same change touches, so `big.txt` is
/// modified in the same staged set. That arming is what the control below reads
/// back, and the control is not a formality: an undetected copy comes back as
/// `A  copy.txt`, which is one field for one file, so the total is right for the
/// wrong reason and the pairing never runs at all.
///
/// The two untracked files beside the copy make this fail from both directions,
/// the same way the rename test does. Pair nothing and the answer is 5, one
/// field per name. Pair every record and it is 3. Only a count that pairs
/// exactly the copy gives 4.
#[test]
fn uncommitted_files_counts_a_staged_copy_as_the_one_file_it_is() {
    let fixture = TestRepo::init();
    fixture.commit_file("big.txt", &numbered_lines(10), "base");
    fixture.git(&["config", "status.renames", "copies"]);

    std::fs::copy(
        fixture.path().join("big.txt"),
        fixture.path().join("copy.txt"),
    )
    .expect("copy a tracked file");
    fixture.write_file("big.txt", &format!("{}line11\n", numbered_lines(10)));
    fixture.git(&["add", "copy.txt", "big.txt"]);
    for name in ["one-more.txt", "two-more.txt"] {
        fixture.write_file(name, "brand new\n");
    }

    let records = porcelain_records(&fixture);
    assert!(
        records.contains("C  copy.txt\0big.txt"),
        "copy detection is not armed, so this test could only pass vacuously: git \
         reports an undetected copy as `A  copy.txt`, one field for one file, and \
         the count below then comes out right without the pairing ever running. \
         Plain git reported {records:?}"
    );

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(4),
        "a copy is one uncommitted file, not one per name its content sits under"
    );
}

/// The *second* status column carries the letter as well, and a count that reads
/// only the first one pairs nothing at all in this fixture.
///
/// A porcelain record opens with two status bytes, one for the index and one for
/// the working tree, and git puts an `R` or a `C` in either. The working-tree
/// spelling arrives when the destination is in the index with no content behind
/// it, which is what `git add -N` records - and what `git add -p` records for a
/// new file, so it reaches a developer who never types `-N`. The record is the
/// same two fields with the index column blank: ` R moved.txt`, NUL, `big.txt`.
///
/// Both working-tree spellings sit in one fixture because one status call
/// reports both. `big.txt` is renamed to `moved.txt`, and `other-copy.txt` is
/// copied from the `other.txt` that the same fixture modifies, which is the
/// source copy detection needs. The controls read both records back through
/// plain git, since a git that stopped detecting either would leave the count
/// measuring an ordinary delete beside an untracked file.
///
/// This fails from both directions too. Pair nothing - which is what reading
/// only the index column does here, because both records hold a space there -
/// and the answer is 7. Pair every record and it is 4. Only a count that pairs
/// exactly the two working-tree records gives 5.
#[test]
fn uncommitted_files_counts_a_working_tree_rename_and_copy_as_the_files_they_are() {
    // Two files of their own content, because git pairs a copy with whichever
    // source matches it best: two files spelled alike leave the pairing free to
    // report the rename and the copy against the same name.
    let other = (1..=10).map(|n| format!("other{n}\n")).collect::<String>();
    let fixture = TestRepo::init();
    fixture.commit_files(
        &[("big.txt", &numbered_lines(10)), ("other.txt", &other)],
        "base",
    );
    fixture.git(&["config", "status.renames", "copies"]);

    std::fs::rename(
        fixture.path().join("big.txt"),
        fixture.path().join("moved.txt"),
    )
    .expect("rename a tracked file in the working tree");
    std::fs::copy(
        fixture.path().join("other.txt"),
        fixture.path().join("other-copy.txt"),
    )
    .expect("copy a tracked file in the working tree");
    fixture.write_file("other.txt", &format!("{other}other11\n"));
    // `-N` records the name in the index and none of the content, which is what
    // puts both destinations in the working-tree half of the status.
    fixture.git(&["add", "-N", "moved.txt", "other-copy.txt"]);
    for name in ["one-more.txt", "two-more.txt"] {
        fixture.write_file(name, "brand new\n");
    }

    let records = porcelain_records(&fixture);
    for expected in [" R moved.txt\0big.txt", " C other-copy.txt\0other.txt"] {
        assert!(
            records.contains(expected),
            "git no longer reports {expected:?} in the working-tree column, so this \
             test could only pass vacuously: an undetected move is a delete beside \
             an untracked file, which is two fields for two files and never pairs. \
             Plain git reported {records:?}"
        );
    }

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(5),
        "a move and a copy reported in the working-tree column are one file each"
    );
}

/// By default git collapses an untracked directory into a single line, so a
/// hundred new files would report as one. The count is meant to convey how much
/// work is sitting outside the commit graph, which makes that a lie worth
/// spending `--untracked-files=all` to avoid.
#[test]
fn uncommitted_files_counts_every_file_inside_an_untracked_directory() {
    let fixture = TestRepo::init();
    fixture.commit_file("tracked.txt", "committed\n", "base");

    let untracked = fixture.path().join("untracked-dir");
    std::fs::create_dir(&untracked).expect("create an untracked directory");
    std::fs::write(untracked.join("one.txt"), "one\n").expect("write a new file");
    std::fs::write(untracked.join("two.txt"), "two\n").expect("write a new file");

    let repo = Repo::open(fixture.path()).expect("open the fixture repository");

    assert_eq!(
        repo.uncommitted_files().expect("count uncommitted files"),
        Uncommitted::new(2),
        "an untracked directory should count its files, not itself"
    );
}

/// A repository with no working tree cannot answer this question at all, and
/// the error is part of the contract rather than an accident of it.
///
/// `git status` needs a working tree to take a status of, and a bare repository
/// has none. `Repo::uncommitted_files` documents that as its ordinary failure,
/// and tells a caller who wants the count as a *caveat* to read the failure as
/// no caveat — `unwrap_or_default`, which `Uncommitted` derives for the purpose.
/// That reading is only safe while the answer really is an error: a count that
/// silently answered zero would say the tree is clean, which is a different
/// statement about a repository that has no tree.
///
/// Nothing pinned the error. `grind`'s
/// `a_repository_with_no_working_tree_is_answered_rather_than_refused` asserts
/// the composite — no caveat printed, and the verdict still right — and an
/// `Ok(0)` here reads to that test exactly as an error does, so it would stay
/// green either way.
#[test]
fn uncommitted_files_refuses_a_repository_with_no_working_tree() {
    let fixture = conflicting_repo();
    let bare = fixture.bare_clone("main");

    let repo = Repo::open(bare.path()).expect("a bare repository is a git repository");

    let error = repo.uncommitted_files().map(|_| ()).expect_err(
        "a bare repository has no working tree to take a status of, so the count has to fail \
         rather than answer zero, which a caller reads as a clean tree",
    );

    let message = format!("{error:#}");
    assert!(
        message.contains("status"),
        "the error has to name the query that could not be answered, or a caller cannot tell \
         this failure from any other: {message}"
    );
}
