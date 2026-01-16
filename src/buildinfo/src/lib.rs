//! Build information library for Buffalo Tools.
//!
//! Provides compile-time captured git information for version strings.
//!
//! # Usage
//!
//! ```rust,ignore
//! use buildinfo::version_string;
//! use clap::Parser;
//!
//! #[derive(Parser)]
//! #[command(version = version_string!())]
//! struct Cli {
//!     // ...
//! }
//! ```

pub use const_format::formatcp;

/// Git commit hash captured at build time (7 characters).
pub const GIT_HASH: &str = env!("BUILD_GIT_HASH");

/// Git dirty status captured at build time ("dirty", "clean", or "unknown").
pub const GIT_DIRTY: &str = env!("BUILD_GIT_DIRTY");

/// Creates a version string in the format "0.1.0 (abc1234, clean)".
///
/// This macro must be used instead of a function because `env!("CARGO_PKG_VERSION")`
/// must be evaluated in the calling crate's context to get that crate's version.
#[macro_export]
macro_rules! version_string {
    () => {
        $crate::formatcp!(
            "{} ({}, {})",
            env!("CARGO_PKG_VERSION"),
            $crate::GIT_HASH,
            $crate::GIT_DIRTY
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_hash_not_empty() {
        assert!(!GIT_HASH.is_empty());
    }

    #[test]
    fn test_git_dirty_valid_value() {
        assert!(
            GIT_DIRTY == "dirty" || GIT_DIRTY == "clean" || GIT_DIRTY == "unknown",
            "GIT_DIRTY should be 'dirty', 'clean', or 'unknown', got: {}",
            GIT_DIRTY
        );
    }

    #[test]
    fn test_version_string_format() {
        let version = version_string!();
        assert!(version.contains('('));
        assert!(version.contains(')'));
        assert!(version.contains(','));
    }
}
