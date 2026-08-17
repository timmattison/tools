//! Guard tests for `repo_guards::target_lints::audit`.
//!
//! The headline test — [`every_target_root_declares_a_position_on_its_crate_lints`]
//! — runs the audit against this repository and **fails today by design**. Five
//! target roots are silent about lints their own crate raises: `cwt`'s two
//! integration tests never mention `unsafe_code` or `clippy::pedantic`, and
//! `bm`'s library and two integration tests never mention `clippy::unwrap_used`
//! or `clippy::expect_used`. Both crates acquired the gap honestly — they moved
//! a manifest `[lints]` table into a crate-root attribute to satisfy the
//! workspace-inheritance guard, and a crate-root attribute reaches one target
//! where the manifest table reached all of them. That red is the point of this
//! commit; the following commits clear it.
//!
//! Every other test is a mutation test. A guard that cannot fail is worse than
//! no guard, because "clean" and "I never looked" print identically. So each
//! shape is built as a real workspace on disk and fed to the real `audit()` —
//! member enumeration, target resolution, parsing, and comparison together, not
//! a string predicate in isolation — and each fail-closed condition is asserted
//! to produce an `Err` rather than a clean verdict.
//!
//! Parallel safety: this workspace's tests share `./target` with the pre-commit
//! hook's own `cargo test`, so two copies of any test here can run at the same
//! moment. Every fixture lives in its own `tempfile::TempDir`, whose name the
//! OS makes unique; nothing is keyed on a fixed path under the temp dir, the
//! repo, or the home dir.

use std::fs;
use std::path::{Path, PathBuf};

use repo_guards::target_lints::{self, Report, TargetLintsError};
use tempfile::TempDir;

/// A crate root that raises one lint, in the plainest form there is.
const RAISES_PEDANTIC: &str = "#![warn(clippy::pedantic)]\n\nfn main() {}\n";

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize repo root {}: {e}", root.display()))
}

/// Create a throwaway workspace root whose members are `crates/*`.
fn synthetic_workspace() -> TempDir {
    let dir = TempDir::new().expect("create fixture workspace dir");
    write_root_manifest(
        dir.path(),
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
    );
    dir
}

/// Overwrite the root manifest of `root` with `contents`.
fn write_root_manifest(root: &Path, contents: &str) {
    fs::write(root.join("Cargo.toml"), contents).expect("write fixture root manifest");
}

/// Create `crates/<name>/Cargo.toml` under `root`, appending `extra_tables`
/// verbatim after the `[package]` table, and return the member directory.
fn write_member(root: &Path, name: &str, extra_tables: &str) -> PathBuf {
    let dir = root.join("crates").join(name);
    fs::create_dir_all(&dir).expect("create fixture member dir");
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{extra_tables}"
        ),
    )
    .expect("write fixture member manifest");
    dir
}

/// Write `relative` (e.g. `tests/cli.rs`) under a member directory, creating
/// intermediate directories.
fn write_source(member_dir: &Path, relative: &str, contents: &str) {
    let path = member_dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture source dir");
    }
    fs::write(&path, contents).expect("write fixture source file");
}

/// Run the audit against a fixture and require a verdict (not an error).
fn audit_fixture(dir: &TempDir) -> Report {
    target_lints::audit(dir.path()).expect("audit the fixture workspace")
}

/// Run the audit against a fixture and require a refusal. Returning the error
/// lets a caller assert on the variant; the assertion message renders the
/// *report* when one came back, so a fail-closed regression says what the guard
/// wrongly concluded instead of just "expected Err".
fn audit_must_refuse(dir: &TempDir) -> TargetLintsError {
    match target_lints::audit(dir.path()) {
        Err(e) => e,
        Ok(report) => panic!(
            "audit should have refused this workspace, but returned a verdict:\n{report}\n\
             (compliant = {})",
            report.is_compliant()
        ),
    }
}

/// The offender list as `(path, missing lints)` pairs, for comparison.
fn offenders(report: &Report) -> Vec<(PathBuf, Vec<String>)> {
    report
        .offenders()
        .iter()
        .map(|offender| (offender.path().to_path_buf(), offender.missing().to_vec()))
        .collect()
}

/// Assert a report names exactly `expected` (paths relative to the fixture
/// root), rendering the whole report when it does not.
fn assert_offenders(report: &Report, expected: &[&str]) {
    let actual: Vec<PathBuf> = report
        .offenders()
        .iter()
        .map(|offender| offender.path().to_path_buf())
        .collect();
    let want: Vec<PathBuf> = expected.iter().map(PathBuf::from).collect();
    assert_eq!(
        actual, want,
        "unexpected offender set; report was:\n{report}"
    );
}

