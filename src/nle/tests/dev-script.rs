//! Guard: the behavior of `scripts/dev.ts` that a person only sees when it goes wrong.
//!
//! Two things it must get right:
//!
//! - `dx` (the Dioxus CLI) and `portplz` are per-account installs. On a shared
//!   machine the account at the keyboard is not always the account that owns
//!   the checkout, and the raw failure reads as "command not found", which
//!   names neither the account nor the repair.
//! - `dx serve` must run in `src/nle` on the port that `portplz` chose, and the
//!   script must give back the exit status of `dx serve`.
//!
//! Each test runs the script with shims on `PATH` instead of the real tools.
//! The script runs under the real `node`, which strips the types itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

/// Directories that hold the shell and the core utilities the shims need.
const SYSTEM_PATH: &str = "/usr/bin:/bin";

/// The flag that silences the warning about a package.json with no `type`.
const NODE_QUIET_FLAG: &str = "--disable-warning=MODULE_TYPELESS_PACKAGE_JSON";

/// The line that must open the script, so that it runs directly.
const SHEBANG: &str = "#!/usr/bin/env -S pnpx tsx";

/// The port that the `portplz` shim answers by default.
const DEFAULT_PORT: &str = "34567";

/// The one file that every shim appends to.
const LOG_FILE: &str = "calls.log";

/// The text of the message about a missing `dx`.
const NO_DX: &str = "dx is not on the PATH of the account";

/// The text of the message about a missing `portplz`.
const NO_PORTPLZ: &str = "portplz is not on the PATH of the account";

/// The one command that the message about a missing `dx` must give, alone on its line.
const DX_INSTALL_COMMAND: &str = "cargo install dioxus-cli";

/// A shim environment: a directory of fake tools plus the variables that steer them.
struct Shims {
    dir: PathBuf,
    port_answer: String,
    version_exit: i32,
    serve_exit: i32,
}

impl Shims {
    /// Build a shim directory name for a test.
    fn new(label: &str) -> Self {
        Self {
            dir: unique_dir(label),
            port_answer: String::from(DEFAULT_PORT),
            version_exit: 0,
            serve_exit: 0,
        }
    }

    /// Make `portplz` print this text instead of a port.
    fn with_port_answer(mut self, answer: &str) -> Self {
        self.port_answer = String::from(answer);
        self
    }

    /// Make `dx --version` exit with this status.
    fn with_version_exit(mut self, code: i32) -> Self {
        self.version_exit = code;
        self
    }

    /// Make `dx` exit with this status for any call but `dx --version`.
    fn with_serve_exit(mut self, code: i32) -> Self {
        self.serve_exit = code;
        self
    }

    /// Write the `dx` shim and the `portplz` shim when asked.
    fn install(&self, install_dx: bool, install_portplz: bool) {
        let log = self.dir.join(LOG_FILE);
        let log = log.display();
        if install_dx {
            write_shim(
                &self.dir.join("dx"),
                &format!(
                    "#!/bin/sh\n\
                     if [ \"$1\" = \"--version\" ]; then echo \"dioxus 0.7.10\"; exit {}; fi\n\
                     echo \"dx $* @ $(pwd -P)\" >> \"{log}\"\n\
                     exit {}\n",
                    self.version_exit, self.serve_exit
                ),
            );
        }
        if install_portplz {
            write_shim(
                &self.dir.join("portplz"),
                &format!(
                    "#!/bin/sh\necho \"portplz @ $(pwd -P)\" >> \"{log}\"\necho '{}'\n",
                    self.port_answer
                ),
            );
        }
    }

    /// Write a `pnpm` shim that logs each call, to prove that the script never runs it.
    fn install_logging_pnpm(&self) {
        let log = self.dir.join(LOG_FILE);
        write_shim(
            &self.dir.join("pnpm"),
            &format!(
                "#!/bin/sh\necho \"pnpm $* @ $(pwd -P)\" >> \"{}\"\nexit 0\n",
                log.display()
            ),
        );
    }

    /// Run the real script with only these shims and the system utilities on `PATH`.
    fn run(&self) -> Output {
        run_script(
            &real_script(),
            &format!("{}:{SYSTEM_PATH}", self.dir.display()),
        )
    }

    /// The lines that the shims logged, in call order.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join(LOG_FILE))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

impl Drop for Shims {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).ok();
    }
}

/// Build a temporary directory whose name no concurrent test run can collide with.
fn unique_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nle-{label}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("can create the temporary directory");
    dir
}

