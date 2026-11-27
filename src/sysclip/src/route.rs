//! The order rule: which clipboard each read and write uses.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::command::{self, Commands};
use crate::native::Native;
use crate::{ClipboardError, COPY_COMMAND, MOSH_OPEN_SOCKET_ENV, PASTE_COMMAND};

/// The two variables of the process that the order rule reads.
pub(crate) struct Environment {
    /// The value of `MOSH_OPEN_SOCKET`.
    pub(crate) mosh_open_socket: Option<OsString>,
    /// The value of `PATH`.
    pub(crate) search_path: Option<OsString>,
}

impl Environment {
    /// The environment of this process.
    pub(crate) fn from_process() -> Self {
        Self {
            mosh_open_socket: std::env::var_os(crate::MOSH_OPEN_SOCKET_ENV),
            search_path: std::env::var_os("PATH"),
        }
    }

    /// Whether the mosh-rs server set its socket. An empty value is not set.
    fn is_mosh_session(&self) -> bool {
        self.mosh_open_socket
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }
}

/// What the router knows about the native clipboard.
enum NativeState<N> {
    /// A mosh session. The native clipboard never opened.
    Skipped,
    /// The native clipboard is open.
    Open(N),
    /// The native clipboard did not open. The error says why.
    Failed(ClipboardError),
}

/// The order rule of the crate. [`Router::open`] holds the full text of it.
pub(crate) struct Router<N> {
    native: NativeState<N>,
    commands: Commands,
}

impl<N: Native> Router<N> {
    /// Chooses the route for `environment`.
    ///
    /// 1. In a mosh session, `opener` never runs. The commands on the search
    ///    path do all of the work. A search path with neither command is an
    ///    error.
    /// 2. Otherwise `opener` runs. When it fails and the search path holds a
    ///    command, the commands are the fallback. When it fails and the search
    ///    path holds no command, its error is the error.
    pub(crate) fn open(
        environment: Environment,
        opener: impl FnOnce() -> Result<N, ClipboardError>,
    ) -> Result<Self, ClipboardError> {
        let mosh = environment.is_mosh_session();
        let commands = Commands::on(environment.search_path);
        let has_command =
            commands.find(COPY_COMMAND).is_some() || commands.find(PASTE_COMMAND).is_some();
        if mosh {
            if !has_command {
                return Err(ClipboardError::new(&format!(
                    "{MOSH_OPEN_SOCKET_ENV} is set, but PATH holds no {COPY_COMMAND} and no {PASTE_COMMAND}"
                )));
            }
            return Ok(Self {
                native: NativeState::Skipped,
                commands,
            });
        }
        let native = match opener() {
            Ok(native) => NativeState::Open(native),
            Err(cause) if has_command => NativeState::Failed(cause),
            Err(cause) => return Err(cause),
        };
        Ok(Self { native, commands })
    }

    /// A router that uses the native clipboard and never a command.
    pub(crate) fn native_only(native: N) -> Self {
        Self {
            native: NativeState::Open(native),
            commands: Commands::none(),
        }
    }

    /// The text on the clipboard.
    pub(crate) fn read(&mut self) -> Result<String, ClipboardError> {
        let cause = match &mut self.native {
            NativeState::Skipped => return self.read_by_command(),
            NativeState::Open(native) => match native.read() {
                Ok(text) => return Ok(text),
                Err(cause) => cause,
            },
            NativeState::Failed(cause) => cause.clone(),
        };
        self.fall_back(cause, PASTE_COMMAND, command::paste)
    }

