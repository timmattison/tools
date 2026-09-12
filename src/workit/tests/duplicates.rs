//! The duplicate-name report reads the same on every run.
//!
//! A workspace cannot carry the same package name twice, so `workit` refuses a
//! tree that holds two packages of one name and prints every name it found
//! twice with every path that holds it. That report is what the user works
//! from: it is the list of directories to rename, to exclude, or to leave out
//! of the search path.
//!
//! The report used to be gathered in a `HashMap`, whose iteration order is
//! seeded afresh in every process. Two runs over one tree therefore printed the
//! same groups in different orders, and so did one run on two machines. That
//! makes the output awkward to compare — against the previous run, against a
//! colleague's, against what a bug report pasted in — for no reason: the tree
//! did not change between the two runs, so the answer must not either.
//!
//! The rule under test: the groups are printed in order by package name, and
//! the paths inside one group are printed in order by path. The tree here holds
//! eight duplicated names, so a run that ordered them by chance would have to
//! draw one arrangement out of forty thousand.
//!
//! Every test here builds its own tree in its own temporary directory, runs the
//! binary over that tree with `--dry-run`, and points `--output` inside the
//! fixture. Nothing is written and nothing outside the temporary directory is
//! read, so two copies of this file can run at the same time and neither one
//! can reach a real repository. No test here runs git, so none of them inherits
//! a git environment either.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// The words the fixture builds a duplicated package name out of, in the order
/// a sorted report has to name them.
///
/// There are eight of them on purpose. One run of a randomly seeded `HashMap`
/// can come out sorted, and a test that measures a single such run proves
/// nothing; eight groups leave one arrangement in `8!` that could pass by
/// chance.
const DUPLICATED_WORDS: [&str; 8] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
];

/// The word whose name the fixture puts in a third place, so the test covers a
/// group of more than the usual two.
const TRIPLED_WORD: &str = "charlie";

/// What every fixture package name starts with. A shared prefix keeps the
/// sorted order of the names the same as the sorted order of the words above.
const NAME_PREFIX: &str = "workit-duplicate-fixture-";

/// The line that opens one group of the report.
const GROUP_PREFIX: &str = "Error: Package name '";
/// What that line ends with, after the name.
const GROUP_SUFFIX: &str = "' appears in multiple locations:";
/// What each path inside a group is written behind.
const PATH_PREFIX: &str = "  - ";

/// A temporary directory and its canonical path.
///
/// The path is canonical because `workit` reports each path relative to the
/// canonical form of the root it was given, and a temporary directory on macOS
/// is reached through a symbolic link. The `TempDir` is returned with it: it
/// deletes the tree when it drops, so the caller has to keep it alive.
fn fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("a temporary directory is created");
    let root = fs::canonicalize(temp.path()).expect("the temporary directory has a canonical path");
    (temp, root)
}

/// Writes a package manifest named `name` in the directory `relative` under
/// `root`, and answers the path of the manifest it wrote — which is the path
/// the duplicate report names.
fn package_at(root: &Path, relative: &str, name: &str) -> PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).expect("the fixture directory is created");
    let manifest = dir.join("Cargo.toml");
    fs::write(
        &manifest,
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .expect("the fixture manifest is written");
    manifest
}

/// Fills `root` with one package of a name nothing else uses and eight names
/// that two or three directories share, and answers the report those packages
/// have to produce: the groups in order by name, each holding its paths in
/// order by path.
///
/// The directory that holds each package is numbered so that its rank runs
/// *against* the alphabet — `alpha` lives under `8-…` and `hotel` under `1-…`.
/// Sorting the paths therefore names the groups backwards, so a report that
/// followed the order the scan walks in would be the exact reverse of this
/// one, and a report that followed the order the tree was built in would be
/// different again: the second location of every name is created first.
fn build_duplicates(root: &Path) -> Vec<(String, Vec<String>)> {
    package_at(root, "0-unique", "workit-duplicate-fixture-unique-package");

    let mut expected = Vec::new();
    for (index, word) in DUPLICATED_WORDS.iter().enumerate() {
        let rank = DUPLICATED_WORDS.len() - index;
        let name = format!("{NAME_PREFIX}{word}");

        let second = package_at(root, &format!("{rank}-{word}-second"), &name);
        let first = package_at(root, &format!("{rank}-{word}-first"), &name);
        let mut paths = vec![first, second];
        if *word == TRIPLED_WORD {
            paths.push(package_at(root, &format!("{rank}-{word}-third"), &name));
        }

        // `first` < `second` < `third` in the alphabet, so the paths are
        // already in the order a sort puts them in.
        expected.push((
            name,
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        ));
    }
    expected
}

