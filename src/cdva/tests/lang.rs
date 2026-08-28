//! The language table, read through the public API.
//!
//! The table is data, so the test is a table too: every extension of every
//! language, and every whole file name, asserted one row at a time. A language
//! that loses its rows in a later slice fails here rather than reporting a tree
//! with no files in it.

use cdva::Language;
use std::path::Path;

/// Reads the language of a path written as a string, which is what every case
/// below has.
fn detect(path: &str) -> Option<Language> {
    Language::from_path(Path::new(path))
}

/// Every extension the tool knows, and the language it names.
///
/// Written in lower case here. The case-insensitive reading has a test of its
/// own below.
const EXTENSIONS: &[(&str, Language)] = &[
    ("rs", Language::Rust),
    ("go", Language::Go),
    ("py", Language::Python),
    ("pyi", Language::Python),
    ("js", Language::JavaScript),
    ("jsx", Language::JavaScript),
    ("mjs", Language::JavaScript),
    ("cjs", Language::JavaScript),
    ("ts", Language::TypeScript),
    ("mts", Language::TypeScript),
    ("cts", Language::TypeScript),
    ("tsx", Language::Tsx),
    ("java", Language::Java),
    ("kt", Language::Kotlin),
    ("kts", Language::Kotlin),
    ("cs", Language::CSharp),
    ("rb", Language::Ruby),
    ("rake", Language::Ruby),
    ("gemspec", Language::Ruby),
    ("swift", Language::Swift),
    ("ex", Language::Elixir),
    ("exs", Language::Elixir),
    ("zig", Language::Zig),
    ("c", Language::C),
    ("h", Language::CHeader),
    ("cc", Language::Cpp),
    ("cpp", Language::Cpp),
    ("cxx", Language::Cpp),
    ("hh", Language::CppHeader),
    ("hpp", Language::CppHeader),
    ("hxx", Language::CppHeader),
    ("php", Language::Php),
    ("sh", Language::Shell),
    ("bash", Language::Shell),
    ("zsh", Language::Shell),
    ("bats", Language::Shell),
    ("ps1", Language::PowerShell),
    ("psm1", Language::PowerShell),
    ("psd1", Language::PowerShell),
    ("bat", Language::Batch),
    ("cmd", Language::Batch),
    ("html", Language::Html),
    ("htm", Language::Html),
    ("xml", Language::Xml),
    ("xsd", Language::Xml),
    ("xsl", Language::Xml),
    ("css", Language::Css),
    ("scss", Language::Scss),
    ("sass", Language::Scss),
    ("json", Language::Json),
    ("yaml", Language::Yaml),
    ("yml", Language::Yaml),
    ("toml", Language::Toml),
    ("ini", Language::Ini),
    ("cfg", Language::Ini),
    ("md", Language::Markdown),
    ("markdown", Language::Markdown),
    ("sql", Language::Sql),
    ("mk", Language::Makefile),
    ("mak", Language::Makefile),
    ("dockerfile", Language::Dockerfile),
    ("lua", Language::Lua),
    ("scala", Language::Scala),
    ("sc", Language::Scala),
    ("hs", Language::Haskell),
    ("nix", Language::Nix),
    ("proto", Language::Protobuf),
    ("graphql", Language::GraphQL),
    ("gql", Language::GraphQL),
    ("pl", Language::Perl),
    ("pm", Language::Perl),
];

/// Every whole file name the tool knows, and the language it names.
const FILE_NAMES: &[(&str, Language)] = &[
    ("Gemfile", Language::Ruby),
    ("Rakefile", Language::Ruby),
    ("Makefile", Language::Makefile),
    ("makefile", Language::Makefile),
    ("GNUmakefile", Language::Makefile),
    ("Dockerfile", Language::Dockerfile),
    ("Containerfile", Language::Dockerfile),
    // A container build file is as often written in lower case as it is in
    // title case, and neither spelling carries an extension for the second rule
    // to read.
    ("dockerfile", Language::Dockerfile),
    ("containerfile", Language::Dockerfile),
];

/// The display name of every language, in the order [`Language::all`] reports.
const NAMES_IN_ORDER: &[&str] = &[
    "Rust",
    "Go",
    "Python",
    "JavaScript",
    "TypeScript",
    "TSX",
    "Java",
    "Kotlin",
    "C#",
    "Ruby",
    "Swift",
    "Elixir",
    "Zig",
    "C",
    "C/C++ Header",
    "C++",
    "C++ Header",
    "PHP",
    "Shell",
    "PowerShell",
    "Batch",
    "HTML",
    "XML",
    "CSS",
    "SCSS",
    "JSON",
    "YAML",
    "TOML",
    "INI",
    "Markdown",
    "SQL",
    "Makefile",
    "Dockerfile",
    "Lua",
    "Scala",
    "Haskell",
    "Nix",
    "Protocol Buffers",
    "GraphQL",
    "Perl",
];

