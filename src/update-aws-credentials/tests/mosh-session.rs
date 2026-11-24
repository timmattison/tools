//! End-to-end tests of `update-aws-credentials` in a mosh-rs session.
//!
//! In a mosh-rs session the person copies the credentials on the client. The
//! server gives each session a `pbpaste` shim, and `sysclip` must read the
//! credentials through it. A change of the call site to a route that skips the
//! shim fails these tests.
//!
//! No child touches the clipboard of the machine. Each child starts with an
//! empty environment. `MOSH_OPEN_SOCKET` is set, and the fake commands of
//! `sysclip::testing::FakeShims` come first on `PATH`. In that route `sysclip`
//! never opens `arboard`.
//!
//! No run reaches `~/.aws`, because `HOME` is a temporary directory of the
//! test. No run reaches AWS STS over the network, because the clipboard text of
//! each test is never a valid set of credentials, and the tool stops at the
//! parser.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Output;

use sysclip::testing::FakeShims;
use tempfile::TempDir;

/// The directory of the fake commands, inside the temporary directory.
const SHIMS_DIRECTORY: &str = "shims";

/// The home directory of the child, inside the temporary directory.
const HOME_DIRECTORY: &str = "home";

/// The directory that the tool writes, inside the home directory.
const AWS_DIRECTORY: &str = ".aws";

/// The message of the tool when the text has no session token.
const NO_SESSION_TOKEN: &str = "Could not find the AWS session token in the clipboard";

/// The message of the tool when the read of the clipboard fails.
const READ_FAILURE: &str = "Failed to read from clipboard";

/// The message of the tool when the clipboard does not open.
const OPEN_FAILURE: &str = "Failed to initialize clipboard";

/// One test: its temporary directory, its fake commands, and its home.
struct Session {
    /// Holds the directory alive until the test ends.
    _scratch: TempDir,
    shims: FakeShims,
    home: PathBuf,
}

impl Session {
    fn new() -> Self {
        let scratch = TempDir::new().expect("the temporary directory is made");
        let shims = FakeShims::install(&scratch.path().join(SHIMS_DIRECTORY));
        let home = scratch.path().join(HOME_DIRECTORY);
        std::fs::create_dir_all(&home).expect("the home directory is made");
        Self {
            _scratch: scratch,
            shims,
            home,
        }
    }

    /// Runs the tool with an environment built from nothing.
    fn run(&self) -> Output {
        let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_update-aws-credentials"));
        command.env_clear();
        self.shims
            .enter_session(&mut command)
            .env("HOME", &self.home)
            .output()
            .expect("the built binary starts")
    }

    /// True when the run made the directory `~/.aws`.
    fn aws_directory_exists(&self) -> bool {
        self.home.join(AWS_DIRECTORY).exists()
    }
}

/// What the run wrote to standard error.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_mosh_session_reads_the_credentials_through_pbpaste() {
    let session = Session::new();
    session.shims.hold(
        "AWS_ACCESS_KEY_ID=AKIAEXAMPLE\nAWS_SECRET_ACCESS_KEY=example\nNOT_A_SESSION_TOKEN=example\n",
    );

    let output = session.run();

    let stderr = stderr(&output);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains(NO_SESSION_TOKEN), "stderr: {stderr}");
    assert!(!stderr.contains(READ_FAILURE), "stderr: {stderr}");
    assert!(!stderr.contains(OPEN_FAILURE), "stderr: {stderr}");
    assert!(!session.aws_directory_exists());
}

#[test]
fn a_refused_paste_in_a_mosh_session_names_the_shim() {
    const REFUSAL: &str = "mosh-open: did not paste: the client allows no paste";
    let session = Session::new();
    session.shims.refuse_paste(REFUSAL);

    let output = session.run();

    let stderr = stderr(&output);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains(READ_FAILURE), "stderr: {stderr}");
    assert!(stderr.contains(REFUSAL), "stderr: {stderr}");
    assert!(!session.aws_directory_exists());
}
