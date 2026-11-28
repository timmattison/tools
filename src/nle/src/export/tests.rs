//! Tests for the export of an edited project.

use std::path::Path;
use std::sync::{Arc, Mutex};

use super::*;
use crate::commands::tests::{block_on, finder_vars, home_with_beta_script, FAILING_BETA};

type Reported = Arc<Mutex<Vec<ExportProgress>>>;

/// A callback that collects each progress into `reported`.
fn collector(reported: &Reported) -> impl FnMut(ExportProgress) + Send + 'static {
    let reported = Arc::clone(reported);
    move |progress| reported.lock().unwrap().push(progress)
}

/// A recording that differs from the fixture, so a copy of the wrong one shows.
fn recording() -> Recording {
    let mut recording = Recording::test_fixture();
    recording.title = "the edited recording".to_string();
    recording
}

/// A `beta` that copies its input to its output, writes its arguments one per
/// line to the output name plus `.args`, and reports one frame line.
#[cfg(unix)]
const COPYING_BETA: &str = "#!/bin/sh\n\
    cp \"$3\" \"$5\"\n\
    printf '%s\\n' \"$@\" > \"$5.args\"\n\
    echo 'Processed frame 30 / 60'\n";

/// The arguments that `COPYING_BETA` wrote for the output file `output`.
#[cfg(unix)]
fn written_args(output: &Path) -> Vec<String> {
    let mut name = output.as_os_str().to_owned();
    name.push(".args");
    std::fs::read_to_string(name)
        .unwrap()
        .lines()
        .map(ToString::to_string)
        .collect()
}

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

#[cfg(unix)]
#[test]
fn a_video_export_runs_beta_on_the_rendered_recording() {
    let home = home_with_beta_script(COPYING_BETA);
    let out = tempfile::tempdir().unwrap();
    let output = out.path().join("out.mp4");
    let canonical_output = out.path().canonicalize().unwrap().join("out.mp4");
    let request = ExportRequest {
        output_path: output.clone(),
        target: ExportTarget::Video {
            theme: Theme::Dracula,
            fps: NonZeroU32::new(30),
            resolution: Some("1280x720".parse().unwrap()),
            optimize_web: true,
        },
    };
    let reported = Reported::default();

    let result = block_on(export_recording_with(
        Arc::new(recording()),
        request,
        collector(&reported),
        finder_vars(&home),
    ));

    assert_eq!(
        *reported.lock().unwrap(),
        [
            ExportProgress::Writing,
            ExportProgress::Frames {
                done: 30,
                total: 60
            }
        ]
    );
    assert_eq!(result.unwrap(), output);
    assert_eq!(Recording::load(&canonical_output).unwrap(), recording());
    let args = written_args(&canonical_output);
    let input = args.get(2).unwrap().clone();
    let expected: Vec<String> = [
        "export",
        "video",
        input.as_str(),
        "--output",
        canonical_output.to_str().unwrap(),
        "--theme",
        "dracula",
        "--fps",
        "30",
        "--resolution",
        "1280x720",
        "--optimize-web",
    ]
    .map(ToString::to_string)
    .to_vec();
    assert_eq!(args, expected);
    assert!(!Path::new(&input).exists(), "the work file stays: {input}");
}

#[cfg(unix)]
#[test]
fn a_web_export_runs_beta_with_the_theme_and_compress() {
    let home = home_with_beta_script(COPYING_BETA);
    let out = tempfile::tempdir().unwrap();
    let canonical_output = out.path().canonicalize().unwrap().join("page.html");
    let request = ExportRequest {
        output_path: out.path().join("page.html"),
        target: ExportTarget::Web {
            theme: Theme::Monokai,
            compress: true,
        },
    };

    let result = block_on(export_recording_with(
        Arc::new(recording()),
        request,
        |_| {},
        finder_vars(&home),
    ));

    result.unwrap();
    assert_eq!(Recording::load(&canonical_output).unwrap(), recording());
    let args = written_args(&canonical_output);
    let input = args.get(2).unwrap().clone();
    let expected: Vec<String> = [
        "export",
        "web",
        input.as_str(),
        "--output",
        canonical_output.to_str().unwrap(),
        "--theme",
        "monokai",
        "--compress",
    ]
    .map(ToString::to_string)
    .to_vec();
    assert_eq!(args, expected);
}

