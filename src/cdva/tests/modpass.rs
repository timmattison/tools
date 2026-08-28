//! The second pass: a `#[cfg(test)] mod <name>;` declaration marks the file it
//! names.
//!
//! Every file here is counted from a source held in this file and a path that
//! is never touched on disk, so nothing below reads the tree it was built from
//! and two copies of this file running at once cannot tread on each other. The
//! paths are the paths the walk would have produced, because that is what the
//! pass matches on.
//!
//! # What the resolution has to get right
//!
//! The module directory of a declaring file is *not* always the directory the
//! file lives in. It is that directory for `mod.rs`, `lib.rs`, and `main.rs`,
//! and the directory joined with the file's own stem for every other file. So
//! `src/foo.rs` declaring `mod bar;` names `src/foo/bar.rs` and not
//! `src/bar.rs`, and a rule that read "the same directory" would miss the first
//! and mark the second. Each row of that table is a test below.

use cdva::{
    lines, resolve_test_modules, Counter, Counts, FileCount, Language, PathRules, Rule, Span,
    TreeRules,
};
use std::path::{Path, PathBuf};

/// A file the pass names: production code to look at, and no test node in it,
/// so only a declaration in another file can move it into the test bucket.
const TARGET: &str =
    "// the checks this module holds\nuse super::add;\n\nfn checked() -> i32 {\n    add(1, 2)\n}\n";

/// The rows of [`TARGET`], which is what a span over the whole of it covers.
const TARGET_ROWS: u32 = 6;

/// A library that moves the test code of `module` into another file.
fn declaring(module: &str) -> String {
    format!("pub fn add(a: i32, b: i32) -> i32 {{\n    a + b\n}}\n\n#[cfg(test)]\nmod {module};\n")
}

/// A library holding a plain `mod <module>;`, which is production code.
fn declaring_plainly(module: &str) -> String {
    format!("pub fn add(a: i32, b: i32) -> i32 {{\n    a + b\n}}\n\nmod {module};\n")
}

/// A library that holds its test code itself, in a module with a body.
fn holding_its_tests(module: &str) -> String {
    format!("pub fn add(a: i32, b: i32) -> i32 {{\n    a + b\n}}\n\n#[cfg(test)]\nmod {module} {{\n    #[test]\n    fn adds() {{\n        assert_eq!(super::add(1, 2), 3);\n    }}\n}}\n")
}

/// One counted file, under the built-in globs and the tree rule.
fn count(path: &str, source: &str) -> FileCount {
    count_under(PathRules::builtin(), path, source)
}

/// One counted file, under globs the caller chose.
fn count_under(rules: PathRules, path: &str, source: &str) -> FileCount {
    let counter = Counter::new(rules).with_tree_rules(TreeRules::new());
    let path = PathBuf::from(path);
    counter
        .count_source(&path, &path, source)
        .unwrap_or_else(|| panic!("`{}` is a language the tool counts", path.display()))
}

/// The counted file of this path, which the caller says is there.
fn find<'files>(files: &'files [FileCount], path: &str) -> &'files FileCount {
    files
        .iter()
        .find(|file| file.path.as_path() == Path::new(path))
        .unwrap_or_else(|| panic!("`{path}` is one of the counted files"))
}

/// Asserts that a whole file is test material under one declaration span.
fn assert_marked(file: &FileCount, module: &str) {
    let path = file.path.display();
    assert_eq!(
        file.production,
        Counts::default(),
        "`{path}` holds no production row once the declaration named it"
    );
    assert_eq!(
        file.test,
        file.total(),
        "`{path}`: every row of it is a test row"
    );
    assert!(
        file.test.total() > 0,
        "`{path}` is worth marking only if it holds a row"
    );

    let declarations: Vec<&Span> = file
        .spans
        .iter()
        .filter(|span| matches!(span.rule, Rule::ModDeclaration(_)))
        .collect();
    assert_eq!(
        declarations,
        vec![&Span {
            first_row: 1,
            last_row: u32::try_from(file.total().total()).expect("a fixture of few rows"),
            rule: Rule::ModDeclaration(module.to_string()),
        }],
        "`{path}` carries one span, over the whole file, naming the module"
    );
}

/// Asserts that a file holds no test row at all.
fn assert_wholly_production(file: &FileCount) {
    let path = file.path.display();
    assert_eq!(
        file.test,
        Counts::default(),
        "`{path}` holds no test row: {:?}",
        file.spans
    );
    assert!(
        !file
            .spans
            .iter()
            .any(|span| matches!(span.rule, Rule::ModDeclaration(_))),
        "`{path}` carries no declaration span: {:?}",
        file.spans
    );
}

#[test]
fn the_target_is_production_code_until_a_declaration_names_it() {
    let counted = count("src/tests.rs", TARGET);

    assert_wholly_production(&counted);
    assert_eq!(
        counted.total(),
        lines::count(TARGET, Language::Rust),
        "nothing but the pass can move this file"
    );
    assert_eq!(
        u32::try_from(counted.total().total()).expect("a fixture of few rows"),
        TARGET_ROWS
    );
}

#[test]
fn a_declaration_in_lib_rs_marks_the_file_beside_it() {
    let declaring_source = declaring("tests");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/tests.rs", TARGET),
    ];
    let before = files[0].clone();

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/tests.rs"), "tests");
    assert_eq!(
        find(&files, "src/lib.rs"),
        &before,
        "the declaring file keeps the two rows the tree rule marked, and nothing else"
    );
}

#[test]
fn a_declaration_in_main_rs_marks_the_file_beside_it() {
    let declaring_source = declaring("tests");
    let mut files = vec![
        count("src/main.rs", &declaring_source),
        count("src/tests.rs", TARGET),
    ];

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/tests.rs"), "tests");
}