/// Runs the binary over `root` and hands back what it did, exit status
/// included: a tree that holds duplicate names is a tree the run refuses, so no
/// run here may assume it succeeded.
///
/// `--dry-run` keeps it from writing a manifest, and the output path names a
/// file inside the fixture, so no run can touch a manifest of the repository it
/// was started from.
fn workit(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workit"))
        .current_dir(root)
        .arg("--path")
        .arg(root)
        .arg("--output")
        .arg(root.join("workspace-manifest.toml"))
        .arg("--dry-run")
        .output()
        .expect("the workit binary runs")
}

/// What the run said on stderr.
fn report_of(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("workit writes UTF-8")
}

/// The duplicate groups the run printed, in the order it printed them, each
/// holding its paths in the order it printed those.
fn duplicate_groups(output: &Output) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();

    for line in report_of(output).lines() {
        if let Some(rest) = line.strip_prefix(GROUP_PREFIX) {
            let name = rest
                .strip_suffix(GROUP_SUFFIX)
                .expect("the line that opens a group ends the way it started");
            groups.push((name.to_string(), Vec::new()));
        } else if let Some(path) = line.strip_prefix(PATH_PREFIX) {
            groups
                .last_mut()
                .expect("a path is listed under the group that holds it")
                .1
                .push(path.to_string());
        }
    }

    groups
}

#[test]
fn the_duplicate_report_names_the_groups_in_order_by_package_name() {
    let (_temp, root) = fixture();
    let expected = build_duplicates(&root);

    let output = workit(&root);

    let reported: Vec<String> = duplicate_groups(&output)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let wanted: Vec<String> = expected.iter().map(|(name, _)| name.clone()).collect();
    assert_eq!(
        reported, wanted,
        "the tree did not change between two runs, so the report must not either: \
         gathering the names in a randomly seeded map prints them in a different \
         order in every process, and in a different order on every machine"
    );
}

#[test]
fn the_paths_inside_one_duplicate_group_are_in_order_by_path() {
    let (_temp, root) = fixture();
    let expected = build_duplicates(&root);

    let output = workit(&root);
    let reported = duplicate_groups(&output);

    for (name, paths) in &expected {
        let found = reported
            .iter()
            .find(|(reported_name, _)| reported_name == name)
            .unwrap_or_else(|| panic!("the report names the duplicated package '{name}'"));
        assert_eq!(
            &found.1, paths,
            "the paths of one group are the answer to 'which directory do I rename', \
             so they are listed in one order however the scan met them"
        );
    }
}

#[test]
fn a_tree_of_duplicate_names_is_refused_and_every_location_is_named() {
    let (_temp, root) = fixture();
    let expected = build_duplicates(&root);

    let output = workit(&root);
    let report = report_of(&output);

    assert!(
        !output.status.success(),
        "a workspace cannot carry one package name twice, so the run refuses the tree: {report}"
    );
    for (name, paths) in &expected {
        assert!(
            report.contains(name.as_str()),
            "the report names every duplicated package, '{name}' included: {report}"
        );
        for path in paths {
            assert!(
                report.contains(path.as_str()),
                "the report names every path that holds '{name}', '{path}' included: {report}"
            );
        }
    }
    assert!(
        report.contains("Workspace creation failed: duplicate package names found."),
        "the run says why it wrote nothing: {report}"
    );
}

#[test]
fn a_package_name_nothing_else_uses_is_not_reported() {
    let (_temp, root) = fixture();
    build_duplicates(&root);

    let output = workit(&root);

    let names: Vec<String> = duplicate_groups(&output)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(
        !names.iter().any(|name| name.ends_with("unique-package")),
        "a name one package holds is not a duplicate: {names:?}"
    );
}
