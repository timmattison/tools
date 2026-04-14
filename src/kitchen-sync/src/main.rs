use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
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

fn main() -> Result<()> {
    let args = Args::parse();

    let temp_dir = shallow_clone(&args.repo_url)?;
    let repo_path = temp_dir.path();

    let root_manifest = repo_path.join("Cargo.toml");
    if !root_manifest.exists() {
        bail!("Not a Rust project (no Cargo.toml found)");
    }

    let manifest = parse_manifest(&root_manifest)?;

    if manifest.get("workspace").is_some() {
        // Phase 2 will implement workspace discovery.
        bail!("workspace support not yet implemented");
    }

    if !is_binary_package(&manifest, repo_path) {
        bail!("No binary packages found in repository");
    }

    install_from_git(&args.repo_url, None)
}

/// Shallow-clone `repo_url` into a new temp directory and return the handle.
fn shallow_clone(repo_url: &str) -> Result<TempDir> {
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    println!("Cloning {}...", repo_url);

    let status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(repo_url)
        .arg(temp_dir.path())
        .status()
        .context("Failed to run git clone (is git installed?)")?;

    if !status.success() {
        bail!("git clone failed with exit status {}", status);
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

/// A package is "binary" iff its Cargo.toml declares a `[[bin]]` section
/// OR `src/main.rs` exists inside the package directory.
fn is_binary_package(manifest: &toml::Value, package_dir: &Path) -> bool {
    if manifest.get("bin").is_some() {
        return true;
    }
    package_dir.join("src").join("main.rs").exists()
}

/// Run `cargo install --git <url>` optionally pinned to a specific package.
fn install_from_git(repo_url: &str, package: Option<&str>) -> Result<()> {
    let label = package.unwrap_or("repository");
    println!("Installing {}...", label);

    let mut cmd = Command::new("cargo");
    cmd.arg("install").arg("--git").arg(repo_url);
    if let Some(pkg) = package {
        cmd.arg("--package").arg(pkg);
    }

    let status = cmd
        .status()
        .context("Failed to run cargo install (is cargo installed?)")?;

    if !status.success() {
        bail!("cargo install failed with exit status {}", status);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_simple_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(&manifest_path, "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
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
    fn library_only_package_is_not_binary() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();
        let manifest: toml::Value =
            toml::from_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(!is_binary_package(&manifest, dir.path()));
    }
}
