//! End-to-end tests for the fork log: each fork that `crap` makes adds one
//! JSON line to `forks.jsonl` under the data directory of the user.
//!
//! The tests run the compiled `crap` against a throwaway `HOME` (see
//! `common::crap_command`), so the log lands in a temporary directory and
//! never in the real data directory.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

/// A resolvable original session id, planted in every throwaway `HOME`.
const ORIG: &str = "11111111-2222-3333-4444-555555555555";
/// The sentinel of the `--here` wire format (see `format_here_output`).
const HERE_SENTINEL: &str = "__CRAP_HERE__";

/// A temp directory of its own for one run. It removes itself on drop.
fn unique_root(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("crap-fl-{tag}-"))
        .tempdir()
        .unwrap()
}

/// Where the binary puts the fork log for `home` (see `dirs::data_dir`).
fn fork_log_path(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/crap/forks.jsonl")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".local/share/crap/forks.jsonl")
    }
}

/// Plants a transcript for `id` under `projects_dir/<folder>` that records
/// `cwd` as its working directory.
fn plant(projects_dir: &Path, folder: &str, id: &str, cwd: &str) {
    let dir = projects_dir.join(folder);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{id}.jsonl")),
        format!("{{\"cwd\":\"{cwd}\"}}\n"),
    )
    .unwrap();
}

/// Makes `root/home` with [`ORIG`] planted (recorded cwd `/x/orig`) and
/// `root/work`. Returns `(home, work)`.
fn setup_here(root: &Path) -> (PathBuf, PathBuf) {
    let home = root.join("home");
    plant(
        &home.join(".claude/projects"),
        "orig-project",
        ORIG,
        "/x/orig",
    );
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    (home, work)
}

/// Runs `crap --here ORIG` from `work` with `HOME` set to `home`.
fn run_here(home: &Path, work: &Path) -> Output {
    common::crap_command(home)
        .current_dir(work)
        .args(["--here", ORIG])
        .output()
        .expect("crap binary should run")
}

/// Returns the lines of the fork log, each parsed as JSON.
fn log_lines(log: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(log)
        .unwrap_or_else(|err| panic!("fork log {} unreadable: {err}", log.display()))
        .lines()
        .map(|line| serde_json::from_str(line).expect("each log line is JSON"))
        .collect()
}

/// Returns the third line of the wire output: the fork id.
fn wire_fork_id(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(2)
        .expect("the wire output has a third line")
        .to_string()
}

#[test]
fn here_adds_one_line_with_every_field() {
    let tmp = unique_root("here");
    let (home, work) = setup_here(tmp.path());

    let output = run_here(&home, &work);
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with(HERE_SENTINEL),
        "{output:?}"
    );

    let lines = log_lines(&fork_log_path(&home));
    assert_eq!(lines.len(), 1, "exactly one line: {lines:?}");
    let line = &lines[0];
    assert_eq!(line.as_object().unwrap().len(), 7, "seven keys: {line}");
    assert_eq!(line["source_id"], ORIG);
    assert_eq!(line["fork_id"], wire_fork_id(&output));
    assert!(common::is_generated_fork_id(
        line["fork_id"].as_str().unwrap()
    ));
    assert_eq!(line["source_user"], "home");
    assert_eq!(line["source_dir"], "/x/orig");
    assert_eq!(
        line["fork_dir"],
        fs::canonicalize(&work).unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(line["kind"], "here");
    let time = line["time"].as_str().unwrap();
    let bytes = time.as_bytes();
    assert!(
        time.len() == 20
            && time.ends_with('Z')
            && bytes[4] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':',
        "time is RFC 3339 in UTC with seconds: {time}"
    );
}

#[test]
fn an_unwritable_log_warns_and_the_fork_starts() {
    let tmp = unique_root("unwritable");
    let (home, work) = setup_here(tmp.path());
    // The parent of the log is a regular file, so the directory cannot exist.
    let log = fork_log_path(&home);
    let crap_dir = log.parent().unwrap();
    fs::create_dir_all(crap_dir.parent().unwrap()).unwrap();
    fs::write(crap_dir, "not a directory").unwrap();

    let output = run_here(&home, &work);

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with(HERE_SENTINEL),
        "{output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not add the fork to the fork log"),
        "{stderr}"
    );
}

