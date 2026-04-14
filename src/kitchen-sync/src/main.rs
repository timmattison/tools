// Crate-local lints locking in review fixes:
//   - clippy::exit catches std::process::exit() which skips destructors
//     (leaked our TempDir before this was enforced).
//   - clippy::format_push_string catches `s.push_str(&format!(...))` which
//     allocates an extra String; `write!(&mut s, ...)` is the idiomatic form.
#![deny(clippy::exit, clippy::format_push_string)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{bail, Context, Result};
use buildinfo::version_string;
use clap::Parser;
use tempfile::TempDir;

/// Install every Rust binary from a git repository
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// Git repository URL (e.g. https://github.com/user/repo)
    repo_url: String,
}

/// A workspace member that produces at least one binary.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BinaryPackage {
    name: String,
}

/// Outcome of attempting to install a single package.
#[derive(Debug)]
struct InstallOutcome {
    package: String,
    error: Option<String>,
}

/// Result of inspecting a candidate workspace-member directory.
#[derive(Debug)]
enum MemberClass {
    Binary(BinaryPackage),
    LibraryOnly,
    NoManifest,
    ParseError(String),
}

/// Classify a member directory into [`MemberClass`].
fn classify_member(dir: &Path) -> MemberClass {
    let manifest_path = dir.join("Cargo.toml");
    if !manifest_path.exists() {
        return MemberClass::NoManifest;
    }
    let manifest = match parse_manifest(&manifest_path) {
        Ok(m) => m,
        Err(e) => return MemberClass::ParseError(e.to_string()),
    };
    if !is_binary_package(&manifest, dir) {
        return MemberClass::LibraryOnly;
    }
    let Some(name) = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
    else {
        return MemberClass::LibraryOnly;
    };
    MemberClass::Binary(BinaryPackage {
        name: name.to_owned(),
    })
}

fn main() -> Result<ExitCode> {
    let args = Args::parse();

    let temp_dir = shallow_clone(&args.repo_url)?;
    let repo_path = temp_dir.path();

    let root_manifest = repo_path.join("Cargo.toml");
    if !root_manifest.exists() {
        bail!("Not a Rust project (no Cargo.toml found)");
    }

    let manifest = parse_manifest(&root_manifest)?;

    let code = if manifest.get("workspace").is_some() {
        run_workspace_install(&args.repo_url, &manifest, repo_path)?
    } else {
        if !is_binary_package(&manifest, repo_path) {
            bail!("No binary packages found in repository");
        }
        install_from_git(&args.repo_url, None)?;
        ExitCode::SUCCESS
    };

    // `temp_dir` drops here (Drop cleans up the clone) before we propagate
    // the exit code.
    drop(temp_dir);
    Ok(code)
}

/// Discover every binary package in a Cargo workspace.
///
/// Expands `workspace.members` globs and optionally includes the root crate
/// (when the root `Cargo.toml` has a `[package]` section alongside
/// `[workspace]`). Library-only members are filtered out.
///
/// # Errors
///
/// Returns an error if a member glob pattern is malformed.
fn discover_workspace_packages(
    repo_root: &Path,
    root_manifest: &toml::Value,
) -> Result<Vec<BinaryPackage>> {
    let patterns = extract_workspace_members(root_manifest);
    let excludes = extract_workspace_exclude(root_manifest);
    let excluded_dirs = resolve_workspace_members(repo_root, &excludes)?;
    let mut member_dirs: Vec<PathBuf> = resolve_workspace_members(repo_root, &patterns)?
        .into_iter()
        .filter(|d| !excluded_dirs.iter().any(|ex| paths_equal(d, ex)))
        .collect();

    // If the root manifest itself has a [package] section, the root crate is
    // part of the workspace and should be considered for install too. Skip if
    // it's already covered by a members glob.
    if root_manifest.get("package").is_some()
        && !member_dirs.iter().any(|d| paths_equal(d, repo_root))
    {
        member_dirs.push(repo_root.to_path_buf());
    }

    collect_binary_packages(&member_dirs)
}

