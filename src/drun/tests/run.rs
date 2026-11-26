//! End-to-end tests of `drun run`, `drun hosts`, and `drun pick`: how `drun`
//! chooses a host and starts a container there.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::time::Duration;

use support::{row, two_hosts, Fixture, GIB_16};

/// The label that `drun run` puts on every container.
const MANAGED: &str = "drun.managed=true";

/// The value that follows `flag` in `call`.
fn value_of<'a>(call: &'a [String], flag: &str) -> Option<&'a str> {
    call.iter()
        .position(|argument| argument == flag)
        .and_then(|index| call.get(index + 1))
        .map(String::as_str)
}

#[test]
fn version_prints_name_number_and_build() {
    let run = Fixture::new().run(&["--version"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stdout.starts_with("drun 0.1.0 ("), "{}", run.stdout);
}

#[test]
fn help_names_the_commands_and_the_config_file() {
    let fixture = Fixture::new();
    for args in [&["--help"][..], &["-h"], &["help"], &["run", "--help"]] {
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        for needle in ["drun run", "drun hosts", "drun pick", "DRUN_CONTEXTS"] {
            assert!(
                run.stdout.contains(needle),
                "{args:?} lacks {needle}: {}",
                run.stdout
            );
        }
        let path = fixture.config_path().display().to_string();
        assert!(
            run.stdout.contains(&path),
            "{args:?} lacks {path}: {}",
            run.stdout
        );
    }
}

#[test]
fn no_arguments_is_a_usage_error() {
    let run = Fixture::new().run(&[]);
    assert_eq!(run.code, Some(2));
    assert!(run.stderr.contains("Usage"), "{}", run.stderr);
}

#[test]
fn run_starts_the_container_on_the_host_with_the_most_free_memory() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "alpine", "echo", "hi"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "output of the container on box-b\n");
    assert!(fixture.calls_of("box-a", "run").is_empty());
    let calls = fixture.calls_of("box-b", "run");
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call = &calls[0];
    assert_eq!(call[..3], ["run", "--label", MANAGED]);
    assert_eq!(call[call.len() - 3..], ["alpine", "echo", "hi"]);
    let name = value_of(call, "--name").expect("drun gives the container a name");
    assert!(run.stderr.contains("box-b"), "{}", run.stderr);
    assert!(run.stderr.contains(name), "{}", run.stderr);
}

#[test]
fn run_avoids_a_host_whose_cpus_are_saturated() {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.host("box-a", GIB_16, GIB_16 * 7 / 10, 8.0, 4);
    fixture.host("box-b", GIB_16, GIB_16 * 4 / 10, 0.5, 4);
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(fixture.calls_of("box-a", "run").is_empty());
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1);
}

#[test]
fn run_passes_every_argument_through_unchanged() {
    let fixture = two_hosts();
    let given = [
        "-h",
        "web",
        "-e",
        "GREETING=héllo 日本語 🎉",
        "--name",
        "job-1",
        "-e",
        "",
        "alpine",
        "echo",
        "--status",
        "-d",
    ];
    let mut args = vec!["run"];
    args.extend(given);
    let run = fixture.run(&args);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let calls = fixture.calls_of("box-b", "run");
    let mut expected = vec!["run", "--label", MANAGED];
    expected.extend(given);
    assert_eq!(
        calls,
        [expected],
        "the -d after the image belongs to the command"
    );
}

#[test]
fn a_foreground_run_exits_with_the_exit_status_of_the_container() {
    let fixture = two_hosts();
    fixture.exit_status("box-b", 3);
    let run = fixture.run(&["run", "alpine", "sh", "-c", "exit 3"]);
    assert_eq!(run.code, Some(3), "{}", run.stderr);
}

#[test]
fn a_detached_run_prints_the_host_and_the_name() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "-d", "alpine", "sleep", "5"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let calls = fixture.calls_of("box-b", "run");
    let name = value_of(&calls[0], "--name").expect("drun gives the container a name");
    assert_eq!(run.stdout, format!("box-b {name}\n"));
}

#[test]
fn a_detached_run_that_docker_refuses_prints_nothing_and_keeps_the_exit_status() {
    let fixture = two_hosts();
    fixture.exit_status("box-b", 125);
    let run = fixture.run(&["run", "-d", "alpine"]);
    assert_eq!(run.code, Some(125));
    assert_eq!(run.stdout, "");
}