#[test]
fn cross_user_fork_adds_a_fork_at_line() {
    let tmp = unique_root("fork-at");
    let root = tmp.path();
    let shared = root.join("shared-cwd");
    fs::create_dir_all(&shared).unwrap();
    let shared_text = shared.to_string_lossy().into_owned();
    plant(
        &root.join("other/.claude/projects"),
        "-proj",
        ORIG,
        &shared_text,
    );
    let home = root.join("home");
    fs::create_dir_all(home.join(".claude/projects")).unwrap();
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();

    let output = common::crap_command(&home)
        .current_dir(&work)
        .args([ORIG, "--user", "other"])
        .output()
        .expect("crap binary should run");
    assert!(output.status.success(), "{output:?}");

    let lines = log_lines(&fork_log_path(&home));
    assert_eq!(lines.len(), 1, "exactly one line: {lines:?}");
    let line = &lines[0];
    assert_eq!(line["kind"], "fork-at");
    assert_eq!(line["source_id"], ORIG);
    assert_eq!(line["source_user"], "other");
    assert_eq!(line["source_dir"], shared_text.as_str());
    assert_eq!(line["fork_dir"], shared_text.as_str());
    assert_eq!(line["fork_id"], wire_fork_id(&output));
    assert!(
        !fork_log_path(&root.join("other")).exists(),
        "nothing writes under the other user"
    );
}

#[test]
fn two_forks_at_the_same_time_give_two_complete_lines() {
    const CHILDREN: usize = 8;
    let tmp = unique_root("parallel");
    let (home, work) = setup_here(tmp.path());

    let children: Vec<_> = (0..CHILDREN)
        .map(|_| {
            common::crap_command(&home)
                .current_dir(&work)
                .args(["--here", ORIG])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("crap binary should start")
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect();
    for output in &outputs {
        assert!(output.status.success(), "{output:?}");
    }

    let lines = log_lines(&fork_log_path(&home));
    assert_eq!(lines.len(), CHILDREN, "{lines:?}");
    let logged: std::collections::BTreeSet<String> = lines
        .iter()
        .map(|line| line["fork_id"].as_str().unwrap().to_string())
        .collect();
    let printed: std::collections::BTreeSet<String> = outputs.iter().map(wire_fork_id).collect();
    assert_eq!(logged.len(), CHILDREN, "every fork id is distinct");
    assert_eq!(logged, printed);
}

/// The path of the user's `ch` alias, end to end: the real shell function runs
/// `crap --here`, a fake `claude` takes the fork, and the log holds the line.
#[cfg(unix)]
#[test]
fn ch_through_the_shell_function_logs_the_fork() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    const BASE_PATH: &str = "/usr/bin:/bin";
    let tmp = unique_root("shell");
    let root = tmp.path();
    let (home, work) = setup_here(root);

    // 1. Install the real shell function into the temp home.
    let setup = common::crap_command(&home)
        .env_clear()
        .env("HOME", &home)
        .env("SHELL", "/bin/bash")
        .env("PATH", BASE_PATH)
        .arg("--shell-setup")
        .output()
        .expect("crap --shell-setup should run");
    assert!(setup.status.success(), "{setup:?}");
    assert!(home.join(".bashrc").is_file(), "{setup:?}");

    // 2. A bin directory with `crap` and a fake `claude` that records argv.
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    symlink(env!("CARGO_BIN_EXE_crap"), bin.join("crap")).unwrap();
    let args_file = root.join("claude_args");
    let claude = bin.join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done > {}\n",
            shellquote::shell_quote(&args_file.to_string_lossy())
        ),
    )
    .unwrap();
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();

    // 3. Run the alias path from `work`.
    let output = std::process::Command::new("bash")
        .env_clear()
        .env("HOME", &home)
        .env("WORK", &work)
        .env("PATH", format!("{}:{BASE_PATH}", bin.display()))
        .args([
            "-c",
            "source \"$HOME/.bashrc\" && cd \"$WORK\" && crap --here 11111111-2222-3333-4444-555555555555",
        ])
        .output()
        .expect("bash should run");

    // 4. The fork ran, and the log names its id.
    assert!(output.status.success(), "{output:?}");
    let args = fs::read_to_string(&args_file).expect("the fake claude ran");
    let args: Vec<&str> = args.lines().collect();
    let at = args
        .iter()
        .position(|arg| *arg == "--fork-session")
        .unwrap_or_else(|| panic!("--fork-session missing: {args:?}"));
    assert_eq!(args[at + 1], "--session-id", "{args:?}");
    let fork_id = args[at + 2];
    let lines = log_lines(&fork_log_path(&home));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "here");
    assert_eq!(lines[0]["fork_id"], fork_id);
}
