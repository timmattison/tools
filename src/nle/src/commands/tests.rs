//! Tests for the command bodies that the Tauri commands call.

use std::fs;
use std::path::Path;

use super::*;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

fn is_gzip(path: &Path) -> bool {
    fs::read(path).unwrap().starts_with(&GZIP_MAGIC)
}

fn write_fixture(path: &Path, compress: bool) {
    Recording::test_fixture().save(path, compress).unwrap();
}

fn options(input_path: &Path, output_path: &str, format: ExportFormat) -> ExportOptions {
    ExportOptions {
        input_path: input_path.to_string_lossy().to_string(),
        output_path: output_path.to_string(),
        format,
        theme: None,
        compress: None,
        fps: None,
        resolution: None,
        optimize_web: None,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

#[test]
fn validate_path_reports_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    let err = validate_path(&missing.to_string_lossy()).unwrap_err();
    assert!(matches!(err, NleError::FileNotFound(_)), "got {err}");
}

#[test]
fn validate_path_returns_the_canonical_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json");
    write_fixture(&file, false);
    let resolved = validate_path(&file.to_string_lossy()).unwrap();
    assert_eq!(resolved, file.canonicalize().unwrap());
}

#[test]
fn validate_export_input_names_the_missing_input() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    let err = validate_export_input(&missing.to_string_lossy()).unwrap_err();
    assert!(matches!(err, NleError::FileNotFound(_)), "got {err}");
    assert!(
        err.to_string().contains("Export input file not found"),
        "got {err}"
    );
}

#[test]
fn load_recording_from_returns_the_recording_and_its_canonical_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json");
    write_fixture(&file, false);
    let loaded = load_recording_from(&file.to_string_lossy()).unwrap();
    assert_eq!(
        loaded.metadata.file_path,
        file.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(loaded.metadata.event_count, 2);
}

#[test]
fn save_recording_to_gzips_a_gz_path_when_compress_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json.gz");
    save_recording_to(&file.to_string_lossy(), &Recording::test_fixture(), None).unwrap();
    assert!(is_gzip(&file));
}

#[test]
fn save_recording_to_writes_plain_json_for_a_json_path_when_compress_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json");
    save_recording_to(&file.to_string_lossy(), &Recording::test_fixture(), None).unwrap();
    assert_eq!(fs::read(&file).unwrap().first(), Some(&b'{'));
}

#[test]
fn save_recording_to_obeys_compress_false_over_a_gz_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json.gz");
    save_recording_to(
        &file.to_string_lossy(),
        &Recording::test_fixture(),
        Some(false),
    )
    .unwrap();
    assert_eq!(fs::read(&file).unwrap().first(), Some(&b'{'));
}

#[test]
fn save_recording_to_obeys_compress_true_over_a_json_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json");
    save_recording_to(
        &file.to_string_lossy(),
        &Recording::test_fixture(),
        Some(true),
    )
    .unwrap();
    assert!(is_gzip(&file));
}

#[test]
fn save_recording_to_rejects_a_missing_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("no-such-dir").join("a.json");
    let err =
        save_recording_to(&file.to_string_lossy(), &Recording::test_fixture(), None).unwrap_err();
    assert!(matches!(err, NleError::SaveError(_)), "got {err}");
    assert!(
        err.to_string().contains("Parent directory does not exist"),
        "got {err}"
    );
}

#[test]
fn list_recordings_in_keeps_only_loadable_json_and_json_gz_files() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("a.json");
    let gzipped = dir.path().join("b.json.gz");
    write_fixture(&plain, false);
    write_fixture(&gzipped, true);
    write_fixture(&dir.path().join("c.txt"), false);
    fs::write(dir.path().join("d.json"), "this is not json").unwrap();

    let mut listed = list_recordings_in(&dir.path().to_string_lossy()).unwrap();
    listed.sort_by(|a, b| a.file_path.cmp(&b.file_path));

    let recording = Recording::test_fixture();
    let mut expected = vec![
        recording.metadata(&plain.to_string_lossy()),
        recording.metadata(&gzipped.to_string_lossy()),
    ];
    expected.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    let paths = |items: &[RecordingMetadata]| {
        items
            .iter()
            .map(|m| (m.file_path.clone(), m.event_count, m.title.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(&listed), paths(&expected));
}

#[test]
fn list_recordings_in_reports_a_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let err = list_recordings_in(&missing.to_string_lossy()).unwrap_err();
    assert!(matches!(err, NleError::FileNotFound(_)), "got {err}");
}

#[test]
fn video_export_args_lists_every_option_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let mut opts = options(&input, "out.mp4", ExportFormat::Video);
    opts.theme = Some("dark".to_string());
    opts.fps = Some(30);
    opts.resolution = Some("1280x720".to_string());
    opts.optimize_web = Some(true);

    let canonical = input.canonicalize().unwrap().to_string_lossy().to_string();
    let mut expected = strings(&["export", "video"]);
    expected.push(canonical);
    expected.extend(strings(&[
        "--output",
        "out.mp4",
        "--theme",
        "dark",
        "--fps",
        "30",
        "--resolution",
        "1280x720",
        "--optimize-web",
    ]));
    assert_eq!(video_export_args(&opts).unwrap(), expected);
}

#[test]
fn video_export_args_with_no_options_is_the_base_list() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let opts = options(&input, "out.mp4", ExportFormat::Video);

    let canonical = input.canonicalize().unwrap().to_string_lossy().to_string();
    let mut expected = strings(&["export", "video"]);
    expected.push(canonical);
    expected.extend(strings(&["--output", "out.mp4"]));
    assert_eq!(video_export_args(&opts).unwrap(), expected);
}

#[test]
fn video_export_args_rejects_the_web_format() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let err = video_export_args(&options(&input, "out.mp4", ExportFormat::Web)).unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(
        err.to_string()
            .contains("export_video requires Video format"),
        "got {err}"
    );
}

#[test]
fn video_export_args_rejects_a_missing_input() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("missing.json");
    let err = video_export_args(&options(&input, "out.mp4", ExportFormat::Video)).unwrap_err();
    assert!(matches!(err, NleError::FileNotFound(_)), "got {err}");
}

#[test]
fn web_export_args_lists_every_option_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let mut opts = options(&input, "out.html", ExportFormat::Web);
    opts.theme = Some("light".to_string());
    opts.compress = Some(true);

    let canonical = input.canonicalize().unwrap().to_string_lossy().to_string();
    let mut expected = strings(&["export", "web"]);
    expected.push(canonical);
    expected.extend(strings(&[
        "--output",
        "out.html",
        "--theme",
        "light",
        "--compress",
    ]));
    assert_eq!(web_export_args(&opts).unwrap(), expected);
}

#[test]
fn web_export_args_rejects_the_video_format() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let err = web_export_args(&options(&input, "out.html", ExportFormat::Video)).unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(
        err.to_string().contains("export_web requires Web format"),
        "got {err}"
    );
}
