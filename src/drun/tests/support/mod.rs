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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The memory of a computer with 16 GiB, in KiB.
pub const GIB_16: u64 = 16_777_216;

/// The text that the fake puts in place of the fixture directory.
const DIRECTORY_MARK: &str = "__FIXTURE_DIRECTORY__";

/// The fake `docker`. It answers from the files that the fixture writes:
///
/// - `hang-<host>`: the host never answers.
/// - `down-<host>`: the host fails every call, with the text of the file.
/// - `probe-<host>`: the answer to the probe container.
/// - `containers-<host>`: the rows of `docker ps`.
/// - `inspect-<host>`, `wait-<host>`: the answers to those commands.
/// - `exit-<host>`: the exit status of every other call.
/// - `help-<command>`: the answer to `docker <command> --help`, which has no
///   `--context`.
/// - `help-env-<command>`: written by the fake on each help call. It holds the
///   `DOCKER_` variables of that call, sorted, one for each line.
const FAKE_DOCKER: &str = r#"#!/bin/sh
# A fake docker for the end-to-end tests of drun.
dir=__FIXTURE_DIRECTORY__
if [ "$2" = "--help" ]; then
    env | grep '^DOCKER_' | sort > "$dir/help-env-$1" || true
    if [ -f "$dir/help-$1" ]; then
        cat "$dir/help-$1"
        exit 0
    fi
    echo "fake docker: no help for $1" >&2
    exit 1
fi
if [ "$1" != "--context" ]; then
    echo "fake docker: the first argument is not --context: $*" >&2
    exit 97
fi
context=$2
shift 2
separator=$(printf '\037')
line=
for argument in "$@"; do
    line="$line$argument$separator"
done
printf '%s\n' "$line" >> "$dir/calls-$context.log"
if [ -f "$dir/hang-$context" ]; then
    exec sleep 30
fi
if [ -f "$dir/down-$context" ]; then
    cat "$dir/down-$context" >&2
    exit 1
fi
case "$1" in
run)
    for argument in "$@"; do
        if [ "$argument" = "drun.probe=true" ]; then
            if [ -f "$dir/probe-$context" ]; then
                cat "$dir/probe-$context"
                exit 0
            fi
            echo "fake docker: no probe answer for $context" >&2
            exit 1
        fi
    done
    echo "output of the container on $context"
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

impl Fixture {
    /// Makes the directory, the fake `docker`, and an empty config home.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("the bin directory");
        std::fs::create_dir_all(dir.path().join("config").join("drun")).expect("the config home");
        std::fs::create_dir_all(dir.path().join("home")).expect("the home directory");
        let quoted = shellquote::shell_quote(&dir.path().display().to_string());
        let docker = bin.join("docker");
        std::fs::write(&docker, FAKE_DOCKER.replace(DIRECTORY_MARK, &quoted))
            .expect("the fake docker");
        make_executable(&docker);
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
        std::fs::read_to_string(self.path().join(format!("help-env-{command}")))
            .unwrap_or_default()
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
        let path = format!("{}:/usr/bin:/bin", self.path().join("bin").display());
        let mut command = Command::new(env!("CARGO_BIN_EXE_drun"));
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
        let started = Instant::now();
        let output = command.output().expect("drun starts");
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

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("the fake docker is executable");
}
