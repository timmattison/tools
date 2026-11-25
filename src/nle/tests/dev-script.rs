//! Guard: the behavior of `scripts/dev.ts` that a person only sees when it goes wrong.
//!
//! Three things it must get right:
//!
//! - The Rust toolchain, the Tauri CLI, `portplz` and `pnpm` are per-account
//!   installs. On a shared machine the account at the keyboard is not always the
//!   account that owns the checkout, and the raw failure then reads as
//!   "no such command: `tauri`", which names neither the account nor the repair.
//! - The frontend dependencies must be installed before the dev server starts.
//! - Vite and Tauri must agree on one port. Vite binds it and Tauri loads it, so
//!   a port that reaches only one of them gives a window that never loads.
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

/// The text of the message about a missing `cargo`.
const NO_CARGO: &str = "cargo is not on the PATH of the account";

/// The text of the message about a missing `portplz`.
const NO_PORTPLZ: &str = "portplz is not on the PATH of the account";

/// The text of the message about a missing `pnpm`.
const NO_PNPM: &str = "pnpm is not on the PATH of the account";

/// A shim environment: a directory of fake tools plus the variables that steer them.
struct Shims {
    dir: PathBuf,
    port_answer: String,
    tauri_installed: bool,
    cargo_exit: i32,
}

impl Shims {
    /// Build a shim directory name for a test.
    fn new(label: &str) -> Self {
        Self {
            dir: unique_dir(label),
            port_answer: String::from(DEFAULT_PORT),
            tauri_installed: true,
            cargo_exit: 0,
        }
    }

    /// Make `cargo tauri --version` report the subcommand as missing.
    fn without_tauri_cli(mut self) -> Self {
        self.tauri_installed = false;
        self
    }

    /// Make `portplz` print this text instead of a port.
    fn with_port_answer(mut self, answer: &str) -> Self {
        self.port_answer = String::from(answer);
        self
    }

    /// Make `cargo` exit with this status for any call but `cargo tauri --version`.
    fn with_cargo_exit(mut self, code: i32) -> Self {
        self.cargo_exit = code;
        self
    }

    /// Write the `cargo` shim, and the `pnpm` and `portplz` shims when asked.
    fn install(&self, install_pnpm: bool, install_portplz: bool) {
        let log = self.dir.join(LOG_FILE);
        let log = log.display();
        if install_pnpm {
            write_executable(
                &self.dir.join("pnpm"),
                &format!("#!/bin/sh\necho \"pnpm $* @ $(pwd -P)\" >> \"{log}\"\nexit 0\n"),
            );
        }
        let tauri_branch = if self.tauri_installed {
            "echo \"tauri-cli 2.0.0\"; exit 0"
        } else {
            "echo \"error: no such command: \\`tauri\\`\" >&2; exit 101"
        };
        write_executable(
            &self.dir.join("cargo"),
            &format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"tauri\" ] && [ \"$2\" = \"--version\" ]; then\n\
                 {tauri_branch}\n\
                 fi\n\
                 echo \"cargo $* @ $(pwd -P)\" >> \"{log}\"\n\
                 exit {}\n",
                self.cargo_exit
            ),
        );
        if install_portplz {
            write_executable(
                &self.dir.join("portplz"),
                &format!(
                    "#!/bin/sh\necho \"portplz @ $(pwd -P)\" >> \"{log}\"\necho '{}'\n",
                    self.port_answer
                ),
            );
        }
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

/// Puts a directory back to mode 0o755 when it goes out of scope, also on a panic.
struct RestoreMode(PathBuf);

impl Drop for RestoreMode {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755)).ok();
        }
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

/// Write a file and give it the execute bit.
fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("cannot make {} executable: {e}", path.display()));
    }
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
fn reports_a_missing_rust_toolchain() {
    let output = run_script(&real_script(), SYSTEM_PATH);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when cargo is missing. stderr was: {stderr}"
    );
    assert!(stderr.contains(NO_CARGO), "stderr was: {stderr}");
}