#[test]
fn a_given_name_is_used_and_no_other_name_is_added() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "--name", "job-1", "-d", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b job-1\n");
    let calls = fixture.calls_of("box-b", "run");
    let names = calls[0]
        .iter()
        .filter(|argument| *argument == "--name")
        .count();
    assert_eq!(names, 1, "{calls:?}");
}

#[test]
fn a_name_that_a_host_uses_already_is_refused() {
    let fixture = two_hosts();
    fixture.containers(
        "box-a",
        &[row("3f2a", "job-1", "alpine", "Up 2 hours", false)],
    );
    let run = fixture.run(&["run", "--name", "job-1", "alpine"]);
    assert_eq!(run.code, Some(125));
    assert!(
        run.stderr.contains("job-1") && run.stderr.contains("box-a"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "run").is_empty());
    assert!(fixture.calls_of("box-b", "run").is_empty());
}

#[test]
fn a_name_with_a_leading_slash_that_a_host_uses_already_is_refused() {
    for spelling in [vec!["--name", "/job-1"], vec!["--name=/job-1"]] {
        let fixture = two_hosts();
        fixture.containers(
            "box-a",
            &[row("3f2a", "job-1", "alpine", "Up 2 hours", false)],
        );
        let mut args = vec!["run"];
        args.extend(spelling.iter().copied());
        args.push("alpine");
        let run = fixture.run(&args);
        assert_eq!(run.code, Some(125), "{spelling:?}: {}", run.stderr);
        assert!(
            run.stderr.contains("already") && run.stderr.contains("box-a"),
            "{spelling:?}: {}",
            run.stderr
        );
        assert!(fixture.calls_of("box-a", "run").is_empty(), "{spelling:?}");
        assert!(fixture.calls_of("box-b", "run").is_empty(), "{spelling:?}");
    }
}

#[test]
fn a_detached_run_with_a_leading_slash_name_prints_the_bare_name() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "-d", "--name", "/job-1", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b job-1\n");
    assert!(
        run.stderr.contains("drun: job-1 runs on box-b"),
        "{}",
        run.stderr
    );
    let calls = fixture.calls_of("box-b", "run");
    assert_eq!(value_of(&calls[0], "--name"), Some("/job-1"));
}

#[test]
fn rm_with_detach_prints_a_warning_and_still_runs() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "-d", "--rm", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains("warning") && run.stderr.contains("--rm"),
        "{}",
        run.stderr
    );
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1);
}

#[test]
fn rm_in_the_foreground_prints_no_warning() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "--rm", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(!run.stderr.contains("warning"), "{}", run.stderr);
}

#[test]
fn a_bind_mount_from_this_computer_prints_a_warning() {
    let fixture = two_hosts();
    std::fs::create_dir(fixture.path().join("data")).unwrap();
    let absolute = fixture.path().join("data").display().to_string();
    for mount in [
        vec!["-v".to_owned(), "./data:/data".to_owned()],
        vec!["-v".to_owned(), format!("{absolute}:/data")],
        vec![
            "--mount".to_owned(),
            format!("type=bind,source={absolute},target=/data"),
        ],
    ] {
        let mut args = vec!["run".to_owned()];
        args.extend(mount.clone());
        args.push("alpine".to_owned());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let run = fixture.run(&args);
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        assert!(run.stderr.contains("warning"), "{mount:?}: {}", run.stderr);
        assert!(run.stderr.contains("box-b"), "{mount:?}: {}", run.stderr);
    }
}

