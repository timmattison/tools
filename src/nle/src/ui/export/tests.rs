//! Tests of the model of the export bar.

use std::num::NonZeroU32;

use super::*;

#[test]
fn output_path_gives_the_path_the_ending_of_the_kind() {
    let cases = [
        ("/tmp/a", ExportKind::Mp4, "/tmp/a.mp4"),
        ("/tmp/a.MP4", ExportKind::Mp4, "/tmp/a.MP4"),
        ("/tmp/a.gif", ExportKind::Mp4, "/tmp/a.mp4"),
        (
            "/tmp/a.gz",
            ExportKind::CompressedRecording,
            "/tmp/a.json.gz",
        ),
        (
            "/tmp/a.json",
            ExportKind::CompressedRecording,
            "/tmp/a.json.gz",
        ),
        ("/tmp/a.json.gz", ExportKind::Recording, "/tmp/a.json"),
        ("/tmp/report.v2", ExportKind::Web, "/tmp/report.v2.html"),
        ("/tmp/日本語", ExportKind::Gif, "/tmp/日本語.gif"),
        ("/tmp/日本語.MP4", ExportKind::Gif, "/tmp/日本語.gif"),
        ("/tmp/.gz", ExportKind::Recording, "/tmp/recording.json"),
    ];
    for (picked, kind, expected) in cases {
        assert_eq!(
            kind.output_path(PathBuf::from(picked)).expect("a path"),
            PathBuf::from(expected),
            "{kind:?} for {picked}"
        );
    }
}

#[test]
fn save_dialog_offers_an_edited_name_in_the_folder_of_the_source() {
    let cases = [
        (
            "/home/user/recordings/demo.json",
            ExportKind::Mp4,
            "demo-edited.mp4",
        ),
        (
            r"C:\x\Demo.JSON.GZ",
            ExportKind::Recording,
            "Demo-edited.json",
        ),
        ("/x/notes.txt", ExportKind::Web, "notes.txt-edited.html"),
        ("/x/日本語.json", ExportKind::Gif, "日本語-edited.gif"),
        (
            "/x/.gz",
            ExportKind::CompressedRecording,
            "recording-edited.json.gz",
        ),
    ];
    for (source, kind, expected) in cases {
        assert_eq!(kind.save_dialog(source).file_name, expected, "{source}");
    }
}

#[test]
fn save_dialog_opens_in_the_parent_folder_of_the_source() {
    let dialog = ExportKind::Gif.save_dialog("/home/user/recordings/demo.json");
    assert_eq!(
        dialog.directory,
        Some(PathBuf::from("/home/user/recordings"))
    );
    assert_eq!(ExportKind::Gif.save_dialog("demo.json").directory, None);
}

#[test]
fn save_dialog_filters_by_the_extension_of_the_kind() {
    let filters = [
        (ExportKind::Recording, "Beta recording", ["json"]),
        (
            ExportKind::CompressedRecording,
            "Gzip Beta recording",
            ["gz"],
        ),
        (ExportKind::Web, "Web page", ["html"]),
        (ExportKind::Mp4, "MP4 video", ["mp4"]),
        (ExportKind::Gif, "GIF animation", ["gif"]),
    ];
    for (kind, name, extensions) in filters {
        let dialog = kind.save_dialog("/x/demo.json");
        assert_eq!(dialog.filter_name, name, "{kind:?}");
        assert_eq!(dialog.extensions, extensions, "{kind:?}");
    }
}

#[test]
fn each_kind_has_a_key_and_a_label_for_the_menu() {
    let menu = [
        (ExportKind::Recording, "json", "Beta recording (.json)"),
        (
            ExportKind::CompressedRecording,
            "json-gz",
            "Beta recording, gzip (.json.gz)",
        ),
        (ExportKind::Web, "html", "Web page (.html)"),
        (ExportKind::Mp4, "mp4", "MP4 video (.mp4)"),
        (ExportKind::Gif, "gif", "GIF animation (.gif)"),
    ];
    assert_eq!(menu.map(|(kind, ..)| kind), ExportKind::ALL);
    for (kind, key, label) in menu {
        assert_eq!(kind.key(), key);
        assert_eq!(kind.label(), label);
        assert_eq!(ExportKind::from_key(key), Some(kind));
    }
    assert_eq!(ExportKind::from_key("pdf"), None);
}

/// The settings of a kind, and the default values for the rest.
fn settings(kind: ExportKind) -> ExportSettings {
    ExportSettings {
        kind,
        ..ExportSettings::default()
    }
}

/// The text of the error of `result`.
fn error_text(result: Result<ExportTarget, NleError>) -> String {
    result.expect_err("an error").to_string()
}

