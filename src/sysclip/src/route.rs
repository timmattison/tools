//! The order rule.

use std::ffi::OsString;

use crate::ClipboardError;
use crate::command::Commands;
use crate::native::Native;

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
        self.mosh_open_socket.as_ref().is_some_and(|v| !v.is_empty())
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
