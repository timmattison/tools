//! Tests of the model of the export bar.

use super::*;

#[test]
fn output_path_gives_the_path_the_ending_of_the_kind() {
    let cases = [
        ("/tmp/a", ExportKind::Mp4, "/tmp/a.mp4"),
        ("/tmp/a.MP4", ExportKind::Mp4, "/tmp/a.MP4"),
        ("/tmp/a.gif", ExportKind::Mp4, "/tmp/a.mp4"),
        ("/tmp/a.gz", ExportKind::CompressedRecording, "/tmp/a.json.gz"),
        ("/tmp/a.json", ExportKind::CompressedRecording, "/tmp/a.json.gz"),
        ("/tmp/a.json.gz", ExportKind::Recording, "/tmp/a.json"),
        ("/tmp/report.v2", ExportKind::Web, "/tmp/report.v2.html"),
        ("/tmp/日本語", ExportKind::Gif, "/tmp/日本語.gif"),
        ("/tmp/日本語.MP4", ExportKind::Gif, "/tmp/日本語.gif"),
        ("/tmp/.gz", ExportKind::Recording, "/tmp/recording.json"),
    ];
    for (picked, kind, expected) in cases {
        assert_eq!(
            kind.output_path(PathBuf::from(picked)),
            PathBuf::from(expected),
            "{kind:?} for {picked}"
        );
    }
}

#[test]
fn save_dialog_offers_an_edited_name_in_the_folder_of_the_source() {
    let cases = [
        ("/home/user/recordings/demo.json", ExportKind::Mp4, "demo-edited.mp4"),
        (r"C:\x\Demo.JSON.GZ", ExportKind::Recording, "Demo-edited.json"),
        ("/x/notes.txt", ExportKind::Web, "notes.txt-edited.html"),
        ("/x/日本語.json", ExportKind::Gif, "日本語-edited.gif"),
        ("/x/.gz", ExportKind::CompressedRecording, "recording-edited.json.gz"),
    ];
    for (source, kind, expected) in cases {
        assert_eq!(kind.save_dialog(source).file_name, expected, "{source}");
    }
}

#[test]
fn save_dialog_opens_in_the_parent_folder_of_the_source() {
    let dialog = ExportKind::Gif.save_dialog("/home/user/recordings/demo.json");
    assert_eq!(dialog.directory, Some(PathBuf::from("/home/user/recordings")));
    assert_eq!(ExportKind::Gif.save_dialog("demo.json").directory, None);
}

#[test]
fn save_dialog_filters_by_the_extension_of_the_kind() {
    let filters = [
        (ExportKind::Recording, "Beta recording", ["json"]),
        (ExportKind::CompressedRecording, "Gzip Beta recording", ["gz"]),
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
        (ExportKind::CompressedRecording, "json-gz", "Beta recording, gzip (.json.gz)"),
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
