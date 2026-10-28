//! Helpers that more than one integration test file of `crap` uses.

/// Whether `field` is a fork id that `crap` generated: a UUID v4 in the
/// lowercase, hyphenated `8-4-4-4-12` form that `claude --session-id` takes.
pub fn is_generated_fork_id(field: &str) -> bool {
    uuid::Uuid::try_parse(field).is_ok_and(|id| {
        id.get_version() == Some(uuid::Version::Random) && id.hyphenated().to_string() == field
    })
}

/// Builds a `Command` for the real `crap` binary with a hermetic environment.
///
/// The command inherits the environment of the test run, except that every
/// variable whose name starts with `XDG_` is removed, and `HOME` is `home`.
/// `crap` writes its fork log under the data directory of the user. On Linux
/// that directory comes from `XDG_DATA_HOME`, so an inherited value would send
/// the log of a test into the real data directory. The function strips by
/// prefix, so a variable that the XDG standard adds later is stripped too.
pub fn crap_command(home: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_crap"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("XDG_") {
            command.env_remove(name);
        }
    }
    command.env("HOME", home);
    command
}