#[test]
fn a_declaration_in_mod_rs_marks_the_file_in_the_same_directory() {
    let declaring_source = declaring("bar");
    let mut files = vec![
        count("src/foo/mod.rs", &declaring_source),
        count("src/foo/bar.rs", TARGET),
    ];

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/foo/bar.rs"), "bar");
}

#[test]
fn a_declaration_in_a_named_module_marks_the_file_under_its_own_directory() {
    let declaring_source = declaring("bar");
    let mut files = vec![
        count("src/foo.rs", &declaring_source),
        count("src/foo/bar.rs", TARGET),
        count("src/bar.rs", TARGET),
    ];

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/foo/bar.rs"), "bar");
    assert_wholly_production(find(&files, "src/bar.rs"));
}

#[test]
fn the_mod_rs_form_of_a_target_is_found_when_the_file_beside_it_is_not_there() {
    let declaring_source = declaring("helpers");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/helpers/mod.rs", TARGET),
    ];

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/helpers/mod.rs"), "helpers");
}

#[test]
fn a_declaration_with_no_cfg_test_marks_nothing() {
    let declaring_source = declaring_plainly("tests");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/tests.rs", TARGET),
    ];
    let before = files.clone();

    resolve_test_modules(&mut files);

    assert_eq!(
        files, before,
        "a plain `mod tests;` is a module of production code"
    );
}

#[test]
fn a_test_module_with_a_body_marks_no_other_file() {
    let declaring_source = holding_its_tests("tests");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/tests.rs", TARGET),
    ];
    let before = files[1].clone();

    resolve_test_modules(&mut files);

    assert_eq!(
        find(&files, "src/tests.rs"),
        &before,
        "the test code is in the declaring file, so no other file moves"
    );
    assert!(
        find(&files, "src/lib.rs").test.total() > 0,
        "the module with the body is still test code itself"
    );
}

#[test]
fn a_declaration_whose_target_was_not_counted_changes_nothing() {
    let declaring_source = declaring("tests");
    let mut files = vec![count("src/lib.rs", &declaring_source)];
    let before = files.clone();

    resolve_test_modules(&mut files);

    assert_eq!(
        files, before,
        "a file outside the walk, or one .gitignore excluded, is silently left alone"
    );
}

#[test]
fn an_empty_target_gets_no_span() {
    let declaring_source = declaring("tests");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/tests.rs", ""),
    ];

    resolve_test_modules(&mut files);

    let target = find(&files, "src/tests.rs");
    assert_eq!(target.total(), Counts::default(), "a file of no rows");
    assert!(
        target.spans.is_empty(),
        "a span over a file of no rows is a region spelled as no region: {:?}",
        target.spans
    );
}

#[test]
fn resolving_twice_is_resolving_once() {
    let declaring_source = declaring("tests");
    let mut once = vec![
        count("src/lib.rs", &declaring_source),
        count("src/tests.rs", TARGET),
    ];
    let mut twice = once.clone();

    resolve_test_modules(&mut once);
    resolve_test_modules(&mut twice);
    resolve_test_modules(&mut twice);

    assert_eq!(twice, once, "the pass is idempotent");
    assert_marked(find(&twice, "src/tests.rs"), "tests");
}

#[test]
fn a_target_the_path_rule_already_marked_keeps_the_one_span_it_had() {
    let rules =
        PathRules::new(&["src/tests.rs".to_string()], &[]).expect("the glob of this test compiles");
    let declaring_source = declaring("tests");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count_under(rules, "src/tests.rs", TARGET),
    ];
    let before = files[1].clone();

    resolve_test_modules(&mut files);

    let target = find(&files, "src/tests.rs");
    assert_eq!(
        target, &before,
        "a file already wholly test material does not move again"
    );
    assert_eq!(
        target.spans,
        vec![Span {
            first_row: 1,
            last_row: TARGET_ROWS,
            rule: Rule::PathGlob("src/tests.rs".to_string()),
        }],
        "the glob marked it, and the declaration adds no second span"
    );
}

#[test]
fn two_files_declaring_one_target_mark_it_once() {
    let declaring_source = declaring("tests");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/main.rs", &declaring_source),
        count("src/tests.rs", TARGET),
    ];

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/tests.rs"), "tests");
}

#[test]
fn a_module_name_of_many_bytes_marks_its_file() {
    let declaring_source = declaring("テスト");
    let mut files = vec![
        count("src/lib.rs", &declaring_source),
        count("src/テスト.rs", TARGET),
    ];

    resolve_test_modules(&mut files);

    assert_marked(find(&files, "src/テスト.rs"), "テスト");
}

#[test]
fn the_two_buckets_sum_to_the_unsplit_count_before_and_after_the_pass() {
    let declaring_source = declaring("tests");
    let sources = [
        ("src/lib.rs", declaring_source.as_str()),
        ("src/tests.rs", TARGET),
        ("src/other.rs", TARGET),
    ];
    let mut files: Vec<FileCount> = sources
        .iter()
        .map(|(path, source)| count(path, source))
        .collect();

    for (index, (path, source)) in sources.iter().enumerate() {
        assert_eq!(
            files[index].total(),
            lines::count(source, Language::Rust),
            "`{path}`: the split changed the count before the pass"
        );
    }

    resolve_test_modules(&mut files);

    for (index, (path, source)) in sources.iter().enumerate() {
        assert_eq!(
            files[index].total(),
            lines::count(source, Language::Rust),
            "`{path}`: the pass changed the count"
        );
        assert_eq!(
            files[index].production + files[index].test,
            files[index].total(),
            "`{path}`: the two buckets sum to the whole file"
        );
    }
}