// ---------------------------------------------------------------------------
// The real repository
// ---------------------------------------------------------------------------

/// The guard, pointed at this repo. Red until the silent test roots take a
/// position.
///
/// The root-count assertion runs first on purpose: a guard that examined zero
/// target roots would report "compliant" for entirely the wrong reason, and
/// that false green is the failure mode this whole file exists to prevent.
#[test]
fn every_target_root_declares_a_position_on_its_crate_lints() {
    let report = target_lints::audit(&repo_root()).expect("audit the workspace");

    assert!(
        report.roots_examined() > 0,
        "the audit examined zero target roots; a guard that scans nothing \
         reports clean for the wrong reason"
    );
    assert!(report.is_compliant(), "{report}");
}

// ---------------------------------------------------------------------------
// The rule: silence is the violation, a stated position is not
// ---------------------------------------------------------------------------

/// The defect this guard exists for: a binary raises a lint, its integration
/// test says nothing, and cargo lints the two differently without a word.
#[test]
fn test_root_silent_about_a_baseline_lint_is_flagged() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        !report.is_compliant(),
        "a test root silent about a lint its crate raises must be flagged, \
         but the audit reported clean"
    );
    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/tests/cli.rs"),
            vec!["clippy::pedantic".to_owned()]
        )],
        "the offender must be named along with the lint it never mentions"
    );
}

/// A test root that `allow`s the baseline lint is compliant. This is the whole
/// design: an opt-out written at the site it applies to is a decision a reviewer
/// can see, which is exactly what an absence is not.
#[test]
fn test_root_that_allows_a_baseline_lint_is_clean() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(
        &member,
        "tests/cli.rs",
        "#![allow(clippy::pedantic, reason = \"a test asserts on shapes the lint dislikes\")]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "an explicit allow is a stated position, not silence; got:\n{report}"
    );
}

/// `expect` is a position too — a stricter one than `allow`, since it fails if
/// the lint stops firing.
#[test]
fn test_root_that_expects_a_baseline_lint_is_clean() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(
        &member,
        "tests/cli.rs",
        "#![expect(clippy::pedantic, reason = \"pinned until the fixtures are reshaped\")]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "an expect is a stated position; got:\n{report}"
    );
}

/// Levels are not compared. A test root may `deny` what the binary only `warn`s,
/// or the other way round; the bar is that it said something.
#[test]
fn a_different_level_still_counts_as_a_position() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(
        &member,
        "tests/cli.rs",
        "#![deny(clippy::pedantic)]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "mentioning is the bar, not matching the level; got:\n{report}"
    );
}

/// A crate whose library and binary raise nothing has no baseline, so its tests
/// owe nothing. Without this, the guard would demand a declaration from every
/// test root in the workspace — a rule nobody could satisfy, and therefore a
/// rule that would be deleted.
#[test]
fn crate_that_raises_nothing_imposes_nothing() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "plain", "");
    write_source(&member, "src/main.rs", "fn main() {}\n");
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "no raised lint means no baseline to mention; got:\n{report}"
    );
    assert_eq!(
        report.roots_examined(),
        2,
        "both roots should still have been examined"
    );
}

/// The baseline is the *union* over library and binary roots, and both of them
/// have to answer for it. This is `bm`'s real shape: `src/main.rs` raises lints
/// that `src/lib.rs` never mentions.
#[test]
fn baseline_unions_library_and_binary_and_binds_both() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "split", "");
    write_source(&member, "src/lib.rs", "#![warn(clippy::unwrap_used)]\n");
    write_source(
        &member,
        "src/main.rs",
        "#![warn(clippy::expect_used)]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![
            (
                PathBuf::from("crates/split/src/lib.rs"),
                vec!["clippy::expect_used".to_owned()]
            ),
            (
                PathBuf::from("crates/split/src/main.rs"),
                vec!["clippy::unwrap_used".to_owned()]
            ),
        ],
        "each root must answer for what the other raised"
    );
}

/// A test root missing two baseline lints reports both, not just the first.
#[test]
fn every_missing_baseline_lint_is_reported() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/main.rs",
        "#![deny(unsafe_code)]\n#![warn(clippy::pedantic)]\n\nfn main() {}\n",
    );
    write_source(
        &member,
        "tests/cli.rs",
        "#![warn(clippy::pedantic)]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/tests/cli.rs"),
            vec!["unsafe_code".to_owned()]
        )],
        "a partially-answered baseline still leaves the unanswered lint outstanding"
    );
}

