//! green_check — what "green" means for a repo, and how that verdict is
//! assembled into a command plan.
//!
//! This module owns the whole definition of the green check: detecting which
//! toolchains a worktree uses (pnpm / cargo / Tauri) and turning that into an
//! ordered list of shell commands. Detection stays hidden behind
//! [`build_check_plan`] — callers never ask "is this a cargo repo?" themselves,
//! they ask what the check *is*. ([`pkg_scripts`] is exported for inspection and
//! tests, [`shell_quote`] because `swt` prints shell command lines for humans to
//! paste too.)
//!
//! The plan always runs inside the worktree being checked, never the parent:
//!
//! - `.swt-check` at the config root — which defaults to the target, but is the
//!   *parent* repo root when `swt` checks a fresh worktree: the escape hatch is a
//!   gitignored, per-developer file, so it is absent from a checkout of HEAD.
//!   Used alone if present, as a shell-quoted absolute path, still run in the
//!   target.
//! - Otherwise, detected from the target and run there, whichever apply,
//!   additively (Tauri repos have both):
//!   - `package.json` declaring at least one of typecheck/tsc/lint/test:
//!     `pnpm install --frozen-lockfile` (only when `pnpm-lock.yaml` exists *and*
//!     `node_modules` does not), then those checks. A `package.json` with none of
//!     those scripts contributes nothing — the install alone verifies nothing and
//!     must never stand in for a check.
//!   - `Cargo.toml` at the root and/or `src-tauri/Cargo.toml`: cargo check + test
//!     + clippy per manifest.
//! - If nothing applies there is no plan, and the caller is expected to say so
//!   rather than report a vacuous green.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The per-developer green-check override script, looked up at the config root.
const OVERRIDE_FILE: &str = ".swt-check";

/// The manifest whose presence marks a directory as a JavaScript project.
const PACKAGE_JSON: &str = "package.json";

/// The lockfile whose presence means dependencies can be installed reproducibly.
const PNPM_LOCKFILE: &str = "pnpm-lock.yaml";

/// The directory whose presence means dependencies are already installed — and
/// therefore that an install would be a mutation, not a setup step.
const NODE_MODULES: &str = "node_modules";

/// The cargo manifest at the worktree root, which cargo finds on its own.
const ROOT_CARGO_MANIFEST: &str = "Cargo.toml";

/// The second cargo manifest a Tauri-shaped repo carries. One constant serves as
/// both the existence probe and the `--manifest-path` value, so the file that is
/// looked for and the file that is checked can never drift apart.
const TAURI_CARGO_MANIFEST: &str = "src-tauri/Cargo.toml";

/// Preferred spelling of a type-checking script.
const TYPECHECK_SCRIPT: &str = "typecheck";
/// Fallback spelling of a typecheck script, used only when `typecheck` is absent.
const TSC_SCRIPT: &str = "tsc";
/// Lint script name.
const LINT_SCRIPT: &str = "lint";
/// Test script name.
const TEST_SCRIPT: &str = "test";

/// Wraps a string so `sh -c` sees exactly one literal argument.
///
/// Check commands are shell strings by design, so the one piece `swt` splices
/// into them — the absolute path of the `.swt-check` override — has to be
/// quoted: a repo root containing a space, a quote or a `$` would otherwise
/// word-split or expand. Single quotes suppress every expansion; the
/// embedded-quote case is handled by closing, escaping, and reopening
/// (`'` → `'\''`).
///
/// The same applies to a command line `swt` *prints* for a human to paste back
/// into their shell, which is why this is public.
///
/// `s` is the raw string to embed. Returns the single-quoted form, safe to
/// concatenate into a command line.
#[must_use]
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Reads the script names declared in a directory's `package.json`.
///
/// `dir` is a directory that may contain a `package.json`. Returns the set of
/// declared script names — empty when the file is missing, unreadable, or
/// unparseable. A broken manifest is never an error here: it simply declares no
/// scripts, and the plan builder then contributes nothing for it, which is the
/// same conservative answer as "this is not a JavaScript project".
#[must_use]
pub fn pkg_scripts(dir: &Path) -> BTreeSet<String> {
    // Reading and then failing to parse are the same answer here — "no scripts
    // I can use" — so a missing file needs no separate existence probe, and the
    // race between probing and reading it never arises.
    let Ok(text) = fs::read_to_string(dir.join(PACKAGE_JSON)) else {
        return BTreeSet::new();
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BTreeSet::new();
    };
    manifest
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .map(|scripts| scripts.keys().cloned().collect())
        .unwrap_or_default()
}

