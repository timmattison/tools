//! Core logic for `bm` (Bulk Move): recursively find files matching a pattern
//! and move them to a destination directory.
//!
//! This crate is split into a small, narrow public surface (a [`run`] entry
//! point plus the pieces it composes) hiding the real work: pattern selection,
//! collision-safe move planning, and a cross-volume move fallback that ordinary
//! `rename(2)` cannot perform.

pub use filewalker::FilterType;

/// Error returned when the user did not specify exactly one search pattern.
#[derive(Debug, thiserror::Error)]
pub enum FilterSelectionError {
    /// Zero, or more than one, of `--suffix`/`--prefix`/`--substring` was given.
    #[error("exactly one of --suffix, --prefix, or --substring must be specified")]
    NotExactlyOne,
}

/// Select the single [`FilterType`] the user asked for.
///
/// Exactly one of `suffix`, `prefix`, or `substring` must be `Some`; anything
/// else is a usage error.
///
/// # Errors
///
/// Returns [`FilterSelectionError::NotExactlyOne`] if the number of supplied
/// patterns is not exactly one.
pub fn select_filter(
    suffix: Option<String>,
    prefix: Option<String>,
    substring: Option<String>,
) -> Result<FilterType, FilterSelectionError> {
    todo!("driven by tests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_filter_returns_suffix_when_only_suffix_given() {
        let filter = select_filter(Some(".mkv".to_string()), None, None).unwrap();
        assert!(matches!(filter, FilterType::Suffix(s) if s == ".mkv"));
    }

    #[test]
    fn select_filter_returns_prefix_when_only_prefix_given() {
        let filter = select_filter(None, Some("IMG_".to_string()), None).unwrap();
        assert!(matches!(filter, FilterType::Prefix(s) if s == "IMG_"));
    }

    #[test]
    fn select_filter_returns_substring_when_only_substring_given() {
        let filter = select_filter(None, None, Some("2024".to_string())).unwrap();
        assert!(matches!(filter, FilterType::Substring(s) if s == "2024"));
    }

    #[test]
    fn select_filter_rejects_when_none_given() {
        assert!(select_filter(None, None, None).is_err());
    }

    #[test]
    fn select_filter_rejects_when_multiple_given() {
        assert!(select_filter(Some(".mkv".to_string()), Some("IMG_".to_string()), None).is_err());
    }
}