/// Compare two paths by canonicalized form, falling back to raw equality
/// if canonicalize fails (e.g. path does not yet exist on disk).
fn paths_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Discover all binary packages in a Cargo workspace and install each one,
/// continuing past individual failures.
fn run_workspace_install(
    repo_url: &str,
    root_manifest: &toml::Value,
    repo_path: &Path,
) -> Result<ExitCode> {
    let packages = discover_workspace_packages(repo_path, root_manifest)?;

    if packages.is_empty() {
        bail!("No binary packages found in repository");
    }

    let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    println!(
        "Found {} binary package{}: {}",
        packages.len(),
        if packages.len() == 1 { "" } else { "s" },
        names.join(", ")
    );
    println!();

    let outcomes = install_packages(repo_url, &packages);

    print!("{}", format_summary(&outcomes));

    let code = u8::try_from(exit_code_for(&outcomes)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

/// Shallow-clone `repo_url` into a new temp directory and return the handle.
///
/// git's stderr is captured so that on failure we can surface the underlying
/// reason (bad URL, network error, auth prompt) rather than just an exit code.
fn shallow_clone(repo_url: &str) -> Result<TempDir> {
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    println!("Cloning {repo_url}...");

    let output = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(repo_url)
        .arg(temp_dir.path())
        .output()
        .context("Failed to run git clone (is git installed?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.trim();
        if reason.is_empty() {
            bail!("git clone failed with exit status {}", output.status);
        } else {
            bail!("git clone failed: {reason}");
        }
    }

    Ok(temp_dir)
}

/// Read and parse a Cargo.toml file into a generic toml::Value.
///
/// # Errors
///
/// Returns an error if the file cannot be read or if its contents are not
/// valid TOML.
fn parse_manifest(path: &Path) -> Result<toml::Value> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))
}

/// A package is "binary" iff its Cargo.toml declares a `[[bin]]` section,
/// `src/main.rs` exists, or `src/bin/` contains at least one `*.rs` file.
///
/// The `src/bin/` case matches Cargo's auto-discovery: any `src/bin/*.rs` is
/// treated as a binary target even without a `[[bin]]` entry.
fn is_binary_package(manifest: &toml::Value, package_dir: &Path) -> bool {
    if manifest.get("bin").is_some() {
        return true;
    }
    if package_dir.join("src").join("main.rs").exists() {
        return true;
    }
    has_rs_file_in(&package_dir.join("src").join("bin"))
}

/// Return true if `dir` exists and contains at least one `*.rs` entry.
fn has_rs_file_in(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|e| e.path().extension().is_some_and(|ext| ext == "rs"))
}

/// Extract the `workspace.members` array from a root Cargo.toml.
fn extract_workspace_members(manifest: &toml::Value) -> Vec<String> {
    extract_workspace_string_array(manifest, "members")
}

/// Extract the `workspace.exclude` array from a root Cargo.toml.
fn extract_workspace_exclude(manifest: &toml::Value) -> Vec<String> {
    extract_workspace_string_array(manifest, "exclude")
}