/// Determines the ordered list of shell commands that constitute the green check
/// for a worktree, based on the files present at its root.
///
/// `target` is the worktree root to inspect, and the directory every command in
/// the returned plan is meant to run in. `config_root` is the directory the
/// `.swt-check` override is looked up in; `None` means the override is looked up
/// in `target` itself, which is the port of the original's `configRoot = target`
/// default parameter. Passing it explicitly is what lets `swt create` honor an
/// uncommitted override that lives in the parent while still verifying the fresh
/// checkout. Both paths are expected to be absolute — every caller derives them
/// from git — because the override path is emitted into a command that runs
/// somewhere else.
///
/// Returns the commands to run in order, or `None` when no check applies.
#[must_use]
pub fn build_check_plan(target: &Path, config_root: Option<&Path>) -> Option<Vec<String>> {
    // Resolved against the config root, run in the target. The escape hatch is
    // documented as a file you *drop* at the repo root — uncommitted, and so
    // absent from the fresh checkout of HEAD that `create` checks. Looking it up
    // in the parent keeps that per-developer override working; running it in the
    // target keeps the check honest about what it is verifying.
    let override_path = config_root.unwrap_or(target).join(OVERRIDE_FILE);
    if override_path.exists() {
        return Some(vec![shell_quote(&override_path.to_string_lossy())]);
    }

    let mut cmds: Vec<String> = Vec::new();

    if target.join(PACKAGE_JSON).exists() {
        let scripts = pkg_scripts(target);
        let mut js_checks: Vec<String> = Vec::new();
        // `tsc` is the fallback spelling, not a second check: a repo declaring
        // both would otherwise type-check itself twice.
        if scripts.contains(TYPECHECK_SCRIPT) {
            js_checks.push("pnpm typecheck".to_string());
        } else if scripts.contains(TSC_SCRIPT) {
            js_checks.push("pnpm exec tsc --noEmit".to_string());
        }
        if scripts.contains(LINT_SCRIPT) {
            js_checks.push("pnpm lint".to_string());
        }
        if scripts.contains(TEST_SCRIPT) {
            js_checks.push("pnpm test --run".to_string());
        }

        // The install verifies nothing on its own — it exists only so the js
        // checks can run in a fresh worktree, which has no node_modules. A plan
        // of just an install would report green having checked nothing, so it
        // rides along with the js checks or not at all.
        //
        // And it only rides along into a tree that is actually fresh. The green
        // check also runs against the parent worktree the user is living in,
        // where an install is not a read-only step: `--frozen-lockfile` prunes
        // extraneous packages and undoes local `pnpm link`s. An existing
        // node_modules is the tell that the dependencies are already there —
        // nothing to set up, and something to lose — so verification inspects
        // that tree without touching it.
        if !js_checks.is_empty() {
            let needs_install =
                target.join(PNPM_LOCKFILE).exists() && !target.join(NODE_MODULES).exists();
            if needs_install {
                cmds.push("pnpm install --frozen-lockfile".to_string());
            }
            cmds.append(&mut js_checks);
        }
    }

    // Rust checks run alongside the package.json ones — Tauri repos have both.
    // `None` is the root manifest, which cargo finds without being told.
    let mut manifests: Vec<Option<&str>> = Vec::new();
    if target.join(ROOT_CARGO_MANIFEST).exists() {
        manifests.push(None);
    }
    if target.join(TAURI_CARGO_MANIFEST).exists() {
        manifests.push(Some(TAURI_CARGO_MANIFEST));
    }
    for manifest in manifests {
        let flag = manifest.map_or_else(String::new, |path| format!(" --manifest-path {path}"));
        cmds.push(format!("cargo check{flag}"));
        cmds.push(format!("cargo test{flag}"));
        cmds.push(format!("cargo clippy{flag} -- -D warnings"));
    }

    (!cmds.is_empty()).then_some(cmds)
}

