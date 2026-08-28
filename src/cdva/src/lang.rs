//! The language table.
//!
//! One list declares every language the tool counts, and everything else in
//! this module is generated from that list: the [`Language`] enum, the display
//! name of each language, the stable order of [`Language::all`], and the
//! extensions and file names that [`Language::from_path`] reads.
//!
//! The list is the single source of truth on purpose. A hand-written enum
//! beside a hand-written table drifts, and the drift is spelled as an absence —
//! a variant that no row mentions is a language the tool silently never
//! detects. Here a variant cannot exist without its row, because the macro
//! writes both from the same line.

use std::path::Path;

/// One row of the language table: a language, the extensions that name it, and
/// the whole file names that name it.
struct Entry {
    language: Language,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
}

/// Declares the language table, and everything derived from it.
///
/// Each line reads `Variant => "Display name", [extensions], [file names];`.
/// Extensions are written in lower case, because [`Language::from_path`]
/// compares them without regard to case. File names are written exactly as they
/// appear on disk, because that comparison is exact.
macro_rules! language_table {
    ($(
        $variant:ident => $name:literal, [$($extension:literal),* $(,)?], [$($file_name:literal),* $(,)?];
    )*) => {
        /// A language that `cdva` counts.
        ///
        /// The order of the variants is the order of the table, which is the
        /// order [`Language::all`] reports and the order the report prints when
        /// it has no other order to follow.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Language {
            $($variant,)*
        }

        impl Language {
            /// The display name of the language, as the report prints it.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Language::$variant => $name,)*
                }
            }
        }

        /// Every language, in the order of the table.
        static ALL: &[Language] = &[$(Language::$variant,)*];

        /// The table itself, in the order of the variants.
        static TABLE: &[Entry] = &[$(
            Entry {
                language: Language::$variant,
                extensions: &[$($extension,)*],
                file_names: &[$($file_name,)*],
            },
        )*];
    };
}

language_table! {
    Rust => "Rust", ["rs"], [];
    Go => "Go", ["go"], [];
    Python => "Python", ["py", "pyi"], [];
    JavaScript => "JavaScript", ["js", "jsx", "mjs", "cjs"], [];
    TypeScript => "TypeScript", ["ts", "mts", "cts"], [];
    Tsx => "TSX", ["tsx"], [];
    Java => "Java", ["java"], [];
    Kotlin => "Kotlin", ["kt", "kts"], [];
    CSharp => "C#", ["cs"], [];
    Ruby => "Ruby", ["rb", "rake", "gemspec"], ["Gemfile", "Rakefile"];
    Swift => "Swift", ["swift"], [];
    Elixir => "Elixir", ["ex", "exs"], [];
    Zig => "Zig", ["zig"], [];
    C => "C", ["c"], [];
    CHeader => "C/C++ Header", ["h"], [];
    Cpp => "C++", ["cc", "cpp", "cxx"], [];
    CppHeader => "C++ Header", ["hh", "hpp", "hxx"], [];
    Php => "PHP", ["php"], [];
    Shell => "Shell", ["sh", "bash", "zsh", "bats"], [];
    PowerShell => "PowerShell", ["ps1", "psm1", "psd1"], [];
    Batch => "Batch", ["bat", "cmd"], [];
    Html => "HTML", ["html", "htm"], [];
    Xml => "XML", ["xml", "xsd", "xsl"], [];
    Css => "CSS", ["css"], [];
    Scss => "SCSS", ["scss", "sass"], [];
    Json => "JSON", ["json"], [];
    Yaml => "YAML", ["yaml", "yml"], [];
    Toml => "TOML", ["toml"], [];
    Ini => "INI", ["ini", "cfg"], [];
    Markdown => "Markdown", ["md", "markdown"], [];
    Sql => "SQL", ["sql"], [];
    Makefile => "Makefile", ["mk", "mak"], ["Makefile", "makefile", "GNUmakefile"];
    Dockerfile => "Dockerfile", ["dockerfile"], ["Dockerfile", "Containerfile"];
    Lua => "Lua", ["lua"], [];
    Scala => "Scala", ["scala", "sc"], [];
    Haskell => "Haskell", ["hs"], [];
    Nix => "Nix", ["nix"], [];
    Protobuf => "Protocol Buffers", ["proto"], [];
    GraphQL => "GraphQL", ["graphql", "gql"], [];
    Perl => "Perl", ["pl", "pm"], [];
}

impl Language {
    /// Every language the tool knows, in a stable order.
    #[must_use]
    pub fn all() -> &'static [Language] {
        ALL
    }

    /// The language of a file, from its extension or its whole file name.
    ///
    /// The whole file name comes first, so `Makefile` is a makefile and not a
    /// file with no extension. That comparison is exact, because a file name is
    /// a name and `MAKEFILE` is a different one. The extension comes second,
    /// and that comparison ignores case, because `.RS` is Rust.
    ///
    /// Returns `None` for a file the tool does not count.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Language> {
        let file_name = path.file_name()?.to_str()?;

        if let Some(entry) = TABLE
            .iter()
            .find(|entry| entry.file_names.contains(&file_name))
        {
            return Some(entry.language);
        }

        let extension = path.extension()?.to_str()?;

        TABLE
            .iter()
            .find(|entry| {
                entry
                    .extensions
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(extension))
            })
            .map(|entry| entry.language)
    }
}
