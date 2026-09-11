//! Starting a child process that cannot reach the terminal gsw is drawing on.
//!
//! Watch mode holds the alternate screen in raw mode, and every child it
//! starts is a program that can try to open the terminal for itself. The rule
//! is the same for each of them, so it is written once here: a `git push`, the
//! shell that answers whether the issue command exists, and the run of that
//! command all pass through this module.

use std::process::Command;

/// Arrange for `command`'s child to run detached from the terminal, so nothing
/// in its process tree can reach the keyboard gsw is reading.
///
/// Each platform names the terminal differently, so each gets its own arm: a
/// session of its own on Unix, no inherited console on Windows. Both deny the
/// same thing — the direct path to the terminal device, the one a closed stdin
/// and captured output streams do not cover, because it bypasses the inherited
/// descriptors entirely.
///
/// The Unix half asks for a new session before the exec. A session leader has
/// no controlling terminal until it deliberately acquires one, and no program
/// git runs does that — so `open("/dev/tty")` returns `ENXIO` for the child,
/// for ssh, and for every credential helper below them. Refused it, OpenSSH
/// sets `use_askpass` and either runs `SSH_ASKPASS` (a GUI prompt, which is
/// fine — it does not touch the pane gsw is drawing on) or gives up at once
/// with a message that reaches the status rows.
#[cfg(unix)]
pub(crate) fn detach_from_terminal(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: the closure runs in the forked child, between `fork` and `exec`,
    // where only async-signal-safe functions may be called. `setsid` is one
    // (POSIX.1-2017, "Signal Concepts"); it is a bare syscall that allocates
    // nothing and takes no lock the parent's other threads could be holding.
    unsafe {
        command.pre_exec(|| {
            // The return value is deliberately dropped. `setsid` fails with
            // EPERM when the caller is already a process group leader, which
            // means a new session was not available — harmless, and not worth
            // failing a push over: the terminal defenses below it still stand,
            // and returning an error here would abort the exec and report a
            // push failure to a user who has done nothing wrong. There is no
            // other failure mode.
            let _ = libc::setsid();
            Ok(())
        });
    }
}

/// `CreateProcess`'s `DETACHED_PROCESS`: the new process does not inherit the
/// console of the process that started it, and Windows will not give it one of
/// its own. Deliberately not combined with `CREATE_NEW_CONSOLE`, which is its
/// opposite and which `CreateProcess` rejects alongside it, and not written as
/// `CREATE_NO_WINDOW`, which only hides a console the child still has and can
/// still read from.
///
/// Spelled out here rather than pulled in from `windows-sys` or `winapi`: it is
/// one integer fixed by the Win32 ABI, and a dependency the whole crate would
/// carry for it is a worse trade than a constant with its value written down.
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// See the Unix half above. Windows has no `/dev/tty`, but it has the same
/// hazard by a different door: a console. OpenSSH for Windows asks for a key
/// passphrase or an unknown-host-key answer by opening `CONIN$` itself, which
/// reaches the console the child inherited no matter what was done to its
/// standard handles — so a closed stdin and captured output streams leave the
/// push free to paint a prompt over gsw's alternate screen and race the event
/// thread for the user's keystrokes, with no timeout to end it.
///
/// [`DETACHED_PROCESS`] closes that door the way `setsid` closes the Unix one.
/// The child inherits no console and cannot be assigned one, so `CONIN$` and
/// `CONOUT$` fail to open for it, for ssh, and for every credential helper
/// below them. Denied the console, OpenSSH does what it does on Unix: it falls
/// back to `SSH_ASKPASS` (a GUI prompt, which does not touch the pane gsw is
/// drawing on) or fails immediately with a message that arrives on the captured
/// stderr and lands in the status rows like any other error. The pipes are
/// unaffected — the flag governs the console, not the standard handles, which
/// the caller has already set.
///
/// **Not covered by any test in this repository.** The Unix half has a runtime
/// test, `the_push_child_cannot_open_the_controlling_terminal`, which plants a
/// fake ssh and asserts the child was refused the terminal. There is no Windows
/// equivalent: no Windows host runs these tests and this repository has no CI,
/// so such a test would be one nobody has ever seen pass or fail. This arm is
/// verified by compiling for `x86_64-pc-windows-msvc` and by nothing else.
#[cfg(windows)]
pub(crate) fn detach_from_terminal(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(DETACHED_PROCESS);
}

/// Neither Unix nor Windows, so there is no terminal this code knows how to
/// take away. The arm exists so the crate still builds for such a target rather
/// than failing to find `detach_from_terminal`; on one, `GIT_TERMINAL_PROMPT=0`
/// and the closed stdin are the whole defense.
#[cfg(not(any(unix, windows)))]
pub(crate) fn detach_from_terminal(_command: &mut Command) {}
