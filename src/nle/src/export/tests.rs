//! Tests for the export of an edited project.

use super::*;

#[test]
fn parse_progress_line_reads_a_frame_line_and_refuses_other_lines() {
    assert_eq!(
        parse_progress_line("Processed frame 31 / 300"),
        Some(ExportProgress::Frames {
            done: 31,
            total: 300
        })
    );
    assert_eq!(
        parse_progress_line("Processed frame 5 / 10 \r"),
        Some(ExportProgress::Frames { done: 5, total: 10 })
    );
    assert_eq!(parse_progress_line("Video export saved to: x.mp4"), None);
    assert_eq!(parse_progress_line("Processed frame 5 / 0"), None);
    assert_eq!(parse_progress_line("Processed frame 7 / 3"), None);
    assert_eq!(parse_progress_line("Processed frame x / 3"), None);
}

#[test]
fn theme_names_round_trip_and_an_unknown_name_is_refused() {
    let names: Vec<&str> = Theme::ALL.iter().map(|theme| theme.as_str()).collect();
    assert_eq!(
        names,
        [
            "auto",
            "dracula",
            "monokai",
            "solarized-dark",
            "solarized-light"
        ]
    );
    for theme in Theme::ALL {
        assert_eq!(theme.as_str().parse::<Theme>().unwrap(), theme);
    }
    assert_eq!(Theme::default(), Theme::Auto);
    for bad in ["", "Dracula", "solarized", "nord"] {
        assert!(
            matches!(bad.parse::<Theme>(), Err(NleError::ExportError(_))),
            "{bad:?} was accepted"
        );
    }
}

#[test]
fn a_resolution_parses_and_displays_and_a_bad_one_is_refused() {
    let resolution: Resolution = "1920x1080".parse().unwrap();
    assert_eq!(resolution.to_string(), "1920x1080");
    for bad in [
        "0x1080",
        "1920x0",
        "1920x",
        "x1080",
        "1920X1080",
        "abc",
        " 1920x1080",
        "1920x1080 ",
        "-1x5",
    ] {
        let Err(NleError::ExportError(text)) = bad.parse::<Resolution>() else {
            panic!("{bad:?} was accepted");
        };
        assert!(text.contains(bad), "got {text}");
        assert!(text.contains("WIDTHxHEIGHT"), "got {text}");
    }
}