/// Extract a named string-array under `[workspace]`. Returns an empty vec
/// if the key is missing or not an array of strings.
fn extract_workspace_string_array(manifest: &toml::Value, key: &str) -> Vec<String> {
    manifest
        .get("workspace")
        .and_then(|w| w.get(key))
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve `workspace.members` patterns (which may contain globs) into concrete
/// directory paths inside `repo_root`. Patterns are joined with `repo_root`
/// before globbing so literal paths like `crates/foo` work even when they do
/// not contain glob metacharacters.
///
/// # Errors
///
/// Returns an error if a glob pattern is malformed.
fn resolve_workspace_members(repo_root: &Path, patterns: &[String]) -> Result<Vec<PathBuf>> {
    let repo_root_canonical = std::fs::canonicalize(repo_root)
        .with_context(|| format!("Failed to canonicalize repo root: {}", repo_root.display()))?;
    let mut resolved = Vec::new();
    for pattern in patterns {
        let joined = repo_root.join(pattern);
        let joined_str = joined
            .to_str()
            .with_context(|| format!("Non-UTF-8 path in workspace member pattern: {pattern}"))?;
        let entries = glob::glob(joined_str)
            .with_context(|| format!("Invalid workspace member pattern: {pattern}"))?;
        for entry in entries {
            let path =
                entry.with_context(|| format!("Failed to resolve member pattern: {pattern}"))?;
            if !path.is_dir() {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).with_context(|| {
                format!("Failed to canonicalize member path: {}", path.display())
            })?;
            if !canonical.starts_with(&repo_root_canonical) {
                bail!(
                    "workspace member {} escapes repo root (pattern: {pattern})",
                    canonical.display()
                );
            }
            resolved.push(canonical);
        }
    }
    Ok(resolved)
}

/// For each member directory, classify it via [`classify_member`] and return
/// the binary packages. Parse errors are written to stderr so the user can
/// tell why a directory was skipped; missing manifests and library-only
/// members are skipped silently (real workspaces may have stray directories
/// that match a glob but aren't packages).
///
/// # Errors
///
/// Currently infallible, but returns `Result` so future validation work
/// (e.g., aborting on duplicate package names) does not require a signature
/// change.
fn collect_binary_packages(member_dirs: &[PathBuf]) -> Result<Vec<BinaryPackage>> {
    let mut packages = Vec::new();
    for dir in member_dirs {
        match classify_member(dir) {
            MemberClass::Binary(pkg) => packages.push(pkg),
            MemberClass::ParseError(err) => {
                eprintln!(
                    "warning: skipping {}: Cargo.toml did not parse ({err})",
                    dir.display()
                );
            }
            MemberClass::LibraryOnly | MemberClass::NoManifest => {}
        }
    }
    Ok(packages)
}

/// Install each package sequentially, returning an outcome per package.
fn install_packages(repo_url: &str, packages: &[BinaryPackage]) -> Vec<InstallOutcome> {
    let total = packages.len();
    packages
        .iter()
        .enumerate()
        .map(|(i, pkg)| {
            println!("Installing {} ({}/{})...", pkg.name, i + 1, total);
            let outcome = match install_from_git(repo_url, Some(&pkg.name)) {
                Ok(()) => InstallOutcome {
                    package: pkg.name.clone(),
                    error: None,
                },
                Err(e) => InstallOutcome {
                    package: pkg.name.clone(),
                    error: Some(e.to_string()),
                },
            };
            if outcome.error.is_some() {
                println!("  FAILED");
            } else {
                println!("  Installed {}", pkg.name);
            }
            outcome
        })
        .collect()
}

/// Return the process exit code to use for a set of install outcomes.
///
/// Returns `1` if any package failed (even if others succeeded) so that
/// CI and scripted callers see partial failures as failures.
fn exit_code_for(outcomes: &[InstallOutcome]) -> i32 {
    if outcomes.iter().any(|o| o.error.is_some()) {
        1
    } else {
        0
    }
}

/// Format a summary of install outcomes as a multi-line string.
///
/// Failed entries show the package name followed by the captured error text,
/// so users don't have to scroll back through the live install output to find
/// out what went wrong.
fn format_summary(outcomes: &[InstallOutcome]) -> String {
    use std::fmt::Write as _;
    let installed = outcomes.iter().filter(|o| o.error.is_none()).count();
    let failed = outcomes.len() - installed;
    let mut out = String::new();
    out.push('\n');
    // writeln! into a String is infallible; unwrap preserves the "can't
    // fail" guarantee loudly if that ever changes.
    writeln!(out, "Summary: {installed} installed, {failed} failed").unwrap();
    if failed > 0 {
        out.push_str("  Failed:\n");
        for outcome in outcomes.iter().filter(|o| o.error.is_some()) {
            let err = outcome.error.as_deref().unwrap_or("");
            writeln!(out, "    {}: {err}", outcome.package).unwrap();
        }
    }
    out
}

/// Run `cmd`, forwarding its stderr to our own stderr while also capturing
/// it so the caller can surface diagnostics in a summary.
///
/// stdout is left inherited (cargo's progress bars remain visible live).
///
/// # Errors
///
/// Returns an `io::Error` from `spawn`, `wait`, or the stderr reader thread.
fn run_streaming(cmd: &mut Command) -> std::io::Result<(std::process::ExitStatus, String)> {
    let mut child = cmd.stderr(Stdio::piped()).spawn()?;
    let stderr = child
        .stderr
        .take()
        .expect("stderr was configured as piped above");
    let reader = std::thread::spawn(move || {
        let mut captured = String::new();
        for line in BufReader::new(stderr).lines() {
            let line = line?;
            eprintln!("{line}");
            captured.push_str(&line);
            captured.push('\n');
        }
        Ok::<String, std::io::Error>(captured)
    });
    let status = child.wait()?;
    let captured = reader
        .join()
        .map_err(|_| std::io::Error::other("stderr reader thread panicked"))??;
    Ok((status, captured))
}

/// Run `cargo install --git <url>` optionally pinned to a specific package.
///
/// On failure the error message includes the tail of cargo's stderr so the
/// caller can surface a useful diagnostic in the summary without the user
/// having to scroll back.
fn install_from_git(repo_url: &str, package: Option<&str>) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("install").arg("--git").arg(repo_url);
    // cargo install selects specific packages via a positional crate name, not --package.
    if let Some(pkg) = package {
        cmd.arg(pkg);
    }

    let (status, captured_stderr) =
        run_streaming(&mut cmd).context("Failed to run cargo install (is cargo installed?)")?;

    if !status.success() {
        let tail = last_lines(&captured_stderr, STDERR_TAIL_LINES);
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("\n{tail}")
        };
        match status.code() {
            Some(code) => bail!("cargo install failed with exit code {code}{detail}"),
            None => bail!("cargo install was terminated by signal{detail}"),
        }
    }

    Ok(())
}

