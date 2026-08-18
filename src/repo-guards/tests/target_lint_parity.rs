//! Guard tests for `repo_guards::target_lints::audit`.
//!
//! Two tests point at this repository, and they check the guard's two halves.
//!
//! [`every_target_root_declares_a_position_on_its_crate_lints`] checks the
//! verdict: no target root here is silent about a lint its own crate raises.
//! It was red when this file was written — `cwt`'s and `bm`'s integration tests
//! had each lost lints to the manifest-to-crate-root migration that satisfying
//! the workspace-inheritance guard requires — and the commits that followed
//! cleared it.
//!
//! [`the_guard_resolves_exactly_the_roots_cargo_builds`] checks the prior
//! question, which a verdict cannot: whether the guard found every root there
//! is. It asks `cargo metadata` rather than trusting the guard's hand-written
//! model of cargo's discovery rules. It was red the day it landed —
//! `src/buildinfo/build.rs` is a target cargo builds and lints that the guard
//! had never enumerated — and the commit that followed taught the guard about
//! build scripts and cleared it.
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

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use repo_guards::target_lints::{self, Report, TargetLintsError};
use tempfile::TempDir;

/// A crate root that raises one lint, in the plainest form there is.
const RAISES_PEDANTIC: &str = "#![warn(clippy::pedantic)]\n\nfn main() {}\n";

/// Every `[package]` key cargo uses to turn target auto-discovery on or off,
/// as cargo documents them and as verified against cargo 1.97.1.
///
/// This is the *ground truth* the guard is measured against, not a mirror of
/// [`target_lints::AUTO_DISCOVERY_KEYS`]. The distinction is the whole point:
/// the list here started as a copy of the guard's own constant, inherited its
/// omission of `autolib`, and so was structurally incapable of noticing it —
/// a fixture set derived from an implementation cannot catch what that
/// implementation forgot. Written independently, it catches a key the guard
/// does not model (every key below is exercised end-to-end by
/// [`auto_discovery_override_refuses`]) and, paired with
/// [`the_guard_models_every_auto_discovery_key_cargo_has`], a key the guard
/// models that cargo does not have.
const CARGO_AUTO_DISCOVERY_KEYS: [&str; 5] = [
    "autobenches",
    "autobins",
    "autoexamples",
    "autolib",
    "autotests",
];

