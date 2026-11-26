//! The fixtures that more than one test file of `wdid` uses: a git repository
//! that holds a cargo workspace, and a cargo home with the records of the
//! installs and the checkouts of cargo.

#![allow(
    dead_code,
    reason = "each test binary uses a different part of this module"
)]

use std::path::{Path, PathBuf};

use gitscratch::testing::TestRepo;
use wdid::catalog::InstallLocation;

/// The binary whose source is the workspace of step 3.
pub const BINARY: &str = "wdid";

/// The URL of the repository in the `git+` records of the fixtures.
pub const REPOSITORY_URL: &str = "https://github.com/timmattison/tools";

/// The checkout directory of the repository. cargo adds a hash of the URL to
/// the name of the repository. The hash is internal to cargo, so the fixtures
/// use a hash that cargo did not calculate.
pub const CHECKOUT_DIRECTORY: &str = "tools-0123456789abcdef";

/// The file in the install root that holds the records of the installs.
pub const RECORDS_FILE: &str = ".crates2.json";

/// The crates of the fixture workspace: the crate directory and the name of
/// the binary.
pub const CRATES: [(&str, &str); 2] = [("one", "wdid-fixture-one"), ("two", "wdid-fixture-two")];

/// A commit that no fixture repository holds.
pub const OTHER_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// A cargo home in which the installed [`BINARY`] came from a checkout of
/// [`workspace_repository`], through `cargo install --git`. The records file
/// also holds a `git+` record of another binary at [`OTHER_COMMIT`] and a
/// record from the registry.
pub struct Installed {
    /// The cargo home.
    pub home: CargoHome,
    /// The checkout of the commit of the record.
    pub checkout: PathBuf,
    /// The full commit of the record.
    pub commit: String,
}

impl Installed {
    /// Makes the cargo home and the checkout.
    pub fn new() -> Self {
        let source = workspace_repository();
        let commit = source.rev_parse("HEAD");
        let home = CargoHome::new();
        let checkout = home.add_checkout(&source, CHECKOUT_DIRECTORY, short(&commit));
        home.write_records(&[
            (&git_key("gsw", REPOSITORY_URL, OTHER_COMMIT), &["gsw"]),
            (&git_key(BINARY, REPOSITORY_URL, &commit), &[BINARY]),
            (
                "ripgrep 14.1.1 (registry+https://github.com/rust-lang/crates.io-index)",
                &["rg"],
            ),
        ]);
        Self {
            home,
            checkout,
            commit,
        }
    }
}

/// The number of characters of a commit in the name of a checkout directory.
pub const SHORT_COMMIT_LENGTH: usize = 7;

/// Gives a git repository whose one commit holds a cargo workspace with the
/// crates of [`CRATES`].
pub fn workspace_repository() -> TestRepo {
    let repository = TestRepo::init();
    let members: Vec<String> = CRATES.iter().map(|(dir, _)| format!("\"{dir}\"")).collect();
    let mut files: Vec<(String, String)> = vec![(
        "Cargo.toml".to_string(),
        format!(
            "[workspace]\nresolver = \"2\"\nmembers = [{}]\n",
            members.join(", ")
        ),
    )];
    for (dir, name) in CRATES {
        std::fs::create_dir_all(repository.path().join(dir).join("src")).unwrap();
        files.push((
            format!("{dir}/Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ));
        files.push((format!("{dir}/src/main.rs"), "fn main() {}\n".to_string()));
    }
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, contents)| (name.as_str(), contents.as_str()))
        .collect();
    repository.commit_files(&borrowed, "add the workspace");
    repository
}

/// Gives the first [`SHORT_COMMIT_LENGTH`] characters of `commit`.
pub fn short(commit: &str) -> &str {
    commit.get(..SHORT_COMMIT_LENGTH).unwrap()
}

/// Gives the key of a `git+` record of `package` at `commit` of `url`.
pub fn git_key(package: &str, url: &str, commit: &str) -> String {
    format!("{package} 0.1.0 (git+{url}#{commit})")
}

/// Writes the records file into `install_root`, with one record for each pair
/// of a key and the names of its binaries. Each record has the fields that
/// cargo writes.
pub fn write_records(install_root: &Path, records: &[(&str, &[&str])]) {
    let installs: serde_json::Map<String, serde_json::Value> = records
        .iter()
        .map(|(key, bins)| {
            (
                (*key).to_string(),
                serde_json::json!({
                    "version_req": null,
                    "bins": bins,
                    "features": [],
                    "all_features": false,
                    "no_default_features": false,
                    "profile": "release",
                    "target": "aarch64-apple-darwin",
                    "rustc": "rustc 1.100.0 (0ef1bc386 2026-11-09)\nbinary: rustc\n",
                }),
            )
        })
        .collect();
    let document = serde_json::json!({ "installs": installs });
    std::fs::create_dir_all(install_root).unwrap();
    std::fs::write(
        install_root.join(RECORDS_FILE),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
}

/// A cargo home in a temporary directory.
pub struct CargoHome {
    dir: tempfile::TempDir,
}

impl CargoHome {
    /// Makes an empty cargo home.
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// The directory of the cargo home.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The location of the installs with this cargo home and no install root.
    pub fn location(&self) -> InstallLocation<'_> {
        InstallLocation {
            cargo_home: Some(self.dir.path().as_os_str()),
            ..InstallLocation::default()
        }
    }

    /// Writes the records file into this cargo home. See [`write_records`].
    pub fn write_records(&self, records: &[(&str, &[&str])]) {
        write_records(self.dir.path(), records);
    }

    /// Clones `repository` into `git/checkouts/<directory>/<name>`, as cargo
    /// does for a `cargo install --git`, and gives the path of the clone.
    pub fn add_checkout(&self, repository: &TestRepo, directory: &str, name: &str) -> PathBuf {
        let checkout = self
            .dir
            .path()
            .join("git")
            .join("checkouts")
            .join(directory)
            .join(name);
        std::fs::create_dir_all(checkout.parent().unwrap()).unwrap();
        let output = childenv::fixture_command("git")
            .arg("clone")
            .arg("-q")
            .arg(repository.path())
            .arg(&checkout)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        checkout
    }
}
