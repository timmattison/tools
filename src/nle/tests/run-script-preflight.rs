//! Guard: `run-nle.sh` must say which tool is missing, and for which account.
//!
//! The Rust toolchain and the Tauri CLI are per-account installs. On a shared
//! machine the account at the keyboard is not always the account that owns the
//! checkout, and the raw failure then reads as "no such command: `tauri`", which
//! names neither the account nor the repair. These tests run the script with a
//! controlled `PATH` and check that it names both.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directories that hold the shell and the core utilities the script needs.
const SYSTEM_PATH: &str = "/usr/bin:/bin";

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

/// Run `run-nle.sh` with an empty environment apart from `PATH` and `HOME`.
///
/// The environment is cleared rather than inherited, so nothing the caller
/// exported can redirect the script at a different toolchain.
fn run_script(path_value: &str, home: &Path) -> Output {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run-nle.sh");
    Command::new(&script)
        .env_clear()
        .env("PATH", path_value)
        .env("HOME", home)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {}: {e}", script.display()))
}

/// Write an executable `cargo` that reports the `tauri` subcommand as missing.
fn fake_cargo_without_tauri(dir: &Path) {
    let cargo = dir.join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\n\
         if [ \"$1\" = \"tauri\" ]; then\n\
         \x20 echo \"error: no such command: \\`tauri\\`\" >&2\n\
         \x20 exit 101\n\
         fi\n\
         exit 0\n",
    )
    .expect("can write the fake cargo");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))
            .expect("can make the fake cargo executable");
    }
}

#[test]
fn reports_a_missing_rust_toolchain() {
    let home = unique_dir("no-cargo");
    let output = run_script(SYSTEM_PATH, &home);
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

    fs::remove_dir_all(&home).ok();
}

#[test]
fn reports_a_missing_tauri_cli() {
    let home = unique_dir("no-tauri");
    let bin = unique_dir("no-tauri-bin");
    fake_cargo_without_tauri(&bin);

    let output = run_script(&format!("{}:{SYSTEM_PATH}", bin.display()), &home);
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

    fs::remove_dir_all(&home).ok();
    fs::remove_dir_all(&bin).ok();
}
