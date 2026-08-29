//! Black-box tests for the `dirhash` binary, driving the real CLI end to end.
//!
//! Each test builds its own temporary tree, so concurrent test runs stay
//! isolated (see the parallel-safety note in the project guidelines).
//!
//! Every fixture root holds an empty `.git` directory. The `ignore` crate
//! applies `.gitignore` rules only inside a git repository, and the directory
//! is what marks one. The test makes the directory itself rather than call
//! `git init`, so no test can reach a real repository.

use std::path::PathBuf;
use std::process::Command;

/// A temporary tree to hash, plus an empty `HOME` for the child process.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("make a temporary directory");
        std::fs::create_dir_all(dir.path().join("root/.git")).expect("make the fixture root");
        std::fs::create_dir_all(dir.path().join("home")).expect("make the fixture home");
        Self { dir }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// Write one file below the fixture root, making its parents as necessary.
    fn write(&self, relative: &str, contents: &str) -> &Self {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("make the parent directory");
        }
        std::fs::write(&path, contents).expect("write the fixture file");
        self
    }

    /// Run the freshly built binary on the fixture root, named by its absolute
    /// path.
    fn run(&self, args: &[&str]) -> Run {
        self.run_from(self.root(), self.root(), args)
    }

    /// Run the freshly built binary from `working_dir`, on the directory named
    /// by `directory`, and require that it exits well.
    ///
    /// This is the one spawn in the file, so no test can reach the binary
    /// without the scrub below. The child gets a scrubbed environment: a stray
    /// `GIT_*` variable or the real global gitignore changes which files the
    /// walker sees, and a test that reads the machine it runs on is a test that
    /// passes for a reason nobody wrote down.
    fn run_from(&self, working_dir: PathBuf, directory: PathBuf, args: &[&str]) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dirhash"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(&key);
            }
        }
        command
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home())
            .current_dir(working_dir);

        let output = command
            .args(args)
            .arg(&directory)
            .output()
            .expect("spawn the dirhash binary");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "dirhash {args:?} {} must succeed; stderr: {stderr}",
            directory.display()
        );
        Run {
            hash: stdout.trim().to_string(),
            note: stderr,
        }
    }
}

/// What one run of the binary said.
struct Run {
    hash: String,
    note: String,
}

impl Run {
    fn assert_note(&self, expected: &str) {
        assert_eq!(self.note.trim(), expected, "the note on stderr");
    }

    fn assert_no_note(&self) {
        assert!(
            self.note.trim().is_empty(),
            "nothing is excluded, so stderr must stay empty; it said: {}",
            self.note
        );
    }
}

#[test]
fn hidden_files_are_counted_as_excluded() {
    let fixture = Fixture::new();
    fixture.write("visible.txt", "a").write(".hidden.txt", "b");

    fixture
        .run(&[])
        .assert_note("Note: 1 hidden file(s) excluded. Use --hidden to include them.");
}

#[test]
fn files_inside_hidden_directories_are_counted_as_excluded() {
    let fixture = Fixture::new();
    fixture
        .write("visible.txt", "a")
        .write(".cache/one.txt", "b")
        .write(".cache/two.txt", "c");

    fixture
        .run(&[])
        .assert_note("Note: 2 hidden file(s) excluded. Use --hidden to include them.");
}

#[test]
fn the_note_separates_the_hidden_count_from_the_ignored_count() {
    let fixture = Fixture::new();
    fixture
        .write(".gitignore", "secret.txt\n")
        .write("visible.txt", "a")
        .write("secret.txt", "b")
        .write(".hidden.txt", "c")
        .write(".cache/one.txt", "d");

    // The unfiltered tree holds 5 files. The default walk hashes visible.txt
    // alone. `.gitignore` removes secret.txt, and the hidden filter removes
    // `.gitignore`, `.hidden.txt`, and `.cache/one.txt`.
    fixture.run(&[]).assert_note(
        "Note: 4 file(s) excluded: 3 hidden, 1 ignored. Use --hidden and --no-ignore to include them.",
    );
}