/// Number of trailing stderr lines to include in a failed-install summary.
const STDERR_TAIL_LINES: usize = 10;

/// Return the last `n` non-empty lines of `s`, joined by newlines.
fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ----- Phase 1 tests -----

    #[test]
    fn parses_simple_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(
            &manifest_path,
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let value = parse_manifest(&manifest_path).unwrap();
        assert_eq!(value["package"]["name"].as_str(), Some("demo"));
    }

    #[test]
    fn detects_bin_section_as_binary_package() {
        let dir = tempfile::tempdir().unwrap();
        let manifest: toml::Value = toml::from_str(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"demo\"\npath = \"src/demo.rs\"\n",
        )
        .unwrap();
        assert!(is_binary_package(&manifest, dir.path()));
    }

    #[test]
    fn detects_main_rs_as_binary_package() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();
        let manifest: toml::Value =
            toml::from_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(is_binary_package(&manifest, dir.path()));
    }

    #[test]
    fn detects_src_bin_auto_discovery_as_binary_package() {
        // Cargo auto-discovers binaries from any `src/bin/*.rs` file even when
        // there is no `[[bin]]` section and no `src/main.rs`. Treat these as
        // binary packages.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("bin")).unwrap();
        fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();
        fs::write(
            dir.path().join("src").join("bin").join("thing.rs"),
            "fn main() {}\n",
        )
        .unwrap();
        let manifest: toml::Value =
            toml::from_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(is_binary_package(&manifest, dir.path()));
    }

    #[test]
    fn empty_src_bin_directory_is_not_binary() {
        // An empty `src/bin/` directory alone (no main.rs, no rs files in it)
        // is not a binary package.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src").join("bin")).unwrap();
        fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();
        let manifest: toml::Value =
            toml::from_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(!is_binary_package(&manifest, dir.path()));
    }

    #[test]
    fn library_only_package_is_not_binary() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();
        let manifest: toml::Value =
            toml::from_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(!is_binary_package(&manifest, dir.path()));
    }

    // ----- Phase 2 tests -----

    #[test]
    fn extracts_workspace_members_array() {
        let manifest: toml::Value = toml::from_str(
            "[workspace]\nresolver = \"2\"\nmembers = [\"src/*\", \"crates/foo\"]\n",
        )
        .unwrap();
        let members = extract_workspace_members(&manifest);
        assert_eq!(members, vec!["src/*".to_string(), "crates/foo".to_string()]);
    }

    #[test]
    fn extracts_empty_members_when_workspace_has_none() {
        let manifest: toml::Value = toml::from_str("[workspace]\nresolver = \"2\"\n").unwrap();
        let members = extract_workspace_members(&manifest);
        assert!(members.is_empty());
    }

    #[test]
    fn resolves_glob_patterns_to_member_dirs() {
        let root = tempfile::tempdir().unwrap();
        // Create src/alpha, src/beta, src/gamma
        for name in ["alpha", "beta", "gamma"] {
            fs::create_dir_all(root.path().join("src").join(name)).unwrap();
            fs::write(
                root.path().join("src").join(name).join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                    name
                ),
            )
            .unwrap();
        }

        let patterns = vec!["src/*".to_string()];
        let resolved = resolve_workspace_members(root.path(), &patterns).unwrap();
        assert_eq!(resolved.len(), 3);
        let names: Vec<String> = resolved
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        assert!(names.contains(&"gamma".to_string()));
    }

    #[test]
    fn resolves_literal_paths_without_glob() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("crates").join("foo")).unwrap();
        fs::write(
            root.path().join("crates").join("foo").join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let patterns = vec!["crates/foo".to_string()];
        let resolved = resolve_workspace_members(root.path(), &patterns).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].file_name().unwrap().to_string_lossy(),
            "foo".to_string()
        );
    }

    #[test]
    fn classify_member_reports_parse_error() {
        // A malformed Cargo.toml should be reported as ParseError, not
        // silently skipped — users otherwise have no way to tell why their
        // binary wasn't picked up.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "this is not = valid [toml").unwrap();
        let kind = classify_member(dir.path());
        assert!(
            matches!(kind, MemberClass::ParseError(_)),
            "expected ParseError, got {kind:?}"
        );
    }

    #[test]
    fn classify_member_reports_no_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let kind = classify_member(dir.path());
        assert!(
            matches!(kind, MemberClass::NoManifest),
            "expected NoManifest, got {kind:?}"
        );
    }

    #[test]
    fn classify_member_reports_library_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"libby\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let kind = classify_member(dir.path());
        assert!(
            matches!(kind, MemberClass::LibraryOnly),
            "expected LibraryOnly, got {kind:?}"
        );
    }

    #[test]
    fn classify_member_reports_binary() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("main.rs"), "fn main(){}").unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"binny\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let kind = classify_member(dir.path());
        match kind {
            MemberClass::Binary(pkg) => assert_eq!(pkg.name, "binny"),
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    #[test]
    fn collect_binary_packages_skips_library_only_members() {
        let root = tempfile::tempdir().unwrap();
        // binary crate: has main.rs
        let bin_dir = root.path().join("binny");
        fs::create_dir_all(bin_dir.join("src")).unwrap();
        fs::write(
            bin_dir.join("Cargo.toml"),
            "[package]\nname = \"binny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(bin_dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();

        // lib crate: only lib.rs, no [[bin]]
        let lib_dir = root.path().join("libby");
        fs::create_dir_all(lib_dir.join("src")).unwrap();
        fs::write(
            lib_dir.join("Cargo.toml"),
            "[package]\nname = \"libby\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(lib_dir.join("src").join("lib.rs"), "").unwrap();

        let members = vec![bin_dir, lib_dir];
        let packages = collect_binary_packages(&members).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "binny");
    }

    // ----- stderr capture helper -----

    #[test]
    fn run_streaming_captures_stderr_and_returns_exit_code() {
        // The helper must run a subprocess, return the exit status, AND give
        // back whatever the child wrote to stderr so the caller can include
        // it in a summary. We use `sh -c` so the test does not depend on any
        // specific real tool being installed.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("printf 'oops line 1\\noops line 2\\n' 1>&2; exit 3");
        let (status, captured) = run_streaming(&mut cmd).unwrap();
        assert_eq!(status.code(), Some(3));
        assert!(
            captured.contains("oops line 1") && captured.contains("oops line 2"),
            "expected captured stderr to contain both lines, got: {captured:?}"
        );
    }

    // ----- Phase 3 tests -----

    #[test]
    fn exit_code_is_failure_when_any_install_failed() {
        // Callers (CI scripts, other tools) need a non-zero exit code if any
        // package failed, even when others succeeded. The original policy
        // (exit 0 if >= 1 succeeded) masked partial failures from automation.
        let all_ok = vec![InstallOutcome {
            package: "a".into(),
            error: None,
        }];
        assert_eq!(exit_code_for(&all_ok), 0);

        let partial = vec![
            InstallOutcome {
                package: "a".into(),
                error: None,
            },
            InstallOutcome {
                package: "b".into(),
                error: Some("boom".into()),
            },
        ];
        assert_eq!(exit_code_for(&partial), 1);

        let all_failed = vec![InstallOutcome {
            package: "a".into(),
            error: Some("boom".into()),
        }];
        assert_eq!(exit_code_for(&all_failed), 1);
    }

    #[test]
    fn summary_with_only_successes_has_no_failed_block() {
        let outcomes = vec![
            InstallOutcome {
                package: "alpha".into(),
                error: None,
            },
            InstallOutcome {
                package: "beta".into(),
                error: None,
            },
        ];
        let out = format_summary(&outcomes);
        assert!(out.contains("Summary: 2 installed, 0 failed"));
        assert!(!out.contains("Failed:"));
    }

    #[test]
    fn summary_shows_error_output_for_failed_packages() {
        let outcomes = vec![
            InstallOutcome {
                package: "alpha".into(),
                error: None,
            },
            InstallOutcome {
                package: "broken".into(),
                error: Some("cargo install failed with exit status 101".into()),
            },
        ];
        let out = format_summary(&outcomes);
        assert!(out.contains("Summary: 1 installed, 1 failed"));
        assert!(out.contains("broken"));
        // This is the Phase 3 requirement: captured error output appears in the
        // summary, not only the package name.
        assert!(
            out.contains("cargo install failed with exit status 101"),
            "expected error text in summary, got:\n{out}"
        );
    }

    #[test]
    fn summary_lists_multiple_failures_with_their_errors() {
        let outcomes = vec![
            InstallOutcome {
                package: "one".into(),
                error: Some("boom one".into()),
            },
            InstallOutcome {
                package: "two".into(),
                error: Some("boom two".into()),
            },
        ];
        let out = format_summary(&outcomes);
        assert!(out.contains("Summary: 0 installed, 2 failed"));
        assert!(out.contains("boom one"), "missing first error:\n{out}");
        assert!(out.contains("boom two"), "missing second error:\n{out}");
    }

    #[test]
    fn discover_includes_root_package_when_root_is_also_workspace() {
        // A real-world pattern: the root Cargo.toml contains BOTH [workspace]
        // and [package] (a root binary crate that also owns a workspace). The
        // root crate must be installed alongside the workspace members.
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"rootcli\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(root.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();

        let foo_dir = root.path().join("crates").join("foo");
        fs::create_dir_all(foo_dir.join("src")).unwrap();
        fs::write(
            foo_dir.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(foo_dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();

        let root_manifest = parse_manifest(&root.path().join("Cargo.toml")).unwrap();
        let packages = discover_workspace_packages(root.path(), &root_manifest).unwrap();
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"rootcli"),
            "missing root crate in {names:?}"
        );
        assert!(names.contains(&"foo"), "missing foo in {names:?}");
    }

    #[test]
    fn discover_respects_workspace_exclude() {
        // workspace.members = ["crates/*"] with workspace.exclude =
        // ["crates/vendored"] must skip the excluded dir even though it
        // matches the glob.
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\nexclude = [\"crates/vendored\"]\n",
        )
        .unwrap();

        for name in ["keep", "vendored"] {
            let dir = root.path().join("crates").join(name);
            fs::create_dir_all(dir.join("src")).unwrap();
            fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            )
            .unwrap();
            fs::write(dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        }

        let root_manifest = parse_manifest(&root.path().join("Cargo.toml")).unwrap();
        let packages = discover_workspace_packages(root.path(), &root_manifest).unwrap();
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"keep"), "expected keep in {names:?}");
        assert!(
            !names.contains(&"vendored"),
            "excluded dir should not appear: {names:?}"
        );
    }

    #[test]
    fn resolve_rejects_members_that_escape_repo_root() {
        // A hostile (or buggy) workspace.members entry using `..` could point
        // outside the cloned repo. Reject it rather than walk up the tree.
        let root = tempfile::tempdir().unwrap();
        // Create a sibling directory outside `root` that would be the target
        // of `../sibling` resolution. Put a Cargo.toml there so glob() would
        // otherwise happily resolve it.
        let sibling = root.path().parent().unwrap().join("sibling-outside-repo");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(
            sibling.join("Cargo.toml"),
            "[package]\nname = \"sneaky\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let patterns = vec!["../sibling-outside-repo".to_string()];
        let err = resolve_workspace_members(root.path(), &patterns).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("outside") || msg.contains("escapes"),
            "expected escape error, got: {msg}"
        );

        fs::remove_dir_all(sibling).ok();
    }

    #[test]
    fn collect_binary_packages_extracts_names_from_manifest() {
        let root = tempfile::tempdir().unwrap();
        let pkg_dir = root.path().join("anywhere");
        fs::create_dir_all(pkg_dir.join("src")).unwrap();
        fs::write(
            pkg_dir.join("Cargo.toml"),
            "[package]\nname = \"my-cool-tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(pkg_dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();

        let packages = collect_binary_packages(&[pkg_dir]).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "my-cool-tool");
    }

    // ----- integration-style tests (use real `git` subprocess) -----

    /// Create a local non-bare git repo with the given files already committed.
    /// Returns the repo path inside a `TempDir` that lives as long as the caller
    /// keeps the handle.
    fn make_local_git_repo(files: &[(&str, &str)]) -> (TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path().to_path_buf();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("git must be on PATH for integration tests");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "commit.gpgsign", "false"]);
        for (path, contents) in files {
            let full = repo.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, contents).unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        (td, repo)
    }

    #[test]
    fn shallow_clone_of_local_repo_preserves_files() {
        let (_src_td, src_repo) = make_local_git_repo(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
            ),
            ("src/main.rs", "fn main() {}\n"),
        ]);

        let cloned = shallow_clone(src_repo.to_str().unwrap()).unwrap();
        assert!(cloned.path().join("Cargo.toml").exists());
        assert!(cloned.path().join("src").join("main.rs").exists());
    }

    #[test]
    fn shallow_clone_of_nonexistent_repo_surfaces_git_error() {
        let bogus = tempfile::tempdir().unwrap();
        let target = bogus.path().join("does-not-exist");
        let err = shallow_clone(target.to_str().unwrap()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("clone failed"),
            "expected 'clone failed' in error, got: {msg}"
        );
    }
}
