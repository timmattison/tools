//! End-to-end tests of the commands that find a container again: `drun ps`,
//! `drun status`, `drun logs`, `drun wait`, `drun exec`, `drun cp`, and
//! `drun rm`.

// The fake docker is a POSIX shell script.
#![cfg(unix)]
// Stricter than the inherited `[workspace.lints]` set; see "Lint Configuration" in CLAUDE.md.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod support;

use support::{call, row, two_hosts, Fixture, LOCAL};

/// The format that `drun status` gives `docker inspect`.
const STATE_FORMAT: &str = "{{.State.Status}} {{.State.ExitCode}}";

/// Two hosts. `box-a` has `job-a`, which `drun` started, and `other`, which
/// it did not. `box-b` has `job-b`, which `drun` started.
fn fleet() -> Fixture {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.containers(
        "box-a",
        &[
            row("aaa111", "job-a", "alpine", "Up 5 minutes", true),
            row("aaa222", "other", "nginx:1.27", "Up 1 hour", false),
        ],
    );
    fixture.containers(
        "box-b",
        &[row(
            "bbb111",
            "job-b",
            "alpine",
            "Exited (0) 2 minutes ago",
            true,
        )],
    );
    fixture
}

#[test]
fn help_names_every_command() {
    let run = Fixture::new().run(&["--help"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    for command in [
        "drun run",
        "drun ps",
        "drun status",
        "drun logs",
        "drun wait",
        "drun exec",
        "drun cp",
        "drun rm",
        "drun hosts",
        "drun pick",
    ] {
        assert!(run.stdout.contains(command), "{command}: {}", run.stdout);
    }
}

#[test]
fn ps_lists_the_containers_that_drun_started_on_every_host() {
    let fixture = fleet();
    let run = fixture.run(&["ps"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert!(
        lines[0].starts_with("HOST") && lines[0].contains("NAME"),
        "{}",
        run.stdout
    );
    assert!(
        lines[1].starts_with("box-a") && lines[1].contains("job-a"),
        "{}",
        run.stdout
    );
    assert!(
        lines[2].starts_with("box-b") && lines[2].contains("job-b"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("other"), "{}", run.stdout);
    let calls = fixture.calls_of("box-a", "ps");
    assert!(
        calls[0]
            .iter()
            .any(|argument| argument == "label=drun.managed=true"),
        "{calls:?}"
    );
}

#[test]
fn ps_reports_a_host_that_does_not_answer() {
    let fixture = fleet();
    fixture.down("box-b", "Cannot connect to box-b\n");
    let run = fixture.run(&["ps"]);
    assert_eq!(run.code, Some(125));
    assert!(run.stdout.contains("job-a"), "{}", run.stdout);
    assert!(run.stderr.contains("box-b"), "{}", run.stderr);
    assert!(
        run.stderr.contains("Cannot connect to box-b"),
        "{}",
        run.stderr
    );
}

#[test]
fn status_prints_the_host_the_state_and_the_exit_status() {
    let fixture = fleet();
    fixture.inspect("box-b", "exited 0\n");
    fixture.inspect("box-a", "running 0\n");
    let run = fixture.run(&["status", "job-b"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b exited 0\n");
    assert_eq!(
        fixture.calls_of("box-b", "inspect"),
        [call(&[
            "inspect",
            "--type",
            "container",
            "--format",
            STATE_FORMAT,
            "job-b"
        ])]
    );
    let run = fixture.run(&["status", "job-a"]);
    assert_eq!(run.stdout, "box-a running\n");
}

#[test]
fn a_container_is_found_by_the_start_of_its_id() {
    let fixture = fleet();
    fixture.inspect("box-b", "exited 0\n");
    let run = fixture.run(&["status", "bbb"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b exited 0\n");
}

#[test]
fn a_name_with_a_leading_slash_is_found_as_docker_finds_it() {
    let fixture = fleet();
    fixture.inspect("box-b", "exited 0\n");
    let run = fixture.run(&["status", "/job-b"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-b exited 0\n");
}

#[test]
fn rm_finds_a_name_with_a_leading_slash_and_passes_it_on_as_written() {
    let fixture = fleet();
    let run = fixture.run(&["rm", "/job-a"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-a", "rm"), [call(&["rm", "/job-a"])]);
}

#[test]
fn a_name_on_no_host_is_not_found_and_nothing_runs() {
    let fixture = fleet();
    let run = fixture.run(&["logs", "ghost"]);
    assert_eq!(run.code, Some(125));
    assert!(
        run.stderr.contains("no host has a container named ghost"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "logs").is_empty());
    assert!(fixture.calls_of("box-b", "logs").is_empty());
}

#[test]
fn not_found_names_the_hosts_that_did_not_answer() {
    let fixture = fleet();
    fixture.down("box-b", "Cannot connect to box-b\n");
    let run = fixture.run(&["logs", "ghost"]);
    assert_eq!(run.code, Some(125));
    assert!(run.stderr.contains("box-b"), "{}", run.stderr);
    assert!(
        run.stderr.contains("Cannot connect to box-b"),
        "{}",
        run.stderr
    );
}

#[test]
fn a_name_on_two_hosts_is_refused_and_both_hosts_are_named() {
    let fixture = fleet();
    fixture.containers(
        "box-b",
        &[row("bbb999", "job-a", "alpine", "Up 1 minute", true)],
    );
    let run = fixture.run(&["logs", "job-a"]);
    assert_eq!(run.code, Some(125));
    assert!(
        run.stderr.contains("job-a (aaa111) on box-a"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("job-a (bbb999) on box-b"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "logs").is_empty());
    assert!(fixture.calls_of("box-b", "logs").is_empty());
}

#[test]
fn the_start_of_two_ids_on_one_host_is_refused_and_each_container_is_named() {
    let fixture = fleet();
    let run = fixture.run(&["logs", "aaa"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(
        run.stderr.contains("job-a (aaa111) on box-a"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("other (aaa222) on box-a"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "logs").is_empty());
}

#[test]
fn the_start_of_an_id_on_each_host_is_refused_and_each_container_is_named() {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.containers(
        "box-a",
        &[row("abc111", "one", "alpine", "Up 1 minute", true)],
    );
    fixture.containers(
        "box-b",
        &[row("abc999", "two", "alpine", "Up 1 minute", true)],
    );
    let run = fixture.run(&["status", "abc"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(
        run.stderr.contains("one (abc111) on box-a"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("two (abc999) on box-b"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "inspect").is_empty());
    assert!(fixture.calls_of("box-b", "inspect").is_empty());
}

#[test]
fn a_name_on_one_host_wins_over_the_start_of_an_id_on_the_other() {
    let fixture = Fixture::new();
    fixture.hosts(&["box-a", "box-b"]);
    fixture.containers(
        "box-a",
        &[row("fff000", "abc", "alpine", "Up 1 minute", true)],
    );
    fixture.containers(
        "box-b",
        &[row("abc123", "job-b", "alpine", "Up 1 minute", true)],
    );
    fixture.inspect("box-a", "running 0\n");
    let run = fixture.run(&["status", "abc"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "box-a running\n");
    assert!(fixture.calls_of("box-b", "inspect").is_empty());
}

#[test]
fn rm_removes_nothing_when_a_name_matches_more_than_one_container() {
    let fixture = fleet();
    let run = fixture.run(&["rm", "job-b", "aaa"]);
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(
        run.stderr.contains("other (aaa222) on box-a"),
        "{}",
        run.stderr
    );
    assert!(fixture.calls_of("box-a", "rm").is_empty());
    assert!(fixture.calls_of("box-b", "rm").is_empty());
}

#[test]
fn logs_go_to_the_host_that_has_the_container() {
    let fixture = fleet();
    let run = fixture.run(&["logs", "-f", "--tail", "10", "job-b"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, "logs on box-b\n");
    assert_eq!(
        fixture.calls_of("box-b", "logs"),
        [call(&["logs", "-f", "--tail", "10", "job-b"])]
    );
    assert!(fixture.calls_of("box-a", "logs").is_empty());
}

#[test]
fn wait_exits_with_the_exit_status_of_the_container() {
    let fixture = fleet();
    fixture.wait_answer("box-b", "3\n");
    let run = fixture.run(&["wait", "job-b"]);
    assert_eq!(run.code, Some(3), "{}", run.stderr);
    assert_eq!(run.stdout, "3\n");
    assert_eq!(
        fixture.calls_of("box-b", "wait"),
        [call(&["wait", "job-b"])]
    );
    fixture.wait_answer("box-b", "0\n");
    assert_eq!(fixture.run(&["wait", "job-b"]).code, Some(0));
}

#[test]
fn exec_runs_on_the_host_that_has_the_container() {
    let fixture = fleet();
    let run = fixture.run(&["exec", "-it", "-e", "A=b", "job-a", "sh", "-c", "echo -d"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls_of("box-a", "exec"),
        [call(&[
            "exec", "-it", "-e", "A=b", "job-a", "sh", "-c", "echo -d"
        ])]
    );
}

#[test]
fn cp_copies_from_the_host_that_has_the_container() {
    let fixture = fleet();
    let run = fixture.run(&["cp", "job-b:/out/result.json", "./result.json"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls_of("box-b", "cp"),
        [call(&["cp", "job-b:/out/result.json", "./result.json"])]
    );
}

#[test]
fn rm_removes_each_container_on_its_own_host() {
    let fixture = fleet();
    let run = fixture.run(&["rm", "-f", "job-a", "job-b"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        fixture.calls_of("box-a", "rm"),
        [call(&["rm", "-f", "job-a"])]
    );
    assert_eq!(
        fixture.calls_of("box-b", "rm"),
        [call(&["rm", "-f", "job-b"])]
    );
}

#[test]
fn rm_removes_nothing_when_one_name_is_not_found() {
    let fixture = fleet();
    let run = fixture.run(&["rm", "job-a", "ghost"]);
    assert_eq!(run.code, Some(125));
    assert!(run.stderr.contains("ghost"), "{}", run.stderr);
    assert!(fixture.calls_of("box-a", "rm").is_empty());
}

#[test]
fn rm_exits_with_the_first_failure_and_still_tries_every_host() {
    let fixture = fleet();
    fixture.exit_status("box-a", 1);
    fixture.exit_status("box-b", 4);
    let run = fixture.run(&["rm", "job-a", "job-b"]);
    assert_eq!(run.code, Some(1), "box-a fails first: {}", run.stderr);
    assert_eq!(fixture.calls_of("box-b", "rm").len(), 1);
}

#[test]
fn commands_that_take_one_name_refuse_other_arguments() {
    let fixture = fleet();
    for args in [
        &["status"][..],
        &["status", "a", "b"],
        &["wait"],
        &["logs"],
        &["exec", "job-a"],
        &["cp", "job-a:/x"],
        &["rm"],
        &["ps", "extra"],
    ] {
        let run = fixture.run(args);
        assert_eq!(run.code, Some(2), "{args:?}: {}", run.stderr);
        for host in [LOCAL, "box-a", "box-b"] {
            assert!(
                fixture.calls(host).is_empty(),
                "{args:?}: {host} got a call: {}",
                run.stderr
            );
        }
    }
}

#[test]
fn a_detached_container_can_be_followed_to_its_result() {
    let fixture = two_hosts();
    let run = fixture.run(&["run", "-d", "alpine", "sleep", "5"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let (host, name) = run
        .stdout
        .trim_end()
        .split_once(' ')
        .expect("drun prints the host and the name");
    assert_eq!(host, "box-b");

    fixture.containers(host, &[row("ccc111", name, "alpine", "Up 1 second", true)]);
    fixture.inspect(host, "running 0\n");
    assert_eq!(fixture.run(&["status", name]).stdout, "box-b running\n");

    fixture.wait_answer(host, "0\n");
    assert_eq!(fixture.run(&["wait", name]).code, Some(0));

    fixture.inspect(host, "exited 0\n");
    assert_eq!(fixture.run(&["status", name]).stdout, "box-b exited 0\n");
}

#[test]
fn a_help_among_the_flags_prints_the_usage_and_asks_no_host() {
    for args in [
        &["exec", "-it", "--help"][..],
        &["logs", "-f", "--help"],
        &["rm", "-f", "--help"],
        &["cp", "-a", "--help"],
        &["rm", "-f", "--help", "job-a"],
        &["logs", "job-b", "--help"],
        &["exec", "-i", "--help=false", "--help", "job-a", "sh"],
        &["logs", "--help"],
        &["exec", "--help"],
        &["cp", "--help"],
        &["rm", "--help"],
        &["logs", "-f", "-h"],
        &["exec", "-it", "-h"],
        &["cp", "-a", "-h"],
        &["rm", "-h", "job-a"],
    ] {
        let fixture = fleet();
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert!(run.stdout.contains("drun logs"), "{args:?}: {}", run.stderr);
        for host in [LOCAL, "box-a", "box-b"] {
            assert!(
                fixture.calls(host).is_empty(),
                "{args:?} called {host}: {}",
                run.stderr
            );
        }
    }
}

#[test]
fn a_help_that_is_off_or_after_the_command_of_exec_does_not_stop_the_command() {
    for args in [
        &["logs", "-f", "--help", "--help=false", "job-b"][..],
        &["logs", "--help", "--help=false", "job-b"],
    ] {
        let fixture = fleet();
        let run = fixture.run(args);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
        assert_eq!(fixture.calls_of("box-b", "logs").len(), 1, "{args:?}");
    }
    let fixture = fleet();
    let run = fixture.run(&["exec", "job-a", "ls", "--help"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(fixture.calls_of("box-a", "exec").len(), 1);
}

#[test]
fn a_closed_stdout_changes_no_exit_status_and_causes_no_panic() {
    let fixture = fleet();
    fixture.inspect("box-b", "exited 0\n");
    fixture.wait_answer("box-b", "3\n");
    for args in [&["ps"][..], &["status", "job-b"], &["wait", "job-b"]] {
        let closed = fixture.assert_closed_stdout_keeps_status(args);
        if args.contains(&"wait") {
            assert_eq!(closed.code, Some(3), "{args:?}: {}", closed.stderr);
        }
    }
}
