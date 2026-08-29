//! The whole pipeline, over the repository this crate is built from.
//!
//! Every other test of this crate reads a fixture, and a fixture is a file
//! somebody wrote to suit the rule it tests. This one reads a tree nobody wrote
//! for it: the walk finds the files, the counter reads each one, the module
//! pass resolves the declarations across them, and the summary adds them up —
//! the whole of a run, in the order a run does it, with no `cdva` binary
//! involved so that nothing here depends on a build artifact.
//!
//! Four things are asserted, and the third is the one worth having:
//!
//! 1. **The tool finds Rust test code.** This is what the issue asks for: a
//!    Rust row whose test share is above zero proves the pipeline holds end to
//!    end. No exact number is pinned. The counts move every time somebody
//!    writes a line, and a test that has to be edited on every commit is a test
//!    people learn to edit rather than to believe.
//! 2. **The sum invariant.** For every counted file of the repository, the
//!    production bucket plus the test bucket equals what the classifier alone
//!    reports, field by field. The whole tool rests on this, and here it is
//!    asserted at the scale of a real tree rather than over fixtures.
//! 3. **All three marking rules fire on real data.** The path rule, the tree
//!    rule, and the cross-file module pass each mark at least one file of this
//!    repository. A rule that quietly stopped working would still pass every
//!    fixture built to suit it, and would still leave a report full of
//!    plausible numbers; this is the assertion that notices.
//!
//!    Three sources of this repository declare `#[cfg(test)] mod <name>;`, and
//!    two of the three produce a module-declaration span. The third,
//!    `src/beta/src/main.rs`, names `src/beta/src/test.rs`, which is itself
//!    nothing but a `#[cfg(test)] mod tests { … }` — so the tree rule has
//!    already marked every row of it, and the module pass leaves a file with no
//!    production row exactly as it found it. That is the pass being idempotent
//!    rather than the pass failing: the one span the file carries names the
//!    rule that really decided it. The assertion is therefore written as "at
//!    least one" and not as a count of the declarations.
//! 4. **More than one language appears**, so a walk that somehow found only one
//!    corner of the tree fails here rather than reading clean.
//!
//! The run reads and never writes, and it names no shared resource, so any
//! number of copies of it may run at once.

use cdva::{
    count, resolve_test_modules, walk, Counter, Counts, FileCount, PathRules, Rule, Summary,
    TreeMode, TreeRules, WalkOptions,
};
use std::path::PathBuf;

/// The label of the Rust row of the summary.
const RUST: &str = "Rust";

/// The directories of the repository root that hold no source anybody wrote.
///
/// Both are build output that the repository's own `.gitignore` names, and
/// `target` alone runs to several gigabytes. They are named here rather than
/// left to the ignore files because this walk turns the ignore files off; see
/// [`roots`].
const BUILD_DIRECTORIES: &[&str] = &["target", "node_modules"];

/// How the walk reads the tree.
///
/// `no_ignore` is on so that a contributor's *global* gitignore, or the
/// `.git/info/exclude` of one checkout, cannot change which files this test
/// reads. Neither of those files is committed, so a walk that obeyed them would
/// measure a different repository on every machine, and the assertions below
/// would then hold or fail for a reason that has nothing to do with the tool.
const HOW: WalkOptions = WalkOptions {
    hidden: false,
    no_ignore: true,
};

/// The root of this repository, found from the manifest directory of this
/// crate.
///
/// `CARGO_MANIFEST_DIR` is `<repository>/src/cdva`, so the root is two levels
/// above it. Nothing here shells out to `git`: a test that ran `git rev-parse`
/// would answer for whatever repository the environment of the run pointed it
/// at, which under a pre-commit hook is not the one the sources are in. An
/// absolute path written here would name one checkout, and this repository is
/// worked in worktrees.
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Every top-level entry of the repository worth walking.
///
/// Turning the ignore files off ([`HOW`]) turns the repository's own
/// `.gitignore` off with them, and that one names `target/`. So the roots are
/// derived from the directory itself and the two build directories are dropped
/// by name. Deriving them rather than listing the source directories is what
/// keeps this from going blind: a directory added to the repository tomorrow is
/// counted without anybody remembering to add it here, and a guard that reads
/// four of seven directories reports clean for exactly the same reason a
/// correct one does.
///
/// A hidden entry is dropped as well, because the walk drops a hidden file
/// anywhere below a root and a hidden root would otherwise be the one exception.
fn roots() -> Vec<PathBuf> {
    let root = repository_root();
    let entries = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("{} reads: {error}", root.display()));

    let mut roots = Vec::new();
    for entry in entries {
        let entry = entry.expect("a directory entry of the repository root reads");
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') || BUILD_DIRECTORIES.contains(&name) {
            continue;
        }
        roots.push(entry.path());
    }
    roots.sort();

    assert!(
        roots.len() > 5,
        "only {} entries of {} are being walked, so this is not the repository",
        roots.len(),
        root.display()
    );
    roots
}

