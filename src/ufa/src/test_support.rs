//! Test-only helpers for anything that touches the *process* environment.
//!
//! `Args::try_parse_from` looks innocent, but clap consults `UNIFI_URL`,
//! `UNIFI_API_KEY`, `UNIFI_INSECURE` and `UNIFI_SITE_MANAGER_API_KEY` on every
//! parse. The process environment is shared by every thread in the test
//! binary, so a test that sets one of those variables and a test that parses
//! arguments are touching the same resource — and one of the environment
//! tests deliberately sets `UNIFI_INSECURE=maybe`, a value clap must reject.
//! Any parse that runs concurrently with it fails for that unrelated reason.
//!
//! Everything that reads or writes the environment therefore goes through
//! this module, which serialises the access and discards whatever `UNIFI_*`
//! settings the binary inherited from the shell that started it. Neither is
//! something a caller can forget, because the only parse entry point offered
//! here does both internally. The `disallowed-methods` ban in `clippy.toml`
//! fails the build if a parse appears anywhere else in the crate.

use crate::Args;
use clap::Parser;
use std::cell::Cell;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, Once, OnceLock};

/// What a debug dump must say in place of a credential.
///
/// Spelled out here rather than read from the production constant it stands
/// for. A test that quotes the value under test proves only that the value
/// equals itself, so this is the expectation and the crate has to meet it.
pub(crate) const REDACTED: &str = "<redacted>";

/// A controller key for a test to hand to something and then look for.
///
/// Long and shaped like nothing else in the crate, so a dump that carries any
/// part of it is unmistakable.
pub(crate) const FAKE_CONTROLLER_KEY: &str = "fake-controller-key-4Kq7Wn2xTb";

/// The same, for the Site Manager credential.
pub(crate) const FAKE_CLOUD_KEY: &str = "fake-cloud-key-9Rd3Vp8sHm";

/// The one lock guarding the process environment for the whole test binary.
fn environment_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

thread_local! {
    /// Whether an [`EnvironmentGuard`] on *this* thread already owns the lock.
    ///
    /// A `std::sync::Mutex` is not reentrant, so a test that sets a variable
    /// (holding the guard for its whole body) and then parses would deadlock
    /// against itself if the parse tried to lock again. Tracking ownership
    /// per thread makes the nested acquisition a no-op instead.
    static HELD_BY_THIS_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// The prefix every setting clap reads for `Args` shares.
///
/// Matching on the prefix rather than on today's four names means a flag
/// somebody adds tomorrow with `env = "UNIFI_SOMETHING"` is covered without
/// anybody remembering to list it here.
const SETTING_PREFIX: &str = "UNIFI_";

/// Drop every `UNIFI_*` setting the test binary inherited from the shell that
/// started it.
///
/// The lock below can serialise what the *tests* write, but an inherited
/// setting is already in place before the first test body runs and stays
/// there for the whole process — so the only way a test can be independent of
/// it is for it not to be there. Tests that want one supply it themselves
/// through [`ScopedVar`], after this has run.
fn discard_inherited_settings() {
    let inherited: Vec<OsString> = std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| name.to_string_lossy().starts_with(SETTING_PREFIX))
        .collect();

    for name in inherited {
        std::env::remove_var(name);
    }
}

/// Exclusive access to the process environment, held for as long as it lives.
///
/// Nesting is allowed: an inner guard taken on a thread that already holds
/// one borrows the outer guard's ownership and releases nothing when dropped.
struct EnvironmentGuard {
    /// The lock itself, or `None` when an enclosing guard already owns it.
    _owned: Option<MutexGuard<'static, ()>>,
    /// Whether dropping this guard hands ownership back.
    owner: bool,
}

impl EnvironmentGuard {
    /// Take the environment lock, blocking until it is free.
    ///
    /// # Returns
    ///
    /// A guard that releases the lock when dropped, unless this thread was
    /// already holding it.
    fn acquire() -> Self {
        if HELD_BY_THIS_THREAD.with(Cell::get) {
            return Self {
                _owned: None,
                owner: false,
            };
        }

        // A test that panics while holding the lock poisons it, which says
        // nothing about the environment itself -- so take it either way
        // rather than turning one failure into a cascade of them.
        let owned = environment_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        HELD_BY_THIS_THREAD.with(|held| held.set(true));

        // The first guard to be taken is the earliest point at which the
        // environment can be touched safely, and no test may observe it
        // before then, so this is where the inherited settings go.
        static DISCARD_INHERITED: Once = Once::new();
        DISCARD_INHERITED.call_once(discard_inherited_settings);

        Self {
            _owned: Some(owned),
            owner: true,
        }
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        if self.owner {
            HELD_BY_THIS_THREAD.with(|held| held.set(false));
        }
    }
}