/// Absolute, canonical path to this repository's root, derived from the crate
/// being compiled rather than the working directory (which `cargo test` does
/// not pin).
fn repo_root() -> PathBuf {
    canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// Resolve a path to its one true spelling, failing loudly when it cannot be.
///
/// Both sides of the cargo-parity comparison go through this. This repository
/// is reachable as both `/Users/...` and `/Volumes/...`, and `repo_root()`
/// arrives with a `../..` in it, so two names for one file would otherwise read
/// as two files and colour the comparison in whichever direction the spellings
/// happened to fall.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", path.display()))
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

/// The set of target roots the guard resolves must equal the set cargo builds.
///
/// Every other test here asks what the guard *concludes* about a root. This one
/// asks the prior question — which roots there are — and answers it by asking
/// the toolchain instead of modelling the toolchain. `cargo metadata` needs no
/// model of cargo's discovery rules because it *is* cargo; the guard's
/// `target_roots` re-derives those rules by hand, from `src/lib.rs` and
/// `tests/*.rs` down to the `<dir>/<name>/main.rs` form. A hand-written copy of
/// someone else's rules drifts, and it drifts silently: a root the guard never
/// opens cannot be reported as silent about its crate's lints, so the crate
/// comes back clean *because* it was never fully read. That is the same false
/// green this whole file exists to prevent, one level further up.
///
/// Two review findings are why this test exists, and they are one defect twice:
///
/// - **Build scripts were never enumerated.** `build.rs` is a target cargo
///   compiles and lints. Verified on cargo 1.97.1: a manifest
///   `[lints.rust] unsafe_code = "deny"` makes an unsafe block in `build.rs` a
///   compile error, while the same lint written `#![deny(unsafe_code)]` in
///   `src/lib.rs` lets it through. So a build script loses its lints in exactly
///   the manifest-to-crate-root migration this guard was written to police, and
///   the guard called the crate clean. `src/buildinfo/build.rs` is this
///   workspace's only build script, and it is why this test was **red the day
///   it landed** — a real occurrence, flagged by the guardrail rather than by a
///   reviewer. The next commit added `TargetKind::Build`; the build-script
///   fixtures further down pin the behavior, and this test pins that the fix
///   matches what cargo actually reports.
/// - **`AUTO_DISCOVERY_KEYS` listed four of cargo's five keys**, omitting
///   `autolib`. Verified on cargo 1.97.1: `autolib = false` with a `src/lib.rs`
///   on disk and an explicit `[[bin]]` beside it makes cargo report the bin and
///   no lib at all, while the guard walked past the key and resolved
///   `src/lib.rs` as a library root — a kind that *raises* the crate baseline,
///   so lints from a file cargo never compiles would be imposed on every target
///   that is real. No manifest here sets it, so this test could not have seen
///   it; [`the_guard_models_every_auto_discovery_key_cargo_has`] is what closes
///   that half, by comparing the guard's modeled set against cargo's own.
///
/// Both were caught by a human reading the model against cargo's documentation,
/// which does not scale and did not have to. Whatever cargo adds next — another
/// target kind, another discovery key — arrives here as a set difference on the
/// first run after it lands, with no one needing to notice.
#[test]
fn the_guard_resolves_exactly_the_roots_cargo_builds() {
    let repo_root = repo_root();
    let report = target_lints::audit(&repo_root).expect("audit the workspace");

    let guard: BTreeSet<PathBuf> = report
        .roots()
        .iter()
        .map(|root| canonical(&repo_root.join(root)))
        .collect();
    let cargo = cargo_target_roots(&repo_root);

    assert!(
        !cargo.is_empty(),
        "`cargo metadata` reported no targets at all for {}; every root the guard \
         resolved would then look invented and every real gap would vanish, so an \
         empty cargo side is a broken test rather than a verdict",
        repo_root.display()
    );

    let missed: BTreeSet<PathBuf> = cargo.difference(&guard).cloned().collect();
    assert!(
        missed.is_empty(),
        "cargo builds {} target root(s) the guard never resolved:\n{}\n\
         This is the false-green direction. A root the guard does not open cannot be \
         reported as silent about a lint its crate raises, so that root keeps the \
         workspace default lints unnoticed and its crate is reported clean on the \
         strength of the roots that were read.",
        missed.len(),
        render_roots(&missed, &repo_root)
    );

    let invented: BTreeSet<PathBuf> = guard.difference(&cargo).cloned().collect();
    assert!(
        invented.is_empty(),
        "the guard resolved {} root(s) cargo does not build:\n{}\n\
         This direction is loud rather than silent, but it is the same drift: the \
         guard would demand a lint attribute in a file cargo never lints, and a model \
         wrong about which files exist is not to be trusted about which files are \
         missing.",
        invented.len(),
        render_roots(&invented, &repo_root)
    );
}

/// Every `src_path` cargo reports, across every target of every workspace
/// package, canonicalized.
///
/// `--no-deps` keeps the answer to this workspace's own packages, and is
/// deliberately not paired with `--offline`: resolution is skipped either way,
/// and `--offline` only adds a way for a cold cache to fail.
///
/// Every step here fails loudly. A parity test that skipped when cargo could
/// not be spawned, or compared against a set it quietly failed to parse, would
/// be an instance of the very defect it exists to catch: a check that reports
/// clean because it never looked.
fn cargo_target_roots(repo_root: &Path) -> BTreeSet<PathBuf> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(repo_root)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "cannot run `cargo metadata` in {}: {e}",
                repo_root.display()
            )
        });

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "`cargo metadata` in {} exited with {}:\n{stderr}",
        repo_root.display(),
        output.status
    );

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("cannot parse the output of `cargo metadata` as JSON: {e}\nstderr:\n{stderr}")
    });

    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| {
            panic!("`cargo metadata` returned no `packages` array\nstderr:\n{stderr}")
        });

    packages
        .iter()
        .flat_map(|package| {
            package
                .get("targets")
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| {
                    panic!("a package in `cargo metadata` has no `targets` array: {package}")
                })
        })
        .map(|target| {
            let src_path = target
                .get("src_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!("a target in `cargo metadata` has no `src_path`: {target}")
                });
            canonical(Path::new(src_path))
        })
        .collect()
}