#[test]
fn a_file_that_is_both_hidden_and_ignored_is_counted_once() {
    let fixture = Fixture::new();
    fixture
        .write(".gitignore", ".secret\n.cache/\n")
        .write("visible.txt", "a")
        .write(".secret", "b")
        .write(".cache/one", "c")
        .write(".cache/two", "d");

    // Every excluded file here is hidden, and all but `.gitignore` are named by
    // an ignore file too, so the two groups can only add up if the subtraction
    // runs in the right order. The unfiltered tree holds 5 files. The default
    // walk hashes visible.txt alone. Turning off the hidden filter alone adds
    // back `.gitignore` and nothing else, because `.gitignore` names `.secret`
    // and `.cache/`; that difference of 1 is the hidden count. The remaining 3 —
    // `.secret`, `.cache/one`, `.cache/two` — are the ignored count. Measuring
    // the ignored group against the hashed walk instead would charge those 3 to
    // both groups and report 5 excluded out of the 5 files on disk.
    fixture.run(&[]).assert_note(
        "Note: 4 file(s) excluded: 1 hidden, 3 ignored. Use --hidden and --no-ignore to include them.",
    );
}

#[test]
fn no_ignore_includes_files_ignored_by_gitignore() {
    let fixture = Fixture::new();
    fixture
        .write(".gitignore", "secret.txt\n")
        .write("visible.txt", "a")
        .write("secret.txt", "b");

    let default = fixture.run(&[]);
    let no_ignore = fixture.run(&["--no-ignore"]);

    assert_ne!(
        default.hash, no_ignore.hash,
        "--no-ignore must pull the gitignored file into the hash"
    );
}

#[test]
fn no_ignore_with_hidden_excludes_nothing() {
    let fixture = Fixture::new();
    fixture
        .write(".gitignore", "secret.txt\n")
        .write(".ignore", "other.txt\n")
        .write("visible.txt", "a")
        .write("secret.txt", "b")
        .write("other.txt", "c")
        .write(".hidden.txt", "d");

    fixture.run(&["--hidden", "--no-ignore"]).assert_no_note();
}

#[test]
fn no_ignore_implies_no_ignore_vcs() {
    let fixture = Fixture::new();
    fixture
        .write(".gitignore", "secret.txt\n")
        .write("visible.txt", "a")
        .write("secret.txt", "b");

    let no_ignore = fixture.run(&["--no-ignore"]);
    let both = fixture.run(&["--no-ignore", "--no-ignore-vcs"]);

    assert_eq!(
        no_ignore.hash, both.hash,
        "--no-ignore already turns off the VCS ignore files"
    );
}

#[test]
fn no_ignore_vcs_keeps_dot_ignore_but_no_ignore_drops_it() {
    let fixture = Fixture::new();
    fixture
        .write(".ignore", "skipped.txt\n")
        .write("visible.txt", "a")
        .write("skipped.txt", "b");

    fixture
        .run(&["--hidden", "--no-ignore-vcs"])
        .assert_note("Note: 1 ignored file(s) excluded. Use --no-ignore to include them.");
    fixture.run(&["--hidden", "--no-ignore"]).assert_no_note();
}

#[test]
fn the_hash_reaches_stdout_alone() {
    let fixture = Fixture::new();
    fixture.write("visible.txt", "a").write(".hidden.txt", "b");

    let run = fixture.run(&[]);
    assert_eq!(run.hash.len(), 64, "SHA-256 prints 64 hex characters");
    assert!(
        run.hash.chars().all(|c| c.is_ascii_hexdigit()),
        "stdout must carry the hash alone; it carried: {}",
        run.hash
    );
    assert!(
        run.note.contains("Note:"),
        "the note belongs on stderr; it said: {}",
        run.note
    );
}

#[test]
fn an_empty_directory_hashes_without_a_note() {
    let fixture = Fixture::new();
    fixture.run(&["--hidden", "--no-ignore"]).assert_no_note();
    assert_eq!(fixture.run(&[]).hash.len(), 64);
}

#[test]
fn the_walk_reaches_a_directory_given_by_a_relative_path() {
    let fixture = Fixture::new();
    fixture.write("visible.txt", "a").write(".hidden.txt", "b");

    let absolute = fixture.run(&[]).hash;
    let relative = fixture
        .run_from(fixture.root(), PathBuf::from("."), &[])
        .hash;

    assert_eq!(
        absolute, relative,
        "the path spelling must not move the hash"
    );
}
