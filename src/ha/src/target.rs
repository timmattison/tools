//! The things `ha` watches: one repository, or every repository of one owner.
//!
//! A target comes from a command line argument or from the config file. It
//! has the form `owner` or `owner/name`. GitHub names are case-insensitive,
//! thus the types here compare, order, and hash on the ASCII lowercase form
//! and keep the case as given for display.

use std::fmt;

/// The name of a GitHub user or organization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerName(String);

/// The name of one repository, with the owner that holds it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoName {
    owner: OwnerName,
    name: String,
}

/// One thing to watch: a repository, or all repositories of an owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// One repository, written `owner/name`.
    Repo(RepoName),
    /// All repositories of one owner, written `owner`.
    Owner(OwnerName),
}

/// The reasons why a text is not a target. Each message quotes the input.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TargetError {
    /// The text has no characters.
    #[error("\"{input}\" is empty. A target is owner or owner/name.")]
    Empty {
        /// The text as given, after the trim.
        input: String,
    },
    /// The text has two or more slashes.
    #[error("\"{input}\" has more than one \"/\". A target is owner or owner/name.")]
    TooManySlashes {
        /// The text as given, after the trim.
        input: String,
    },
    /// A slash has nothing before it or nothing after it.
    #[error("\"{input}\" has an empty part. A target is owner or owner/name.")]
    EmptyPart {
        /// The text as given, after the trim.
        input: String,
    },
    /// A part is `.` or `..`.
    #[error("\"{input}\" has a part that is \".\" or \"..\". A name cannot be a dot.")]
    DotPart {
        /// The text as given, after the trim.
        input: String,
    },
    /// A part holds a character that GitHub names cannot hold.
    #[error(
        "\"{input}\" holds the character {character:?}. A name holds only letters, digits, \"-\", \"_\", and \".\"."
    )]
    BadCharacter {
        /// The text as given, after the trim.
        input: String,
        /// The first character that is not allowed.
        character: char,
    },
    /// An owner was expected, and the text has a slash.
    #[error("\"{input}\" has a \"/\". An owner name has none.")]
    NotOwner {
        /// The text as given, after the trim.
        input: String,
    },
    /// A repository was expected, and the text has no slash.
    #[error("\"{input}\" has no \"/\". A repository is owner/name.")]
    NotRepo {
        /// The text as given, after the trim.
        input: String,
    },
}

impl Target {
    /// Parses `owner` or `owner/name`. Spaces around the text are ignored.
    ///
    /// # Errors
    ///
    /// Returns a [`TargetError`] when the text is empty, has more than one
    /// slash, has an empty or dot-only part, or holds a character that a
    /// GitHub name cannot hold.
    pub fn parse(input: &str) -> Result<Target, TargetError> {
        let input = input.trim();
        let parts: Vec<&str> = input.split('/').collect();
        match parts.as_slice() {
            [owner] => Ok(Target::Owner(OwnerName((*owner).to_owned()))),
            [owner, name] => Ok(Target::Repo(RepoName {
                owner: OwnerName((*owner).to_owned()),
                name: (*name).to_owned(),
            })),
            _ => Err(TargetError::TooManySlashes {
                input: input.to_owned(),
            }),
        }
    }
}

impl RepoName {
    /// Parses `owner/name`. Spaces around the text are ignored.
    ///
    /// # Errors
    ///
    /// Returns a [`TargetError`] when the text is not exactly `owner/name`
    /// with valid parts.
    pub fn parse(_input: &str) -> Result<RepoName, TargetError> {
        Err(TargetError::Empty {
            input: String::new(),
        })
    }

    /// The owner that holds the repository.
    #[must_use]
    pub fn owner(&self) -> &OwnerName {
        &self.owner
    }

    /// The name of the repository, without the owner.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl OwnerName {
    /// Parses an owner name. Spaces around the text are ignored.
    ///
    /// # Errors
    ///
    /// Returns a [`TargetError`] when the text has a slash, or when it is
    /// empty, dot-only, or holds a character that a GitHub name cannot hold.
    pub fn parse(_input: &str) -> Result<OwnerName, TargetError> {
        Err(TargetError::Empty {
            input: String::new(),
        })
    }

    /// The name as given, with its case.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OwnerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for RepoName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Repo(repo) => repo.fmt(f),
            Target::Owner(owner) => owner.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(text: &str) -> OwnerName {
        OwnerName(text.to_owned())
    }

    fn repo(owner_text: &str, name: &str) -> RepoName {
        RepoName {
            owner: owner(owner_text),
            name: name.to_owned(),
        }
    }

    #[test]
    fn an_owner_parses_as_an_owner_target() {
        assert_eq!(
            Target::parse("timmattison"),
            Ok(Target::Owner(owner("timmattison")))
        );
    }

    #[test]
    fn a_repository_parses_with_owner_and_name() {
        let parsed = Target::parse("timmattison/tools").expect("a valid target");
        assert_eq!(parsed, Target::Repo(repo("timmattison", "tools")));
        let Target::Repo(found) = parsed else {
            panic!("expected a repository target");
        };
        assert_eq!(found.owner().as_str(), "timmattison");
        assert_eq!(found.name(), "tools");
    }

    #[test]
    fn spaces_around_the_text_are_ignored() {
        assert_eq!(
            Target::parse("  timmattison/tools\t\n"),
            Ok(Target::Repo(repo("timmattison", "tools")))
        );
        assert_eq!(Target::parse(" abc "), Ok(Target::Owner(owner("abc"))));
    }

    #[test]
    fn display_gives_the_text_as_given() {
        let target = Target::parse("Lock-Free-Labs/nvr-tools").expect("valid");
        assert_eq!(target.to_string(), "Lock-Free-Labs/nvr-tools");
        let target = Target::parse("Lock-Free-Labs").expect("valid");
        assert_eq!(target.to_string(), "Lock-Free-Labs");
    }
}