    /// Puts `text` on the clipboard.
    pub(crate) fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        let cause = match &mut self.native {
            NativeState::Skipped => return self.write_by_command(text),
            NativeState::Open(native) => match native.write(text) {
                Ok(()) => return Ok(()),
                Err(cause) => cause,
            },
            NativeState::Failed(cause) => cause.clone(),
        };
        self.fall_back(cause, COPY_COMMAND, |program| command::copy(program, text))
    }

    /// Reads in a mosh session. A failure of the command is the answer.
    fn read_by_command(&self) -> Result<String, ClipboardError> {
        let program = self.required(PASTE_COMMAND)?;
        command::paste(&program)
    }

    /// Writes in a mosh session. A failure of the command is the answer.
    fn write_by_command(&self, text: &str) -> Result<(), ClipboardError> {
        let program = self.required(COPY_COMMAND)?;
        command::copy(&program, text)
    }

    /// The path of the command `name` in a mosh session.
    fn required(&self, name: &str) -> Result<PathBuf, ClipboardError> {
        self.commands.find(name).ok_or_else(|| {
            ClipboardError::new(&format!(
                "{MOSH_OPEN_SOCKET_ENV} is set, but PATH holds no {name}"
            ))
        })
    }

    /// Runs the command `name` after the native clipboard failed with `cause`.
    ///
    /// With no such command on the search path, `cause` is the error. When the
    /// command fails too, the error names both causes.
    fn fall_back<T>(
        &self,
        cause: ClipboardError,
        name: &str,
        run: impl FnOnce(&Path) -> Result<T, ClipboardError>,
    ) -> Result<T, ClipboardError> {
        let Some(program) = self.commands.find(name) else {
            return Err(cause);
        };
        run(&program).map_err(|second| {
            ClipboardError::new(&format!("{cause}, and {name} failed too: {second}"))
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{COPY_COMMAND, PASTE_COMMAND};
    use gitscratch::testing::executable::write_executable;
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    const COPY_STUB: &str = "#!/bin/sh\nexec /bin/cat > \"${0%/*}/copied\"\n";
    const PASTE_STUB: &str = "#!/bin/sh\nexec /bin/cat \"${0%/*}/held\"\n";
    const REFUSING_STUB: &str =
        "#!/bin/sh\necho 'mosh-open: did not paste: the client refused' >&2\nexit 1\n";
    const REFUSAL: &str = "mosh-open: did not paste: the client refused";

    /// A native clipboard with canned answers and a record of its calls.
    #[derive(Clone, Default)]
    struct Fake {
        read: Option<Result<String, ClipboardError>>,
        write: Option<Result<(), ClipboardError>>,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl Native for Fake {
        fn read(&mut self) -> Result<String, ClipboardError> {
            self.calls.borrow_mut().push("read".to_string());
            self.read.clone().expect("no canned read answer")
        }
        fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
            self.calls.borrow_mut().push(format!("write {text}"));
            self.write.clone().expect("no canned write answer")
        }
    }

    fn failure(text: &str) -> ClipboardError {
        ClipboardError::new(&text)
    }

    fn install(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        write_executable(&path, body).unwrap();
    }

    /// A search path that holds the stubs `(name, body)` and the files that
    /// they use.
    fn shims(stubs: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("held"), "held text").unwrap();
        for (name, body) in stubs {
            install(dir.path(), name, body);
        }
        dir
    }

    fn environment(mosh: Option<&str>, dir: &tempfile::TempDir) -> Environment {
        Environment {
            mosh_open_socket: mosh.map(OsString::from),
            search_path: Some(dir.path().as_os_str().to_owned()),
        }
    }

    fn no_open() -> Result<Fake, ClipboardError> {
        panic!("the native clipboard opened in a mosh session");
    }

    fn both_stubs() -> tempfile::TempDir {
        shims(&[(COPY_COMMAND, COPY_STUB), (PASTE_COMMAND, PASTE_STUB)])
    }

    fn copied(dir: &tempfile::TempDir) -> Option<String> {
        std::fs::read_to_string(dir.path().join("copied")).ok()
    }

    #[test]
    fn a_mosh_session_goes_through_the_commands() {
        let dir = both_stubs();
        let mut router = Router::open(environment(Some("/tmp/sock"), &dir), no_open).unwrap();
        router.write("日本語 café 🎉").unwrap();
        assert_eq!(copied(&dir).as_deref(), Some("日本語 café 🎉"));
        assert_eq!(router.read().unwrap(), "held text");
    }

    #[test]
    fn a_failed_command_in_a_mosh_session_is_the_answer() {
        let dir = shims(&[
            (COPY_COMMAND, REFUSING_STUB),
            (PASTE_COMMAND, REFUSING_STUB),
        ]);
        let mut router = Router::open(environment(Some("/tmp/sock"), &dir), no_open).unwrap();
        assert_eq!(router.read().unwrap_err().to_string(), REFUSAL);
        assert_eq!(router.write("x").unwrap_err().to_string(), REFUSAL);
    }

    #[test]
    fn an_empty_mosh_socket_is_not_a_mosh_session() {
        let dir = both_stubs();
        let fake = Fake {
            read: Some(Ok("native text".to_string())),
            ..Fake::default()
        };
        let mut router = Router::open(environment(Some(""), &dir), || Ok(fake.clone())).unwrap();
        assert_eq!(router.read().unwrap(), "native text");
    }

    #[test]
    fn a_mosh_session_with_no_command_fails_at_open() {
        let dir = shims(&[]);
        let error = Router::<Fake>::open(environment(Some("/tmp/sock"), &dir), no_open)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "MOSH_OPEN_SOCKET is set, but PATH holds no pbcopy and no pbpaste"
        );
    }

    #[test]
    fn a_mosh_session_with_one_command_names_the_missing_one() {
        let dir = shims(&[(COPY_COMMAND, COPY_STUB)]);
        let mut router = Router::open(environment(Some("/tmp/sock"), &dir), no_open).unwrap();
        assert_eq!(
            router.read().unwrap_err().to_string(),
            "MOSH_OPEN_SOCKET is set, but PATH holds no pbpaste"
        );
        router.write("x").unwrap();
        let dir = shims(&[(PASTE_COMMAND, PASTE_STUB)]);
        let mut router = Router::open(environment(Some("/tmp/sock"), &dir), no_open).unwrap();
        assert_eq!(
            router.write("x").unwrap_err().to_string(),
            "MOSH_OPEN_SOCKET is set, but PATH holds no pbcopy"
        );
    }

    #[test]
    fn a_native_success_never_runs_a_command() {
        let dir = both_stubs();
        let fake = Fake {
            read: Some(Ok("native text".to_string())),
            write: Some(Ok(())),
            ..Fake::default()
        };
        let mut router = Router::open(environment(None, &dir), || Ok(fake.clone())).unwrap();
        assert_eq!(router.read().unwrap(), "native text");
        router.write("x").unwrap();
        assert_eq!(copied(&dir), None);
        assert_eq!(*fake.calls.borrow(), ["read", "write x"]);
    }

    #[test]
    fn a_failed_opener_falls_back_to_the_commands() {
        let dir = both_stubs();
        let mut router = Router::<Fake>::open(environment(None, &dir), || {
            Err(failure("The selected clipboard is not supported"))
        })
        .unwrap();
        router.write("x").unwrap();
        assert_eq!(copied(&dir).as_deref(), Some("x"));
        assert_eq!(router.read().unwrap(), "held text");
    }

    #[test]
    fn a_failed_native_operation_falls_back_to_the_commands() {
        let dir = both_stubs();
        let fake = Fake {
            read: Some(Err(failure("read failed"))),
            write: Some(Err(failure("write failed"))),
            ..Fake::default()
        };
        let mut router = Router::open(environment(None, &dir), || Ok(fake.clone())).unwrap();
        assert_eq!(router.read().unwrap(), "held text");
        router.write("y").unwrap();
        assert_eq!(copied(&dir).as_deref(), Some("y"));
        assert_eq!(*fake.calls.borrow(), ["read", "write y"]);
    }

    #[test]
    fn with_no_command_the_native_error_is_the_error() {
        let dir = shims(&[]);
        let error = Router::<Fake>::open(environment(None, &dir), || Err(failure("no display")))
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "no display");
        let fake = Fake {
            read: Some(Err(failure("read failed"))),
            write: Some(Err(failure("write failed"))),
            ..Fake::default()
        };
        let mut router = Router::open(environment(None, &dir), || Ok(fake.clone())).unwrap();
        assert_eq!(router.read().unwrap_err().to_string(), "read failed");
        assert_eq!(router.write("x").unwrap_err().to_string(), "write failed");
    }

    #[test]
    fn when_both_fail_the_error_names_both_causes() {
        let dir = shims(&[
            (COPY_COMMAND, REFUSING_STUB),
            (PASTE_COMMAND, REFUSING_STUB),
        ]);
        let fake = Fake {
            read: Some(Err(failure("read failed."))),
            write: Some(Err(failure("write failed."))),
            ..Fake::default()
        };
        let mut router = Router::open(environment(None, &dir), || Ok(fake.clone())).unwrap();
        assert_eq!(
            router.read().unwrap_err().to_string(),
            format!("read failed, and pbpaste failed too: {REFUSAL}")
        );
        assert_eq!(
            router.write("x").unwrap_err().to_string(),
            format!("write failed, and pbcopy failed too: {REFUSAL}")
        );
        let mut router =
            Router::<Fake>::open(environment(None, &dir), || Err(failure("no display."))).unwrap();
        assert_eq!(
            router.read().unwrap_err().to_string(),
            format!("no display, and pbpaste failed too: {REFUSAL}")
        );
    }

    #[test]
    fn native_only_never_runs_a_command() {
        let dir = both_stubs();
        let fake = Fake {
            read: Some(Err(failure("read failed"))),
            write: Some(Err(failure("write failed"))),
            ..Fake::default()
        };
        let mut router = Router::native_only(fake);
        assert_eq!(router.read().unwrap_err().to_string(), "read failed");
        assert_eq!(router.write("x").unwrap_err().to_string(), "write failed");
        assert_eq!(copied(&dir), None);
    }
}
