//! The path rule, read through the public API.
//!
//! The rule is a table of globs, so the test is a table too: every built-in
//! glob beside a path it must mark, asserted at the root of the walk and again
//! three directories down. The second half is the one that matters. A glob such
//! as `tests/**` compiled as written matches only at the root, so it would miss
//! `src/cdva/tests/lang.rs` — the common case — and the miss reads as a clean
//! production verdict that nobody can tell from a correct one.

use cdva::{PathRules, PathVerdict};
use std::collections::BTreeSet;
use std::path::Path;

/// Every built-in glob beside one path it must mark.
///
/// Each example is chosen so that the glob beside it is the *first* built-in
/// glob that matches it, because that is the glob the verdict names. A path
/// such as `tests/foo_test.go` would be marked by `*_test.go` instead, and
/// would therefore pin the wrong row.
///
/// The set of globs named here is asserted equal to
/// [`PathRules::builtin_globs`], so a glob added to the table without an
/// example shows up as a set difference rather than as silence.
const EXAMPLES: &[(&str, &str)] = &[
    ("*_test.go", "main_test.go"),
    ("tests/**", "tests/helper.rs"),
    ("benches/**", "benches/throughput.rs"),
    ("*.test.*", "app.test.ts"),
    ("*.spec.*", "app.spec.ts"),
    ("__tests__/**", "__tests__/app.js"),
    ("__mocks__/**", "__mocks__/fs.js"),
    ("*.cy.*", "login.cy.ts"),
    ("e2e/**", "e2e/login.ts"),
    ("test_*.py", "test_math.py"),
    ("*_test.py", "math_test.py"),
    ("conftest.py", "conftest.py"),
    ("src/test/**", "src/test/java/app/Main.groovy"),
    ("*Test.java", "AppTest.java"),
    ("*Tests.java", "AppTests.java"),
    ("*IT.java", "AppIT.java"),
    ("*Test.kt", "AppTest.kt"),
    ("*Tests.cs", "AppTests.cs"),
    ("*.Tests/**", "App.Tests/Runner.cs"),
    ("spec/**", "spec/spec_helper.rb"),
    ("*_spec.rb", "user_spec.rb"),
    ("*_test.rb", "user_test.rb"),
    ("*_test.c", "matrix_test.c"),
    ("*_test.cc", "matrix_test.cc"),
    ("*_test.cpp", "matrix_test.cpp"),
    ("test/**", "test/helper.js"),
    ("Tests/**", "Tests/Helper.cs"),
    ("*Tests.swift", "AppTests.swift"),
    ("*_test.exs", "math_test.exs"),
    ("*Test.php", "UserTest.php"),
    ("*.bats", "install.bats"),
    ("testdata/**", "testdata/input.json"),
    ("__snapshots__/**", "__snapshots__/App.snap"),
    ("fixtures/**", "fixtures/sample.json"),
];

/// The directories that the nested half of the anchoring test puts in front of
/// every example. None of them is itself a name the built-in table marks, so a
/// nested example is marked by the same glob as the one at the root.
const NESTING: &str = "a/b/c/";

/// The verdict of the built-in table for a path written as a string, which is
/// what every case below has.
fn builtin_verdict(path: &str) -> PathVerdict {
    PathRules::builtin().verdict(Path::new(path))
}

/// The test verdict naming a glob, which reads better in an assertion than the
/// `to_string` at every call site.
fn marked(glob: &str) -> PathVerdict {
    PathVerdict::Test(glob.to_string())
}

/// Compiles rules from globs written as string slices, which is what the cases
/// below have and what `PathRules::new` does not take.
fn rules(test_globs: &[&str], production_globs: &[&str]) -> PathRules {
    let test: Vec<String> = test_globs.iter().map(|g| (*g).to_string()).collect();
    let production: Vec<String> = production_globs.iter().map(|g| (*g).to_string()).collect();
    PathRules::new(&test, &production).expect("every glob in this file compiles")
}

// ---------------------------------------------------------------------------
// The built-in table, and the anchoring that makes it reach.
// ---------------------------------------------------------------------------

