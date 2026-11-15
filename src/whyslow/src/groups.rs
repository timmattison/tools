//! The groups of processes.
//!
//! A person who finds the cause of a slow Mac does not look for one process.
//! The cause is often the sum of many small processes: 176 Claude Code
//! processes at 1.6% each, or 12 `clang` processes at 20% each. Thus `whyslow`
//! puts each process into one group, and adds the numbers of each group.
//!
//! The tables below name the groups that `whyslow` knows. A process that is in
//! no table is a group of its own, and its name is the name of the group.

/// The names of the compilers, the linkers, and the build tools.
pub const COMPILER_NAMES: [&str; 24] = [
    "clang",
    "clang++",
    "cc",
    "c++",
    "cc1",
    "cc1plus",
    "gcc",
    "g++",
    "ld",
    "ld64",
    "lld",
    "ld.lld",
    "rust-lld",
    "rustc",
    "clippy-driver",
    "cargo",
    "cargo-clippy",
    "sccache",
    "swift-frontend",
    "swiftc",
    "cmake",
    "make",
    "gmake",
    "ninja",
];

/// The start of a name that belongs to the compilers. Cargo names each build
/// script `build-script-<hash>`.
pub const COMPILER_PREFIXES: [&str; 1] = ["build-script-"];

/// The names of the processes of Spotlight.
pub const SPOTLIGHT_NAMES: [&str; 5] = [
    "mds",
    "mds_stores",
    "mdworker",
    "mdworker_shared",
    "corespotlightd",
];

/// The start of a name that belongs to Spotlight.
pub const SPOTLIGHT_PREFIXES: [&str; 1] = ["spotlightknowledged"];

/// The label of the group of the Claude Code processes.
const CLAUDE_CODE_LABEL: &str = "Claude Code";

/// The label of the group of the compilers and the linkers.
const COMPILERS_LABEL: &str = "Compilers and linkers";

/// The label of the group of Spotlight.
const SPOTLIGHT_LABEL: &str = "Spotlight";

/// The group of one process.
///
/// The derived order puts the known groups first, in the order of the
/// variants, and then the named groups in alphabetical order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// Each process of each Claude Code session, and the processes that
    /// support it.
    ClaudeCode,
    /// The compilers, the linkers, and the build tools.
    Compilers,
    /// The processes of Spotlight.
    Spotlight,
    /// Any other process. The group holds the processes of one name.
    Named(String),
}

impl Group {
    /// Gives the text that a person reads for this group: `Claude Code`,
    /// `Compilers and linkers`, `Spotlight`, or the name of the process.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::ClaudeCode => CLAUDE_CODE_LABEL,
            Self::Compilers => COMPILERS_LABEL,
            Self::Spotlight => SPOTLIGHT_LABEL,
            Self::Named(name) => name,
        }
    }
}

/// Gives the group of one process.
///
/// `name` is the name of the executable: the last part of the `comm` column.
/// `claude` is true when `occ` classified the process as a session, an
/// unreadable session, or a support process of Claude Code.
///
/// The rules apply in this order. A Claude Code process goes into
/// [`Group::ClaudeCode`] whatever its name, because `top` names such a process
/// `2.1.325`, the file name of its version. Then a compiler name, or a name
/// that starts with a compiler prefix, goes into [`Group::Compilers`]. Then
/// the same rule applies to Spotlight. Any other name is a group of its own.
/// The match of a name is exact and case-sensitive.
#[must_use]
pub fn group_of(name: &str, claude: bool) -> Group {
    if claude {
        Group::ClaudeCode
    } else if in_table(name, &COMPILER_NAMES, &COMPILER_PREFIXES) {
        Group::Compilers
    } else if in_table(name, &SPOTLIGHT_NAMES, &SPOTLIGHT_PREFIXES) {
        Group::Spotlight
    } else {
        Group::Named(name.to_owned())
    }
}