#[test]
fn counting_this_repository_holds_the_invariant_and_fires_every_rule() {
    let found = walk(&roots(), HOW).expect("the repository this crate is built from can be walked");
    assert!(
        found.len() > 500,
        "the walk found {} files, which is not this repository",
        found.len()
    );

    let counter =
        Counter::new(PathRules::builtin()).with_tree_rules(TreeRules::new(), TreeMode::Auto);

    // One read per file, and the classifier's own answer kept beside the split
    // one. Reading a file twice would let a save between the two reads look
    // like a broken invariant.
    let mut files: Vec<FileCount> = Vec::new();
    let mut unsplit: Vec<Counts> = Vec::new();
    for (path, relative) in &found {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(source) = String::from_utf8(bytes) else {
            continue;
        };
        let Some(counted) = counter.count_source(path, relative, &source) else {
            continue;
        };
        unsplit.push(count(&source, counted.language));
        files.push(counted);
    }

    // The cross-file pass, which is the last thing a run does before it adds
    // the counts up. It runs here so that everything below reads the numbers a
    // real run would print.
    resolve_test_modules(&mut files);
    let summary = Summary::new(files);

    assert_eq!(
        summary.files.len(),
        unsplit.len(),
        "a file was counted without its unsplit count, so the comparison below \
         would silently skip the tail"
    );
    assert!(
        summary.files.len() > 300,
        "only {} files of this repository were counted",
        summary.files.len()
    );

    // 2. The sum invariant, over every counted file of the whole repository.
    for (file, expected) in summary.files.iter().zip(&unsplit) {
        let total = file.total();
        let named = file.path.display();
        assert_eq!(total.blank, expected.blank, "{named}: the blank rows");
        assert_eq!(total.comment, expected.comment, "{named}: the comment rows");
        assert_eq!(total.code, expected.code, "{named}: the code rows");
    }

    // 3. Every marking rule, on a tree nobody built to suit it.
    let mut glob_files = 0_usize;
    let mut tree_files = 0_usize;
    let mut declared: Vec<String> = Vec::new();
    for file in &summary.files {
        let mut has_glob = false;
        let mut has_tree = false;
        for span in &file.spans {
            match &span.rule {
                Rule::PathGlob(_) => has_glob = true,
                Rule::TreeNode(_) => has_tree = true,
                Rule::ModDeclaration(module) => {
                    declared.push(format!("{} (mod {module})", file.path.display()));
                }
            }
        }
        glob_files += usize::from(has_glob);
        tree_files += usize::from(has_tree);
    }
    declared.sort();

    assert!(
        glob_files > 0,
        "no file of this repository carries a path-rule span, although it holds \
         a tests/ directory in nearly every crate"
    );
    assert!(
        tree_files > 0,
        "no file of this repository carries a tree-rule span, so the parse, the \
         query, or the needle filter has stopped marking anything"
    );
    assert!(
        !declared.is_empty(),
        "no file of this repository carries a module-declaration span, although \
         three sources declare `#[cfg(test)] mod <name>;` and the files they \
         name are all under src/ where the walk reads them. The cross-file pass \
         is the one rule with no evidence in the file it marks, so nothing else \
         here can notice it failing"
    );

    // 1. The Rust row, which is what the issue asks to see.
    let rust = summary
        .rows
        .iter()
        .find(|row| row.label == RUST)
        .unwrap_or_else(|| panic!("this repository holds Rust, so the summary has a {RUST} row"));
    assert!(
        rust.code() > 10_000,
        "the {RUST} row reports {} rows of code, which is far too few for this \
         repository",
        rust.code()
    );
    assert!(
        rust.test_percent() > 0.0,
        "the {RUST} row reports no test code at all, so the split found nothing \
         in a repository whose crates are all tested"
    );

    // 4. More than one language.
    assert!(
        summary.rows.len() > 1,
        "the summary holds one language row, so the walk read one corner of the \
         tree"
    );
}