#[cfg(test)]
mod tests {
    use super::{build_check_plan, pkg_scripts, shell_quote, OVERRIDE_FILE};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// A `package.json` declaring every check script the plan builder looks for.
    const PKG_ALL_CHECKS: &str =
        r#"{"name":"fixture","scripts":{"typecheck":"true","lint":"true","test":"true"}}"#;
    /// The same, but with a `tsc` script standing in for `typecheck`.
    const PKG_TSC: &str =
        r#"{"name":"fixture","scripts":{"tsc":"true","lint":"true","test":"true"}}"#;
    /// Both spellings at once, to pin which one wins.
    const PKG_TYPECHECK_AND_TSC: &str =
        r#"{"name":"fixture","scripts":{"typecheck":"true","tsc":"true"}}"#;
    /// Scripts, but none the green check knows how to use.
    const PKG_NO_CHECKS: &str = r#"{"name":"fixture","scripts":{"build":"true"}}"#;
    /// Several irrelevant scripts, so "no check" is not an artifact of having one script.
    const PKG_ONLY_IRRELEVANT: &str =
        r#"{"name":"fixture","scripts":{"build":"true","dev":"true","start":"true"}}"#;
    /// One relevant script buried among irrelevant ones.
    const PKG_BUILD_AND_TEST: &str =
        r#"{"name":"fixture","scripts":{"build":"true","test":"true"}}"#;
    /// A manifest with no `scripts` block at all.
    const PKG_NO_SCRIPTS_BLOCK: &str = r#"{"name":"fixture"}"#;
    /// Two named scripts, for reading back rather than for planning.
    const PKG_TYPECHECK_AND_LINT: &str =
        r#"{"name":"fixture","scripts":{"typecheck":"true","lint":"true"}}"#;
    /// Not JSON at all.
    const PKG_MALFORMED: &str = "{ this is not json";

    /// A `package.json` fixture entry with every check script.
    const FULL_PKG: (&str, &str) = ("package.json", PKG_ALL_CHECKS);
    /// Lockfile fixture entry; only its existence matters to the plan.
    const LOCKFILE: (&str, &str) = ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
    /// A file *inside* `node_modules`, so the fixture materializes the directory.
    const NODE_MODULES: (&str, &str) = ("node_modules/.modules.yaml", "hoistPattern:\n  - '*'\n");
    /// Minimal manifest that makes a directory auto-detect as a cargo repo.
    const CARGO: (&str, &str) = ("Cargo.toml", "[package]\nname = \"fixture\"\n");
    /// The second manifest a Tauri-shaped repo carries.
    const TAURI_CARGO: (&str, &str) = (
        "src-tauri/Cargo.toml",
        "[package]\nname = \"fixture-tauri\"\n",
    );
    /// A trivial always-green override script.
    const SWT_CHECK: (&str, &str) = (OVERRIDE_FILE, "#!/bin/sh\nexit 0\n");