#[test]
fn a_recording_export_writes_the_file_and_reports_writing() {
    let out = tempfile::tempdir().unwrap();
    let output = out.path().join("out.json.gz");
    let request = ExportRequest {
        output_path: output.clone(),
        target: ExportTarget::Recording { compress: None },
    };
    let reported = Reported::default();

    let result = block_on(export_recording_with(
        Arc::new(recording()),
        request,
        collector(&reported),
        |_| None,
    ));

    assert_eq!(*reported.lock().unwrap(), [ExportProgress::Writing]);
    assert_eq!(result.unwrap(), output);
    assert!(std::fs::read(&output).unwrap().starts_with(&[0x1f, 0x8b]));
    assert_eq!(Recording::load(&output).unwrap(), recording());
}

#[test]
fn a_recording_export_keeps_the_save_error_of_a_refused_name() {
    let out = tempfile::tempdir().unwrap();
    let request = ExportRequest {
        output_path: out.path().join("out.txt"),
        target: ExportTarget::Recording { compress: None },
    };

    let result = block_on(export_recording_with(
        Arc::new(recording()),
        request,
        |_| {},
        |_| None,
    ));

    assert!(matches!(result, Err(NleError::SaveError(_))), "{result:?}");
}

/// An output path that is not valid UTF-8 is refused before the export
/// starts: no stage, no file, and no run of `beta`. APFS refuses such a name,
/// thus this test runs only on Linux.
#[cfg(target_os = "linux")]
#[test]
fn an_output_path_that_is_not_utf8_is_refused_before_the_export() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let home = home_with_beta_script(COPYING_BETA);
    let video = ExportTarget::Video {
        theme: Theme::Auto,
        fps: None,
        resolution: None,
        optimize_web: false,
    };
    let cases: [(&[u8], ExportTarget); 4] = [
        (b"x\xff.json", ExportTarget::Recording { compress: None }),
        (
            b"x\xff.html",
            ExportTarget::Web {
                theme: Theme::Auto,
                compress: false,
            },
        ),
        (b"x\xff.mp4", video.clone()),
        (b"x\xff.gif", video),
    ];
    for (name, target) in cases {
        let out = tempfile::tempdir().unwrap();
        let request = ExportRequest {
            output_path: out.path().join(OsStr::from_bytes(name)),
            target,
        };
        let reported = Reported::default();

        let result = block_on(export_recording_with(
            Arc::new(recording()),
            request,
            collector(&reported),
            finder_vars(&home),
        ));

        let Err(NleError::ExportError(text)) = result else {
            panic!("{name:?}: expected an ExportError, got {result:?}");
        };
        assert!(text.contains("not valid UTF-8"), "{text}");
        assert!(reported.lock().unwrap().is_empty(), "{name:?}");
        let written: Vec<_> = std::fs::read_dir(out.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(written.is_empty(), "{name:?} wrote {written:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_failed_beta_gives_an_export_error_and_removes_the_work_file() {
    let home = home_with_beta_script(FAILING_BETA);
    let out = tempfile::tempdir().unwrap();
    let request = ExportRequest {
        output_path: out.path().join("out.mp4"),
        target: ExportTarget::Video {
            theme: Theme::Auto,
            fps: None,
            resolution: None,
            optimize_web: false,
        },
    };

    let result = block_on(export_recording_with(
        Arc::new(recording()),
        request,
        |_| {},
        finder_vars(&home),
    ));

    let Err(NleError::ExportError(text)) = result else {
        panic!("expected an ExportError, got {result:?}");
    };
    // FAILING_BETA prints its arguments, and the third one is the work file.
    let input = text
        .split_whitespace()
        .find(|word| word.ends_with(WORK_FILE_NAME))
        .unwrap_or_else(|| panic!("no work file in {text}"));
    assert!(!Path::new(input).exists(), "the work file stays: {input}");
}
