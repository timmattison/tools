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