#[test]
fn the_example_table_names_every_built_in_glob() {
    let examples: BTreeSet<&str> = EXAMPLES.iter().map(|(glob, _)| *glob).collect();
    let builtin: BTreeSet<&str> = PathRules::builtin_globs().iter().copied().collect();

    let unexampled: Vec<&&str> = builtin.difference(&examples).collect();
    assert!(
        unexampled.is_empty(),
        "these built-in globs have no example path in this file: {unexampled:?}"
    );

    let unknown: Vec<&&str> = examples.difference(&builtin).collect();
    assert!(
        unknown.is_empty(),
        "these examples name a glob the built-in table does not hold: {unknown:?}"
    );

    assert_eq!(
        EXAMPLES.len(),
        PathRules::builtin_globs().len(),
        "one of the two tables holds a duplicate row"
    );
}

#[test]
fn every_built_in_glob_marks_its_example_at_the_root() {
    for (glob, example) in EXAMPLES {
        assert_eq!(
            builtin_verdict(example),
            marked(glob),
            "the glob `{glob}` must mark `{example}` at the root of the walk"
        );
    }
}

#[test]
fn every_built_in_glob_marks_its_example_three_directories_deep() {
    for (glob, example) in EXAMPLES {
        let nested = format!("{NESTING}{example}");
        assert_eq!(
            builtin_verdict(&nested),
            marked(glob),
            "the glob `{glob}` must mark `{nested}`, which is the common case"
        );
    }
}

// ---------------------------------------------------------------------------
// What the table must leave alone.
// ---------------------------------------------------------------------------

/// Paths that hold production code, in this repository and in three others.
const PRODUCTION_PATHS: &[&str] = &["src/cdva/src/lang.rs", "README.md", "main.go", "lib/foo.rb"];

#[test]
fn a_production_path_is_unmarked() {
    for path in PRODUCTION_PATHS {
        assert_eq!(
            builtin_verdict(path),
            PathVerdict::Unmarked,
            "`{path}` holds production code"
        );
    }
}

/// Paths that hold the letters of a built-in glob without matching it. Each one
/// is a boundary the table would cross if a glob were written loosely.
const NEAR_MISSES: &[&str] = &[
    "contest/foo.rs",
    "latest/foo.rs",
    "my_tests.go",
    "attestation.py",
];

#[test]
fn a_near_miss_is_unmarked() {
    for path in NEAR_MISSES {
        assert_eq!(
            builtin_verdict(path),
            PathVerdict::Unmarked,
            "`{path}` only looks like test material"
        );
    }
}

#[test]
fn a_star_never_crosses_a_directory_separator() {
    // `*_test.go` names a file, not a directory. A `*` that crossed a `/` would
    // let the directory `b_test.go` mark every file under it.
    assert_eq!(
        builtin_verdict("a/b_test.go/c.go"),
        PathVerdict::Unmarked,
        "`*_test.go` must match a file name, not a path that runs through one"
    );

    // `*.test.*` is the discriminating case: the trailing `*` would have to
    // swallow `d/bar.ts` for this path to match.
    assert_eq!(
        builtin_verdict("app.test.d/bar.ts"),
        PathVerdict::Unmarked,
        "the trailing `*` of `*.test.*` must not reach past the directory"
    );
}

// ---------------------------------------------------------------------------
// The globs of the user, and the order they are read in.
// ---------------------------------------------------------------------------

#[test]
fn a_user_test_glob_marks_a_path_the_table_misses() {
    let path = Path::new("src/verification/checks.rs");
    assert_eq!(
        PathRules::builtin().verdict(path),
        PathVerdict::Unmarked,
        "the built-in table must miss this path for the case to mean anything"
    );

    let rules = rules(&["verification/**"], &[]);
    assert_eq!(rules.verdict(path), marked("verification/**"));
}

