use std::path::PathBuf;

/// Expand a leading `~/` in a path to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// The largest page that a command returns.
pub const MAX_PAGE_SIZE: i64 = 500;

/// Clamp the paging arguments of the frontend. A negative limit means no
/// limit in SQLite and a negative offset means zero, so neither goes through.
/// Returns `(offset, limit)`.
pub fn page_bounds(offset: i64, limit: i64) -> (i64, i64) {
    (offset.max(0), limit.clamp(1, MAX_PAGE_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_bounds_clamps_both_arguments() {
        assert_eq!(page_bounds(-5, -1), (0, 1));
        assert_eq!(page_bounds(3, 0), (3, 1));
        assert_eq!(page_bounds(3, 50), (3, 50));
        assert_eq!(page_bounds(3, 100_000), (3, MAX_PAGE_SIZE));
    }
}