/// Tells whether `name` is one of `names`, or starts with one of `prefixes`.
fn in_table(name: &str, names: &[&str], prefixes: &[&str]) -> bool {
    names.contains(&name) || prefixes.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each name of the compiler table goes into the compilers. The test
    /// reads the table, so a name that someone adds has a test at once.
    #[test]
    fn each_compiler_name_goes_into_the_compilers() {
        for name in COMPILER_NAMES {
            assert_eq!(group_of(name, false), Group::Compilers, "name {name}");
        }
    }

    /// Each name of the Spotlight table goes into Spotlight.
    #[test]
    fn each_spotlight_name_goes_into_spotlight() {
        for name in SPOTLIGHT_NAMES {
            assert_eq!(group_of(name, false), Group::Spotlight, "name {name}");
        }
    }

    /// A name that starts with a prefix of the table goes into the group of
    /// that table. Cargo gives each build script a name with a hash.
    #[test]
    fn a_name_with_a_known_prefix_goes_into_its_group() {
        for prefix in COMPILER_PREFIXES {
            let name = format!("{prefix}0a1b2c3d");
            assert_eq!(group_of(&name, false), Group::Compilers, "name {name}");
        }
        for prefix in SPOTLIGHT_PREFIXES {
            let name = format!("{prefix}d");
            assert_eq!(group_of(&name, false), Group::Spotlight, "name {name}");
        }
    }

    /// A process that occ classified as Claude Code goes into its group.
    /// `top` names such a process `2.1.325`, so the name cannot decide.
    #[test]
    fn a_claude_code_process_goes_into_claude_code() {
        assert_eq!(group_of("2.1.325", true), Group::ClaudeCode);
    }

    /// The Claude Code flag wins over the name. A process that occ names a
    /// part of Claude Code stays in that group, also when its name is the name
    /// of a compiler.
    #[test]
    fn the_claude_code_flag_wins_over_a_compiler_name() {
        assert_eq!(group_of("clang", true), Group::ClaudeCode);
    }

    /// A name that no table holds is a group of its own, with its own name.
    #[test]
    fn an_unknown_name_is_a_group_of_its_own() {
        assert_eq!(group_of("node", false), Group::Named("node".to_owned()));
        let forty: Vec<Group> = (0..40).map(|_| group_of("node", false)).collect();
        assert!(forty.iter().all(|g| g == &Group::Named("node".to_owned())));
    }

    /// No name goes into two groups. The test checks that the two tables
    /// share no name, and that no prefix of one table matches a name of the
    /// other, so the order of the rules never decides.
    #[test]
    fn no_name_belongs_to_two_groups() {
        for name in COMPILER_NAMES {
            assert!(!SPOTLIGHT_NAMES.contains(&name), "name {name}");
            for prefix in SPOTLIGHT_PREFIXES {
                assert!(!name.starts_with(prefix), "name {name}");
            }
        }
        for name in SPOTLIGHT_NAMES {
            for prefix in COMPILER_PREFIXES {
                assert!(!name.starts_with(prefix), "name {name}");
            }
        }
        for compiler in COMPILER_PREFIXES {
            for spotlight in SPOTLIGHT_PREFIXES {
                assert!(!compiler.starts_with(spotlight));
                assert!(!spotlight.starts_with(compiler));
            }
        }
    }

    /// The match is exact and case-sensitive: `Clang` is not `clang`.
    #[test]
    fn the_match_is_case_sensitive() {
        assert_eq!(group_of("Clang", false), Group::Named("Clang".to_owned()));
    }

    /// Each variant has the label that a person reads.
    #[test]
    fn each_group_has_its_label() {
        assert_eq!(Group::ClaudeCode.label(), "Claude Code");
        assert_eq!(Group::Compilers.label(), "Compilers and linkers");
        assert_eq!(Group::Spotlight.label(), "Spotlight");
        assert_eq!(Group::Named("日本語 🎉".to_owned()).label(), "日本語 🎉");
    }
}
