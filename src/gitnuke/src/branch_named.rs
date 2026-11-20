//! Helpers that check whether a branch name is safe to delete.
//!
//! `names_branch` finds a branch name in a commit message as a whole token.
//! `ask` puts a yes or no question to the person at the terminal.

/// Tell whether `message` names `branch` as a whole token.
///
/// The character before the match and the character after it must not be a
/// name character. The start and the end of the message count as boundaries.
/// An empty `branch` names nothing.
///
/// A substring match is the defect this check exists to prevent. `issue-70`
/// is a substring of `Branch: issue-704`, so a substring match lets that typo
/// through.
pub(crate) fn names_branch(message: &str, branch: &str) -> bool {
    !branch.is_empty() && message.contains(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_at_end_of_message() {
        assert!(names_branch("Squash\n\nBranch: issue-42", "issue-42"));
        assert!(!names_branch("Squash\n\nBranch: issue-704", "issue-70"));
    }

    #[test]
    fn shorter_number_is_not_a_match() {
        assert!(!names_branch("Branch: issue-70", "issue-7"));
        assert!(names_branch("Branch: issue-7", "issue-7"));
    }

    #[test]
    fn brackets_are_boundaries() {
        assert!(names_branch("fix: thing (issue-42)", "issue-42"));
        assert!(!names_branch("fix: thing (xissue-42)", "issue-42"));
    }

    #[test]
    fn match_at_start_of_message() {
        assert!(names_branch("issue-42 merged", "issue-42"));
        assert!(!names_branch("issue-421 merged", "issue-42"));
    }

    #[test]
    fn crlf_is_a_boundary() {
        assert!(names_branch("Branch: issue-42\r\n", "issue-42"));
        assert!(!names_branch("Branch: issue-42x\r\n", "issue-42"));
    }

    #[test]
    fn multi_byte_neighbours_do_not_panic() {
        assert!(names_branch("日本語 issue-42 🎉", "issue-42"));
        assert!(names_branch("日本語issue-42🎉", "issue-42"));
        assert!(!names_branch("日本語 issue-42.🎉", "issue-42"));
        assert!(!names_branch("café issue-42-é", "issue-42"));
    }

    #[test]
    fn slash_names_match_whole() {
        assert!(names_branch("Branch: feature/login", "feature/login"));
        assert!(!names_branch("Branch: feature/login/v2", "feature/login"));
        assert!(!names_branch("Branch: feature/login", "login"));
    }

    #[test]
    fn a_later_occurrence_counts() {
        assert!(names_branch("Branch: issue-421 and issue-42", "issue-42"));
        assert!(!names_branch("Branch: issue-421 and issue-420", "issue-42"));
        assert!(!names_branch("anything", ""));
    }

    #[test]
    fn nul_separated_messages() {
        assert!(names_branch("first\0Branch: issue-42\0", "issue-42"));
        assert!(!names_branch("first\0Branch: issue-420\0", "issue-42"));
    }
}