/// Parse a command line the way `main` does, without racing the environment.
///
/// This is the only way a test may reach clap's argv parsers: it takes the
/// environment lock itself, so no caller can forget to. The
/// `disallowed-methods` ban in `clippy.toml` rejects any parse written
/// anywhere else in the crate.
///
/// # Arguments
///
/// * `argv` - The argument vector, including the program name.
///
/// # Returns
///
/// The parsed arguments, or the error clap raised for them.
#[expect(
    clippy::disallowed_methods,
    reason = "the one sanctioned parse: the environment lock is held above it"
)]
pub(crate) fn parse_args_for_test<I, T>(argv: I) -> Result<Args, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let _environment = EnvironmentGuard::acquire();

    Args::try_parse_from(argv)
}

/// Run `body` with exclusive access to the process environment.
///
/// [`ScopedVar`] covers the test that sets one variable it knows the name of.
/// This covers the test that makes something *else* write into the
/// environment — a `.env` load, say — where the names come from the file
/// rather than from the test, and the test has to read them back and take them
/// out again before the next test can run.
///
/// # Arguments
///
/// * `body` - The work to run while the lock is held.
///
/// # Returns
///
/// Whatever `body` returned.
pub(crate) fn with_environment_lock<T>(body: impl FnOnce() -> T) -> T {
    let _environment = EnvironmentGuard::acquire();

    body()
}

/// The one lock guarding the process umask for the whole test binary.
///
/// The umask belongs to the process, exactly as the environment does, and
/// every file the process creates while it is lowered gets the wider mode. So
/// a test that states a umask and a test that creates a file are touching the
/// same resource, and they get a lock of the same shape.
#[cfg(unix)]
fn umask_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// The process umask, in force for as long as this guard lives.
#[cfg(unix)]
struct UmaskGuard {
    /// The umask that was in force before the guard set its own.
    previous: libc::mode_t,
    /// The lock, held so no other test creates a file under this umask.
    _owned: MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl UmaskGuard {
    /// Take the umask lock and set the umask to `mask`.
    ///
    /// # Arguments
    ///
    /// * `mask` - The umask to put in force.
    ///
    /// # Returns
    ///
    /// A guard that puts the previous umask back, and releases the lock, when
    /// dropped.
    fn set(mask: libc::mode_t) -> Self {
        // A test that panics while holding the lock poisons it, which says
        // nothing about the umask itself -- so take it either way rather than
        // turning one failure into a cascade of them.
        let owned = umask_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // SAFETY: `umask` takes a mode, puts it in force for the process, and
        // hands back the one it replaced. It reads and writes no memory of
        // this program, and it has no failure mode.
        let previous = unsafe { libc::umask(mask) };

        Self {
            previous,
            _owned: owned,
        }
    }
}

#[cfg(unix)]
impl Drop for UmaskGuard {
    fn drop(&mut self) {
        // SAFETY: as above. The restore runs on the unwind path as well, so a
        // failing assertion cannot leave every later file wide open.
        unsafe {
            libc::umask(self.previous);
        }
    }
}

/// Run `body` with the process umask set to `mask`.
///
/// A test of the mode a file is *created* at has to state the umask, because
/// the kernel subtracts the umask from the mode the create asks for. A
/// developer whose shell sets `umask 077` would otherwise watch a file created
/// at 0o666 land at 0o600 and read that as the code doing the right thing.
///
/// # Arguments
///
/// * `mask` - The umask to hold for the body.
/// * `body` - The work to run under it.
///
/// # Returns
///
/// Whatever `body` returned.
#[cfg(unix)]
pub(crate) fn with_umask<T>(mask: libc::mode_t, body: impl FnOnce() -> T) -> T {
    let _umask = UmaskGuard::set(mask);

    body()
}

/// Sets an environment variable for as long as it is held, then removes it —
/// so a failing assertion cannot leak state into the next test.
///
/// Holding one also holds the environment lock, which is what keeps the
/// variable invisible to every other test's parse.
pub(crate) struct ScopedVar {
    name: &'static str,
    _environment: EnvironmentGuard,
}

impl ScopedVar {
    /// Set `name` to `value` until the returned value is dropped.
    ///
    /// # Arguments
    ///
    /// * `name` - The environment variable to set.
    /// * `value` - The value to give it.
    ///
    /// # Returns
    ///
    /// A guard that removes the variable, and releases the environment lock,
    /// when dropped.
    pub(crate) fn set(name: &'static str, value: &str) -> Self {
        let environment = EnvironmentGuard::acquire();
        std::env::set_var(name, value);

        Self {
            name,
            _environment: environment,
        }
    }
}

impl Drop for ScopedVar {
    fn drop(&mut self) {
        std::env::remove_var(self.name);
    }
}

/// The lock only serialises the variables the *tests* set. A variable the
/// test binary **inherited** is set before any test runs and stays set for
/// the whole process, so no amount of locking hides it — and `ufa`'s own
/// developers are exactly the people likely to have `UNIFI_URL` and friends
/// exported in the shell they run `cargo test` from.
#[cfg(test)]
mod inherited_environment_tests {
    use crate::Args;
    use clap::{Arg, Command, CommandFactory};
    use std::collections::BTreeSet;
    use std::process::Command as ChildProcess;