// ---------------------------------------------------------------------------
// Syntactic forms: the guard parses, so every spelling reduces to one answer
// ---------------------------------------------------------------------------

/// Several lints in one attribute — the live shape in `kitchen-sync`'s
/// `src/main.rs`. A matcher that read only the first argument would report the
/// second as compliant forever.
#[test]
fn multiple_lints_in_one_attribute_all_count() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/main.rs",
        "#![deny(clippy::exit, clippy::format_push_string)]\n\nfn main() {}\n",
    );
    write_source(
        &member,
        "tests/cli.rs",
        "#![allow(clippy::exit, reason = \"the harness shells out\")]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/tests/cli.rs"),
            vec!["clippy::format_push_string".to_owned()]
        )],
        "both lints of the deny must enter the baseline, and both arguments of the allow must be read"
    );
}

/// A bare lint name with no tool prefix is rendered without one, so
/// `unsafe_code` matches `unsafe_code` and not `clippy::unsafe_code`.
#[test]
fn unprefixed_lint_names_round_trip() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/main.rs",
        "#![deny(unsafe_code)]\n\nfn main() {}\n",
    );
    write_source(
        &member,
        "tests/cli.rs",
        "#![forbid(unsafe_code)]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "a bare lint name must match itself; got:\n{report}"
    );
}

/// `reason = "..."` is a name-value pair, not a lint. Collecting it would invent
/// a lint called `reason` that no root could ever mention — every crate using
/// the modern attribute form would go red at once.
#[test]
fn a_reason_is_not_a_lint_name() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/main.rs",
        "#![warn(clippy::pedantic, reason = \"stricter than the workspace set\")]\n\nfn main() {}\n",
    );
    write_source(
        &member,
        "tests/cli.rs",
        "#![warn(clippy::pedantic)]\n\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "`reason` must not be collected as a lint; got:\n{report}"
    );
}

/// `#![cfg_attr(not(test), warn(lint))]` has attribute path `cfg_attr`, so it is
/// neither a raise nor a mention. This is `bm`'s `src/lib.rs` exactly: a lint
/// that applies in some configurations is not a position the crate holds in all
/// of them, and the guard must not read it as one.
#[test]
fn cfg_attr_wrapped_lints_neither_raise_nor_mention() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/lib.rs",
        "#![cfg_attr(not(test), warn(clippy::unwrap_used))]\n",
    );
    write_source(
        &member,
        "src/main.rs",
        "#![warn(clippy::unwrap_used)]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/src/lib.rs"),
            vec!["clippy::unwrap_used".to_owned()]
        )],
        "a cfg_attr-wrapped lint is not a mention, so the library is still silent"
    );
}

/// An *outer* `#[warn(...)]` configures the item it is attached to, not the
/// target. Counting it would let one annotated function stand in for the whole
/// file.
#[test]
fn outer_attributes_are_not_target_wide_positions() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(
        &member,
        "tests/cli.rs",
        "#[warn(clippy::pedantic)]\n#[test]\nfn works() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/tests/cli.rs"),
            vec!["clippy::pedantic".to_owned()]
        )],
        "an outer attribute on one item is not a position for the target"
    );
}

// ---------------------------------------------------------------------------
// Target resolution: the guard must see every root, and only the roots
// ---------------------------------------------------------------------------

/// `tests/common/mod.rs` is a module a test root `include!`s or `mod`s, not a
/// target cargo builds. Three crates in this repo have one; treating them as
/// roots would invent violations against files that are never compiled alone.
#[test]
fn test_helper_modules_are_not_target_roots() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(
        &member,
        "tests/cli.rs",
        "#![warn(clippy::pedantic)]\n\nmod common;\n\n#[test]\nfn works() {}\n",
    );
    write_source(&member, "tests/common/mod.rs", "pub fn helper() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "tests/common/mod.rs is not a target root; got:\n{report}"
    );
    assert_eq!(
        report.roots_examined(),
        2,
        "only src/main.rs and tests/cli.rs are targets"
    );
}

/// The directory form of a test target — `tests/<name>/main.rs` — *is* a root,
/// and it is the one file in such a directory that is. Missing it would exempt
/// every multi-file integration test.
#[test]
fn directory_shaped_test_targets_are_roots() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "tests/suite/main.rs", "#[test]\nfn works() {}\n");
    write_source(&member, "tests/suite/helper.rs", "pub fn helper() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/tests/suite/main.rs"]);
    assert_eq!(
        report.roots_examined(),
        2,
        "helper.rs beside a main.rs is not itself a target root"
    );
}

