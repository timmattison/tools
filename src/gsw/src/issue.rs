//! Opening the issue that the branch names, from watch mode.
//!
//! The `G` key of watch mode runs one command in the user's own interactive
//! shell. That command is a shell function, so only a shell can find it and
//! only a shell can run it. This module asks the shell both questions.

/// The variable that names the command `G` runs.
pub(crate) const ISSUE_COMMAND_ENV: &str = "GSW_ISSUE_COMMAND";

/// The command `G` runs when the environment names none.
///
/// This repository ships no `ggs`. It is a shell function that the user
/// supplies, and it is the default here because it is the name that the plans
/// of this repository are written with. [`ISSUE_COMMAND_ENV`] names a
/// different one.
const DEFAULT_ISSUE_COMMAND: &str = "ggs";

/// The command that `G` runs.
///
/// A newtype rather than a `String`, because the value holds one rule that
/// every reader of it depends on: it is never empty. An empty name asks the
/// shell about nothing, and it runs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueCommand(String);

impl IssueCommand {
    /// The command that `value` names, or `None` where the feature is off.
    ///
    /// `value` is the value of [`ISSUE_COMMAND_ENV`], which the caller reads.
    /// The environment is process-global state, and this function takes the
    /// value as an argument so a test of it touches no such state.
    ///
    /// An absent value gives [`DEFAULT_ISSUE_COMMAND`]. A value with nothing
    /// but space in it turns the feature off, which is the one way to say "do
    /// not do this at all" on a public repository whose default names one
    /// person's shell function.
    pub(crate) fn new(value: Option<&str>) -> Option<Self> {
        match value {
            None => Some(Self(DEFAULT_ISSUE_COMMAND.to_string())),
            Some(named) => {
                let named = named.trim();
                (!named.is_empty()).then(|| Self(named.to_string()))
            }
        }
    }

    /// The name of the command, which is never empty.
    pub(crate) fn name(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name that `value` resolves to, as a plain string, or `None`.
    fn resolved(value: Option<&str>) -> Option<String> {
        IssueCommand::new(value).map(|command| command.name().to_string())
    }

    #[test]
    fn an_absent_variable_names_the_default_command() {
        assert_eq!(
            resolved(None),
            Some(DEFAULT_ISSUE_COMMAND.to_string()),
            "an unset GSW_ISSUE_COMMAND must give the default name",
        );
    }

    #[test]
    fn a_variable_with_a_name_in_it_names_that_command() {
        assert_eq!(
            resolved(Some("myfunc")),
            Some("myfunc".to_string()),
            "GSW_ISSUE_COMMAND must name the command that G runs",
        );
    }

    #[test]
    fn an_empty_variable_turns_the_feature_off() {
        // The one way to say "do not do this at all". A public repository must
        // not make one person's shell function a constant with no way out.
        assert_eq!(resolved(Some("")), None, "an empty value must turn G off");
        assert_eq!(
            resolved(Some("   ")),
            None,
            "a value with nothing but space in it must turn G off",
        );
    }

    #[test]
    fn the_space_around_a_name_is_dropped() {
        // A variable written in an rc file collects space. The name inside it
        // is still the name.
        assert_eq!(resolved(Some("  ggs  ")), Some("ggs".to_string()));
    }
}
