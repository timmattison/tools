//! End-to-end tests of `drun run`, `drun hosts`, and `drun pick`: how `drun`
//! chooses a host and starts a container there.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use std::time::Duration;

use support::{row, Fixture, GIB_16};

/// The label that `drun run` puts on every container.
const MANAGED: &str = "drun.managed=true";

/// Two hosts. `box-a` uses 90% of its memory, and `box-b` uses 20%.
fn two_hosts() -> Fixture {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.host("box-a", GIB_16, GIB_16 / 10, 0.2, 4);
    fixture.host("box-b", GIB_16, GIB_16 * 8 / 10, 0.2, 4);
    fixture
}

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
    let run = fixture.run(&[
        "run",
        "-v",
        "data:/data",
        "-v",
        "/srv/only-on-the-host:/x",
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
            probes[0].iter().any(|argument| argument == "busybox:1.37"),
            "{probes:?}"
        );
    }
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
fn run_without_an_image_is_a_usage_error() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "-d"]);
    assert_eq!(run.code, Some(2));
    assert!(run.stderr.contains("needs an image"), "{}", run.stderr);
    assert!(fixture.calls("box-a").is_empty());
}
