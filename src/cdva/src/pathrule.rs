//! The path rule: the globs that mark a whole file as test material.
//!
//! This is the cheap rule. It reads a path and nothing else, it runs before
//! anything opens the file, and a file it marks never needs a parse. The tree
//! rule of a later slice reads only what this rule leaves [`Unmarked`].
//!
//! [`Unmarked`]: PathVerdict::Unmarked

use anyhow::Result;
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

/// The globs that mark a path, which are the built-in table and the globs the
/// user added on the command line.
#[derive(Debug)]
pub struct PathRules {
    _private: (),
}

impl PathRules {
    /// The built-in table, plus the globs of the user.
    ///
    /// # Errors
    ///
    /// Returns an error naming the glob that failed to compile.
    pub fn new(test_globs: &[String], production_globs: &[String]) -> Result<Self> {
        let _ = (test_globs, production_globs);
        Ok(Self { _private: () })
    }

    /// The built-in table alone.
    #[must_use]
    pub fn builtin() -> Self {
        Self { _private: () }
    }

    /// The verdict for a path, which must be relative to the root of the walk.
    #[must_use]
    pub fn verdict(&self, path: &Path) -> PathVerdict {
        let _ = path;
        PathVerdict::Unmarked
    }

    /// Every built-in glob, for the test that pins the table.
    #[must_use]
    pub const fn builtin_globs() -> &'static [&'static str] {
        BUILTIN_TEST_GLOBS
    }
}
