//! Pins that no `nwt` run of this suite reads configuration from the home
//! directory of the person who runs the suite.
//!
//! `nwt` loads `~/.nwt.toml` through `dirs::home_dir`, which reads `HOME`. The
//! git children of `nwt` read `~/.gitconfig`, and `$XDG_CONFIG_HOME/git/config`
//! when that variable is set. A host `~/.nwt.toml` that sets `quiet = true`
//! removes each stderr line that a test reads. A host file that sets `checkout`
//! sends each run through `-c`. So a test that inherits the home of the host
//! gives an answer that depends on the machine, and a green run on one machine
//! proves nothing about the next machine.
//!
//! `support::nwt_command` is the one entrance through which every test starts
//! `nwt`, so the rule is there. This file holds the rule in two ways: the
//! command that `nwt_command` builds, and a run of the real binary under a
//! hostile home.
//!
//! The hostile run needs a hostile `HOME` in the process that calls
//! `nwt_command`. A change to the environment of this process changes it for
//! each sibling test thread too. So the test starts this test binary again as a
//! child process, with the hostile home on the child only, and names an ignored
//! helper test.

mod support;

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gitscratch::shed_inherited_git_environment;
use support::{init_repo, nanos, nwt_command, run_git};
use tempfile::TempDir;

/// The variable that names the home directory, for `nwt` and for git.
const HOME: &str = "HOME";

/// The variable that moves the global git configuration out of the home
/// directory.
const XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";

/// The configuration file of `nwt`, relative to the home directory.
const NWT_CONFIG: &str = ".nwt.toml";

/// The global git configuration file, relative to the home directory.
const GIT_CONFIG: &str = ".gitconfig";

/// The global git configuration file that git reads when `XDG_CONFIG_HOME` is
/// not set, relative to the home directory.
const XDG_GIT_CONFIG: &str = ".config/git/config";

/// The directory, relative to the hostile home, that the hostile run names in
/// `XDG_CONFIG_HOME`.
const HOSTILE_XDG_DIR: &str = "xdg";

/// The git configuration file under a directory that `XDG_CONFIG_HOME` names.
const GIT_CONFIG_UNDER_XDG: &str = "git/config";

/// Text that `nwt` cannot read as TOML.
///
/// A run that reads a file with this text stops with exit 12 before it makes
/// anything. So each read of the file changes the answer, whatever key the
/// read looks for.
const BROKEN_NWT_CONFIG: &str = "[a host nwt configuration reached the run\n";

/// Text that git cannot read as configuration.
///
/// Git stops with `fatal: bad config line 1` on each command that reads a
/// global file with this text. So each git child that reads it fails, whatever
/// key the child looks for.
const BROKEN_GIT_CONFIG: &str = "[a host git configuration reached the run\n";

/// The name of the helper test that the hostile run starts.
const HELPER_TEST_NAME: &str = "private_home_helper_runs_nwt_under_the_hostile_home";

/// The variable that carries the repository to the helper. The helper does
/// nothing when the variable is absent.
const HELPER_REPO_VARIABLE: &str = "NWT_PRIVATE_HOME_HELPER_REPO";

/// The variable that carries the branch name to the helper.
const HELPER_BRANCH_VARIABLE: &str = "NWT_PRIVATE_HOME_HELPER_BRANCH";

/// The value that `command` states for `name`.
///
/// `None` means that `command` does not mention `name`, so the child inherits
/// the value of this process. `Some(None)` means that `command` removes `name`.
/// `Some(Some(value))` means that `command` sets `name` to `value`.
fn stated(command: &Command, name: &str) -> Option<Option<OsString>> {
    command
        .get_envs()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.map(OsString::from))
}

/// The home directory that `nwt_command` gives its child.
///
/// # Panics
///
/// Panics when the command does not set `HOME`, or sets it to an empty value.
/// An empty `HOME` is no private home: `dirs::home_dir` then falls back to the
/// home directory in the password database, which is the home of the host.
fn home_of(command: &Command) -> PathBuf {
    let home = match stated(command, HOME) {
        Some(Some(home)) => home,
        Some(None) => panic!("nwt_command removes {HOME}, so nwt falls back to the host home"),
        None => panic!("nwt_command does not set {HOME}, so nwt reads the home of the host"),
    };
    assert!(
        !home.is_empty(),
        "nwt_command sets an empty {HOME}, so nwt falls back to the host home"
    );
    PathBuf::from(home)
}

