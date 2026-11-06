#![allow(
    dead_code,
    reason = "each test binary uses a different part of the shared fixture"
)]
//! The shared fixture of the `sidecar` tests.
//!
//! A fixture makes one directory under the temporary directory. The directory holds a
//! host repository, its bare remote, one bare remote for each sidecar, a fake `gh`,
//! and a `.gitconfig`. Every remote is a local path, thus no test touches the network.
//! The name of the directory holds the process number, the time, and a counter, thus
//! two runs at the same time never share a fixture.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The owner in the path of each remote.
pub const OWNER: &str = "fixture-owner";

/// The name of the host repository.
pub const HOST: &str = "megarepo";

/// The variables that the fixture removes from each run of the binary.
const SCRUBBED_VARIABLES: [&str; 6] = [
    "CI",
    "SIDECAR_TOKEN",
    "CARGO_TERM_COLOR",
    "FORCE_COLOR",
    "CLICOLOR_FORCE",
    "NO_COLOR",
];

/// The number of fixtures that this process made.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The fake `gh`. The marker `@ROOT@` stands for the quoted fixture root.
const FAKE_GH: &str = r#"#!/bin/sh
# The fake gh of the sidecar tests. It records each call and answers from the fixture.
root=@ROOT@
printf '%s\n' "$*" >> "$root/gh-calls.log"
method=GET
path=
name=
while [ "$#" -gt 0 ]; do
  case "$1" in
    api) ;;
    -X) shift; method=$1 ;;
    -f|-F) shift; case "$1" in name=*) name=${1#name=} ;; esac ;;
    --jq) shift ;;
    *) if [ -z "$path" ]; then path=$1; fi ;;
  esac
  shift