    /// Set in the child so its copy of this test does not fork forever.
    const CHILD_MARKER: &str = "UFA_INHERITED_ENVIRONMENT_CHILD";

    /// A setting no flag of `Args` declares, used to stand in for the flag
    /// somebody adds tomorrow.
    const PROBE_SETTING: &str = "UNIFI_PROBE";

    /// The settings this crate declares today.
    ///
    /// This is a floor and not a list to keep up to date: the derivation must
    /// reach every one of these, and it may reach more, so a flag added
    /// tomorrow raises the floor rather than breaking it.
    ///
    /// It exists because both halves of the comparison below read clap's
    /// metadata. Were the walk of that metadata to break, both halves would go
    /// empty together and agree with each other. `UNIFI_SITE_MANAGER_API_KEY`
    /// is the one that pins the recursion, since `ufa config cloud` declares it
    /// rather than the top-level command.
    const KNOWN_SETTINGS: [&str; 4] = [
        "UNIFI_URL",
        "UNIFI_API_KEY",
        "UNIFI_INSECURE",
        "UNIFI_SITE_MANAGER_API_KEY",
    ];

    /// The value every inherited setting carries.
    ///
    /// One value serves for all of them, and it is the value that made
    /// `UNIFI_INSECURE` the sharpest of the four: clap cannot parse it as a
    /// boolean, so it fails *every* parse in the binary rather than merely
    /// changing what one of them returns. Giving it to a string-valued setting
    /// costs nothing, because no test asks for it, and giving it to the
    /// boolean flag somebody adds tomorrow keeps that flag as sharp as
    /// `UNIFI_INSECURE` is today.
    const HOSTILE_VALUE: &str = "maybe";

    /// The environment a child of this test binary inherits, hostile setting by
    /// hostile setting.
    ///
    /// The names come from `command` rather than from a list beside this
    /// module, so a flag added to `Args` tomorrow reaches the child without
    /// anybody remembering to write it down twice. A list is safe the day it is
    /// written and quietly stops covering the surface after that.
    fn hostile_environment(command: &Command) -> Vec<(String, String)> {
        settings_read_by(command)
            .into_iter()
            .map(|name| (name, HOSTILE_VALUE.to_owned()))
            .collect()
    }

    /// Every environment variable `command` reads, including the ones its
    /// subcommands declare.
    ///
    /// The recursion is not decoration. `UNIFI_SITE_MANAGER_API_KEY` is
    /// declared on `ufa config cloud` rather than on the top-level command, so
    /// a walk of `get_arguments()` alone finds three of the four settings and
    /// calls that the whole set.
    fn settings_read_by(command: &Command) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        collect_settings(command, &mut found);
        found
    }

    /// Add every setting `command` and its subcommands read to `found`.
    fn collect_settings(command: &Command, found: &mut BTreeSet<String>) {
        for argument in command.get_arguments() {
            if let Some(setting) = argument.get_env() {
                found.insert(setting.to_string_lossy().into_owned());
            }
        }
        for subcommand in command.get_subcommands() {
            collect_settings(subcommand, found);
        }
    }