#[test]
fn settings_make_the_target_of_a_recording() {
    assert_eq!(
        settings(ExportKind::Recording).target().expect("a target"),
        ExportTarget::Recording {
            compress: Some(false)
        }
    );
    assert_eq!(
        settings(ExportKind::CompressedRecording)
            .target()
            .expect("a target"),
        ExportTarget::Recording {
            compress: Some(true)
        }
    );
}

#[test]
fn settings_make_the_target_of_a_web_page() {
    let web = ExportSettings {
        theme: Theme::Monokai,
        compress_web: true,
        optimize_web: true,
        ..settings(ExportKind::Web)
    };
    assert_eq!(
        web.target().expect("a target"),
        ExportTarget::Web {
            theme: Theme::Monokai,
            compress: true
        }
    );
}

#[test]
fn settings_make_the_target_of_a_video() {
    let mp4 = ExportSettings {
        theme: Theme::Dracula,
        fps: " 30 ".into(),
        resolution: "1280x720".into(),
        optimize_web: true,
        compress_web: true,
        ..settings(ExportKind::Mp4)
    };
    assert_eq!(
        mp4.target().expect("a target"),
        ExportTarget::Video {
            theme: Theme::Dracula,
            fps: NonZeroU32::new(30),
            resolution: "1280x720".parse().ok(),
            optimize_web: true
        }
    );
    assert_eq!(
        settings(ExportKind::Gif).target().expect("a target"),
        ExportTarget::Video {
            theme: Theme::Auto,
            fps: None,
            resolution: None,
            optimize_web: false
        }
    );
}

#[test]
fn settings_refuse_a_frame_rate_that_is_not_above_zero() {
    for text in ["0", "abc", "-1", "2.5"] {
        let mp4 = ExportSettings {
            fps: format!(" {text} "),
            ..settings(ExportKind::Mp4)
        };
        let message = error_text(mp4.target());
        assert!(
            message.contains(&format!(
                "the frame rate \"{text}\" is not a whole number above zero"
            )),
            "{message}"
        );
        assert!(message.starts_with("Export failed: "), "{message}");
    }
}

#[test]
fn settings_refuse_a_resolution_that_is_not_valid() {
    let mp4 = ExportSettings {
        resolution: "big".into(),
        ..settings(ExportKind::Mp4)
    };
    let message = error_text(mp4.target());
    assert!(message.contains("the resolution \"big\""), "{message}");
}

#[test]
fn settings_ignore_a_field_that_the_kind_does_not_use() {
    for kind in [ExportKind::Recording, ExportKind::Web] {
        let bad = ExportSettings {
            fps: "abc".into(),
            resolution: "big".into(),
            ..settings(kind)
        };
        assert!(bad.target().is_ok(), "{kind:?}");
    }
}

#[test]
fn the_request_holds_the_path_with_the_ending_of_the_kind() {
    let request = settings(ExportKind::Mp4)
        .request(PathBuf::from("/tmp/out"))
        .expect("a request");
    assert_eq!(request.output_path, PathBuf::from("/tmp/out.mp4"));
    assert_eq!(
        request.target,
        settings(ExportKind::Mp4).target().expect("a target")
    );
    let bad = ExportSettings {
        fps: "0".into(),
        ..settings(ExportKind::Gif)
    };
    assert!(bad.request(PathBuf::from("/tmp/out")).is_err());
}

/// The save dialog of Linux can give a name that is not valid UTF-8. APFS
/// refuses such a name, thus this test runs only on Linux.
#[cfg(target_os = "linux")]
#[test]
fn the_request_refuses_a_path_that_is_not_utf8() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let pickeds: [&[u8]; 3] = [b"/tmp/x\xff.mp4", b"/tmp/x\xff", b"/tmp/\xff/out"];
    let kinds = [
        ExportKind::Recording,
        ExportKind::CompressedRecording,
        ExportKind::Web,
        ExportKind::Mp4,
        ExportKind::Gif,
    ];
    for picked in pickeds {
        for kind in kinds {
            let result = settings(kind).request(PathBuf::from(OsStr::from_bytes(picked)));
            let Err(NleError::ExportError(text)) = result else {
                panic!("{kind:?} for {picked:?}: expected an ExportError, got {result:?}");
            };
            assert!(text.contains("not valid UTF-8"), "{text}");
        }
    }
}

#[test]
fn the_progress_text_names_the_stage() {
    let cases = [
        (None, "Export started"),
        (
            Some(ExportProgress::Rendering),
            "Step 1 of 2: render the edit",
        ),
        (Some(ExportProgress::Writing), "Step 2 of 2: write the file"),
        (
            Some(ExportProgress::Frames {
                done: 30,
                total: 300,
            }),
            "Step 2 of 2: frame 30 of 300",
        ),
    ];
    for (progress, expected) in cases {
        assert_eq!(progress_text(progress), expected, "{progress:?}");
    }
}