done
remotes="$root/remotes"
case "$method $path" in
  "GET user")
    cat "$root/gh-user"; exit 0 ;;
  "GET repos/"*)
    repo=${path#repos/}
    if [ -d "$remotes/$repo.git" ]; then
      if [ -f "$remotes/$repo.visibility" ]; then cat "$remotes/$repo.visibility"; else echo private; fi
      exit 0
    fi
    echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
  "POST user/repos")
    owner=$(cat "$root/gh-user")
    git init --quiet --bare --initial-branch=main "$remotes/$owner/$name.git" || exit 1
    echo "$owner/$name"; exit 0 ;;
  "POST orgs/"*"/repos")
    owner=${path#orgs/}; owner=${owner%/repos}
    git init --quiet --bare --initial-branch=main "$remotes/$owner/$name.git" || exit 1
    echo "$owner/$name"; exit 0 ;;
esac
echo "fake gh: unexpected call: $*" >&2
exit 2
"#;

/// The result of one run of the `sidecar` binary.
#[derive(Debug)]
pub struct Run {
    /// The exit code. It is -1 when a signal ended the process.
    pub code: i32,
    /// The standard output.
    pub stdout: String,
    /// The standard error.
    pub stderr: String,
}

/// One fixture. `Drop` removes its directory.
pub struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Makes a host whose committed `sidecar.toml` names `names` and whose committed
    /// `.gitignore` is `/data/`, a main checkout at `code/megarepo`, and a seeded bare
    /// sidecar remote for each name.
    pub fn new(names: &[&str]) -> Fixture {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "sidecar-test-{}-{nanos}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("make the fixture root");
        let root = fs::canonicalize(&root).expect("canonicalize the fixture root");
        let fixture = Fixture { root };
        fixture.write(
            &fixture.gitconfig(),
            "[user]\n\tname = Fixture User\n\temail = fixture@example.com\n\
             [init]\n\tdefaultBranch = main\n\
             [commit]\n\tgpgsign = false\n\
             [core]\n\tautocrlf = input\n",
        );
        fixture.write(&fixture.root.join("gh-user"), OWNER);
        fixture.write_fake_gh();
        fixture.make_host_remote(names);
        fs::create_dir_all(fixture.root.join("code")).expect("make the code directory");
        fixture.git(
            &fixture.root.join("code"),
            &[
                "clone",
                "--quiet",
                &fixture.remote(HOST).to_string_lossy(),
                &fixture.main_checkout().to_string_lossy(),
            ],
        );
        for name in names {
            fixture.make_sidecar_remote(name);
        }
        fixture
    }

    /// The root of the fixture.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.gitconfig` that every git command of the fixture reads.
    pub fn gitconfig(&self) -> PathBuf {
        self.root.join("home").join(".gitconfig")
    }

    /// The main checkout of the host.
    pub fn main_checkout(&self) -> PathBuf {
        self.root.join("code").join(HOST)
    }

    /// Makes a worktree of the host for a new branch and returns its path.
    pub fn add_host_worktree(&self, branch: &str) -> PathBuf {
        let path = self
            .root
            .join("code")
            .join(format!("{HOST}-worktrees"))
            .join(branch);
        self.git(
            &self.main_checkout(),
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                &path.to_string_lossy(),
            ],
        );
        path
    }

    /// The bare remote of the repository `repository`.
    pub fn remote(&self, repository: &str) -> PathBuf {
        self.root
            .join("remotes")
            .join(OWNER)
            .join(format!("{repository}.git"))
    }

    /// The bare remote of the sidecar `name`.
    pub fn sidecar_remote(&self, name: &str) -> PathBuf {
        self.remote(&format!("{HOST}-data-{name}"))
    }

    /// Makes a bare sidecar remote with one commit on `main`. The commit holds
    /// `README.md` and `data.txt`.
    pub fn make_sidecar_remote(&self, name: &str) {
        let seed = self.root.join(format!("seed-{name}"));
        fs::create_dir_all(&seed).expect("make the seed directory");
        self.git(&seed, &["init", "--quiet", "-b", "main"]);
        self.write(&seed.join("README.md"), &format!("data of {name}\n"));
        self.write(&seed.join("data.txt"), "1\n");
        self.git(&seed, &["add", "README.md", "data.txt"]);
        self.git(&seed, &["commit", "--quiet", "-m", "seed"]);
        let remote = self.sidecar_remote(name);
        fs::create_dir_all(remote.parent().expect("a remote has a parent"))
            .expect("make the remote directory");
        self.git(
            &self.root,
            &[
                "clone",
                "--quiet",
                "--bare",
                &seed.to_string_lossy(),
                &remote.to_string_lossy(),
            ],
        );
    }

    /// Makes a bare sidecar remote with no commit.
    pub fn make_empty_sidecar_remote(&self, name: &str) {
        let remote = self.sidecar_remote(name);
        fs::create_dir_all(remote.parent().expect("a remote has a parent"))
            .expect("make the remote directory");
        self.git(
            &self.root,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                &remote.to_string_lossy(),
            ],
        );
    }

    /// The place of the main clone of the sidecar `name`.
    pub fn main_clone(&self, name: &str) -> PathBuf {
        self.root.join("code").join(format!("{HOST}-data-{name}"))
    }

    /// The directory of the worktrees of the sidecar `name`.
    pub fn worktrees_dir(&self, name: &str) -> PathBuf {
        self.root
            .join("code")
            .join(format!("{HOST}-data-{name}-worktrees"))
    }

    /// The place of the link of the sidecar `name` in `worktree`.
    pub fn link(&self, worktree: &Path, name: &str) -> PathBuf {
        worktree.join("data").join(name)
    }

    /// Writes `text` to `path` and makes the parent directories.
    pub fn write(&self, path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("a path has a parent"))
            .expect("make the parent directories");
        fs::write(path, text).expect("write the file");
    }

    /// Runs git in `cwd`, asserts success, and returns the trimmed standard output.
    pub fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.try_git(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?} in {} failed: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Runs git in `cwd` and returns the whole output.
    pub fn try_git(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        gitscratch::shed_inherited_git_environment(&mut command);
        command
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.gitconfig())
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("HOME", self.root.join("home"))
            .stdin(Stdio::null());
        command.output().expect("git starts")
    }

    /// Makes the command that runs the `sidecar` binary in `cwd`, with a prepared
    /// environment. The fake `gh` comes first in `PATH`.
    pub fn sidecar(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sidecar"));
        gitscratch::shed_inherited_git_environment(&mut command);
        for variable in SCRUBBED_VARIABLES {
            command.env_remove(variable);
        }
        let mut paths = vec![self.root.join("bin")];
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        command
            .current_dir(cwd)
            .args(args)
            .env("PATH", std::env::join_paths(paths).expect("join PATH"))
            .env("HOME", self.root.join("home"))
            .env("GIT_CONFIG_GLOBAL", self.gitconfig())
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdin(Stdio::null());
        command
    }

    /// Runs the binary in `cwd`.
    pub fn run(&self, cwd: &Path, args: &[&str]) -> Run {
        self.run_with_env(cwd, args, &[])
    }

    /// Runs the binary in `cwd` with more variables, set after the preparation.
    pub fn run_with_env(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut command = self.sidecar(cwd, args);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().expect("the binary starts");
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// The lines of `gh-calls.log`. The list is empty when the fake `gh` never ran.
    pub fn gh_calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("gh-calls.log"))
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Sets the login that the fake `gh` gives for `gh api user`.
    pub fn set_gh_user(&self, login: &str) {
        self.write(&self.root.join("gh-user"), login);
    }

    /// Sets the visibility that the fake `gh` gives for `repository`.
    pub fn set_visibility(&self, repository: &str, visibility: &str) {
        self.write(
            &self
                .root
                .join("remotes")
                .join(OWNER)
                .join(format!("{repository}.visibility")),
            visibility,
        );
    }

    /// Writes the fake `gh` and makes it executable.
    fn write_fake_gh(&self) {
        use std::os::unix::fs::PermissionsExt;

        let script = FAKE_GH.replace(
            "@ROOT@",
            &shellquote::shell_quote(&self.root.to_string_lossy()),
        );
        let path = self.root.join("bin").join("gh");
        self.write(&path, &script);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make the fake gh executable");
    }

    /// Makes the bare remote of the host from a scratch repository.
    fn make_host_remote(&self, names: &[&str]) {
        let seed = self.root.join("seed-host");
        fs::create_dir_all(&seed).expect("make the seed directory");
        self.git(&seed, &["init", "--quiet", "-b", "main"]);
        let quoted: Vec<String> = names.iter().map(|name| format!("\"{name}\"")).collect();
        self.write(
            &seed.join("sidecar.toml"),
            &format!("sidecars = [{}]\n", quoted.join(", ")),
        );
        self.write(&seed.join(".gitignore"), "/data/\n");
        self.git(&seed, &["add", "sidecar.toml", ".gitignore"]);
        self.git(&seed, &["commit", "--quiet", "-m", "host"]);
        let remote = self.remote(HOST);
        fs::create_dir_all(remote.parent().expect("a remote has a parent"))
            .expect("make the remote directory");
        self.git(
            &self.root,
            &[
                "clone",
                "--quiet",
                "--bare",
                &seed.to_string_lossy(),
                &remote.to_string_lossy(),
            ],
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
