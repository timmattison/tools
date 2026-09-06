//! Guard: the behavior of `run-nle.sh` that a person only sees when it goes wrong.
//!
//! Two things it must get right:
//!
//! - The Rust toolchain, the Tauri CLI and `portplz` are per-account installs.
//!   On a shared machine the account at the keyboard is not always the account
//!   that owns the checkout, and the raw failure then reads as
//!   "no such command: `tauri`", which names neither the account nor the repair.
//! - Vite and Tauri must agree on one port. Vite binds it and Tauri loads it, so
//!   a port that reaches only one of them gives a window that never loads.
//!
//! Each test runs the script with shims on `PATH` instead of the real tools.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directories that hold the shell and the core utilities the script needs.
const SYSTEM_PATH: &str = "/usr/bin:/bin";

/// The variable the script exports so the Vite config can read the port.
const PORT_VARIABLE: &str = "NLE_DEV_PORT";

/// A shim environment: a directory of fake tools plus the variables that steer them.
struct Shims {
    dir: PathBuf,
    record: PathBuf,
    port_answer: String,
    tauri_installed: bool,
}

impl Shims {
    /// Build a shim directory that holds a working `cargo` and a `portplz`.
    fn new(label: &str) -> Self {
        let dir = unique_dir(label);
        let record = dir.join("cargo-argv.txt");
        Self {
            dir,
            record,
            port_answer: String::from("34567"),
            tauri_installed: true,
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

    /// Write the `cargo` shim, and the `portplz` shim unless `portplz` is dropped.
    fn install(&self, install_portplz: bool) {
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
                 {{\n\
                 echo \"args: $*\"\n\
                 echo \"{PORT_VARIABLE}=${{{PORT_VARIABLE}:-<unset>}}\"\n\
                 }} > \"{}\"\n\
                 exit 0\n",
                self.record.display()
            ),
        );
        if install_portplz {
            write_executable(
                &self.dir.join("portplz"),
                &format!("#!/bin/sh\necho '{}'\n", self.port_answer),
            );
        }
    }

    /// Run the script with only these shims and the system utilities on `PATH`.
    fn run(&self) -> Output {
        run_script(&format!("{}:{SYSTEM_PATH}", self.dir.display()))
    }

    /// Read back what the `cargo` shim was asked to do.
    fn recorded(&self) -> String {
        fs::read_to_string(&self.record).unwrap_or_default()
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

/// Run `run-nle.sh` with an empty environment apart from `PATH` and `HOME`.
///
/// The environment is cleared rather than inherited, so nothing the caller
/// exported can redirect the script at a different toolchain.
fn run_script(path_value: &str) -> Output {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run-nle.sh");
    Command::new(&script)
        .env_clear()
        .env("PATH", path_value)
        .env("HOME", std::env::temp_dir())
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", script.display()))
}

#[test]
fn reports_a_missing_rust_toolchain() {
    let output = run_script(SYSTEM_PATH);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when cargo is missing. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("cargo"),
        "the message must name cargo. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("account"),
        "the message must say the toolchain belongs to one account. stderr was: {stderr}"
    );
}

#[test]
fn reports_a_missing_tauri_cli() {
    let shims = Shims::new("no-tauri").without_tauri_cli();
    shims.install(true);
    let output = shims.run();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when the Tauri CLI is missing. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("cargo install tauri-cli"),
        "the message must give the command that installs the CLI. stderr was: {stderr}"
    );
}

#[test]
fn reports_a_missing_portplz() {
    let shims = Shims::new("no-portplz");
    shims.install(false);
    let output = shims.run();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when portplz is missing. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("portplz"),
        "the message must name portplz. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("account"),
        "the message must say portplz belongs to one account. stderr was: {stderr}"
    );
}

#[test]
fn hands_the_portplz_port_to_both_vite_and_tauri() {
    let shims = Shims::new("port-wiring");
    shims.install(true);
    let output = shims.run();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "the script must reach the launch. stderr was: {stderr}"
    );

    let recorded = shims.recorded();
    assert!(
        recorded.contains("tauri dev"),
        "the script must launch the application. It ran: {recorded}"
    );
    assert!(
        recorded.contains("http://localhost:34567"),
        "Tauri must load the port portplz chose. It ran: {recorded}"
    );
    assert!(
        recorded.contains(&format!("{PORT_VARIABLE}=34567")),
        "Vite must bind the port portplz chose. The environment held: {recorded}"
    );
}

#[test]
fn refuses_an_answer_from_portplz_that_is_not_a_port() {
    let shims = Shims::new("bad-port").with_port_answer("not-a-port");
    shims.install(true);
    let output = shims.run();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the script must stop with exit 1 when portplz gives no port. stderr was: {stderr}"
    );
    assert!(
        stderr.contains("portplz"),
        "the message must name portplz. stderr was: {stderr}"
    );
}