/// Render a set of absolute roots as one indented repo-relative path per line,
/// so a failure reads as a list of files rather than as two sets to diff by eye.
fn render_roots(roots: &BTreeSet<PathBuf>, repo_root: &Path) -> String {
    roots
        .iter()
        .map(|root| {
            format!(
                "    {}",
                root.strip_prefix(repo_root).unwrap_or(root).display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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

/// A `cfg_attr`-wrapped lint does **not** raise. A lint that applies only under
/// `not(test)` is not a position the crate holds in every configuration, so it
/// cannot bind sibling targets — least of all the test targets its own predicate
/// excludes. Were it to raise, the silent `tests/cli.rs` here would be an
/// offender.
#[test]
fn cfg_attr_wrapped_lints_do_not_raise() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/lib.rs",
        "#![cfg_attr(not(test), warn(clippy::unwrap_used))]\n",
    );
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "a conditionally-applied lint must not impose a baseline on siblings; got:\n{report}"
    );
    assert_eq!(
        report.roots_examined(),
        2,
        "both roots should still have been examined"
    );
}

/// A `cfg_attr`-wrapped lint **does** mention. This is `bm`'s real shape: the
/// library states its position on `clippy::unwrap_used` more precisely than a
/// bare `#![warn(...)]` would — it names the lint *and* the configuration it
/// holds in — and the mention bar asks for nothing more than that the file name
/// the lint. Reading it as silence would demand a redundant unconditional
/// mention be bolted on top of the more careful declaration.
#[test]
fn cfg_attr_wrapped_lints_are_a_mention() {
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

    assert!(
        report.is_compliant(),
        "naming a lint inside cfg_attr is a stated position, not silence; got:\n{report}"
    );
}

/// `cfg_attr` takes *several* attributes after its predicate. A walk that read
/// only the first would report the rest silent forever — the false-green shape
/// this whole file exists to prevent. The unanswered third lint proves the guard
/// is still able to flag while both wrapped mentions land.
#[test]
fn every_attribute_after_a_cfg_predicate_mentions() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/lib.rs",
        "#![cfg_attr(unix, warn(clippy::unwrap_used), allow(clippy::expect_used))]\n",
    );
    write_source(
        &member,
        "src/main.rs",
        "#![warn(clippy::unwrap_used, clippy::expect_used)]\n#![deny(unsafe_code)]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/src/lib.rs"),
            vec!["unsafe_code".to_owned()]
        )],
        "both attributes after the predicate must be read, leaving only the lint neither names"
    );
}

/// `cfg_attr` nests. The walk recurses instead of special-casing one level, so
/// an inner mention counts however many wrappers sit above it.
#[test]
fn nested_cfg_attr_mentions_are_found() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/lib.rs",
        "#![cfg_attr(unix, cfg_attr(test, warn(clippy::unwrap_used)))]\n",
    );
    write_source(
        &member,
        "src/main.rs",
        "#![warn(clippy::unwrap_used)]\n#![deny(unsafe_code)]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/src/lib.rs"),
            vec!["unsafe_code".to_owned()]
        )],
        "a mention nested two cfg_attrs deep still counts"
    );
}

/// Only *lint-level* attributes inside a `cfg_attr` count. `doc = "..."` and
/// `feature(...)` are conditionally applied too, and neither says anything about
/// a lint, so the library here is still silent.
#[test]
fn non_lint_attributes_inside_cfg_attr_mention_nothing() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(
        &member,
        "src/lib.rs",
        "#![cfg_attr(unix, doc = \"unix only\")]\n#![cfg_attr(docsrs, feature(doc_cfg))]\n",
    );
    write_source(&member, "src/main.rs", RAISES_PEDANTIC);

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/src/lib.rs"),
            vec!["clippy::pedantic".to_owned()]
        )],
        "a non-lint attribute inside cfg_attr is not a position on any lint"
    );
}

/// The cfg predicate is cfg syntax, never a lint name. `not`, `test`, and
/// `cfg_attr` are spelled here as lints the binary raises, so a walk that
/// harvested the predicate would report the library compliant on all three.
#[test]
fn a_cfg_predicate_is_never_a_lint_name() {
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
        "#![warn(clippy::unwrap_used)]\n#![deny(cfg_attr, not, test)]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert_eq!(
        offenders(&report),
        vec![(
            PathBuf::from("crates/tool/src/lib.rs"),
            vec!["cfg_attr".to_owned(), "not".to_owned(), "test".to_owned()]
        )],
        "only the wrapped lint may be mentioned; the predicate contributes nothing"
    );
}

/// `cfg_attr` nests without limit in the grammar, so the walk over it is
/// recursive and a generated or hostile file could drive it into the stack. Past
/// the bound the guard refuses, because a root it only partly read is a root it
/// cannot vouch for.
#[test]
fn pathologically_nested_cfg_attr_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    let mut attribute = "warn(clippy::unwrap_used)".to_owned();
    for _ in 0..64 {
        attribute = format!("cfg_attr(unix, {attribute})");
    }
    write_source(
        &member,
        "src/main.rs",
        &format!("#![{attribute}]\n\nfn main() {{}}\n"),
    );

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(&error, TargetLintsError::CfgAttrTooDeep { path } if path.ends_with("src/main.rs")),
        "nesting past the bound must be an error naming the file, got: {error}"
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
// Build scripts: a target cargo lints, declared in a shape no other target has
// ---------------------------------------------------------------------------
//
// Every fixture below was checked against cargo 1.97.1 with a throwaway crate
// and `cargo metadata --no-deps --format-version 1`, reading the `custom-build`
// target it reports. The `build` key is a *scalar* inside `[package]` — a path
// or a bool — not an array-of-tables with a `path` field like `[[bin]]`, so its
// forms are enumerated here one fixture apiece rather than inherited from the
// declared-path machinery the other kinds share.