#[test]
fn reports_a_missing_tauri_cli() {
    let shims = Shims::new("no-tauri").without_tauri_cli();
    shims.install(true, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when the Tauri CLI is missing. stderr was: {stderr}"
    );
    let install_line = stderr
        .lines()
        .find(|line| line.contains("cargo install tauri-cli"))
        .unwrap_or_else(|| {
            panic!("the message must give the command that installs the CLI. stderr was: {stderr}")
        });
    assert_eq!(
        install_line.trim(),
        "cargo install tauri-cli",
        "the command must be the plain one, with no version pin and no flags to retype. \
         stderr was: {stderr}"
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
fn reports_a_missing_pnpm() {
    let shims = Shims::new("no-pnpm");
    shims.install(false, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when pnpm is missing. stderr was: {stderr}"
    );
    assert!(stderr.contains(NO_PNPM), "stderr was: {stderr}");
}

#[test]
fn reports_a_checkout_the_account_cannot_write() {
    let shims = Shims::new("read-only");
    shims.install(true, true);

    let root = unique_dir("read-only-checkout");
    let nle = root.join("nle");
    let scripts = nle.join("scripts");
    fs::create_dir_all(&scripts).expect("can create the copy of the scripts directory");
    let source = crate_dir().join("scripts");
    for entry in fs::read_dir(&source).expect("can list the scripts directory") {
        let path = entry.expect("can read an entry").path();
        if path.is_file() {
            let name = path.file_name().expect("a file has a name");
            fs::copy(&path, scripts.join(name)).expect("can copy a script");
        }
    }

    #[cfg(unix)]
    let output = {
        use std::os::unix::fs::PermissionsExt;
        let _restore = RestoreMode(nle.clone());
        fs::set_permissions(&nle, fs::Permissions::from_mode(0o555))
            .expect("can make the directory read-only");
        run_script(
            &scripts.join("dev.ts"),
            &format!("{}:{SYSTEM_PATH}", shims.dir.display()),
        )
    };
    #[cfg(not(unix))]
    let output = run_script(
        &scripts.join("dev.ts"),
        &format!("{}:{SYSTEM_PATH}", shims.dir.display()),
    );
    // The guard has dropped here and restored the mode, so the cleanup can run.
    fs::remove_dir_all(&root).ok();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 in a checkout it cannot write. stderr was: {stderr}"
    );
    assert!(stderr.contains("cannot write to"), "stderr was: {stderr}");
}

#[test]
fn installs_the_frontend_then_hands_the_port_to_vite_and_tauri() {
    let shims = Shims::new("wiring");
    shims.install(true, true);
    let output = shims.run();
    let stderr = stderr_of(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the script must reach the launch. stderr was: {stderr}"
    );

    let calls = shims.calls();
    let nle = fs::canonicalize(crate_dir()).expect("can resolve the crate directory");
    let frontend = nle.join("frontend");
    let install_at = calls
        .iter()
        .position(|line| *line == format!("pnpm install @ {}", frontend.display()))
        .unwrap_or_else(|| {
            panic!("pnpm install must run in the frontend. The calls were: {calls:?}")
        });
    let dev_at = calls
        .iter()
        .position(|line| line.starts_with("cargo tauri dev"))
        .unwrap_or_else(|| {
            panic!("the script must start cargo tauri dev. The calls were: {calls:?}")
        });
    assert!(
        install_at < dev_at,
        "the install must come before the launch. The calls were: {calls:?}"
    );
    let dev = &calls[dev_at];
    assert!(
        dev.ends_with(&format!("@ {}", nle.display())),
        "cargo tauri dev must run in src/nle. It ran: {dev}"
    );
    assert!(
        dev.contains(&format!("http://localhost:{DEFAULT_PORT}")),
        "Tauri must load the port portplz chose. It ran: {dev}"
    );
    assert!(
        dev.contains(&format!("--port {DEFAULT_PORT} --strictPort")),
        "Vite must bind the port portplz chose. It ran: {dev}"
    );
}

#[test]
fn forwards_the_exit_status_of_tauri_dev() {
    let shims = Shims::new("exit-status").with_cargo_exit(7);
    shims.install(true, true);
    let output = shims.run();
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(7),
        "the script must exit with the status of cargo tauri dev. stderr was: {stderr}"
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
