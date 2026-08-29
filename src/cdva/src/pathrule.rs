//! The path rule: the globs that mark a whole file as test material.
//!
//! This is the cheap rule. It reads a path and nothing else, it runs before
//! anything opens the file, and a file it marks never needs a parse. The tree
//! rule of a later slice reads only what this rule leaves [`Unmarked`].
//!
//! # Anchoring
//!
//! A glob is written the way a reader thinks of it — `tests/**`, `*_test.go` —
//! and compiled so it matches at any depth. `tests/**` compiled as written
//! matches `tests/lang.rs` at the root of the walk and nothing else, so it
//! would miss `src/cdva/tests/lang.rs`, which is the common case. Worse, the
//! miss is spelled as a clean production verdict that reads exactly like a
//! correct one. So every pattern that does not already say otherwise is
//! prefixed with `**/` before it is compiled. A pattern that begins with `/` is
//! the escape hatch for a user who means "only at the top": the slash comes off
//! and the rest is compiled as written, anchored to the root.
//!
//! The verdict always names the pattern as it was written, never the rewrite.
//!
//! [`Unmarked`]: PathVerdict::Unmarked

use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::Path;

/// Every glob of the built-in table, in the order the rule reads them.
///
/// The set is deliberately language-agnostic. A directory glob such as
/// `test/**` marks every file under it whatever the language, because a
/// directory named `test` holds test material in any language, and a rule that
/// asked the language first would have to name every language that spells a
/// test directory that way. That is a decision, not an oversight: a `.json`
/// fixture under `testdata/` is test material exactly as a `.go` file there is.
const BUILTIN_TEST_GLOBS: &[&str] = &[
    "*_test.go",
    "tests/**",
    "benches/**",
    "*.test.*",
    "*.spec.*",
    "__tests__/**",
    "__mocks__/**",
    "*.cy.*",
    "e2e/**",
    "test_*.py",
    "*_test.py",
    "conftest.py",
    "src/test/**",
    "*Test.java",
    "*Tests.java",
    "*IT.java",
    "*Test.kt",
    "*Tests.cs",
    "*.Tests/**",
    "spec/**",
    "*_spec.rb",
    "*_test.rb",
    "*_test.c",
    "*_test.cc",
    "*_test.cpp",
    "test/**",
    "Tests/**",
    "*Tests.swift",
    "*_test.exs",
    "*Test.php",
    "*.bats",
    "testdata/**",
    "__snapshots__/**",
    "fixtures/**",
];

/// Why a path landed in a bucket, so `--explain` can name the reason.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PathVerdict {
    /// The glob that marked this path as test material.
    Test(String),
    /// A glob of the user that held this path out of the test bucket.
    Production(String),
    /// No glob matched. The tree rule decides.
    Unmarked,
}

/// One compiled set of globs, beside the text they were written as.
///
/// The two halves are index for index, because [`GlobSet::matches`] answers
/// with the sequence numbers of the globs that matched and the verdict has to
/// name one of them. The compiled glob carries the anchored rewrite and the
/// text carries what the user typed, so the rewrite never reaches a reader.
#[derive(Debug)]
struct GlobGroup {
    set: GlobSet,
    patterns: Vec<String>,
}

impl GlobGroup {
    /// Compiles a group of globs, anchoring each one as the module doc says.
    ///
    /// # Errors
    ///
    /// Returns an error naming the glob that failed to compile.
    fn new<'a>(patterns: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut builder = GlobSetBuilder::new();
        let mut texts = Vec::new();
        for pattern in patterns {
            builder.add(compile(pattern)?);
            texts.push(pattern.to_string());
        }
        let set = builder
            .build()
            .context("the globs of the path rule did not compile into a set")?;
        Ok(Self {
            set,
            patterns: texts,
        })
    }

    /// The text of the first glob of this group that matches a path, where
    /// first means the earliest in the order the group was built.
    ///
    /// [`GlobSet::matches`] hands back the sequence numbers grouped by the
    /// strategy that found each one, not in ascending order, so the lowest one
    /// is taken rather than the first one.
    fn first_match(&self, path: &Path) -> Option<&str> {
        let index = self.set.matches(path).into_iter().min()?;
        self.patterns.get(index).map(String::as_str)
    }
}

/// Rewrites a glob so it reaches every depth of the walk, unless it says
/// otherwise. See the module doc.
fn anchor(pattern: &str) -> String {
    if let Some(rooted) = pattern.strip_prefix('/') {
        rooted.to_string()
    } else if pattern.starts_with("**/") {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    }
}

/// Compiles one glob, anchored, with `*` held inside one path component.
///
/// # Errors
///
/// Returns an error naming the glob that failed to compile.
fn compile(pattern: &str) -> Result<Glob> {
    GlobBuilder::new(&anchor(pattern))
        .literal_separator(true)
        .build()
        .with_context(|| format!("the glob `{pattern}` did not compile"))
}

/// The globs that mark a path, which are the built-in table and the globs the
/// user added on the command line.
#[derive(Debug)]
pub struct PathRules {
    production: GlobGroup,
    user_test: GlobGroup,
    builtin: GlobGroup,
}

impl PathRules {
    /// The built-in table, plus the globs of the user.
    ///
    /// # Errors
    ///
    /// Returns an error naming the glob that failed to compile.
    pub fn new(test_globs: &[String], production_globs: &[String]) -> Result<Self> {
        Ok(Self {
            production: GlobGroup::new(production_globs.iter().map(String::as_str))?,
            user_test: GlobGroup::new(test_globs.iter().map(String::as_str))?,
            builtin: GlobGroup::new(BUILTIN_TEST_GLOBS.iter().copied())?,
        })
    }

    /// The built-in table alone.
    ///
    /// # Panics
    ///
    /// Panics when a glob of the built-in table above does not compile. That is
    /// a bug in the table and not anything a caller can reach; a test compiles
    /// every row of it.
    #[must_use]
    pub fn builtin() -> Self {
        Self::new(&[], &[]).expect("every glob of the built-in table compiles")
    }

    /// The verdict for a path, which must be relative to the root of the walk.
    ///
    /// The groups are read in one order, and the first match wins: a
    /// `--production-glob` of the user, then a `--test-glob` of the user, then
    /// the built-in table. So a glob of the user overrides the table, and a
    /// production glob overrides a test glob — which is what lets a repository
    /// that names its tests some other way say so.
    #[must_use]
    pub fn verdict(&self, path: &Path) -> PathVerdict {
        if let Some(glob) = self.production.first_match(path) {
            return PathVerdict::Production(glob.to_string());
        }
        if let Some(glob) = self.user_test.first_match(path) {
            return PathVerdict::Test(glob.to_string());
        }
        if let Some(glob) = self.builtin.first_match(path) {
            return PathVerdict::Test(glob.to_string());
        }
        PathVerdict::Unmarked
    }

    /// Every built-in glob, for the test that pins the table.
    #[must_use]
    pub const fn builtin_globs() -> &'static [&'static str] {
        BUILTIN_TEST_GLOBS
    }
}
