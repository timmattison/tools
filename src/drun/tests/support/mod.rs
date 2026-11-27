//! A fake `docker` program and a config home for the end-to-end tests of
//! `drun`.
//!
//! The fake is first on `PATH`. It reads its answers from files in the
//! fixture directory, and it writes each call to `calls-<host>.log`, so no
//! test reaches a real Docker host. `drun` starts with an empty environment,
//! so no `DOCKER_*`, `DRUN_*`, or `GIT_*` variable of the person who runs the
//! tests can change what it does.

#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use gitscratch::testing::executable::write_executable;

/// The memory of a computer with 16 GiB, in KiB.
pub const GIB_16: u64 = 16_777_216;

/// The text that the fake puts in place of the fixture directory.
const DIRECTORY_MARK: &str = "__FIXTURE_DIRECTORY__";

/// The name under which the fake records a call without `--context`, which
/// goes to the docker of this computer. It is not a Docker context name, so
/// no host of a test can have it.
pub const LOCAL: &str = "@local";

/// The fake `docker`. A call with `--context <host>` goes to that host, and a
/// call without it goes to [`LOCAL`]. It answers from the files that the
/// fixture writes:
///
/// - `hang-<host>`: the host never answers.
/// - `down-<host>`: the host fails every call, with the text of the file.
/// - `probe-<host>`: the answer to the probe container.
/// - `no-image-<host>`: the host does not have the image of the probe. As
///   docker 29 does, a probe with `--pull never` then fails with exit status
///   125, and `docker image inspect` fails with exit status 1. Both write
///   `No such image` to stderr. A probe with any other `--pull` value gets the
///   image first, as a pull does. A pull that succeeds removes the file.
/// - `pull-down-<host>`: each pull on the host fails, with the text of the
///   file. The implicit pull of a probe without `--pull never` fails too.
/// - `containers-<host>`: the rows of `docker ps`.
/// - `inspect-<host>`, `wait-<host>`: the answers to those commands.
/// - `exit-<host>`: the exit status of every other call.
/// - `help-<command>`: the answer to `docker <command> --help`, which has no
///   `--context`. The words of a command of more than one word are joined
///   with `-`, as in `help-buildx-create`.
/// - `help-env-<command>`: written by the fake on each help call. It holds the
///   `DOCKER_` variables of that call, sorted, one for each line.
/// - `stdin-<host>`: written by the fake on `docker login --password-stdin`.
///   It holds what the call read from stdin.
/// - `git-env-<host>`: written by the fake on each call, a help call
///   included. It holds the `GIT_` variables of the last call to that host,
///   sorted, one for each line.
///
/// A call to [`LOCAL`] whose command is a command of `drun`, as `run` or
/// `ps`, stops with exit status 97. `drun` sends those commands to a host,
/// so such a call is a defect of `drun`.
const FAKE_DOCKER: &str = r#"#!/bin/sh
# A fake docker for the end-to-end tests of drun.
dir=__FIXTURE_DIRECTORY__
if [ "$1" = "--context" ]; then
    context=$2
    shift 2
else
    context=@local
fi
# Gets the image of the probe on the host, or fails with the text of
# pull-down-<host>.
pull_image() {
    if [ -f "$dir/pull-down-$context" ]; then
        cat "$dir/pull-down-$context" >&2
        return 1
    fi
    rm -f "$dir/no-image-$context"
}
env | grep '^GIT_' | sort > "$dir/git-env-$context" || true
separator=$(printf '\037')
line=
last=
for argument in "$@"; do
    line="$line$argument$separator"
    last=$argument
done
if [ "$context" = @local ] && [ "$last" = "--help" ]; then
    name=
    for argument in "$@"; do
        if [ "$argument" = "--help" ]; then
            break
        fi
        name="${name:+$name-}$argument"
    done
    env | grep '^DOCKER_' | sort > "$dir/help-env-$name" || true
    if [ -f "$dir/help-$name" ]; then
        cat "$dir/help-$name"
        exit 0
    fi
    echo "fake docker: no help for $name" >&2
    exit 1
fi
printf '%s\n' "$line" >> "$dir/calls-$context.log"
if [ "$context" = @local ]; then
    case "$1" in
    run|ps|logs|wait|exec|cp|rm)
        echo "fake docker: drun gave its own command to the docker of this computer: $*" >&2
        exit 97
        ;;
    esac