#[test]
fn the_bind_mount_warning_names_its_condition_and_points_at_drun_cp() {
    let fixture = two_hosts();
    std::fs::create_dir(fixture.path().join("data")).unwrap();
    let absolute = fixture.path().join("data").display().to_string();
    let run = fixture.run(&["run", "-v", &format!("{absolute}:/data"), "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains("When box-b is another computer"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("drun cp"), "{}", run.stderr);
    assert!(!run.stderr.contains("docker cp"), "{}", run.stderr);
}

#[test]
fn a_named_volume_or_a_path_that_is_not_on_this_computer_prints_no_warning() {
    let fixture = two_hosts();
    // The fixture owns its directory, and no test makes `absent` in it, so
    // neither path exists on any computer that runs the test.
    let absent = format!("{}/absent:/x", fixture.path().display());
    let run = fixture.run(&[
        "run",
        "-v",
        "data:/data",
        "-v",
        &absent,
        "-v",
        "./absent:/y",
        "alpine",
    ]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(!run.stderr.contains("warning"), "{}", run.stderr);
}

#[test]
fn a_host_that_fails_the_probe_is_skipped_with_a_warning() {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.host("box-a", GIB_16, GIB_16, 0.0, 4);
    fixture.down(
        "box-a",
        "Cannot connect to the Docker daemon at unix:///var/run/docker.sock\n",
    );
    fixture.host("box-b", GIB_16, GIB_16 / 2, 1.0, 4);
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stderr.contains("box-a"), "{}", run.stderr);
    assert!(run.stderr.contains("Cannot connect"), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1);
}

#[test]
fn a_host_that_does_not_answer_in_time_is_skipped() {
    let fixture = Fixture::new();
    fixture.config("contexts = [\"box-a\", \"box-b\"]\nhost_timeout_secs = 1\n");
    fixture.host("box-a", GIB_16, GIB_16, 0.0, 4);
    fixture.hang("box-a");
    fixture.host("box-b", GIB_16, GIB_16 / 2, 1.0, 4);
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.elapsed < Duration::from_secs(20), "{:?}", run.elapsed);
    assert!(run.stderr.contains("no answer in 1 s"), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1);
}

#[test]
fn when_no_host_answers_nothing_runs() {
    let fixture = two_hosts();
    fixture.down("box-a", "Cannot connect to box-a\n");
    fixture.down("box-b", "Cannot connect to box-b\n");
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(125));
    assert!(
        run.stderr.contains("Cannot connect to box-a"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("Cannot connect to box-b"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "run").is_empty());
    assert!(fixture.calls_of("box-b", "run").is_empty());
}

#[test]
fn the_probe_container_removes_itself() {
    let fixture = two_hosts();
    let run = fixture.run(&["pick"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    for host in ["box-a", "box-b"] {
        let probes = fixture.probes(host);
        assert_eq!(probes.len(), 1, "{host}: {probes:?}");
        assert_eq!(probes[0][0], "run");
        assert!(
            probes[0].iter().any(|argument| argument == "--rm"),
            "{probes:?}"
        );
        assert!(
            probes[0].iter().any(|argument| argument == PROBE_IMAGE),
            "{probes:?}"
        );
    }
}

/// The image of the probe container.
const PROBE_IMAGE: &str = "busybox:1.37";

/// The calls of `host` that get or use the image of the probe, in order:
/// `probe` for the probe container, `image inspect`, and `pull`.
fn image_steps(fixture: &Fixture, host: &str) -> Vec<Vec<String>> {
    fixture
        .calls(host)
        .into_iter()
        .filter(|call| {
            call.iter().any(|argument| argument == "drun.probe=true")
                || call
                    .first()
                    .is_some_and(|first| first == "image" || first == "pull")
        })
        .collect()
}

/// `true` when `call` is a probe that cannot pull its image.
fn is_probe_without_a_pull(call: &[String]) -> bool {
    call.iter().any(|argument| argument == "drun.probe=true")
        && value_of(call, "--pull") == Some("never")
}

#[test]
fn a_host_with_the_probe_image_answers_the_probe_without_a_pull() {
    let fixture = two_hosts();
    let run = fixture.run(&["pick"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b\n");
    for host in ["box-a", "box-b"] {
        let steps = image_steps(&fixture, host);
        assert_eq!(steps.len(), 1, "{host}: {steps:?}");
        assert!(is_probe_without_a_pull(&steps[0]), "{host}: {steps:?}");
    }
}

#[test]
fn a_host_without_the_probe_image_pulls_it_and_answers_the_probe_again() {
    let fixture = two_hosts();
    fixture.without_probe_image("box-b");
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(!run.stderr.contains("skipped"), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1, "{}", run.stderr);
    assert!(fixture.calls_of("box-a", "run").is_empty());
    let steps = image_steps(&fixture, "box-b");
    assert_eq!(steps.len(), 4, "{steps:?}");
    assert!(is_probe_without_a_pull(&steps[0]), "{steps:?}");
    assert_eq!(
        (steps[1][0].as_str(), steps[1][1].as_str()),
        ("image", "inspect"),
        "{steps:?}"
    );
    assert_eq!(
        steps[1].last().map(String::as_str),
        Some(PROBE_IMAGE),
        "{steps:?}"
    );
    assert_eq!(steps[2], ["pull", PROBE_IMAGE], "{steps:?}");
    assert!(is_probe_without_a_pull(&steps[3]), "{steps:?}");
    assert_eq!(image_steps(&fixture, "box-a").len(), 1);
}

#[test]
fn a_pull_of_the_probe_image_writes_a_note_on_stderr_and_not_on_stdout() {
    let fixture = two_hosts();
    fixture.without_probe_image("box-b");
    let run = fixture.run(&["pick"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b\n");
    let notes: Vec<&str> = run
        .stderr
        .lines()
        .filter(|line| line.contains("box-b") && line.contains(PROBE_IMAGE))
        .collect();
    assert_eq!(notes.len(), 1, "{}", run.stderr);
    assert!(notes[0].starts_with("drun: "), "{}", run.stderr);
    assert!(notes[0].contains("120 s"), "{}", run.stderr);
    assert!(
        !run.stderr.contains("box-a"),
        "a host with the image gets no note: {}",
        run.stderr
    );
}

#[test]
fn a_host_whose_pull_fails_is_skipped_with_the_pull_command() {
    let fixture = two_hosts();
    fixture.without_probe_image("box-b");
    fixture.pull_down(
        "box-b",
        "Error response from daemon: Get \"https://registry-1.docker.io/v2/\": \
         net/http: request canceled while waiting for connection\n",
    );
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stderr.contains("skipped box-b"), "{}", run.stderr);
    assert!(
        run.stderr
            .contains(&format!("docker --context box-b pull {PROBE_IMAGE}")),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("request canceled"), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-a", "run").len(), 1, "{}", run.stderr);
    assert!(fixture.calls_of("box-b", "run").is_empty());
}

#[test]
fn pick_prints_only_the_chosen_host() {
    let fixture = two_hosts();
    let run = fixture.run(&["pick"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b\n");
    assert!(fixture.calls_of("box-b", "run").is_empty());
}

#[test]
fn hosts_shows_every_host_and_marks_the_chosen_one() {
    let fixture = two_hosts();
    fixture.config("contexts = [\"box-a\", \"box-b\", \"box-c\"]\n");
    fixture.down("box-c", "Cannot connect to box-c\n");
    let run = fixture.run(&["hosts"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{}", run.stdout);
    assert!(lines[0].starts_with("HOST"), "{}", run.stdout);
    assert!(
        lines[1].starts_with("box-a") && lines[1].contains("90%"),
        "{}",
        run.stdout
    );
    assert!(!lines[1].contains("chosen"), "{}", run.stdout);
    assert!(
        lines[2].starts_with("box-b") && lines[2].ends_with("chosen"),
        "{}",
        run.stdout
    );
    assert!(
        lines[3].starts_with("box-c") && lines[3].contains("Cannot connect"),
        "{}",
        run.stdout
    );
}

#[test]
fn hosts_fails_when_no_host_answers() {
    let fixture = two_hosts();
    fixture.down("box-a", "down\n");
    fixture.down("box-b", "down\n");
    let run = fixture.run(&["hosts"]);
    assert_eq!(run.code, Some(125));
    assert_eq!(run.stdout.lines().count(), 3, "{}", run.stdout);
}

#[test]
fn without_hosts_drun_explains_how_to_set_them() {
    let fixture = Fixture::new();
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(125));
    assert!(run.stderr.contains("DRUN_CONTEXTS"), "{}", run.stderr);
    let path = fixture.config_path().display().to_string();
    assert!(run.stderr.contains(&path), "{}", run.stderr);
}

#[test]
fn drun_contexts_overrides_the_config_file() {
    let fixture = two_hosts();
    let run = fixture.run_with(&["run", "alpine"], &[("DRUN_CONTEXTS", "box-a")]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-a", "run").len(), 1);
    assert!(fixture.calls("box-b").is_empty());
}

#[test]
fn an_unknown_docker_run_flag_is_refused_before_any_host_is_asked() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "--frobnicate", "alpine"]);
    assert_eq!(run.code, Some(2));
    assert!(run.stderr.contains("--frobnicate=VALUE"), "{}", run.stderr);
    assert!(fixture.calls("box-a").is_empty());
    assert!(fixture.calls("box-b").is_empty());
    let run = fixture.run(&["run", "--frobnicate=1", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
}

#[test]
fn an_empty_name_is_refused_before_any_host_is_asked() {
    for spelling in [
        vec!["--name", ""],
        vec!["--name="],
        vec!["--name", "/"],
        vec!["--name=/"],
    ] {
        let fixture = two_hosts();
        let mut args = vec!["run", "-d"];
        args.extend(spelling.iter().copied());
        args.extend(["alpine", "true"]);
        let run = fixture.run(&args);
        assert_eq!(run.code, Some(2), "{spelling:?}: {}", run.stderr);
        assert!(run.stdout.is_empty(), "{spelling:?}: {}", run.stdout);
        assert!(
            run.stderr.contains("--name"),
            "{spelling:?}: {}",
            run.stderr
        );
        assert!(fixture.calls("box-a").is_empty(), "{spelling:?}");
        assert!(fixture.calls("box-b").is_empty(), "{spelling:?}");
    }
}

#[test]
fn a_help_among_the_flags_of_run_prints_the_usage_and_asks_no_host() {
    for args in [
        &["run", "-d", "--rm", "--help", "alpine"][..],
        &["run", "-it", "--help"],
        &["run", "-i", "--help=false", "--help", "alpine"],
    ] {
        let fixture = two_hosts();
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert!(run.stdout.contains("drun run"), "{args:?}: {}", run.stdout);
        assert!(
            !run.stdout.lines().any(|line| line.starts_with("box-")),
            "{args:?} names a container: {}",
            run.stdout
        );
        assert!(fixture.calls("box-a").is_empty(), "{args:?}");
        assert!(fixture.calls("box-b").is_empty(), "{args:?}");
    }
}

#[test]
fn a_help_that_is_off_or_after_the_image_does_not_stop_the_run() {
    for args in [
        &["run", "-i", "--help", "--help=false", "alpine"][..],
        &["run", "--help", "--help=false", "alpine"],
        &["run", "alpine", "--help"],
    ] {
        let fixture = two_hosts();
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert_eq!(fixture.calls_of("box-b", "run").len(), 1, "{args:?}");
    }
}

#[test]
fn run_without_an_image_is_a_usage_error() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "-d"]);
    assert_eq!(run.code, Some(2));
    assert!(run.stderr.contains("needs an image"), "{}", run.stderr);
    assert!(fixture.calls("box-a").is_empty());
}

#[test]
fn a_host_whose_containers_cannot_be_read_gives_a_warning_and_the_run_still_starts() {
    let fixture = two_hosts();
    fixture.containers("box-b", &["not json".to_owned()]);
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr
            .contains("drun: warning: cannot read the container names of box-b"),
        "{}",
        run.stderr
    );
    assert!(
        !run.stderr
            .contains("cannot read the container names of box-a"),
        "{}",
        run.stderr
    );
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1);
}

#[test]
fn a_host_that_is_down_is_reported_once_as_skipped() {
    let fixture = two_hosts();
    fixture.down("box-a", "Cannot connect to box-a\n");
    let run = fixture.run(&["run", "alpine"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stderr.contains("drun: skipped box-a"), "{}", run.stderr);
    assert!(
        !run.stderr.contains("cannot read the container names"),
        "{}",
        run.stderr
    );
    assert_eq!(fixture.calls_of("box-b", "run").len(), 1);
}

#[test]
fn a_closed_stdout_changes_no_exit_status_and_causes_no_panic() {
    let fixture = two_hosts();
    for args in [
        &["--version"][..],
        &["--help"],
        &["hosts"],
        &["pick"],
        &["run", "-d", "alpine"],
    ] {
        let closed = fixture.assert_closed_stdout_keeps_status(args);
        if args.contains(&"-d") {
            assert_eq!(closed.code, Some(0), "{args:?}: {}", closed.stderr);
            assert!(
                closed.stderr.contains("runs on box-b"),
                "{args:?}: {}",
                closed.stderr
            );
        }
    }
}
