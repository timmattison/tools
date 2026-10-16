//! End-to-end tests of `drun buildx create`, which puts a new builder on the
//! least busy host, and of the other `buildx` command lines, which go to the
//! docker of this computer without a change.
//!
//! The docker of this computer keeps the builders, so every call goes there.
//! A host gets only the probe.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{Fixture, GIB_16, LOCAL};

/// `words` as the arguments of one call that the fake recorded.
fn call(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// Two hosts. `box-a` uses 90% of its memory, and `box-b` uses 20%.
fn two_hosts() -> Fixture {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.host("box-a", GIB_16, GIB_16 / 10, 0.2, 4);
    fixture.host("box-b", GIB_16, GIB_16 * 8 / 10, 0.2, 4);
    fixture
}

/// Makes sure that `drun` asked no host anything, not even for the probe.
fn assert_no_host_asked(fixture: &Fixture, args: &[&str]) {
    for host in ["box-a", "box-b"] {
        assert_eq!(
            fixture.calls(host),
            Vec::<Vec<String>>::new(),
            "{args:?}: {host}"
        );
    }
}

#[test]
fn create_puts_the_builder_on_the_least_busy_host() {
    let fixture = two_hosts();
    let args = [
        "buildx",
        "create",
        "--name",
        "misc-multiarch",
        "--driver",
        "docker-container",
        "--use",
    ];
    let run = fixture.run(&args);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let mut expected = call(&args);
    expected.push("box-b".to_owned());
    assert_eq!(fixture.calls(LOCAL), [expected]);
    for host in ["box-a", "box-b"] {
        assert_eq!(fixture.probes(host).len(), 1, "{host}");
        assert_eq!(
            fixture.calls(host).len(),
            1,
            "{host} gets the probe and nothing more"
        );
    }
    assert!(
        run.stderr
            .contains("drun: the builder misc-multiarch runs on box-b ("),
        "{}",
        run.stderr
    );
}

#[test]
fn builder_create_is_the_same_as_buildx_create() {
    let fixture = two_hosts();
    let run = fixture.run(&["builder", "create", "--use"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls(LOCAL),
        [call(&["builder", "create", "--use", "box-b"])]
    );
    assert!(
        run.stderr.contains("drun: the new builder runs on box-b ("),
        "{}",
        run.stderr
    );
}

#[test]
fn create_finds_the_host_after_the_flags_of_buildx() {
    let args = [
        "buildx",
        "--builder",
        "other",
        "-D",
        "create",
        "--bootstrap",
    ];
    let fixture = two_hosts();
    let run = fixture.run(&args);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let mut expected = call(&args);
    expected.push("box-b".to_owned());
    assert_eq!(fixture.calls(LOCAL), [expected]);
}

#[test]
fn create_with_an_endpoint_another_driver_or_the_leave_flag_goes_to_docker_unchanged() {
    let commands: [&[&str]; 6] = [
        &["buildx", "create", "--name", "n", "box-a"],
        &["buildx", "create", "box-a", "--name", "n"],
        &["buildx", "create", "--name", "n", "--", "box-a"],
        &["buildx", "create", "--driver", "kubernetes", "--name", "k"],
        &[
            "buildx",
            "create",
            "--driver=remote",
            "tcp://buildkitd.example:1234",
        ],
        &["buildx", "create", "--leave", "--name", "n", "--node", "n0"],
    ];
    for args in commands {
        let fixture = two_hosts();
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert_eq!(fixture.calls(LOCAL), [call(args)], "{args:?}");
        assert_no_host_asked(&fixture, args);
    }
}

#[test]
fn the_help_of_create_goes_to_docker_and_asks_no_host() {
    let fixture = two_hosts();
    fixture.help("buildx-create", "the help of buildx create\n");
    let run = fixture.run(&["buildx", "create", "--help"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "the help of buildx create\n");
    assert_no_host_asked(&fixture, &["buildx", "create", "--help"]);

    let args = ["buildx", "create", "-h"];
    let run = fixture.run(&args);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.calls(LOCAL), [call(&args)]);
    assert_no_host_asked(&fixture, &args);
}

#[test]
fn create_skips_a_host_that_does_not_answer() {
    let fixture = two_hosts();
    fixture.down("box-b", "Cannot connect to the Docker daemon");
    let run = fixture.run(&["buildx", "create", "--name", "n"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls(LOCAL),
        [call(&["buildx", "create", "--name", "n", "box-a"])]
    );
    assert!(run.stderr.contains("drun: skipped box-b"), "{}", run.stderr);
}

#[test]
fn create_stops_with_125_when_no_host_answers() {
    let fixture = two_hosts();
    fixture.down("box-a", "Cannot connect to the Docker daemon");
    fixture.down("box-b", "Cannot connect to the Docker daemon");
    let run = fixture.run(&["buildx", "create", "--name", "n"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(run.stderr.contains("no host answered"), "{}", run.stderr);
    assert!(fixture.calls(LOCAL).is_empty());
}

#[test]
fn create_stops_with_125_when_no_hosts_are_set() {
    let fixture = Fixture::new();
    let run = fixture.run(&["buildx", "create", "--name", "n"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(run.stderr.contains("no hosts are set"), "{}", run.stderr);
    assert!(fixture.calls(LOCAL).is_empty());
}

#[test]
fn create_exits_with_the_exit_status_of_docker() {
    let fixture = two_hosts();
    fixture.exit_status(LOCAL, 1);
    let run = fixture.run(&["buildx", "create", "--name", "n"]);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    assert_eq!(fixture.calls(LOCAL).len(), 1);
}

#[test]
fn an_unknown_flag_is_refused_before_any_host_is_asked() {
    for (args, needle) in [
        (
            &["buildx", "create", "--frobnicate", "x"][..],
            "drun does not know the flag --frobnicate of docker buildx create",
        ),
        (
            &["buildx", "--frobnicate", "create"],
            "drun does not know the flag --frobnicate of docker buildx.",
        ),
    ] {
        let fixture = two_hosts();
        let run = fixture.run(args);
        assert_eq!(run.code, Some(2), "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains(needle), "{args:?}: {}", run.stderr);
        assert!(fixture.calls(LOCAL).is_empty(), "{args:?}");
        assert_no_host_asked(&fixture, args);
    }
}

#[test]
fn the_help_shows_buildx_create() {
    let run = Fixture::new().run(&["--help"]);
    assert!(
        run.stdout
            .contains("drun buildx [docker buildx options] create [docker buildx create options]"),
        "{}",
        run.stdout
    );
}