fi
if [ -f "$dir/hang-$context" ]; then
    exec sleep 30
fi
if [ -f "$dir/down-$context" ]; then
    cat "$dir/down-$context" >&2
    exit 1
fi
case "$1" in
run)
    probe=
    image=
    pull=missing
    previous=
    for argument in "$@"; do
        if [ "$previous" = "--pull" ]; then
            pull=$argument
        fi
        if [ "$previous" = "drun.probe=true" ]; then
            image=$argument
        fi
        case "$argument" in
        --pull=*)
            pull=${argument#--pull=}
            ;;
        drun.probe=true)
            probe=yes
            ;;
        esac
        previous=$argument
    done
    if [ -n "$probe" ]; then
        if [ -f "$dir/no-image-$context" ]; then
            if [ "$pull" = never ]; then
                echo "docker: Error response from daemon: No such image: $image" >&2
                exit 125
            fi
            pull_image || exit 125
        fi
        if [ -f "$dir/probe-$context" ]; then
            cat "$dir/probe-$context"
            exit 0
        fi
        echo "fake docker: no probe answer for $context" >&2
        exit 1
    fi
    echo "output of the container on $context"
    ;;
image)
    if [ "$2" = inspect ]; then
        if [ -f "$dir/no-image-$context" ]; then
            echo "[]"
            echo "Error response from daemon: No such image: $last" >&2
            exit 1
        fi
        echo "sha256:0b8c1d5e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c"
        exit 0
    fi
    echo "$1 on $context"
    ;;
pull)
    if pull_image; then
        echo "$last: Pulling from library/busybox"
        echo "Status: Downloaded newer image for $last"
        exit 0
    fi
    exit 1
    ;;
ps)
    if [ -f "$dir/containers-$context" ]; then
        case "$*" in
        *"label=drun.managed=true"*)
            grep -F '"Labels":"drun.managed=true"' "$dir/containers-$context"
            ;;
        *)
            cat "$dir/containers-$context"
            ;;
        esac
    fi
    exit 0
    ;;
inspect)
    cat "$dir/inspect-$context"
    ;;
wait)
    cat "$dir/wait-$context"
    ;;
login)
    for argument in "$@"; do
        if [ "$argument" = "--password-stdin" ]; then
            cat > "$dir/stdin-$context"
        fi
    done
    echo "Login Succeeded"
    ;;
*)
    echo "$1 on $context"
    ;;
esac
if [ -f "$dir/exit-$context" ]; then
    exit "$(cat "$dir/exit-$context")"
fi
exit 0
"#;

/// A directory with a fake `docker`, a config home, and the answers of the
/// fake.
pub struct Fixture {
    dir: tempfile::TempDir,
}

/// The result of one run of `drun`.
#[derive(Debug)]
pub struct Run {
    /// The exit status, or `None` when a signal stopped `drun`.
    pub code: Option<i32>,
    /// What `drun` wrote to stdout.
    pub stdout: String,
    /// What `drun` wrote to stderr.
    pub stderr: String,
    /// How long the run took.
    pub elapsed: Duration,
}

/// The output of the probe container for a host with `total_kib` of memory,
/// `available_kib` of it free, a one-minute load average of `load`, and
/// `cpus` CPUs.
pub fn probe_answer(total_kib: u64, available_kib: u64, load: f64, cpus: u32) -> String {
    format!(
        "MemTotal:       {total_kib} kB\nMemFree:         1000 kB\nMemAvailable:   {available_kib} kB\n\
         @@drun@@\n{load:.2} 0.50 0.40 1/389 12345\n@@drun@@\n{cpus}\n"
    )
}

/// One row of `docker ps --format '{{json .}}'`.
pub fn row(id: &str, name: &str, image: &str, status: &str, managed: bool) -> String {
    let labels = if managed { "drun.managed=true" } else { "" };
    format!(
        "{{\"Command\":\"\\\"sh\\\"\",\"ID\":\"{id}\",\"Image\":\"{image}\",\
         \"Labels\":\"{labels}\",\"Names\":\"{name}\",\"Ports\":\"\",\
         \"State\":\"running\",\"Status\":\"{status}\"}}"
    )
}