/// A conventional `build.rs` is a target root, and silence there is the same
/// violation it is anywhere else.
///
/// This is the defect in its live form. Verified on cargo 1.97.1: with
/// `[lints.rust] unsafe_code = "deny"` in the manifest, an unsafe block in
/// `build.rs` fails the build; with the identical lint moved to
/// `#![deny(unsafe_code)]` at the top of `src/lib.rs`, the same `build.rs`
/// compiles clean. So a build script loses its lints in exactly the
/// manifest-to-crate-root migration this guard was written to police.
#[test]
fn build_script_silent_about_a_baseline_lint_is_flagged() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/lib.rs", "#![deny(unsafe_code)]\n");
    write_source(&member, "build.rs", "fn main() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/build.rs"]);
    assert_eq!(
        report.offenders()[0].kind(),
        "build script",
        "the report should say what kind of target is silent"
    );
}

/// A build script that states a position is compliant, by the same rule every
/// other target answers to: mention the lint, at any level, in writing.
#[test]
fn build_script_that_states_a_position_is_clean() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");
    write_source(
        &member,
        "build.rs",
        "#![allow(clippy::pedantic, reason = \"a build script prints to stdout by protocol\")]\n\nfn main() {}\n",
    );

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "an explicit allow in a build script is a stated position, not silence; got:\n{report}"
    );
    assert_eq!(
        report.roots_examined(),
        2,
        "both the library and the build script should have been examined"
    );
}

/// A build script **mentions but never raises**. Nothing depends on a build
/// script — it is compiled for the build machine, run once, and never linked
/// into the crate — so a lint it alone raises is a position that target holds,
/// not one the crate holds. Were it to raise, the silent library and test here
/// would both be offenders, and adding one strict build script would go red
/// across a crate that never asked for the lint.
#[test]
fn a_build_script_does_not_raise_its_crates_baseline() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "");
    write_source(&member, "src/lib.rs", "pub fn f() {}\n");
    write_source(
        &member,
        "build.rs",
        "#![deny(unsafe_code)]\n\nfn main() {}\n",
    );
    write_source(&member, "tests/cli.rs", "#[test]\nfn works() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "a build script's own lints must not bind its siblings; got:\n{report}"
    );
    assert_eq!(
        report.roots_examined(),
        3,
        "all three roots should still have been examined"
    );
}

/// `build = "custom-build.rs"` names the build script explicitly, and the
/// declaration *replaces* the convention rather than adding to it: cargo reports
/// exactly one `custom-build` target, at the declared path, and the stray
/// `build.rs` beside it is not a target at all. A guard that discovered the
/// convention regardless would demand a lint attribute in a file cargo never
/// compiles.
#[test]
fn explicitly_declared_build_script_paths_are_audited() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "build = \"custom-build.rs\"\n");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");
    write_source(&member, "custom-build.rs", "fn main() {}\n");
    write_source(&member, "build.rs", "fn main() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/custom-build.rs"]);
    assert_eq!(
        report.roots_examined(),
        2,
        "the declared path replaces the conventional one; build.rs is not a target here"
    );
}

/// `build = true` is the conventional path spelled out loud, and cargo means it
/// literally: it reports a `custom-build` target at `build.rs` and compiles it.
#[test]
fn build_true_resolves_the_conventional_build_script() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "build = true\n");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");
    write_source(&member, "build.rs", "fn main() {}\n");

    let report = audit_fixture(&ws);

    assert_offenders(&report, &["crates/tool/build.rs"]);
}

/// `build = false` turns the build script off. Cargo reports no `custom-build`
/// target even with a `build.rs` sitting on disk, and does not compile it — so
/// demanding a lint attribute in that file would be a violation invented against
/// a file the build never reads.
#[test]
fn build_false_resolves_no_build_script() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "build = false\n");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");
    write_source(&member, "build.rs", "fn main() {}\n");

    let report = audit_fixture(&ws);

    assert!(
        report.is_compliant(),
        "a disabled build script is not a target to audit; got:\n{report}"
    );
    assert_eq!(
        report.roots_examined(),
        1,
        "only the library is a target when `build = false`"
    );
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