/// Write `contents` to `relative` under `dir`, and make the parent directories
/// first.
fn write_under(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    let parent = path.parent().expect("a file path has a parent");
    fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    fs::write(&path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

#[test]
fn nwt_command_gives_the_child_a_private_home() {
    let dir = TempDir::new().expect("create a temporary directory");
    let home = home_of(&nwt_command(dir.path()));

    assert_ne!(
        Some(home.as_os_str()),
        env::var_os(HOME).as_deref(),
        "the child must not get the home of the host"
    );
    assert!(
        home.is_dir(),
        "the private home {} must be a directory",
        home.display()
    );
    for config in [NWT_CONFIG, GIT_CONFIG, XDG_GIT_CONFIG] {
        assert!(
            !home.join(config).exists(),
            "the private home must hold no {config}, but {} holds one",
            home.display()
        );
    }
}

#[test]
fn the_private_home_is_private_to_this_process_and_lies_in_the_target_directory() {
    let dir = TempDir::new().expect("create a temporary directory");
    let home = home_of(&nwt_command(dir.path()));

    // A second copy of this test binary runs with another process id. The id
    // in the name thus keeps two concurrent runs out of each other's home.
    let name = home
        .file_name()
        .expect("the private home has a name")
        .to_string_lossy()
        .into_owned();
    assert!(
        name.contains(&std::process::id().to_string()),
        "the name {name} of the private home must carry the process id {}",
        std::process::id()
    );

    // `cargo clean` removes the target directory, so a home there does not
    // stay behind in the temporary directory of the system.
    let target_tmp = fs::canonicalize(env!("CARGO_TARGET_TMPDIR"))
        .expect("resolve the temporary directory of cargo");
    let resolved =
        fs::canonicalize(&home).unwrap_or_else(|e| panic!("resolve {}: {e}", home.display()));
    assert!(
        resolved.starts_with(&target_tmp),
        "the private home {} must lie under {}",
        resolved.display(),
        target_tmp.display()
    );
}

#[test]
fn each_nwt_command_of_one_process_gets_the_same_private_home() {
    let dir = TempDir::new().expect("create a temporary directory");

    // One directory for each process, and not one for each spawn, so a run of
    // the suite leaves few directories behind.
    assert_eq!(
        home_of(&nwt_command(dir.path())),
        home_of(&nwt_command(dir.path())),
        "two calls in one process must give the same private home"
    );
}

#[test]
fn nwt_command_removes_xdg_config_home() {
    let dir = TempDir::new().expect("create a temporary directory");

    // Git reads `$XDG_CONFIG_HOME/git/config` before `~/.gitconfig`. A value
    // that the host sets thus gives the child a global git configuration from
    // outside the private home. With the variable gone, git reads
    // `$HOME/.config/git/config`, which is in the private home.
    assert_eq!(
        stated(&nwt_command(dir.path()), XDG_CONFIG_HOME),
        Some(None),
        "nwt_command must remove {XDG_CONFIG_HOME} from the child"
    );
}

#[test]
fn a_hostile_home_of_the_host_reaches_no_nwt_run() {
    // The repository is made here, under the home of this process, because
    // the git of the fixture reads the global configuration too, and the
    // hostile home breaks each git that reads it.
    let (_repo_temp, repo) = init_repo();
    let branch = format!("private-home-{}-{}", std::process::id(), nanos());

    let hostile = TempDir::new().expect("create the hostile home");
    write_under(hostile.path(), NWT_CONFIG, BROKEN_NWT_CONFIG);
    write_under(hostile.path(), GIT_CONFIG, BROKEN_GIT_CONFIG);
    let hostile_xdg = hostile.path().join(HOSTILE_XDG_DIR);
    write_under(&hostile_xdg, GIT_CONFIG_UNDER_XDG, BROKEN_GIT_CONFIG);

    // The helper runs git through `nwt`, so it sheds the inherited git
    // environment like each other git-running child of this suite. The
    // hostile values go on the child only.
    let mut helper = Command::new(env::current_exe().expect("the path of this test binary"));
    shed_inherited_git_environment(&mut helper);
    let output = helper
        .args(["--exact", HELPER_TEST_NAME, "--ignored"])
        .env(HOME, hostile.path())
        .env(XDG_CONFIG_HOME, &hostile_xdg)
        .env(HELPER_REPO_VARIABLE, &repo)
        .env(HELPER_BRANCH_VARIABLE, &branch)
        .stdin(Stdio::null())
        .output()
        .expect("start the helper");

    assert!(
        output.status.success(),
        "the nwt run under the hostile home failed, so it read a configuration of the \
         host home.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // A helper that never ran `nwt` also exits 0: libtest runs no test when
    // the name matches none, and the helper returns early when its variable
    // is absent. The branch proves that the run happened.
    assert!(
        run_git(
            &repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}")
            ]
        ),
        "the helper made no branch {branch}, so it never ran nwt.\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

/// The `nwt` run that the hostile-home test starts in a child process.
///
/// It runs `nwt -b <branch>` in the repository that [`HELPER_REPO_VARIABLE`]
/// names, and demands that the run works.
#[test]
#[ignore = "the hostile-home test starts this helper in a child process"]
fn private_home_helper_runs_nwt_under_the_hostile_home() {
    let Some(repo) = env::var_os(HELPER_REPO_VARIABLE) else {
        return;
    };
    let branch = env::var(HELPER_BRANCH_VARIABLE).expect("the branch of the helper");

    // The run proves nothing unless the hostile home reached this process.
    let home = PathBuf::from(env::var_os(HOME).expect("the helper has a home"));
    assert_eq!(
        fs::read_to_string(home.join(NWT_CONFIG)).ok().as_deref(),
        Some(BROKEN_NWT_CONFIG),
        "the helper must run under the hostile home, but {HOME} is {}",
        home.display()
    );

    let output = nwt_command(Path::new(&repo))
        .args(["-b", &branch, "--no-copy-env", "--no-bootstrap-hooks"])
        .output()
        .expect("run the nwt binary");

    assert!(
        output.status.success(),
        "nwt failed ({:?}) under the hostile home:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
