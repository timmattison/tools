//! The fake commands of a mosh-rs session, for the end-to-end tests of tools
//! that take `sysclip`.
//!
//! In a mosh-rs session `sysclip` runs `pbcopy` and `pbpaste` from `PATH`. It
//! never opens the clipboard of the machine. A child that runs with these fakes
//! thus never touches the clipboard of the person at the keyboard.
//!
//! Take this module with the feature `testing`, as a dev-dependency only. A
//! build without the feature holds none of it.

use std::path::{Path, PathBuf};
use std::process::Command;

use gitscratch::testing::executable::write_executable;

/// The file that the fake commands keep the text in. It sits beside them.
const CLIPBOARD_FILE: &str = "clipboard";

/// The name of the socket that a mosh-rs session names. The fake commands
/// ignore it, so no socket has to exist.
const SOCKET_NAME: &str = "mosh-open.sock";

/// The system directories that stay on `PATH` behind the fakes. The scripts
/// need `sh` and `cat`.
const SYSTEM_PATH: &str = "/usr/bin:/bin";

/// The fake `pbcopy`. It finds its own directory through `${0%/*}`, so no path
/// goes into the script.
const FAKE_PBCOPY: &str = "#!/bin/sh\nexec /bin/cat > \"${0%/*}/clipboard\"\n";

/// The fake `pbpaste`. It prints the file that the fake `pbcopy` writes.
const FAKE_PBPASTE: &str = "#!/bin/sh\nexec /bin/cat \"${0%/*}/clipboard\"\n";

/// The fake `pbcopy` and `pbpaste` of one test, in one directory.
#[derive(Debug)]
pub struct FakeShims {
    /// The directory that holds the fake commands and the clipboard file.
    dir: PathBuf,
}

impl FakeShims {
    /// Writes an executable fake `pbcopy` and `pbpaste` into `dir`.
    ///
    /// The function makes `dir` when it is missing. `pbcopy` keeps its
    /// standard input in the file `clipboard` beside it. `pbpaste` prints that
    /// file.
    ///
    /// # Panics
    ///
    /// Panics when a directory or a file cannot be written. This is a test
    /// double, and a test cannot go on without it.
    #[must_use]
    pub fn install(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).expect("the directory of the fake commands is made");
        let shims = Self {
            dir: dir.to_path_buf(),
        };
        shims.write_command(crate::COPY_COMMAND, FAKE_PBCOPY);
        shims.write_command(crate::PASTE_COMMAND, FAKE_PBPASTE);
        shims
    }

    /// Replaces `pbpaste` with one that refuses.
    ///
    /// The new `pbpaste` writes `message` and a newline to standard error and
    /// exits 1, the way the shim of a client that allows no paste does.
    ///
    /// # Panics
    ///
    /// Panics when the file cannot be written.
    pub fn refuse_paste(&self, message: &str) {
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' {} >&2\nexit 1\n",
            shellquote::shell_quote(message)
        );
        self.write_command(crate::PASTE_COMMAND, &script);
    }

    /// The directory of the fake commands.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file that the fake commands keep the text in.
    #[must_use]
    pub fn clipboard_file(&self) -> PathBuf {
        self.dir.join(CLIPBOARD_FILE)
    }

    /// Puts `text` where the fake `pbpaste` reads it.
    ///
    /// # Panics
    ///
    /// Panics when the file cannot be written.
    pub fn hold(&self, text: &str) {
        std::fs::write(self.clipboard_file(), text).expect("the clipboard file is written");
    }

    /// What the fake `pbcopy` kept.
    ///
    /// # Panics
    ///
    /// Panics when the file is missing or does not hold UTF-8 text.
    #[must_use]
    pub fn held(&self) -> String {
        std::fs::read_to_string(self.clipboard_file()).expect("the clipboard file is read")
    }

    /// A path for `MOSH_OPEN_SOCKET`, inside the directory of the fakes.
    ///
    /// No socket exists there, and none is necessary: `sysclip` opens no
    /// socket.
    #[must_use]
    pub fn socket(&self) -> PathBuf {
        self.dir.join(SOCKET_NAME)
    }

    /// Makes `command` run in a mosh-rs session whose clipboard is these fakes.
    ///
    /// It sets `PATH` to the directory of the fakes followed by the system
    /// directories. It sets [`MOSH_OPEN_SOCKET_ENV`](crate::MOSH_OPEN_SOCKET_ENV)
    /// to [`FakeShims::socket`]. It returns `command`. Use it on a child whose
    /// environment the caller cleared.
    pub fn enter_session<'c>(&self, command: &'c mut Command) -> &'c mut Command {
        let path = format!("{}:{SYSTEM_PATH}", self.dir.display());
        command
            .env("PATH", path)
            .env(crate::MOSH_OPEN_SOCKET_ENV, self.socket())
    }

    /// Writes the executable `name` with the text `script`.
    ///
    /// The command is written with `write_executable`, because a test runs it.
    /// On Linux, a script that the test process held open for writing gives
    /// `ETXTBSY` at its run.
    fn write_command(&self, name: &str, script: &str) {
        let path = self.dir.join(name);
        write_executable(&path, script).expect("the fake command is written");
    }
}