/// `build = "missing.rs"` names a build script that is not on disk. Cargo
/// reports the target and then fails to compile it (`error: couldn't read
/// `missing.rs``), so a guard that quietly skipped it would be reporting on a
/// smaller set than the build has — the same defect a typo'd `[[test]] path`
/// already refuses over.
#[test]
fn declared_build_script_that_does_not_exist_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "build = \"missing.rs\"\n");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(
            &error,
            TargetLintsError::MissingDeclaredTarget { kind: "build", declared, .. }
                if declared == "missing.rs"
        ),
        "a declared build script that is not on disk must be an error naming it, got: {error}"
    );
}

/// `build = true` without a `build.rs` is the same refusal, and cargo agrees:
/// it reports the target at `build.rs` and then fails with `couldn't read
/// `build.rs``. So `true` is a *declaration* of the conventional path, not a
/// best-effort look for one — treating it as "discover if present" would let the
/// guard's root set silently disagree with cargo's.
#[test]
fn build_true_without_a_build_script_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "build = true\n");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(
            &error,
            TargetLintsError::MissingDeclaredTarget { kind: "build", declared, .. }
                if declared == "build.rs"
        ),
        "`build = true` with no build.rs must be an error naming the file cargo would compile, got: {error}"
    );
}

/// A `build` key that is neither a path nor a bool. Cargo rejects such a
/// manifest outright; reading it here as "this crate has no build script" would
/// drop a target from the audit on the strength of a value the guard did not
/// understand.
#[test]
fn build_key_that_is_neither_path_nor_bool_refuses() {
    let ws = synthetic_workspace();
    let member = write_member(ws.path(), "tool", "build = 3\n");
    write_source(&member, "src/lib.rs", "#![warn(clippy::pedantic)]\n");

    let error = audit_must_refuse(&ws);

    assert!(
        matches!(&error, TargetLintsError::MalformedBuildKey { value, .. } if value == "3"),
        "an ill-typed `build` key must be an error quoting the value, got: {error}"
    );
}

/// `autotests = false` (and its siblings) change which roots cargo builds. This
/// guard models the default rules only, so it refuses rather than enumerate a
/// set that disagrees with the build.
///
/// One fixture per key cargo actually has — [`CARGO_AUTO_DISCOVERY_KEYS`] —
/// because a key the guard does not model is not a refusal but a silent guess,
/// and it guesses wrong. `autolib = false` is the sharpest of the five: cargo
/// then reports no library target at all, verified on cargo 1.97.1 with a
/// `src/lib.rs` on disk and an explicit `[[bin]]` beside it, so a guard that
/// walks past the key resolves `src/lib.rs` as a library root. A library root
/// is one of the two kinds that *raise* the crate baseline, so lints from a
/// file cargo never compiles would be imposed on every target that is real.
/// Each fixture therefore carries both a library and a binary root, so every
/// key has something its setting could change.
#[test]
fn auto_discovery_override_refuses() {
    for key in CARGO_AUTO_DISCOVERY_KEYS {
        let ws = synthetic_workspace();
        let member = write_member(ws.path(), "tool", &format!("{key} = false\n"));
        write_source(&member, "src/lib.rs", RAISES_PEDANTIC);
        write_source(&member, "src/main.rs", RAISES_PEDANTIC);

        let error = audit_must_refuse(&ws);

        assert!(
            matches!(&error, TargetLintsError::AutoDiscoveryOverride { key: found, .. } if *found == key),
            "`{key}` must be an error, got: {error}"
        );
    }
}

/// The other direction of the same fact: the guard must not refuse a key cargo
/// does not have, and must not skip one it does.
///
/// [`auto_discovery_override_refuses`] proves each of cargo's keys is refused,
/// which catches an omission. It cannot catch the reverse — an invented key
/// would refuse manifests cargo reads happily — and neither can any fixture,
/// because there is no manifest to write for a key that does not exist. So the
/// two lists are compared directly, and this is what keeps them from drifting
/// apart again in either direction.
#[test]
fn the_guard_models_every_auto_discovery_key_cargo_has() {
    let modeled: BTreeSet<&str> = target_lints::AUTO_DISCOVERY_KEYS.into_iter().collect();
    let cargo: BTreeSet<&str> = CARGO_AUTO_DISCOVERY_KEYS.into_iter().collect();

    assert_eq!(
        modeled, cargo,
        "the guard models a different set of auto-discovery keys than cargo has; \
         a key cargo has and the guard omits is guessed at rather than refused, and \
         a key the guard invents refuses a manifest cargo would accept"
    );
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
