//! The pattern that a file name must match.

use std::ffi::OsStr;
use std::fmt;
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use regex::bytes::{Regex, RegexBuilder};

/// How a pattern treats the case of a letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    /// `a` matches `a` and `A`. This is the default of `ff`.
    Insensitive,
    /// `a` matches `a` only. The `-s` flag selects it.
    Sensitive,
}

impl Case {
    fn is_insensitive(self) -> bool {
        self == Self::Insensitive
    }
}

/// A pattern that `ff` cannot use.
#[derive(Debug)]
pub struct PatternError(String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PatternError {}

/// A compiled pattern. It matches the name of one entry, never a path.
#[derive(Debug)]
pub struct Matcher(Kind);

#[derive(Debug)]
enum Kind {
    Glob(GlobMatcher),
    Regex(Regex),
}

impl Matcher {
    /// Compiles a glob, for example `*.mp4` or `120.mp4`. The glob must match
    /// the whole name.
    ///
    /// # Errors
    ///
    /// Returns [`PatternError`] when the glob is not valid, or when it holds a
    /// `/`. A name never holds a `/`, thus such a glob can never match.
    pub fn glob(pattern: &str, case: Case) -> Result<Self, PatternError> {
        if pattern.contains('/') {
            return Err(PatternError(format!(
                "the glob {pattern:?} holds a '/', but it matches a file name, and a file name holds no '/'"
            )));
        }
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(case.is_insensitive())
            .literal_separator(true)
            .build()
            .map_err(|error| PatternError(format!("the glob {pattern:?} is not valid: {error}")))?;
        Ok(Self(Kind::Glob(glob.compile_matcher())))
    }

    /// Compiles a regular expression. It matches when it finds a match
    /// anywhere in the name. Use `^` and `$` to match the whole name.
    ///
    /// # Errors
    ///
    /// Returns [`PatternError`] when the regular expression is not valid.
    pub fn regex(pattern: &str, case: Case) -> Result<Self, PatternError> {
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(case.is_insensitive())
            .build()
            .map_err(|error| {
                PatternError(format!(
                    "the regular expression {pattern:?} is not valid: {error}"
                ))
            })?;
        Ok(Self(Kind::Regex(regex)))
    }

    /// Tells if `name`, the last component of a path, matches the pattern.
    #[must_use]
    pub fn is_match(&self, name: &OsStr) -> bool {
        match &self.0 {
            Kind::Glob(glob) => glob.is_match(Path::new(name)),
            Kind::Regex(regex) => regex.is_match(name.as_encoded_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Case, Matcher};
    use std::ffi::OsStr;

    fn matches(matcher: &Matcher, name: &str) -> bool {
        matcher.is_match(OsStr::new(name))
    }

    #[test]
    fn a_glob_matches_the_whole_name_in_any_case_by_default() {
        let matcher = Matcher::glob("120.mp4", Case::Insensitive).expect("a valid glob");
        assert!(matches(&matcher, "120.mp4"));
        assert!(matches(&matcher, "120.MP4"));
        assert!(!matches(&matcher, "1120.mp4"));
        assert!(!matches(&matcher, "120.mp4.part"));
    }

    #[test]
    fn a_case_sensitive_glob_matches_only_the_same_case() {
        let matcher = Matcher::glob("*.mp4", Case::Sensitive).expect("a valid glob");
        assert!(matches(&matcher, "clip.mp4"));
        assert!(!matches(&matcher, "clip.MP4"));
    }

    #[test]
    fn a_glob_matches_a_name_with_multi_byte_characters() {
        let matcher = Matcher::glob("日本*.TXT", Case::Insensitive).expect("a valid glob");
        assert!(matches(&matcher, "日本語.txt"));
        assert!(matches(&matcher, "日本🎉café.txt"));
        assert!(!matches(&matcher, "語日本.txt"));
    }

    #[test]
    fn a_glob_that_holds_a_slash_is_refused_because_a_name_holds_none() {
        let error = Matcher::glob("Users/*.mp4", Case::Insensitive)
            .expect_err("a glob with a slash can never match a name");
        assert!(error.to_string().contains('/'), "{error}");
    }

    #[test]
    fn an_invalid_glob_is_refused() {
        assert!(Matcher::glob("[unclosed", Case::Insensitive).is_err());
    }

    #[test]
    fn a_regex_matches_anywhere_in_the_name_in_any_case_by_default() {
        let matcher = Matcher::regex(r"^1\d0\.mp4$", Case::Insensitive).expect("a valid regex");
        assert!(matches(&matcher, "120.MP4"));
        assert!(!matches(&matcher, "x120.mp4"));

        let unanchored = Matcher::regex("mp4", Case::Insensitive).expect("a valid regex");
        assert!(matches(&unanchored, "clip.MP4.part"));
    }

    #[test]
    fn a_case_sensitive_regex_matches_only_the_same_case() {
        let matcher = Matcher::regex("Clip", Case::Sensitive).expect("a valid regex");
        assert!(matches(&matcher, "Clip.mp4"));
        assert!(!matches(&matcher, "clip.mp4"));
    }

    #[test]
    fn an_invalid_regex_is_refused() {
        assert!(Matcher::regex("(unclosed", Case::Insensitive).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_can_match() {
        use std::os::unix::ffi::OsStrExt;
        let name = OsStr::from_bytes(b"clip-\xff.mp4");
        let glob = Matcher::glob("*.mp4", Case::Insensitive).expect("a valid glob");
        let regex = Matcher::regex(r"\.mp4$", Case::Insensitive).expect("a valid regex");
        assert!(glob.is_match(name));
        assert!(regex.is_match(name));
    }
}