/// Write a shim and give it the execute bit.
///
/// On Unix, the writer of `gitscratch` writes the file from a child process.
/// Thus this process holds no write descriptor that a fork on another test
/// thread can copy, and Linux does not refuse the run of the shim (`ETXTBSY`).
fn write_shim(path: &Path, body: &str) {
    #[cfg(unix)]
    gitscratch::testing::executable::write_executable(path, body)
        .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    #[cfg(not(unix))]
    fs::write(path, body).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// The directory of the nle crate.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The script under test.
fn real_script() -> PathBuf {
    crate_dir().join("scripts").join("dev.ts")
}

/// Find the real `node` on the `PATH` of this test process.
fn node() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("node"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("these tests need Node.js 22.18 or newer on PATH"))
}

/// Run a copy or the original of `dev.ts` with an empty environment apart from `PATH` and `HOME`.
///
/// The environment is cleared rather than inherited, so nothing the caller
/// exported can redirect the script at a different toolchain.
fn run_script(script: &Path, path_value: &str) -> Output {
    childenv::fixture_command(node())
        .arg(NODE_QUIET_FLAG)
        .arg(script)
        .env_clear()
        .env("PATH", path_value)
        .env("HOME", std::env::temp_dir())
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", script.display()))
}

/// Give the standard error of a run as text.
fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn starts_through_tsx_and_is_executable() {
    let script = real_script();
    let text = fs::read_to_string(&script).expect("can read the script");
    assert_eq!(
        text.lines().next(),
        Some(SHEBANG),
        "the first line must be the shebang"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&script)
            .expect("can stat the script")
            .permissions()
            .mode();
        assert_ne!(
            mode & 0o111,
            0,
            "the script must have an execute bit, mode was {mode:o}"
        );
    }
}

#[test]
fn reports_a_missing_dx() {
    let shims = Shims::new("no-dx");
    shims.install(false, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when dx is missing. stderr was: {stderr}"
    );
    assert!(stderr.contains(NO_DX), "stderr was: {stderr}");
    let install_line = stderr
        .lines()
        .find(|line| line.trim() == DX_INSTALL_COMMAND)
        .unwrap_or_else(|| {
            panic!("the message must give the command that installs dx. stderr was: {stderr}")
        });
    assert_eq!(
        install_line.trim(),
        DX_INSTALL_COMMAND,
        "the command must be the plain one, with no version pin and no flags to retype. \
         stderr was: {stderr}"
    );
}

#[test]
fn reports_a_dx_that_fails_its_version_check() {
    let shims = Shims::new("dx-version").with_version_exit(3);
    shims.install(true, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when dx --version fails. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("dx --version failed with exit status 3"),
        "stderr was: {stderr}"
    );
}

#[test]
fn reports_a_missing_portplz() {
    let shims = Shims::new("no-portplz");
    shims.install(true, false);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when portplz is missing. stderr was: {stderr}"
    );
    assert!(stderr.contains(NO_PORTPLZ), "stderr was: {stderr}");
}

#[test]
fn refuses_an_answer_from_portplz_that_is_not_a_port() {
    let shims = Shims::new("bad-port").with_port_answer("not-a-port");
    shims.install(true, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when portplz gives no port. stderr was: {stderr}"
    );
    assert!(stderr.contains("not-a-port"), "stderr was: {stderr}");
    assert!(stderr.contains("not a port number"), "stderr was: {stderr}");
}

#[test]
fn starts_dx_serve_in_src_nle_on_the_port_of_portplz() {
    let shims = Shims::new("wiring");
    shims.install(true, true);
    shims.install_logging_pnpm();
    let output = shims.run();
    let stderr = stderr_of(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the script must reach the launch. stderr was: {stderr}"
    );

    let calls = shims.calls();
    let nle = fs::canonicalize(crate_dir()).expect("can resolve the crate directory");
    let expected = format!(
        "dx serve --desktop --port {DEFAULT_PORT} @ {}",
        nle.display()
    );
    assert!(
        calls.contains(&expected),
        "the calls must hold the line '{expected}'. The calls were: {calls:?}"
    );
    assert!(
        !calls.iter().any(|line| line.starts_with("pnpm")),
        "the script must run no pnpm command. The calls were: {calls:?}"
    );
}

#[test]
fn forwards_the_exit_status_of_dx_serve() {
    let shims = Shims::new("exit-status").with_serve_exit(7);
    shims.install(true, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(7),
        "the script must exit with the status of dx serve. stderr was: {stderr}"
    );
}

#[test]
fn the_pure_logic_passes_its_unit_tests() {
    let tests = crate_dir().join("scripts").join("dev-utils.test.ts");
    let output = childenv::fixture_command(node())
        .arg(NODE_QUIET_FLAG)
        .arg("--test")
        .arg(&tests)
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", tests.display()));

    assert!(
        output.status.success(),
        "the unit tests of dev-utils.ts must pass.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