    /// The names of the settings the child inherits.
    fn inherited_names(command: &Command) -> BTreeSet<String> {
        hostile_environment(command)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Re-run the whole suite in a child that inherited a `UNIFI_*`
    /// environment, which is the one thing a test cannot simulate in-process:
    /// by the time any test body runs, an inherited variable has already been
    /// set for the entire process.
    #[test]
    fn the_suite_ignores_the_unifi_variables_it_inherited() {
        if std::env::var_os(CHILD_MARKER).is_some() {
            return;
        }

        let binary = std::env::current_exe().expect("the test binary must be locatable");
        let mut child = ChildProcess::new(&binary);
        child.env(CHILD_MARKER, "1");
        for (name, value) in hostile_environment(&Args::command()) {
            child.env(name, value);
        }

        let output = child
            .output()
            .unwrap_or_else(|error| panic!("{} must be runnable: {error}", binary.display()));

        assert!(
            output.status.success(),
            "the suite must pass with UNIFI_* exported the way a ufa developer's \
             shell exports them, got:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The child must inherit every setting clap reads, so the test above
    /// covers the whole surface rather than the part somebody remembered.
    #[test]
    fn the_child_inherits_every_setting_clap_reads() {
        let inherited = inherited_names(&Args::command());

        for setting in KNOWN_SETTINGS {
            assert!(
                inherited.contains(setting),
                "{setting} is declared on the command, so the child must \
                 inherit it, got {inherited:?}"
            );
        }

        assert_eq!(
            inherited,
            settings_read_by(&Args::command()),
            "the child's environment and clap's own metadata must name the same \
             settings, so nothing clap reads goes uncovered and nothing the \
             child carries is invented here"
        );
    }

    /// The point of the whole arrangement: a flag added to `Args` tomorrow is
    /// covered without anybody editing this module. A hand-written list cannot
    /// do that, and the test above cannot tell the difference, because a list
    /// that is right today is right today either way.
    #[test]
    fn a_setting_no_list_names_is_still_inherited() {
        let with_a_new_flag = Args::command().arg(
            Arg::new("probe")
                .long("probe")
                .env(PROBE_SETTING)
                .value_parser(clap::value_parser!(bool)),
        );

        assert!(
            inherited_names(&with_a_new_flag).contains(PROBE_SETTING),
            "a flag declared on the command must reach the child's environment \
             without being listed by hand, got {:?}",
            inherited_names(&with_a_new_flag)
        );
    }

    /// The environment has to be hostile, not merely present. Every value must
    /// be one the crate's boolean parser rejects, so a flag added tomorrow with
    /// a boolean value breaks every parse in the binary the way
    /// `UNIFI_INSECURE` does today, rather than quietly reading as `false`.
    #[test]
    fn every_inherited_value_defeats_a_boolean_parse() {
        for (name, value) in hostile_environment(&Args::command()) {
            assert!(
                crate::parse_bool_env(&value).is_err(),
                "{name}={value} parses as a boolean, so a boolean flag would \
                 accept it instead of failing on it"
            );
        }
    }
}

#[cfg(test)]
mod lock_tests {
    use super::{parse_args_for_test, ScopedVar};

    /// The whole point of the lock: a variable one test sets is gone again by
    /// the time anybody else can parse, so no parse fails for a reason that
    /// belongs to another test.
    #[test]
    fn a_scoped_variable_is_removed_once_the_lock_is_released() {
        drop(ScopedVar::set("UNIFI_INSECURE", "maybe"));

        parse_args_for_test(["ufa", "devices", "stats", "--all"])
            .expect("a released ScopedVar must leave nothing behind for the next parse");
    }

    /// Setting a variable and then parsing is the shape most of these tests
    /// take, and the lock is not reentrant — so the helper must nest inside a
    /// `ScopedVar` rather than deadlock against it.
    #[test]
    fn parsing_nests_inside_a_scoped_variable_without_deadlocking() {
        let _var = ScopedVar::set("UNIFI_URL", "https://nested.example");

        let args = parse_args_for_test(["ufa", "info"]).expect("ufa info must parse");

        assert_eq!(args.url.as_deref(), Some("https://nested.example"));
    }
}