#[test]
fn every_extension_names_its_language() {
    for &(extension, expected) in EXTENSIONS {
        let path = format!("source.{extension}");
        assert_eq!(
            detect(&path),
            Some(expected),
            "{path} should be {}",
            expected.name()
        );
    }
}

#[test]
fn an_extension_is_read_without_regard_to_case() {
    for &(extension, expected) in EXTENSIONS {
        let upper = extension.to_uppercase();
        let path = format!("source.{upper}");
        assert_eq!(
            detect(&path),
            Some(expected),
            "{path} should be {}",
            expected.name()
        );
    }

    assert_eq!(detect("main.RS"), Some(Language::Rust));
    assert_eq!(detect("App.Tsx"), Some(Language::Tsx));
    assert_eq!(detect("build.Dockerfile"), Some(Language::Dockerfile));
}

#[test]
fn every_file_name_names_its_language() {
    for &(file_name, expected) in FILE_NAMES {
        assert_eq!(
            detect(file_name),
            Some(expected),
            "{file_name} should be {}",
            expected.name()
        );
    }
}

#[test]
fn a_whole_file_name_is_read_before_an_extension() {
    // A whole file name comes first, so a name the table knows resolves even
    // though it carries no extension for the second rule to read.
    assert_eq!(detect("Makefile"), Some(Language::Makefile));
    assert_eq!(detect("Rakefile"), Some(Language::Ruby));

    // And the rest of the path never enters either rule: a directory that looks
    // like a file of another language changes nothing.
    assert_eq!(detect("src/main.py/Makefile"), Some(Language::Makefile));
    assert_eq!(detect("Makefile.d/main.rs"), Some(Language::Rust));
}

#[test]
fn a_whole_file_name_is_matched_exactly() {
    // The extension rule ignores case; this one does not. `MAKEFILE` is a
    // different name, and no extension rule reads it either.
    assert_eq!(detect("MAKEFILE"), None);
    assert_eq!(detect("gemfile"), None);
    assert_eq!(detect("RAKEFILE"), None);
    assert_eq!(detect("DOCKERFILE"), None);
    assert_eq!(detect("CONTAINERFILE"), None);
}

#[test]
fn a_directory_in_the_path_does_not_name_the_language() {
    assert_eq!(detect("a/b/c/main.rs"), Some(Language::Rust));
    assert_eq!(detect("/absolute/path/to/lib.go"), Some(Language::Go));
    assert_eq!(detect("./relative/App.tsx"), Some(Language::Tsx));
}

#[test]
fn an_unknown_extension_is_not_counted() {
    assert_eq!(detect("mystery.qqq"), None);
    assert_eq!(detect("archive.tar.gz"), None);
    assert_eq!(detect("photo.png"), None);
}

#[test]
fn a_file_with_no_extension_is_not_counted() {
    assert_eq!(detect("README"), None);
    assert_eq!(detect("LICENSE"), None);
    assert_eq!(detect("src/bin/some-program"), None);
}

#[test]
fn a_dot_file_carries_no_extension() {
    // `Path::extension` reads nothing from a leading dot, so a dot file is a
    // file with a name and no extension.
    assert_eq!(detect(".gitignore"), None);
    assert_eq!(detect(".rs"), None);
}

#[test]
fn a_path_with_no_file_name_is_not_counted() {
    assert_eq!(detect(".."), None);
    assert_eq!(detect("/"), None);
    assert_eq!(detect(""), None);
}

#[test]
fn all_reports_every_language_once_in_a_stable_order() {
    let names: Vec<&str> = Language::all().iter().map(|l| l.name()).collect();
    assert_eq!(names, NAMES_IN_ORDER);
}

#[test]
fn every_language_has_a_distinct_display_name() {
    let mut names: Vec<&str> = Language::all().iter().map(|l| l.name()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "two languages share a display name");
}

#[test]
fn every_language_is_reachable_from_a_path() {
    for &language in Language::all() {
        let found = EXTENSIONS
            .iter()
            .chain(FILE_NAMES.iter())
            .any(|&(_, candidate)| candidate == language);
        assert!(
            found,
            "{} has no extension and no file name in the table",
            language.name()
        );
    }
}
