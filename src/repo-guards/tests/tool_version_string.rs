//! Guard test for the CLAUDE.md rule that every tool in this repository "must
//! display version information including git hash and dirty status when
//! `--version` or `-V` is used", via the `buildinfo` crate.
//!
//! The rule is easy to break silently: `#[clap(author, version, about)]` looks
//! completely reasonable, compiles, and produces a working `--version` — it
//! just prints the bare `CARGO_PKG_VERSION` with no build information, which
//! nobody notices until they need to know which commit a binary came from.
//! ufa shipped that way. This test makes the omission fail `cargo test`.
//!
//! It asserts on the source rather than on a built binary deliberately: running
//! the real `ufa --version` would mean building the whole crate (and its TLS
//! stack) from a test in an unrelated crate. The two things it checks —
//! `buildinfo` being a declared dependency, and `version_string!` being wired
//! into the clap `version` attribute — are jointly sufficient, because clap
//! only honors a `version = ...` expression it can actually evaluate.
//!
//! Like `pre_commit_hook.rs`, this covers *existing* correct behavior, so a TDD
//! red phase is impossible — it passes immediately. The mutation check that
//! substitutes for red (revert ufa's clap attribute to a bare `version`, watch
//! this test fail, restore, watch it pass) is documented in the commit body.
//!
//! Parallel safety: this test only reads files that are already checked in and
//! creates nothing, so concurrent copies cannot interfere with each other.
//!
//! TODO: widen this to every binary crate in the workspace. `beta` is the known
//! remaining gap — it does not use `buildinfo` either — and fixing it is out of
//! scope for this branch, so the assertion is deliberately scoped to `ufa`
//! rather than looping over `src/*`. Whoever fixes `beta` should replace
//! `TOOLS_REQUIRING_BUILDINFO_VERSION` with a discovered list of every crate
//! that has a `[[bin]]` target or a `src/main.rs`.

use std::fs;
use std::path::{Path, PathBuf};

/// Crates whose `--version` output this test currently enforces.
///
/// See the module-level TODO: this is scoped rather than discovered, because
/// `beta` does not satisfy the rule yet.
const TOOLS_REQUIRING_BUILDINFO_VERSION: &[&str] = &["ufa"];

/// The dependency every tool must pull in to get git build information.
const BUILDINFO_CRATE: &str = "buildinfo";

/// The macro that expands to `"<version> (<hash>, <clean|dirty>)"`.
const VERSION_MACRO: &str = "version_string!";

/// Absolute path to `src/<crate_name>` within the repository.
fn crate_dir(crate_name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(crate_name);
    fs::canonicalize(&dir)
        .unwrap_or_else(|e| panic!("cannot canonicalize crate dir {}: {e}", dir.display()))
}

/// Read a file, failing with the path in the message rather than a bare
/// `NotFound`.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Returns true if `manifest` declares `dependency` under `[dependencies]`.
///
/// Hand-rolled rather than pulled from a TOML crate so this guard stays
/// dependency-free like the rest of `repo-guards`. It understands the three
/// shapes used in this workspace: `dep.workspace = true`, `dep = { ... }`, and
/// a `[dependencies.dep]` sub-table.
fn declares_dependency(manifest: &str, dependency: &str) -> bool {
    let sub_table = format!("[dependencies.{dependency}]");
    let mut in_dependencies = false;

    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') {
            if line == sub_table {
                return true;
            }
            in_dependencies = line == "[dependencies]";
            continue;
        }

        if !in_dependencies {
            continue;
        }

        // `buildinfo.workspace = true` -> key `buildinfo.workspace` -> head
        // `buildinfo`. Quotes are stripped so `"buildinfo" = ...` also matches.
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let head = key.trim().split('.').next().unwrap_or_default();
        if head.trim_matches('"') == dependency {
            return true;
        }
    }

    false
}

/// Every tool must declare `buildinfo` so the git hash is available at all.
#[test]
fn tools_declare_the_buildinfo_dependency() {
    for tool in TOOLS_REQUIRING_BUILDINFO_VERSION {
        let manifest_path = crate_dir(tool).join("Cargo.toml");
        let manifest = read(&manifest_path);

        assert!(
            declares_dependency(&manifest, BUILDINFO_CRATE),
            "{} must declare a `{BUILDINFO_CRATE}` dependency so `--version` can \
             report the git hash and dirty status (CLAUDE.md); see {}",
            tool,
            manifest_path.display()
        );
    }
}

/// Declaring the dependency is not enough — the clap `version` attribute has to
/// actually use it, otherwise `--version` still prints the bare package version.
#[test]
fn tools_wire_version_string_into_the_version_flag() {
    for tool in TOOLS_REQUIRING_BUILDINFO_VERSION {
        let main_path = crate_dir(tool).join("src/main.rs");
        let main_rs = read(&main_path);

        assert!(
            main_rs.contains(VERSION_MACRO),
            "{} must expand `{VERSION_MACRO}` so `--version` reports the git hash \
             and dirty status (CLAUDE.md); see {}",
            tool,
            main_path.display()
        );

        // A bare `version` in the clap attribute silently wins over an unused
        // import, so require the macro to be attached to the version flag.
        assert!(
            main_rs.contains(&format!("version = {VERSION_MACRO}")),
            "{}'s clap attribute must read `version = {VERSION_MACRO}()`, not a \
             bare `version`, or clap falls back to CARGO_PKG_VERSION alone; see {}",
            tool,
            main_path.display()
        );
    }
}

mod dependency_parsing_tests {
    use super::{declares_dependency, BUILDINFO_CRATE};

    /// Mutation coverage for the parser itself: if `declares_dependency` were
    /// quietly broken, the guard above would pass forever without checking
    /// anything.
    #[test]
    fn recognizes_the_workspace_inheritance_shape() {
        assert!(declares_dependency(
            "[dependencies]\nbuildinfo.workspace = true\n",
            BUILDINFO_CRATE
        ));
    }

    #[test]
    fn recognizes_inline_table_and_sub_table_shapes() {
        assert!(declares_dependency(
            "[dependencies]\nbuildinfo = { path = \"../buildinfo\" }\n",
            BUILDINFO_CRATE
        ));
        assert!(declares_dependency(
            "[dependencies.buildinfo]\npath = \"../buildinfo\"\n",
            BUILDINFO_CRATE
        ));
    }

    #[test]
    fn rejects_a_manifest_without_the_dependency() {
        assert!(!declares_dependency(
            "[dependencies]\nclap = \"4\"\nserde = \"1\"\n",
            BUILDINFO_CRATE
        ));
    }

    #[test]
    fn ignores_matches_outside_the_dependencies_section() {
        // A dev-dependency or a commented-out line must not satisfy the guard:
        // neither makes `buildinfo` available to the binary target.
        assert!(!declares_dependency(
            "[dev-dependencies]\nbuildinfo.workspace = true\n",
            BUILDINFO_CRATE
        ));
        assert!(!declares_dependency(
            "[dependencies]\n# buildinfo.workspace = true\n",
            BUILDINFO_CRATE
        ));
    }
}