#[test]
fn a_user_production_glob_beats_a_user_test_glob() {
    let rules = rules(&["verification/**"], &["verification/golden/**"]);

    assert_eq!(
        rules.verdict(Path::new("verification/golden/data.rs")),
        PathVerdict::Production("verification/golden/**".to_string()),
        "a production glob of the user wins over a test glob of the user"
    );
    assert_eq!(
        rules.verdict(Path::new("verification/checks.rs")),
        marked("verification/**"),
        "the test glob still holds everywhere the production glob does not reach"
    );
}

#[test]
fn a_user_production_glob_beats_the_built_in_table() {
    let path = Path::new("tests/support/server.rs");
    assert_eq!(
        PathRules::builtin().verdict(path),
        marked("tests/**"),
        "the built-in table must mark this path for the override to mean anything"
    );

    let rules = rules(&[], &["tests/support/**"]);
    assert_eq!(
        rules.verdict(path),
        PathVerdict::Production("tests/support/**".to_string()),
        "a glob of the user overrides the built-in table"
    );
}

#[test]
fn a_leading_slash_anchors_a_glob_to_the_root() {
    // `qa` is not a name the built-in table knows, so the only verdict either
    // way comes from the glob under test.
    let anchored = rules(&["/qa/**"], &[]);
    assert_eq!(
        anchored.verdict(Path::new("qa/checks.rs")),
        marked("/qa/**"),
        "an anchored glob still matches at the root"
    );
    assert_eq!(
        anchored.verdict(Path::new("src/qa/checks.rs")),
        PathVerdict::Unmarked,
        "an anchored glob must not reach below the root"
    );

    let free = rules(&["qa/**"], &[]);
    assert_eq!(free.verdict(Path::new("qa/checks.rs")), marked("qa/**"));
    assert_eq!(
        free.verdict(Path::new("src/qa/checks.rs")),
        marked("qa/**"),
        "a glob with no leading slash matches at any depth"
    );
}

#[test]
fn the_verdict_names_the_glob_as_the_user_wrote_it() {
    assert_eq!(
        builtin_verdict("src/cdva/tests/lang.rs"),
        marked("tests/**"),
        "the verdict reads back the pattern of the table, not the anchored rewrite of it"
    );

    let anchored = rules(&["/qa/**"], &[]);
    assert_eq!(
        anchored.verdict(Path::new("qa/checks.rs")),
        marked("/qa/**"),
        "the leading slash the user typed stays in the verdict"
    );
}

// ---------------------------------------------------------------------------
// The edges.
// ---------------------------------------------------------------------------

#[test]
fn an_invalid_glob_names_itself_in_the_error() {
    let broken = "src/[unclosed".to_string();

    let error = PathRules::new(&[broken.clone()], &[])
        .expect_err("an unclosed character class is not a glob");
    let message = format!("{error:#}");
    assert!(
        message.contains(&broken),
        "the error must name the glob that failed to compile, and said: {message}"
    );

    let error = PathRules::new(&[], &[broken.clone()])
        .expect_err("a production glob is compiled the same way");
    let message = format!("{error:#}");
    assert!(
        message.contains(&broken),
        "the error must name the glob that failed to compile, and said: {message}"
    );
}

#[test]
fn no_user_globs_is_the_built_in_table() {
    let empty = PathRules::new(&[], &[]).expect("the built-in table compiles");

    for (glob, example) in EXAMPLES {
        assert_eq!(
            empty.verdict(Path::new(example)),
            marked(glob),
            "`PathRules::new(&[], &[])` must read `{example}` as `PathRules::builtin()` does"
        );
    }
    for path in PRODUCTION_PATHS {
        assert_eq!(empty.verdict(Path::new(path)), PathVerdict::Unmarked);
    }
}

#[test]
fn a_path_of_multi_byte_characters_matches_without_a_panic() {
    assert_eq!(
        builtin_verdict("テスト/日本語_test.go"),
        marked("*_test.go"),
        "a directory and a file name outside ASCII must reach the same verdict"
    );
    assert_eq!(builtin_verdict("tests/café.rs"), marked("tests/**"));
    assert_eq!(
        builtin_verdict("src/日本語/café.rs"),
        PathVerdict::Unmarked,
        "and a production path outside ASCII stays production"
    );
}