    /// The js checks a fully-scripted `package.json` produces, without any install.
    const JS_CHECKS: &[&str] = &["pnpm typecheck", "pnpm lint", "pnpm test --run"];
    /// The install that rides along with js checks in a fresh worktree.
    const INSTALL: &str = "pnpm install --frozen-lockfile";
    /// The plan a root `Cargo.toml` alone produces.
    const CARGO_PLAN: &[&str] = &["cargo check", "cargo test", "cargo clippy -- -D warnings"];
    /// The plan a `src-tauri/Cargo.toml` adds after the root manifest's.
    const TAURI_CARGO_PLAN: &[&str] = &[
        "cargo check --manifest-path src-tauri/Cargo.toml",
        "cargo test --manifest-path src-tauri/Cargo.toml",
        "cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings",
    ];

    /// Materializes a fixture directory containing the given files.
    ///
    /// The directory is a `TempDir` with a randomized name, so two concurrent
    /// copies of this test binary — the pre-commit hook's `cargo test` racing a
    /// manual run — never share a fixture path.
    fn fixture(files: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::Builder::new()
            .prefix("swt-green-check-")
            .tempdir()
            .expect("fixture temp dir");
        for (rel_path, contents) in files {
            let full = dir.path().join(rel_path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).expect("fixture parent directory");
            }
            fs::write(&full, contents).expect("fixture file");
        }
        dir
    }

    /// Borrows a plan as string slices so it can be compared against a literal list.
    fn as_strs(plan: &Option<Vec<String>>) -> Option<Vec<&str>> {
        plan.as_ref()
            .map(|cmds| cmds.iter().map(String::as_str).collect())
    }

    /// Single-quotes a path the way a shell needs it, built independently of
    /// [`shell_quote`] so the override tests are not tautological. This is the
    /// original TypeScript test's `'${p.replaceAll("'", "'\\''")}'`.
    fn quoted(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
    }

    /// Asserts a target directory's self-configured plan equals `expected`.
    fn assert_plan(target: &Path, expected: Option<&[&str]>, why: &str) {
        let plan = build_check_plan(target, None);
        assert_eq!(
            as_strs(&plan).as_deref(),
            expected,
            "{why} (target {})",
            target.display()
        );
    }

    /// Every command in a plan that is a pnpm install, for pinpointing failures.
    fn installs_in(plan: &Option<Vec<String>>) -> Vec<&str> {
        as_strs(plan)
            .unwrap_or_default()
            .into_iter()
            .filter(|cmd| cmd.contains("pnpm install"))
            .collect()
    }

    /// Concatenates plan fragments into one expected command list.
    fn joined<'a>(parts: &[&[&'a str]]) -> Vec<&'a str> {
        parts.iter().flat_map(|part| part.iter().copied()).collect()
    }

    #[test]
    fn shell_quote_wraps_a_plain_string_in_single_quotes() {
        assert_eq!(shell_quote("check"), "'check'");
    }

    #[test]
    fn shell_quote_protects_spaces_and_expansions() {
        assert_eq!(
            shell_quote("/repos/my repo/.swt-check"),
            "'/repos/my repo/.swt-check'",
            "a space must not word-split into two arguments"
        );
        assert_eq!(
            shell_quote("$HOME/.swt-check"),
            "'$HOME/.swt-check'",
            "single quotes must suppress expansion, not perform it"
        );
        assert_eq!(
            shell_quote("a `touch pwned` b"),
            "'a `touch pwned` b'",
            "command substitution must stay literal"
        );
    }

    #[test]
    fn shell_quote_closes_escapes_and_reopens_around_an_embedded_quote() {
        assert_eq!(shell_quote("wei'rd"), r"'wei'\''rd'");
        assert_eq!(
            shell_quote("/a/wei'rd $root/.swt-check"),
            r"'/a/wei'\''rd $root/.swt-check'",
            "a path with both a quote and a `$` must survive intact"
        );
        assert_eq!(
            shell_quote("''"),
            r"''\'''\'''",
            "consecutive quotes each escape"
        );
    }

    #[test]
    fn an_empty_directory_has_no_plan() {
        assert_plan(
            fixture(&[]).path(),
            None,
            "nothing to detect means nothing to run",
        );
    }

    #[test]
    fn a_root_cargo_manifest_needs_no_manifest_path() {
        assert_plan(
            fixture(&[CARGO]).path(),
            Some(CARGO_PLAN),
            "the root manifest is cargo's default",
        );
    }

    #[test]
    fn a_src_tauri_manifest_alone_is_checked_by_path() {
        assert_plan(
            fixture(&[TAURI_CARGO]).path(),
            Some(TAURI_CARGO_PLAN),
            "a nested manifest must be named explicitly",
        );
    }

    #[test]
    fn the_root_manifest_is_checked_before_the_src_tauri_one() {
        assert_plan(
            fixture(&[CARGO, TAURI_CARGO]).path(),
            Some(joined(&[CARGO_PLAN, TAURI_CARGO_PLAN]).as_slice()),
            "both manifests are checked, root first",
        );
    }

    #[test]
    fn a_fully_scripted_package_json_maps_each_script_to_its_check() {
        assert_plan(
            fixture(&[FULL_PKG, LOCKFILE]).path(),
            Some(joined(&[&[INSTALL], JS_CHECKS]).as_slice()),
            "typecheck, lint and test each contribute their command",
        );
    }

    #[test]
    fn a_tsc_script_substitutes_for_a_missing_typecheck_script() {
        assert_plan(
            fixture(&[("package.json", PKG_TSC), LOCKFILE]).path(),
            Some(&[
                INSTALL,
                "pnpm exec tsc --noEmit",
                "pnpm lint",
                "pnpm test --run",
            ]),
            "tsc is the fallback spelling of a typecheck",
        );
    }

    #[test]
    fn typecheck_wins_over_tsc_when_both_are_declared() {
        assert_plan(
            fixture(&[("package.json", PKG_TYPECHECK_AND_TSC)]).path(),
            Some(&["pnpm typecheck"]),
            "the two are alternatives, not a pair",
        );
    }

    #[test]
    fn a_package_json_with_no_check_scripts_contributes_nothing() {
        assert_plan(
            fixture(&[("package.json", PKG_NO_CHECKS), LOCKFILE]).path(),
            None,
            "an install alone would report green having verified nothing",
        );
        assert_plan(
            fixture(&[("package.json", PKG_NO_SCRIPTS_BLOCK), LOCKFILE]).path(),
            None,
            "no scripts block is the same as no check scripts",
        );
    }

    #[test]
    fn a_checkless_package_json_adds_no_install_to_a_cargo_plan() {
        assert_plan(
            fixture(&[("package.json", PKG_NO_CHECKS), LOCKFILE, CARGO]).path(),
            Some(CARGO_PLAN),
            "the js side contributes nothing at all, install included",
        );
    }

    #[test]
    fn a_lone_test_script_still_gets_its_install() {
        assert_plan(
            fixture(&[("package.json", PKG_BUILD_AND_TEST), LOCKFILE]).path(),
            Some(&[INSTALL, "pnpm test --run"]),
            "one relevant script is enough to need dependencies",
        );
    }

    #[test]
    fn a_tauri_shaped_repo_runs_js_checks_then_both_cargo_manifests() {
        assert_plan(
            fixture(&[FULL_PKG, LOCKFILE, CARGO, TAURI_CARGO]).path(),
            Some(joined(&[&[INSTALL], JS_CHECKS, CARGO_PLAN, TAURI_CARGO_PLAN]).as_slice()),
            "pnpm and cargo checks are additive, pnpm first",
        );
    }

    // `pnpm install --frozen-lockfile` verifies nothing on its own — it exists
    // only to make the js checks runnable in a fresh worktree. A plan that is
    // just an install would report green having checked nothing, so the install
    // rides along with the js checks or not at all.
    #[test]
    fn the_install_never_appears_without_a_js_check_to_run() {
        let dir = fixture(&[("package.json", PKG_ONLY_IRRELEVANT), LOCKFILE, CARGO]);
        let plan = build_check_plan(dir.path(), None);
        assert_eq!(
            installs_in(&plan),
            Vec::<&str>::new(),
            "plan must not install without a js check to run: {plan:?}"
        );
    }

    // The install is a *setup* step smuggled into a *verification* step, and it
    // is not inert: `--frozen-lockfile` prunes extraneous packages and undoes
    // local `pnpm link`s. That is fine in a fresh worktree, which has nothing to
    // lose, and unacceptable in the parent worktree the user lives in. An
    // existing node_modules is the tell.
    #[test]
    fn the_install_appears_only_with_a_lockfile_and_no_node_modules() {
        let cases: [(bool, bool, bool); 4] = [
            // (lockfile present, node_modules present, install expected)
            (true, false, true),
            (true, true, false),
            (false, false, false),
            (false, true, false),
        ];
        for (lockfile, node_modules, expect_install) in cases {
            let mut files = vec![FULL_PKG];
            if lockfile {
                files.push(LOCKFILE);
            }
            if node_modules {
                files.push(NODE_MODULES);
            }
            let dir = fixture(&files);
            let plan = build_check_plan(dir.path(), None);
            let expected: Vec<&str> = if expect_install {
                joined(&[&[INSTALL], JS_CHECKS])
            } else {
                JS_CHECKS.to_vec()
            };
            assert_eq!(
                as_strs(&plan).as_deref(),
                Some(expected.as_slice()),
                "lockfile={lockfile} node_modules={node_modules}: install expected={expect_install}"
            );
        }
    }

    // Dropping the install is the *only* thing node_modules may change: the
    // checks themselves, their order, and the cargo commands after them stay
    // identical.
    #[test]
    fn node_modules_removes_the_install_and_nothing_else() {
        let files = [("package.json", PKG_TSC), LOCKFILE, CARGO];
        let fresh_dir = fixture(&files);
        let fresh = build_check_plan(fresh_dir.path(), None);

        let mut populated_files = files.to_vec();
        populated_files.push(NODE_MODULES);
        let populated_dir = fixture(&populated_files);
        let populated = build_check_plan(populated_dir.path(), None);

        assert_eq!(
            installs_in(&fresh),
            vec![INSTALL],
            "the fresh tree needs an install"
        );
        let fresh_without_install: Vec<&str> = as_strs(&fresh)
            .unwrap_or_default()
            .into_iter()
            .filter(|cmd| !cmd.contains("pnpm install"))
            .collect();
        assert_eq!(
            as_strs(&populated).as_deref(),
            Some(fresh_without_install.as_slice()),
            "a populated tree differs from a fresh one by exactly the install"
        );
    }

    // A tree with no js checks never had an install to drop, so node_modules is
    // a no-op there — it must not add, remove, or reorder anything.
    #[test]
    fn node_modules_changes_nothing_when_there_are_no_js_checks() {
        let files = [("package.json", PKG_NO_CHECKS), LOCKFILE, CARGO];
        let bare = fixture(&files);
        let mut populated_files = files.to_vec();
        populated_files.push(NODE_MODULES);
        let populated = fixture(&populated_files);

        assert_eq!(
            as_strs(&build_check_plan(populated.path(), None)),
            as_strs(&build_check_plan(bare.path(), None)),
            "node_modules is irrelevant without js checks"
        );
        assert_plan(
            populated.path(),
            Some(CARGO_PLAN),
            "only the cargo checks apply",
        );
    }

    #[test]
    fn an_override_in_the_target_is_the_whole_plan_as_a_quoted_path() {
        let dir = fixture(&[SWT_CHECK]);
        let expected = quoted(&dir.path().join(OVERRIDE_FILE));
        assert_plan(
            dir.path(),
            Some(&[expected.as_str()]),
            "the override replaces detection entirely",
        );
    }

    #[test]
    fn an_override_wins_alone_over_package_json_and_cargo() {
        let dir = fixture(&[SWT_CHECK, FULL_PKG, LOCKFILE, CARGO, TAURI_CARGO]);
        let expected = quoted(&dir.path().join(OVERRIDE_FILE));
        assert_plan(
            dir.path(),
            Some(&[expected.as_str()]),
            "nothing is appended to an override, not even cargo checks",
        );
    }

    #[test]
    fn the_override_is_looked_up_in_the_config_root_not_the_target() {
        let config_root = fixture(&[SWT_CHECK]);
        let target = fixture(&[FULL_PKG, LOCKFILE, CARGO]);
        let plan = build_check_plan(target.path(), Some(config_root.path()));
        let expected = quoted(&config_root.path().join(OVERRIDE_FILE));
        assert_eq!(
            as_strs(&plan).as_deref(),
            Some([expected.as_str()].as_slice()),
            "a parent-only override must beat the target's auto-detection"
        );
    }

    #[test]
    fn an_override_in_the_target_is_ignored_when_a_config_root_is_given() {
        let config_root = fixture(&[]);
        let target = fixture(&[SWT_CHECK, CARGO]);
        let plan = build_check_plan(target.path(), Some(config_root.path()));
        assert_eq!(
            as_strs(&plan).as_deref(),
            Some(CARGO_PLAN),
            "the override lives in the parent; a checkout of HEAD must not supply one"
        );
    }

    #[test]
    fn no_override_anywhere_still_yields_no_plan() {
        let config_root = fixture(&[]);
        let target = fixture(&[]);
        assert_eq!(
            build_check_plan(target.path(), Some(config_root.path())),
            None,
            "a missing override does not invent a check"
        );
    }

    // The emitted command is handed to `sh -c`, so a repo root containing a
    // space, a quote or a `$` has to survive that round trip intact. This is the
    // non-tautological half: it runs the emitted string the way `swt` does.
    #[cfg(unix)]
    #[test]
    fn a_config_root_with_a_space_and_a_quote_runs_under_sh_c() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let holder = fixture(&[]);
        let config_root = holder.path().join("wei'rd $config root");
        fs::create_dir_all(&config_root).expect("weird config root");
        let script = config_root.join(OVERRIDE_FILE);
        fs::write(&script, "#!/bin/sh\nexit 7\n").expect("override script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("make executable");

        let target = fixture(&[CARGO]);
        let plan =
            build_check_plan(target.path(), Some(&config_root)).expect("override yields a plan");
        assert_eq!(plan, vec![quoted(&script)]);

        let status = Command::new("sh")
            .arg("-c")
            .arg(&plan[0])
            .current_dir(target.path())
            .status()
            .expect("sh should run");
        assert_eq!(
            status.code(),
            Some(7),
            "sh could not run the quoted override: {}",
            plan[0]
        );
    }

    #[test]
    fn pkg_scripts_is_empty_without_a_package_json() {
        assert_eq!(pkg_scripts(fixture(&[]).path()), BTreeSet::new());
    }

    #[test]
    fn pkg_scripts_returns_the_declared_script_names() {
        let dir = fixture(&[("package.json", PKG_TYPECHECK_AND_LINT)]);
        let expected: BTreeSet<String> = ["lint", "typecheck"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(pkg_scripts(dir.path()), expected);
    }

    #[test]
    fn pkg_scripts_is_empty_for_a_manifest_without_a_scripts_block() {
        let dir = fixture(&[("package.json", PKG_NO_SCRIPTS_BLOCK)]);
        assert_eq!(pkg_scripts(dir.path()), BTreeSet::new());
    }

    #[test]
    fn pkg_scripts_is_empty_for_a_malformed_manifest_instead_of_failing() {
        let dir = fixture(&[("package.json", PKG_MALFORMED)]);
        assert_eq!(
            pkg_scripts(dir.path()),
            BTreeSet::new(),
            "an unparseable manifest declares no scripts rather than exploding"
        );
        assert_plan(
            dir.path(),
            None,
            "an unparseable manifest contributes no checks either",
        );
    }
}
