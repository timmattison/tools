//! The order rule.

use std::ffi::OsString;

use crate::command::Commands;
use crate::native::Native;
use crate::ClipboardError;

pub(crate) struct Environment {
    pub(crate) mosh_open_socket: Option<OsString>,
    pub(crate) search_path: Option<OsString>,
}

impl Environment {
    pub(crate) fn from_process() -> Self {
        Self {
            mosh_open_socket: std::env::var_os(crate::MOSH_OPEN_SOCKET_ENV),
            search_path: std::env::var_os("PATH"),
        }
    }

    fn is_mosh_session(&self) -> bool {
        self.mosh_open_socket
            .as_ref()
            .is_some_and(|v| !v.is_empty())
    }
}

enum NativeState<N> {
    Skipped,
    Open(N),
    Failed(ClipboardError),
}

pub(crate) struct Router<N> {
    native: NativeState<N>,
    commands: Commands,
}

impl<N: Native> Router<N> {
    pub(crate) fn open(
        environment: Environment,
        _opener: impl FnOnce() -> Result<N, ClipboardError>,
    ) -> Result<Self, ClipboardError> {
        let _ = environment.is_mosh_session();
        let _ = NativeState::<N>::Skipped;
        let _ = NativeState::<N>::Failed(ClipboardError::new(&"x"));
        let _ = Commands::on(environment.search_path);
        Err(ClipboardError::new(&"placeholder"))
    }

    pub(crate) fn native_only(native: N) -> Self {
        Self {
            native: NativeState::Open(native),
            commands: Commands::none(),
        }
    }

    pub(crate) fn read(&mut self) -> Result<String, ClipboardError> {
        let _ = (&self.native, &self.commands);
        Err(ClipboardError::new(&"placeholder"))
    }

    pub(crate) fn write(&mut self, _text: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError::new(&"placeholder"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{COPY_COMMAND, PASTE_COMMAND};
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;
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
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        let dir = shims(&[(COPY_COMMAND, REFUSING_STUB), (PASTE_COMMAND, REFUSING_STUB)]);
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
        let mut router =
            Router::open(environment(Some(""), &dir), || Ok(fake.clone())).unwrap();
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
        let dir = shims(&[(COPY_COMMAND, REFUSING_STUB), (PASTE_COMMAND, REFUSING_STUB)]);
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
        let mut router = Router::<Fake>::open(environment(None, &dir), || {
            Err(failure("no display."))
        })
        .unwrap();
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