/// `words` as the arguments of one call that the fake recorded.
pub fn call(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// Two hosts. `box-a` uses 90% of its memory, and `box-b` uses 20%.
pub fn two_hosts() -> Fixture {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.host("box-a", GIB_16, GIB_16 / 10, 0.2, 4);
    fixture.host("box-b", GIB_16, GIB_16 * 8 / 10, 0.2, 4);
    fixture
}

impl Fixture {
    /// Makes the directory, the fake `docker`, and an empty config home.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("the bin directory");
        std::fs::create_dir_all(dir.path().join("config").join("drun")).expect("the config home");
        std::fs::create_dir_all(dir.path().join("home")).expect("the home directory");
        let quoted = shellquote::shell_quote(&dir.path().display().to_string());
        // `write_executable` and not `std::fs::write`: on Linux, a fork in a
        // sibling test thread can copy the write descriptor, and the run of
        // the fake then fails with ETXTBSY.
        write_executable(
            bin.join("docker"),
            FAKE_DOCKER.replace(DIRECTORY_MARK, &quoted),
        )
        .expect("the fake docker");
        Self { dir }
    }

    /// The fixture directory.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The path of the config file that `drun` reads.
    pub fn config_path(&self) -> PathBuf {
        self.path().join("config").join("drun").join("config.toml")
    }

    /// Writes `text` as the config file.
    pub fn config(&self, text: &str) {
        std::fs::write(self.config_path(), text).expect("the config file");
    }

    /// Writes a config file that names `hosts`.
    pub fn hosts(&self, hosts: &[&str]) {
        let quoted: Vec<String> = hosts.iter().map(|host| format!("\"{host}\"")).collect();
        self.config(&format!("contexts = [{}]\n", quoted.join(", ")));
    }

    /// Makes `host` answer the probe with these figures.
    pub fn host(&self, host: &str, total_kib: u64, available_kib: u64, load: f64, cpus: u32) {
        self.write(
            &format!("probe-{host}"),
            &probe_answer(total_kib, available_kib, load, cpus),
        );
    }

    /// Makes `host` lack the image of the probe. A pull that succeeds gets the
    /// image.
    pub fn without_probe_image(&self, host: &str) {
        self.write(&format!("no-image-{host}"), "");
    }

    /// Makes each pull on `host` fail, with `message` on stderr.
    pub fn pull_down(&self, host: &str, message: &str) {
        self.write(&format!("pull-down-{host}"), message);
    }

    /// Makes `host` fail every call, with `message` on stderr.
    pub fn down(&self, host: &str, message: &str) {
        self.write(&format!("down-{host}"), message);
    }

    /// Makes `host` never answer.
    pub fn hang(&self, host: &str) {
        self.write(&format!("hang-{host}"), "");
    }

    /// Makes every call to `host` that is not a probe or a list stop with
    /// `code`.
    pub fn exit_status(&self, host: &str, code: i32) {
        self.write(&format!("exit-{host}"), &code.to_string());
    }

    /// Makes `host` list `rows` for `docker ps`.
    pub fn containers(&self, host: &str, rows: &[String]) {
        let mut text = rows.join("\n");
        text.push('\n');
        self.write(&format!("containers-{host}"), &text);
    }

    /// Makes `host` answer `docker inspect` with `text`.
    pub fn inspect(&self, host: &str, text: &str) {
        self.write(&format!("inspect-{host}"), text);
    }

    /// Makes `host` answer `docker wait` with `text`.
    pub fn wait_answer(&self, host: &str, text: &str) {
        self.write(&format!("wait-{host}"), text);
    }

    /// Makes `docker <command> --help` print `text`.
    pub fn help(&self, command: &str, text: &str) {
        self.write(&format!("help-{command}"), text);
    }

    /// The `DOCKER_` variables that `docker <command> --help` ran with, sorted,
    /// one for each line. The text is empty when the fake has no record of a
    /// call.
    pub fn help_environment(&self, command: &str) -> String {
        std::fs::read_to_string(self.path().join(format!("help-env-{command}"))).unwrap_or_default()
    }

    /// The `GIT_` variables of the last call to `host`, sorted, one for each
    /// line. The text is empty when the fake has no record of a call.
    pub fn git_environment(&self, host: &str) -> String {
        std::fs::read_to_string(self.path().join(format!("git-env-{host}"))).unwrap_or_default()
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.path().join(name), text).expect("an answer file");
    }

    /// Runs `drun` with `args` in the fixture directory.
    pub fn run(&self, args: &[&str]) -> Run {
        self.run_with(args, &[])
    }

    /// Runs `drun` with `args` and the extra environment variables `env`.
    pub fn run_with(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        self.run_full(args, env, None)
    }

    /// Runs `drun` with `args`, and writes `stdin` to its stdin.
    pub fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Run {
        self.run_full(args, &[], Some(stdin))
    }

    /// What `docker login --password-stdin` read from stdin on `host`. The
    /// text is empty when the fake has no record of such a call.
    pub fn stdin_of(&self, host: &str) -> String {
        std::fs::read_to_string(self.path().join(format!("stdin-{host}"))).unwrap_or_default()
    }

    /// The command that starts `drun` with `args` and the extra environment
    /// variables `env`, in an empty environment. The caller sets the streams.
    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Command {
        let path = format!("{}:/usr/bin:/bin", self.path().join("bin").display());
        let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_drun"));
        command
            .args(args)
            .current_dir(self.path())
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.path().join("home"))
            .env("XDG_CONFIG_HOME", self.path().join("config"));
        for (key, value) in env {
            command.env(key, value);
        }
        command
    }

    /// Runs `drun` with `args`, with a stdout whose reader closed before
    /// `drun` started, as when the reader of a pipe exits first. Each write to
    /// stdout fails with a broken pipe. The `stdout` of the result is empty.
    fn run_with_closed_stdout(&self, args: &[&str]) -> Run {
        let (reader, writer) = std::io::pipe().expect("a pipe");
        drop(reader);
        let mut command = self.command(args, &[]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::piped());
        let started = Instant::now();
        let child = command.spawn().expect("drun starts");
        drop(command);
        let output = child.wait_with_output().expect("drun stops");
        Run {
            code: output.status.code(),
            stdout: String::new(),
            stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
            elapsed: started.elapsed(),
        }
    }

    /// Runs `drun` with `args` normally, and then with a closed stdout. Asserts
    /// that the exit status is the same and that `drun` did not panic. Gives
    /// the closed run.
    pub fn assert_closed_stdout_keeps_status(&self, args: &[&str]) -> Run {
        let normal = self.run(args);
        let closed = self.run_with_closed_stdout(args);
        assert_eq!(
            closed.code, normal.code,
            "{args:?}: the exit status changed: {}",
            closed.stderr
        );
        assert!(
            !closed.stderr.contains("panicked"),
            "{args:?}: {}",
            closed.stderr
        );
        closed
    }

    /// Runs `drun` with `args`, the extra environment variables `env`, and
    /// `stdin` on its stdin. Without `stdin`, stdin is empty.
    fn run_full(&self, args: &[&str], env: &[(&str, &str)], stdin: Option<&str>) -> Run {
        let mut command = self.command(args, env);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let mut child = command.spawn().expect("drun starts");
        let mut input = child.stdin.take().expect("the stdin of drun");
        if let Some(text) = stdin {
            input
                .write_all(text.as_bytes())
                .expect("drun takes its stdin");
        }
        drop(input);
        let output = child.wait_with_output().expect("drun stops");
        Run {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
            elapsed: started.elapsed(),
        }
    }

    /// Every call that the fake got for `host`, each as its list of
    /// arguments after `--context <host>`.
    pub fn calls(&self, host: &str) -> Vec<Vec<String>> {
        let Ok(text) = std::fs::read_to_string(self.path().join(format!("calls-{host}.log")))
        else {
            return Vec::new();
        };
        // Each argument ends with the unit separator, so an empty argument
        // stays an empty argument.
        text.lines()
            .map(|line| match line.strip_suffix('\u{1f}') {
                Some(body) => body.split('\u{1f}').map(str::to_owned).collect(),
                None => Vec::new(),
            })
            .collect()
    }

    /// The calls for `host` whose docker command is `subcommand`. For `run`,
    /// the probe is not in the list.
    pub fn calls_of(&self, host: &str, subcommand: &str) -> Vec<Vec<String>> {
        self.calls(host)
            .into_iter()
            .filter(|call| call.first().is_some_and(|first| first == subcommand))
            .filter(|call| !call.iter().any(|argument| argument == "drun.probe=true"))
            .collect()
    }

    /// The probe calls for `host`.
    pub fn probes(&self, host: &str) -> Vec<Vec<String>> {
        self.calls(host)
            .into_iter()
            .filter(|call| call.iter().any(|argument| argument == "drun.probe=true"))
            .collect()
    }
}
