//! End-to-end tests of the commands that `drun` does not know. `drun` gives
//! each of them to the docker of this computer without a change, so a script
//! can run `drun` in place of `docker`.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{Fixture, LOCAL};

/// `words` as the arguments of one call that the fake recorded.
fn call(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// A fixture with two hosts, which no test in this file must ask.
fn two_hosts() -> Fixture {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture
}

/// Makes sure that `drun` asked no host anything, not even for the probe.
fn assert_no_host_asked(fixture: &Fixture) {
    for host in ["box-a", "box-b"] {
        assert_eq!(fixture.calls(host), Vec::<Vec<String>>::new(), "{host}");
    }
}

#[test]
fn a_command_that_drun_does_not_know_goes_to_the_docker_of_this_computer() {
    let fixture = Fixture::new();
    let run = fixture.run(&["frobnicate", "--flag", "value"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls(LOCAL),
        [call(&["frobnicate", "--flag", "value"])]
    );
    assert_eq!(run.stdout, "frobnicate on @local\n");
}

#[test]
fn the_buildx_commands_of_a_build_script_go_to_the_docker_of_this_computer() {
    let fixture = two_hosts();
    let commands: [&[&str]; 6] = [
        &["buildx", "inspect", "misc-multiarch"],
        &["buildx", "use", "misc-multiarch"],
        &[
            "buildx",
            "build",
            "--platform",
            "linux/amd64,linux/arm64",
            "--tag",
            "registry.example/app:dev",
            "--push",
            ".",
        ],
        &[
            "buildx",
            "imagetools",
            "inspect",
            "--raw",
            "registry.example/app:dev",
        ],
        &["builder", "prune", "--force"],
        &["version"],
    ];
    for args in commands {
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
    }
    let expected: Vec<Vec<String>> = commands.iter().map(|args| call(args)).collect();
    assert_eq!(fixture.calls(LOCAL), expected);
    assert_no_host_asked(&fixture);
}

#[test]
fn a_docker_flag_before_the_command_goes_to_docker_with_the_command() {
    let fixture = two_hosts();
    let run = fixture.run(&[
        "--context",
        "box-a",
        "run",
        "--rm",
        "--platform",
        "linux/amd64",
        "busybox:1.37",
        "true",
    ]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls("box-a"),
        [call(&[
            "run",
            "--rm",
            "--platform",
            "linux/amd64",
            "busybox:1.37",
            "true"
        ])],
        "no probe, no label, and no name"
    );
    assert!(fixture.calls("box-b").is_empty());
    assert!(fixture.calls(LOCAL).is_empty());
}

#[test]
fn the_exit_status_of_docker_is_the_exit_status_of_drun() {
    let fixture = Fixture::new();
    fixture.exit_status(LOCAL, 7);
    let run = fixture.run(&["buildx", "inspect", "missing-builder"]);
    assert_eq!(run.code, Some(7), "{}", run.stderr);
}

#[test]
fn docker_reads_the_stdin_of_drun() {
    let fixture = Fixture::new();
    let run = fixture.run_with_stdin(
        &[
            "login",
            "--username",
            "AWS",
            "--password-stdin",
            "registry.example",
        ],
        "the-password\n",
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.stdin_of(LOCAL), "the-password\n");
    assert_eq!(run.stdout, "Login Succeeded\n");
}

#[test]
fn an_argument_keeps_its_characters() {
    let fixture = Fixture::new();
    let run = fixture.run(&["tag", "café-日本語-🎉", ""]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.calls(LOCAL), [call(&["tag", "café-日本語-🎉", ""])]);
}

#[test]
fn drun_stops_with_125_when_docker_is_not_installed() {
    let fixture = Fixture::new();
    let run = fixture.run_with(&["buildx", "ls"], &[("PATH", "/drun-test-no-such-directory")]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(run.stderr.contains("cannot start docker"), "{}", run.stderr);
}

#[test]
fn the_help_and_the_version_stay_the_help_and_the_version_of_drun() {
    let fixture = Fixture::new();
    for args in [&["--help"][..], &["-h"], &["help"], &["--version"], &["-V"]] {
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert!(run.stdout.contains("drun"), "{args:?}: {}", run.stdout);
    }
    assert!(fixture.calls(LOCAL).is_empty());
}

#[test]
fn the_help_says_that_other_commands_go_to_docker() {
    let run = Fixture::new().run(&["--help"]);
    assert!(
        run.stdout.contains("drun DOCKER-COMMAND [ARG...]"),
        "{}",
        run.stdout
    );
}