/// A bench root is bound by the baseline like any other target.
#[test]
fn bench_roots_are_audited() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "benches/throughput.rs", "fn main() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/benches/throughput.rs"]);
    assert_eq!(
        report.offenders()[0].kind(),
        "bench",
        "the report should say what kind of target is silent"
    );
}

/// An example root is bound too — examples are the code users copy, so a lint
/// the crate raises applying everywhere *except* the examples is backwards.
#[test]
fn example_roots_are_audited() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/lib.rs", "#![warn(clippy::unwrap_used)]\n");
    write_source(&member, "examples/demo.rs", "fn main() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/examples/demo.rs"]);
    assert_eq!(report.offenders()[0].kind(), "example");
}

/// A `[[test]] path` pointing outside `tests/` is found through the manifest,
/// which auto-discovery alone would never reach.
#[test]
fn explicitly_declared_test_paths_are_audited() {
    let ws = synthetic_workspace();
    let member = write_member(
        ws.path(),
        "tool",
        "[[test]]\nname = \"custom\"\npath = \"checks/custom.rs\"\n",
    );
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "checks/custom.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/checks/custom.rs"]);
}

/// `src/bin/*.rs` binaries raise the baseline like any other binary.
#[test]
fn src_bin_binaries_contribute_to_the_baseline() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/lib.rs", "#![warn(clippy::unwrap_used)]\n");
    write_source(
        &member,
        "src/bin/helper.rs",
        "#![warn(clippy::unwrap_used)]\n#![deny(unsafe_code)]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/src/lib.rs"),
            vec!["unsafe_code".to_owned()]
        )],
        "a src/bin binary is a baseline-raising target, and the library must answer it"
    );
}

/// `[[bin]] path = "src/main.rs"` — the spelling nearly every crate in this repo
/// uses — names the same file auto-discovery finds. Counting it twice would
/// inflate the root count and print the same offender twice.
#[test]
fn a_declared_path_and_its_conventional_twin_are_one_root() {
    let ws = synthetic_workspace();
    let member = write_member(
        ws.path(),
        "tool",
        "[[bin]]\nname = \"tool\"\npath = \"src/main.rs\"\n",
    );
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/tests/cli.rs"]);
    assert_eq!(
        report.roots_examined(),
        2,
        "the declared bin and the discovered bin are the same file"
    );
}

/// An explicit `[lib] path` is honored instead of the conventional location.
#[test]
fn explicitly_declared_library_paths_are_audited() {
    let ws = synthetic_workspace();
    let member = write_member(
        ws.path(),
        "tool",
        "[lib]\nname = \"tool\"\npath = \"core/api.rs\"\n",
    );
    write_source(&member, "core/api.rs", "#![warn(clippy::unwrap_used)]\n");
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/tests/cli.rs"]);
}

/// One crate's baseline never leaks into another's. Sibling crates in the same
/// workspace are independent policies.
#[test]
fn baselines_do_not_leak_between_crates() {
    let ws = synthetic_workspace();
    let strict = write_member(ws.path(), "strict", "");
    write_source(&strict, "src/main.rs", RAISES_PEDANTIC);
    write_source(
        &strict,
        "tests/cli.rs",
        "#![warn(clippy::pedantic)]\n\n#[test]\nfn works() {}\n",
    );

    let relaxed = write_member(ws.path(), "relaxed", "");
    write_source(&relaxed, "src/main.rs", "fn main() {}\n");
    write_source(&relaxed, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "a sibling crate's baseline must not bind this one; got:\n{report}"
    );
    assert_eq!(report.crates_examined(), 2);
    assert_eq!(report.roots_examined(), 4);
}

// ---------------------------------------------------------------------------
// Display: the message has to be usable without opening the source
// ---------------------------------------------------------------------------

/// The failure text names the silent root, the lints it never mentions, and
/// both ways to answer — raise it, or opt out in writing. Formatting lives in
/// `Report`, not at the call site, so this is where it is pinned.
#[test]
fn failure_message_names_the_root_the_lint_and_both_remedies() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let rendered = audit_fixture(&ws).to_string();

    assert!(
        rendered
            .contains("crates/tool/tests/cli.rs (test target) never mentions: clippy::pedantic"),
        "the message must name the root and the lint, got:\n{rendered}"
    );
    assert!(
        rendered.contains("    #![warn(clippy::pedantic)]"),
        "the message must show how to raise the lint, got:\n{rendered}"
    );
    assert!(
        rendered.contains("    #![allow(clippy::pedantic, reason = \"...\")]"),
        "the message must show the written opt-out, got:\n{rendered}"
    );
}

