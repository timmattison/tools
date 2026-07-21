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
//! this module, which serialises the access. Acquiring the lock is not
//! something a caller can forget, because the only parse entry point offered
//! here takes it internally — and the guard test below fails the build if a
//! parse appears anywhere else in the crate.

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
            let entries = fs::read_dir(directory)
                .unwrap_or_else(|error| panic!("{} must be readable: {error}", directory.display()));

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
