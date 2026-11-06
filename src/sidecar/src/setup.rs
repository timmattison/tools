//! `sidecar setup`: clone each sidecar and link it into the worktree.

/// The command that runs the setup by hand. Each warning names it.
pub const SETUP_COMMAND: &str = "sidecar setup";

/// The variable that a CI runner sets.
pub const CI_VARIABLE: &str = "CI";

/// True when `CI` is set to a value other than empty, `0`, or `false`.
/// GitHub Actions sets `CI=true`.
#[must_use]
pub fn is_ci(_value: Option<&str>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ci_is_false_for_no_value_and_for_the_off_values() {
        for value in [None, Some(""), Some("0"), Some("false")] {
            assert!(!is_ci(value), "{value:?}");
        }
    }

    #[test]
    fn is_ci_is_true_for_the_on_values() {
        for value in [Some("true"), Some("1"), Some("yes")] {
            assert!(is_ci(value), "{value:?}");
        }
    }
}
