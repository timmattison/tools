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
//! here does both internally — and the guard test below fails the build if a
//! parse appears anywhere else in the crate.

use crate::Args;
use clap::Parser;
use std::cell::Cell;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, Once, OnceLock};

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
/// environment lock itself, so no caller can forget to, and the guard test
/// below rejects any parse written anywhere else in the crate.
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
    use std::process::Command;

    /// Set in the child so its copy of this test does not fork forever.
    const CHILD_MARKER: &str = "UFA_INHERITED_ENVIRONMENT_CHILD";

    /// A hostile spelling of every `UNIFI_*` variable clap reads.
    ///
    /// `UNIFI_INSECURE=maybe` is the sharpest of them: clap cannot parse it,
    /// so it fails *every* parse in the binary rather than merely changing
    /// what one of them returns.
    const INHERITED: &[(&str, &str)] = &[
        ("UNIFI_URL", "https://inherited.example"),
        ("UNIFI_API_KEY", "inherited-key"),
        ("UNIFI_INSECURE", "maybe"),
        ("UNIFI_SITE_MANAGER_API_KEY", "inherited-cloud-key"),
    ];

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
        let mut command = Command::new(&binary);
        command.env(CHILD_MARKER, "1");
        for (name, value) in INHERITED {
            command.env(name, value);
        }

        let output = command
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

#[cfg(test)]
mod guard_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The directory holding this crate's sources.
    ///
    /// Baked in at compile time, so the scan does not depend on the working
    /// directory the test binary happens to be started from.
    const SOURCE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

    /// The one file allowed to reach clap's argv parsers directly: this one,
    /// which wraps them in the environment lock.
    const HELPER_FILE: &str = "test_support.rs";

    /// clap's argv-taking entry points, which read the environment as well as
    /// the argument vector. `parse_from` is a substring of `try_parse_from`,
    /// so the one needle catches both spellings.
    const CLAP_ARGV_PARSER: &str = "parse_from";

    /// Find every direct use of a clap argv parser in `source`.
    ///
    /// This reads source text rather than exercising the compiled code,
    /// because the point is to catch a call somebody *adds* — which no amount
    /// of testing the calls that exist today can do.
    ///
    /// Comment lines are ignored so that prose may still name the thing it is
    /// warning about.
    ///
    /// # Arguments
    ///
    /// * `source` - The Rust source to scan.
    ///
    /// # Returns
    ///
    /// The 1-based line numbers of the offending calls, in source order.
    fn direct_parse_calls(source: &str) -> Vec<usize> {
        source
            .lines()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && trimmed.contains(CLAP_ARGV_PARSER)
            })
            .map(|(index, _)| index + 1)
            .collect()
    }

    /// Every `.rs` file under this crate's source root except the helper
    /// itself, as (path relative to the root, contents) pairs.
    ///
    /// The tree is walked at run time rather than listed as `include_str!`
    /// arguments so that a *newly added* file is covered without anybody
    /// remembering to register it.
    fn scannable_sources() -> Vec<(String, String)> {
        fn walk(directory: &Path, root: &Path, found: &mut Vec<(String, String)>) {
            let entries = fs::read_dir(directory).unwrap_or_else(|error| {
                panic!("{} must be readable: {error}", directory.display())
            });

            for entry in entries {
                let path = entry.expect("a directory entry must be readable").path();

                if path.is_dir() {
                    walk(&path, root, found);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                if path.file_name().is_some_and(|name| name == HELPER_FILE) {
                    continue;
                }

                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let contents = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{relative} must be readable: {error}"));
                found.push((relative, contents));
            }
        }

        let root = PathBuf::from(SOURCE_ROOT);
        let mut found = Vec::new();
        walk(&root, &root, &mut found);
        found.sort();
        found
    }

    /// No test may parse arguments without the environment lock, which means
    /// no test may call clap's argv parsers itself.
    #[test]
    fn no_test_reaches_clap_argv_parsing_directly() {
        let sources = scannable_sources();

        assert!(
            sources.iter().any(|(path, _)| path == "main.rs"),
            "the scan must actually reach the crate sources, found {:?}",
            sources.iter().map(|(path, _)| path).collect::<Vec<_>>()
        );

        let offenders: Vec<String> = sources
            .iter()
            .flat_map(|(path, source)| {
                direct_parse_calls(source)
                    .into_iter()
                    .map(move |line| format!("{path}:{line}"))
            })
            .collect();

        assert!(
            offenders.is_empty(),
            "these call clap's argv parsers directly, so they read the shared \
             process environment without holding the environment lock and fail \
             at random when another test is mid-`std::env::set_var`: \
             {offenders:?}. Parse through \
             `crate::test_support::parse_args_for_test` instead."
        );
    }

    /// The scan is only worth anything if it can actually fail, so feed it a
    /// call of exactly the shape it is meant to catch.
    #[test]
    fn the_direct_parse_guard_can_fail() {
        let bypassing_source = "\
#[test]
fn a_future_test_somebody_adds() {
    let args = crate::Args::try_parse_from([\"ufa\", \"info\"]).expect(\"parses\");
    assert!(args.url.is_none());
}
";

        assert_eq!(
            direct_parse_calls(bypassing_source),
            vec![3],
            "a direct clap parse must be reported, on the line that makes it"
        );
    }

    /// Prose that names the banned call is not itself a bypass, or the ban
    /// could never be explained in a comment.
    #[test]
    fn the_direct_parse_guard_ignores_comments() {
        let documented_source = "\
/// Never call `Args::try_parse_from` here.
// Not even parse_from on its own.
fn documented() {}
";

        assert!(
            direct_parse_calls(documented_source).is_empty(),
            "a comment naming the call must not be reported as a call"
        );
    }
}