/// A clean audit says how much it looked at — so a passing CI log distinguishes
/// "checked 90 roots" from "checked nothing".
#[test]
fn clean_message_reports_the_counts() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", "fn main() {}\n");

    let rendered = audit_fixture(&ws).to_string();

    assert!(
        rendered.contains("Checked 1 target roots across 1 workspace members"),
        "a clean report should state what it examined, got:\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// Fail closed: none of these may yield a clean verdict
// ---------------------------------------------------------------------------

/// No workspace at all: the member set cannot be enumerated.
#[test]
fn unenumerable_workspace_refuses() {
    let dir = TempDir::new().expect("create fixture dir");

    assert!(
        matches!(
            target_lints::audit(dir.path()),
            Err(TargetLintsError::Members(_))
        ),
        "a workspace whose members cannot be enumerated must be an error, not a clean verdict"
    );
}

/// A member manifest that is not valid TOML. Unreadable is not compliant.
#[test]
fn unparsable_member_manifest_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    fs::write(member.join("Cargo.toml"), "[package\nname = ").expect("write invalid TOML");

    assert!(
        matches!(audit_must_refuse(&ws), TargetLintsError::Members(_)),
        "an unparsable member manifest must be an error, never a pass"
    );
}

/// A target root that is not valid Rust. A file the guard cannot parse is a
/// file whose lint attributes it cannot see, which is not the same as a file
/// that has none.
#[test]
fn unparsable_target_root_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    write_source(&member, "tests/cli.rs", "fn works( {\n");

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(&error, TargetLintsError::ParseRoot { path, .. } if path.ends_with("tests/cli.rs")),
        "an unparsable target root must be an error naming the file, got: {error}"
    );
}

/// A target root that cannot be read at all. A dangling symlink is the portable
/// way to build one; the point is that an I/O failure must never be mistaken
/// for "this file mentions nothing".
#[cfg(unix)]
#[test]
fn unreadable_target_root_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);
    fs::create_dir_all(member.join("tests")).expect("create fixture tests dir");
    std::os::unix::fs::symlink("nowhere.rs", member.join("tests").join("cli.rs"))
        .expect("create dangling symlink");

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(&error, TargetLintsError::ReadRoot { path, .. } if path.ends_with("tests/cli.rs")),
        "an unreadable target root must be an error naming the file, got: {error}"
    );
}

/// A manifest declaring a target that is not on disk. Silently skipping it
/// would let a typo'd `path` shrink the audited set while the guard kept
/// passing.
#[test]
fn declared_target_path_that_does_not_exist_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(
        ws.path(),
        "tool",
        "[[test]]\nname = \"custom\"\npath = \"checks/typo.rs\"\n",
    );
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(
            &error,
            TargetLintsError::MissingDeclaredTarget { kind: "test", declared, .. }
                if declared == "checks/typo.rs"
        ),
        "a declared target that is not on disk must be an error naming it, got: {error}"
    );
}

/// `autotests = false` (and its siblings) change which roots cargo builds. This
/// guard models the default rules only, so it refuses rather than enumerate a
/// set that disagrees with the build.
#[test]
fn auto_discovery_override_refuses() {
    for key in ["autotests", "autobins", "autobenches", "autoexamples"] {
        let ws = synthetic_workspace();
        let dir = ws.path().join("crates").join("tool");
        fs::create_dir_all(&dir).expect("create fixture member dir");
        fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{key} = false\n"
            ),
        )
        .expect("write fixture member manifest");
        write_source(&dir, "src/main.rs", RAISES_PEDANTIC);

        let error = audit_must_refuse(&ws);

        assert!(
            matches!(&error, TargetLintsError::AutoDiscoveryOverride { key: found, .. } if *found == key),
            "`{key}` must be an error, got: {error}"
        );
    }
}

/// A member crate with a manifest but no target roots at all. Reporting it
/// clean would be reporting on nothing.
#[test]
fn member_with_no_target_roots_refuses() {
    let ws = synthetic_workspace();
    write_member(ws.path(), "hollow", "");

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(&error, TargetLintsError::NoTargetRoots { dir } if dir.ends_with("crates/hollow")),
        "a member with no targets must be an error naming it, got: {error}"
    );
}
