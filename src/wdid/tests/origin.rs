//! Tests of the origin of a definition: the parse of the text that the shell
//! gives, the rule that says which definitions are mine, and the text that a
//! person reads.

use std::path::PathBuf;

use wdid::origin::{place, Origin, Places};

/// Makes an origin in `file`, at `line` when it is given.
fn origin(file: &str, line: Option<u32>) -> Origin {
    Origin {
        file: PathBuf::from(file),
        line,
    }
}

/// The places of a person whose home is `/home/me`, with oh-my-zsh in
/// `/home/me/.oh-my-zsh` and its custom directory inside it.
fn places() -> Places {
    Places {
        home: Some(PathBuf::from("/home/me")),
        zsh: Some(PathBuf::from("/home/me/.oh-my-zsh")),
        zsh_custom: Some(PathBuf::from("/home/me/.oh-my-zsh/custom")),
    }
}

#[test]
fn a_trace_gives_the_file_and_the_line() {
    assert_eq!(
        Origin::from_trace("/home/me/.zshrc:12"),
        Some(origin("/home/me/.zshrc", Some(12)))
    );
    assert_eq!(
        Origin::from_trace("/dev/fd/11:2"),
        Some(origin("/dev/fd/11", Some(2)))
    );
}

#[test]
fn a_trace_splits_at_its_last_colon() {
    assert_eq!(
        Origin::from_trace("/home/me/a:b/x.zsh:7"),
        Some(origin("/home/me/a:b/x.zsh", Some(7)))
    );
}

#[test]
fn a_trace_without_a_line_number_is_a_file_with_no_line() {
    for text in [
        "/home/me/x.zsh",
        "/home/me/x.zsh:",
        "/home/me/x.zsh:+3",
        "/home/me/x:y",
    ] {
        assert_eq!(Origin::from_trace(text), Some(origin(text, None)), "{text}");
    }
}

#[test]
fn a_trace_that_is_empty_or_relative_is_unknown() {
    for text in ["", ":3", "relative.zsh:3", "(eval):1"] {
        assert_eq!(Origin::from_trace(text), None, "{text}");
    }
}

#[test]
fn a_trace_keeps_a_multibyte_path() {
    assert_eq!(
        Origin::from_trace("/home/me/日本語/🎉 café.zsh:3"),
        Some(origin("/home/me/日本語/🎉 café.zsh", Some(3)))
    );
}

#[test]
fn a_file_is_the_whole_text_with_no_line() {
    assert_eq!(
        Origin::from_file("/home/me/.zshrc"),
        Some(origin("/home/me/.zshrc", None))
    );
    assert_eq!(
        Origin::from_file("/home/me/x.zsh:3"),
        Some(origin("/home/me/x.zsh:3", None))
    );
}

/// bash gives `(null)` or `environment` for a function that no file defined.
#[test]
fn a_file_that_is_empty_or_relative_is_unknown() {
    for text in ["", "(null)", "environment", "./x.zsh"] {
        assert_eq!(Origin::from_file(text), None, "{text}");
    }
}

#[test]
fn a_place_is_an_absolute_path_only() {
    assert_eq!(place("/home/me"), Some(PathBuf::from("/home/me")));
    assert_eq!(place(""), None);
    assert_eq!(place("home/me"), None);
}

#[test]
fn an_unknown_origin_is_mine() {
    assert!(places().is_mine(None));
    assert!(Places::default().is_mine(None));
}

#[test]
fn a_file_under_home_is_mine() {
    let places = places();
    for file in ["/home/me/.zshrc", "/home/me/.config/zsh/aliases.zsh"] {
        assert!(places.is_mine(Some(&origin(file, Some(1)))), "{file}");
    }
}

#[test]
fn a_file_under_the_oh_my_zsh_directory_is_not_mine() {
    let places = places();
    for file in [
        "/home/me/.oh-my-zsh/plugins/git/git.plugin.zsh",
        "/home/me/.oh-my-zsh/lib/directories.zsh",
    ] {
        assert!(!places.is_mine(Some(&origin(file, Some(1)))), "{file}");
    }
}

#[test]
fn a_file_under_the_custom_directory_is_mine() {
    let places = places();
    let file = origin("/home/me/.oh-my-zsh/custom/mine.zsh", Some(1));
    assert!(places.is_mine(Some(&file)));
    let outside_home = Places {
        zsh_custom: Some(PathBuf::from("/opt/zsh-custom")),
        ..places
    };
    assert!(outside_home.is_mine(Some(&origin("/opt/zsh-custom/x.zsh", Some(1)))));
}

#[test]
fn a_file_outside_home_is_not_mine() {
    let places = places();
    for file in [
        "/dev/fd/11",
        "/usr/share/zsh/5.9/functions/_git",
        "/etc/zshrc",
        "/home/me2/.zshrc",
    ] {
        assert!(!places.is_mine(Some(&origin(file, None))), "{file}");
    }
}

#[test]
fn with_no_home_only_an_unknown_origin_is_mine() {
    let places = Places {
        home: None,
        ..places()
    };
    assert!(!places.is_mine(Some(&origin("/home/me/.zshrc", Some(1)))));
    assert!(places.is_mine(None));
}

#[test]
fn with_no_oh_my_zsh_each_file_under_home_is_mine() {
    let places = Places {
        zsh: None,
        zsh_custom: None,
        ..places()
    };
    let file = origin("/home/me/.oh-my-zsh/plugins/git/git.plugin.zsh", Some(1));
    assert!(places.is_mine(Some(&file)));
}

#[test]
fn the_text_of_an_origin_puts_a_tilde_for_home() {
    let places = places();
    assert_eq!(
        places.show(&origin("/home/me/.zshrc", Some(12))),
        "~/.zshrc:12"
    );
    assert_eq!(places.show(&origin("/home/me/.zshrc", None)), "~/.zshrc");
    assert_eq!(places.show(&origin("/home/me", None)), "~");
}

#[test]
fn the_text_of_an_origin_outside_home_is_the_whole_path() {
    let places = places();
    assert_eq!(places.show(&origin("/dev/fd/11", Some(2))), "/dev/fd/11:2");
    assert_eq!(
        places.show(&origin("/home/me2/.zshrc", None)),
        "/home/me2/.zshrc"
    );
    let no_home = Places::default();
    assert_eq!(
        no_home.show(&origin("/home/me/.zshrc", Some(1))),
        "/home/me/.zshrc:1"
    );
}

#[test]
fn the_text_of_an_origin_keeps_a_multibyte_path() {
    assert_eq!(
        places().show(&origin("/home/me/日本語/🎉.zsh", Some(3))),
        "~/日本語/🎉.zsh:3"
    );
}
