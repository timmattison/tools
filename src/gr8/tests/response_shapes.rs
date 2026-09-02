//! End-to-end tests for the shapes of a `gh api rate_limit` response, driving
//! the real binary against a stub `gh`.
//!
//! GitHub changes the rate limit response without notice. It removes a
//! resource, it adds a resource, and a resource that it adds can hold a
//! different set of fields. These tests pin what gr8 does with each shape.
//!
//! The stub `gh` is a shell script in a throwaway directory, and the run gets
//! that directory as its whole `PATH`. So the test reads no `gh` configuration
//! of the person who started it, and it reaches no network.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

/// A response that holds two resources gr8 can read and one that it cannot.
///
/// `brand_new` has no `used` field. It stands for a resource that GitHub adds
/// with a set of fields that gr8 does not know.
const RESPONSE_WITH_A_RESOURCE_OF_A_NEW_SHAPE: &str = r#"
{
    "resources": {
        "core": {
            "limit": 5000,
            "used": 0,
            "remaining": 5000,
            "reset": 1788313546
        },
        "graphql": {
            "limit": 5000,
            "used": 0,
            "remaining": 5000,
            "reset": 1788313546
        },
        "brand_new": {
            "limit": 100,
            "remaining": 100,
            "reset": 1788313546
        }
    },
    "rate": {
        "limit": 5000,
        "used": 0,
        "remaining": 5000,
        "reset": 1788313546
    }
}
"#;

/// A response whose resource map is empty.
///
/// It stands for a response that gr8 cannot use at all, whatever the reason.
const RESPONSE_WITH_NO_RESOURCES: &str = r#"
{
    "resources": {},
    "rate": {
        "limit": 5000,
        "used": 0,
        "remaining": 5000,
        "reset": 1788313546
    }
}
"#;

/// A response whose every resource holds a set of fields that gr8 does not
/// know, so gr8 can read no rate limit from it.
const RESPONSE_WITH_ONLY_A_RESOURCE_OF_A_NEW_SHAPE: &str = r#"
{
    "resources": {
        "brand_new": {
            "limit": 100,
            "remaining": 100,
            "reset": 1788313546
        }
    }
}
"#;

/// Runs gr8 against a stub `gh` that prints the given response.
///
/// The stub is the only program on the `PATH` of the run, and the run gets no
/// other environment variable. Thus the run cannot reach the real `gh`, the
/// real GitHub, or the configuration of the person who started the test.
fn run_gr8_against(response: &str) -> Output {
    let directory = tempfile::Builder::new()
        .prefix("gr8-response-shapes-")
        .tempdir()
        .expect("the test must be able to make a temporary directory");
    let stub = directory.path().join("gh");
    // The stub uses `printf`, which every POSIX shell holds as a builtin. The
    // run gets the directory of the stub as its whole `PATH`, so a stub that
    // calls an external program finds nothing to call.
    fs::write(&stub, format!("#!/bin/sh\nprintf '%s\\n' '{response}'\n"))
        .expect("the test must be able to write the stub gh");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))
        .expect("the stub gh must be executable");

    Command::new(env!("CARGO_BIN_EXE_gr8"))
        .env_clear()
        .env("PATH", directory.path())
        .output()
        .expect("gr8 must run")
}

#[test]
fn shows_the_resources_that_it_can_read_beside_one_that_it_cannot() {
    let output = run_gr8_against(RESPONSE_WITH_A_RESOURCE_OF_A_NEW_SHAPE);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "one resource of a new shape must not stop gr8: {stderr}"
    );
    assert!(
        stdout.contains("core"),
        "gr8 must still show core: {stdout}"
    );
    assert!(
        stdout.contains("graphql"),
        "gr8 must still show graphql: {stdout}"
    );
}

#[test]
fn names_the_resource_whose_numbers_it_could_not_read() {
    let output = run_gr8_against(RESPONSE_WITH_A_RESOURCE_OF_A_NEW_SHAPE);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Skipped 1 resource that gr8 could not read: brand_new"),
        "gr8 must name the resource that it skipped: {stdout}"
    );
}

#[test]
fn stops_when_the_response_holds_no_resource() {
    let output = run_gr8_against(RESPONSE_WITH_NO_RESOURCES);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "a response that holds no resource must not be a success: {stdout}"
    );
    assert!(
        stderr.contains("held no resources"),
        "gr8 must say that the response held no resources: {stderr}"
    );
}

#[test]
fn stops_when_it_can_read_no_resource_of_the_response() {
    let output = run_gr8_against(RESPONSE_WITH_ONLY_A_RESOURCE_OF_A_NEW_SHAPE);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !output.status.success(),
        "a response with no readable resource must not be a success: {stdout}"
    );
    assert!(
        stderr.contains("no resource that gr8 can read"),
        "gr8 must say that it could read no resource: {stderr}"
    );
    assert!(
        stderr.contains("brand_new"),
        "gr8 must name the resource that it could not read: {stderr}"
    );
}
