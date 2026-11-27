//! Tests for the command bodies that the application commands call.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
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

fn out_path(dir: &tempfile::TempDir, name: &str) -> String {
    dir.path().join(name).to_string_lossy().to_string()
}

fn canonical_out(dir: &tempfile::TempDir, name: &str) -> String {
    dir.path()
        .canonicalize()
        .unwrap()
        .join(name)
        .to_string_lossy()
        .to_string()
}

/// Run `future` to its end on a multi-thread tokio runtime of its own.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
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
    let mut opts = options(&input, &out_path(&dir, "out.mp4"), ExportFormat::Video);
    opts.theme = Some("dark".to_string());
    opts.fps = Some(30);
    opts.resolution = Some("1280x720".to_string());
    opts.optimize_web = Some(true);

    let canonical = input.canonicalize().unwrap().to_string_lossy().to_string();
    let mut expected = strings(&["export", "video"]);
    expected.push(canonical);
    expected.extend(strings(&[
        "--output",
        &canonical_out(&dir, "out.mp4"),
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
    let opts = options(&input, &out_path(&dir, "out.mp4"), ExportFormat::Video);

    let canonical = input.canonicalize().unwrap().to_string_lossy().to_string();
    let mut expected = strings(&["export", "video"]);
    expected.push(canonical);
    expected.extend(strings(&["--output", &canonical_out(&dir, "out.mp4")]));
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
    let mut opts = options(&input, &out_path(&dir, "out.html"), ExportFormat::Web);
    opts.theme = Some("light".to_string());
    opts.compress = Some(true);

    let canonical = input.canonicalize().unwrap().to_string_lossy().to_string();
    let mut expected = strings(&["export", "web"]);
    expected.push(canonical);
    expected.extend(strings(&[
        "--output",
        &canonical_out(&dir, "out.html"),
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

#[test]
fn save_recording_to_refuses_a_name_without_a_recording_ending() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["notes.txt", ".zshrc"] {
        let file = dir.path().join(name);
        let err = save_recording_to(&file.to_string_lossy(), &Recording::test_fixture(), None)
            .unwrap_err();
        assert!(matches!(err, NleError::SaveError(_)), "got {err}");
        assert!(err.to_string().contains("must end in"), "got {err}");
        assert!(!file.exists(), "{name} was written");
    }
}

#[test]
fn save_recording_to_ignores_the_case_of_the_ending() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("REC.JSON");
    save_recording_to(&file.to_string_lossy(), &Recording::test_fixture(), None).unwrap();
    assert!(file.exists());
}

#[test]
fn save_recording_to_gzips_an_uppercase_gz_path_when_compress_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("REC.JSON.GZ");
    save_recording_to(&file.to_string_lossy(), &Recording::test_fixture(), None).unwrap();
    assert!(is_gzip(&file));
}

#[test]
fn list_recordings_in_ignores_the_case_of_the_ending() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("A.JSON");
    let gzipped = dir.path().join("B.Json.Gz");
    write_fixture(&plain, false);
    write_fixture(&gzipped, true);

    let mut listed: Vec<String> = list_recordings_in(&dir.path().to_string_lossy())
        .unwrap()
        .into_iter()
        .map(|m| m.file_path)
        .collect();
    listed.sort();

    let mut expected = vec![
        plain.to_string_lossy().into_owned(),
        gzipped.to_string_lossy().into_owned(),
    ];
    expected.sort();
    assert_eq!(listed, expected);
}

#[test]
fn video_export_args_refuses_a_name_without_a_video_ending() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let output = dir.path().join("out.txt");
    let err = video_export_args(&options(
        &input,
        &output.to_string_lossy(),
        ExportFormat::Video,
    ))
    .unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(err.to_string().contains("must end in"), "got {err}");
}

#[test]
fn video_export_args_refuses_a_missing_output_directory() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let output = dir.path().join("missing-dir").join("out.mp4");
    let err = video_export_args(&options(
        &input,
        &output.to_string_lossy(),
        ExportFormat::Video,
    ))
    .unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(
        err.to_string().contains("Parent directory does not exist"),
        "got {err}"
    );
}

#[test]
fn web_export_args_refuses_a_name_without_a_web_ending() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let output = dir.path().join("out.mp4");
    let err = web_export_args(&options(
        &input,
        &output.to_string_lossy(),
        ExportFormat::Web,
    ))
    .unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(err.to_string().contains("must end in"), "got {err}");
}

#[test]
fn video_export_args_gives_beta_the_canonical_output_path() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let output = dir.path().join("sub").join("..").join("out.gif");
    fs::create_dir(dir.path().join("sub")).unwrap();
    let args = video_export_args(&options(
        &input,
        &output.to_string_lossy(),
        ExportFormat::Video,
    ))
    .unwrap();
    let expected = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("out.gif")
        .to_string_lossy()
        .to_string();
    let at = args.iter().position(|a| a == "--output").unwrap();
    assert_eq!(args[at + 1], expected);
}

#[cfg(unix)]
#[test]
fn save_recording_to_writes_through_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("rec.json");
    fs::write(&target, b"old").unwrap();
    let link = dir.path().join("latest.json");
    std::os::unix::fs::symlink("rec.json", &link).unwrap();
    save_recording_to(&link.to_string_lossy(), &Recording::test_fixture(), None).unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced"
    );
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("rec.json"));
    assert!(
        fs::read(&target).unwrap().starts_with(b"{"),
        "the target was not written"
    );
}

#[cfg(unix)]
#[test]
fn save_recording_to_refuses_a_link_to_a_file_without_a_recording_ending() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("notes.txt");
    fs::write(&target, b"keep").unwrap();
    let link = dir.path().join("latest.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let err =
        save_recording_to(&link.to_string_lossy(), &Recording::test_fixture(), None).unwrap_err();
    assert!(matches!(err, NleError::SaveError(_)), "got {err}");
    assert!(err.to_string().contains("must end in"), "got {err}");
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link was replaced"
    );
}

#[cfg(unix)]
#[test]
fn save_recording_to_refuses_a_loop_of_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("a.json");
    let second = dir.path().join("b.json");
    std::os::unix::fs::symlink("b.json", &first).unwrap();
    std::os::unix::fs::symlink("a.json", &second).unwrap();
    let err =
        save_recording_to(&first.to_string_lossy(), &Recording::test_fixture(), None).unwrap_err();
    assert!(matches!(err, NleError::SaveError(_)), "got {err}");
    assert!(err.to_string().contains("symbolic links"), "got {err}");
}

/// Make a chain of `count` relative symbolic links in `dir` that ends at
/// `rec.json`, and return the first link of the chain.
#[cfg(unix)]
fn link_chain(dir: &Path, count: usize) -> std::path::PathBuf {
    for index in 0..count {
        let target = if index + 1 == count {
            "rec.json".to_string()
        } else {
            format!("link-{}.json", index + 1)
        };
        std::os::unix::fs::symlink(target, dir.join(format!("link-{index}.json"))).unwrap();
    }
    dir.join("link-0.json")
}

#[cfg(unix)]
#[test]
fn save_recording_to_writes_through_a_chain_of_max_links_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("rec.json");
    fs::write(&target, b"old").unwrap();
    let first = link_chain(dir.path(), MAX_LINKS);
    save_recording_to(&first.to_string_lossy(), &Recording::test_fixture(), None).unwrap();
    assert!(
        fs::read(&target).unwrap().starts_with(b"{"),
        "the end of the chain was not written"
    );
}

#[cfg(unix)]
#[test]
fn save_recording_to_refuses_a_chain_of_more_than_max_links_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("rec.json");
    fs::write(&target, b"keep").unwrap();
    let first = link_chain(dir.path(), MAX_LINKS + 1);
    let err =
        save_recording_to(&first.to_string_lossy(), &Recording::test_fixture(), None).unwrap_err();
    assert!(matches!(err, NleError::SaveError(_)), "got {err}");
    assert!(err.to_string().contains("symbolic links"), "got {err}");
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn save_recording_to_refuses_a_json_link_to_a_gzip_name() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("b.json.gz");
    fs::write(&target, b"keep").unwrap();
    let link = dir.path().join("a.json");
    std::os::unix::fs::symlink("b.json.gz", &link).unwrap();
    let err = save_recording_to(
        &link.to_string_lossy(),
        &Recording::test_fixture(),
        Some(false),
    )
    .unwrap_err();
    assert!(matches!(err, NleError::SaveError(_)), "got {err}");
    assert!(err.to_string().contains("must end in .json"), "got {err}");
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn save_recording_to_refuses_a_gzip_link_to_a_json_name() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("b.json");
    fs::write(&target, b"keep").unwrap();
    let link = dir.path().join("a.json.gz");
    std::os::unix::fs::symlink("b.json", &link).unwrap();
    let err = save_recording_to(
        &link.to_string_lossy(),
        &Recording::test_fixture(),
        Some(true),
    )
    .unwrap_err();
    assert!(matches!(err, NleError::SaveError(_)), "got {err}");
    assert!(
        err.to_string().contains("must end in .json.gz"),
        "got {err}"
    );
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn video_export_args_refuses_an_mp4_link_to_a_gif_name() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let link = dir.path().join("out.mp4");
    std::os::unix::fs::symlink(dir.path().join("real.gif"), &link).unwrap();
    let err = video_export_args(&options(
        &input,
        &link.to_string_lossy(),
        ExportFormat::Video,
    ))
    .unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(err.to_string().contains("must end in .mp4"), "got {err}");
}

#[cfg(unix)]
#[test]
fn video_export_args_accepts_a_link_whose_ending_differs_only_in_case() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let link = dir.path().join("OUT.MP4");
    std::os::unix::fs::symlink(dir.path().join("real.mp4"), &link).unwrap();
    let args = video_export_args(&options(
        &input,
        &link.to_string_lossy(),
        ExportFormat::Video,
    ))
    .unwrap();
    let at = args.iter().position(|a| a == "--output").unwrap();
    assert_eq!(args[at + 1], canonical_out(&dir, "real.mp4"));
}

#[cfg(unix)]
#[test]
fn video_export_args_gives_beta_the_target_of_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let link = dir.path().join("out.mp4");
    std::os::unix::fs::symlink(dir.path().join("real.mp4"), &link).unwrap();
    let args = video_export_args(&options(
        &input,
        &link.to_string_lossy(),
        ExportFormat::Video,
    ))
    .unwrap();
    let expected = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("real.mp4")
        .to_string_lossy()
        .to_string();
    let at = args.iter().position(|a| a == "--output").unwrap();
    assert_eq!(args[at + 1], expected);
}

#[cfg(unix)]
#[test]
fn web_export_args_refuses_a_link_to_a_file_without_a_web_ending() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let target = dir.path().join("notes.txt");
    fs::write(&target, b"keep").unwrap();
    let link = dir.path().join("page.html");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let err =
        web_export_args(&options(&input, &link.to_string_lossy(), ExportFormat::Web)).unwrap_err();
    assert!(matches!(err, NleError::ExportError(_)), "got {err}");
    assert!(err.to_string().contains("must end in"), "got {err}");
}

/// A link whose chain ends at a path that is not valid UTF-8 is refused.
/// `beta` takes the output path as text, and a lossy text names a different
/// file. APFS keeps such a target in a link, but it refuses such a file name,
/// thus each link ends at a missing file.
#[cfg(unix)]
#[test]
fn export_args_refuse_a_link_to_a_name_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.json");
    write_fixture(&input, false);
    let cases: [(&str, &[u8], ExportFormat); 2] = [
        ("out.mp4", b"x\xff.mp4", ExportFormat::Video),
        ("page.html", b"x\xff.html", ExportFormat::Web),
    ];
    for (name, target, format) in cases {
        let link = dir.path().join(name);
        std::os::unix::fs::symlink(OsStr::from_bytes(target), &link).unwrap();
        let export = options(&input, &link.to_string_lossy(), format);
        let result = match export.format {
            ExportFormat::Video => video_export_args(&export),
            ExportFormat::Web => web_export_args(&export),
        };
        let Err(NleError::ExportError(text)) = result else {
            panic!("{name}: expected an ExportError, got {result:?}");
        };
        assert!(text.contains("not valid UTF-8"), "{name}: {text}");
    }
}

#[test]
fn run_blocking_runs_the_work_on_another_thread() {
    let caller = std::thread::current().id();
    let worker =
        block_on(async { run_blocking(|| Ok(std::thread::current().id())).await }).unwrap();
    assert_ne!(caller, worker);
}

#[test]
fn run_blocking_runs_the_work_on_the_blocking_pool_of_the_callers_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_name("nle-run-blocking-test")
        .enable_all()
        .build()
        .unwrap();
    let name = runtime
        .block_on(run_blocking(|| {
            Ok(std::thread::current().name().map(str::to_owned))
        }))
        .unwrap();
    assert_eq!(name.as_deref(), Some("nle-run-blocking-test"));
}

#[test]
fn run_blocking_turns_a_panic_into_a_task_error() {
    let result: Result<(), NleError> = block_on(async { run_blocking(|| panic!("boom")).await });
    assert!(
        matches!(result, Err(NleError::TaskError(_))),
        "got {result:?}"
    );
}

#[test]
fn load_recording_command_returns_the_recording() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.json");
    write_fixture(&file, false);
    let loaded = block_on(load_recording(file.to_string_lossy().to_string())).unwrap();
    assert_eq!(loaded.metadata.event_count, 2);
}

#[test]
fn list_recordings_command_lists_one_saved_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(&dir.path().join("a.json"), false);
    let listed = block_on(list_recordings(dir.path().to_string_lossy().to_string())).unwrap();
    assert_eq!(listed.len(), 1);
}

const FINDER_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
const HOMEBREW_ARM_BIN: &str = "/opt/homebrew/bin";
const HOMEBREW_INTEL_BIN: &str = "/usr/local/bin";

// The names of the variables, written here again and not taken from the code
// under test, so that a wrong name in that code fails these tests.
const PATH_NAME: &str = "PATH";
const CARGO_HOME_NAME: &str = "CARGO_HOME";
const HOME_NAME: &str = "HOME";

/// A lookup over fixed pairs, in place of `std::env::var_os`.
fn vars(pairs: &[(&'static str, &str)]) -> impl Fn(&'static str) -> Option<OsString> {
    let map: HashMap<&'static str, OsString> = pairs
        .iter()
        .map(|(name, value)| (*name, OsString::from(value)))
        .collect();
    move |name| map.get(name).cloned()
}

fn split(path: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(path).collect()
}

fn paths(entries: &[&str]) -> Vec<PathBuf> {
    entries.iter().map(PathBuf::from).collect()
}

/// A `beta` that prints its arguments, as `echo` does.
#[cfg(unix)]
const ECHO_BETA: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\"\n";

/// A `beta` that succeeds.
#[cfg(unix)]
const SUCCEEDING_BETA: &str = "#!/bin/sh\nexit 0\n";

/// A `beta` that prints its arguments on stderr, then fails.
#[cfg(unix)]
pub(crate) const FAILING_BETA: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >&2\nexit 1\n";

/// A `beta` that succeeds only when its stdin is `/dev/null`.
#[cfg(unix)]
const NULL_STDIN_BETA: &str = "#!/bin/sh\n\
    if [ /dev/fd/0 -ef /dev/null ]; then exit 0; fi\n\
    echo 'the stdin of beta is not /dev/null' >&2\n\
    exit 1\n";

/// A temporary home whose `.cargo/bin` holds a `beta` script with the text
/// `script`.
///
/// A script, not a link to a system program: the Rust coreutils of Ubuntu
/// 26.04 refuse to run under a name other than their own. The writer of
/// `gitscratch` stops the ETXTBSY race of a written script on Linux.
#[cfg(unix)]
pub(crate) fn home_with_beta_script(script: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".cargo").join("bin");
    fs::create_dir_all(&bin).unwrap();
    gitscratch::testing::executable::write_executable(bin.join("beta"), script).unwrap();
    home
}

/// The variables of a run from Finder, with `home` as the home directory.
#[cfg(unix)]
pub(crate) fn finder_vars(home: &tempfile::TempDir) -> impl Fn(&'static str) -> Option<OsString> {
    vars(&[
        (PATH_NAME, FINDER_PATH),
        (HOME_NAME, home.path().to_str().unwrap()),
    ])
}

/// True when `name` is a variable that `childenv::command` removes.
fn is_git_variable(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(childenv::GIT_ENVIRONMENT_PREFIX))
}

#[cfg(unix)]
#[test]
fn beta_command_finds_beta_in_cargo_bin_from_the_finder_path() {
    let home = home_with_beta_script(ECHO_BETA);

    let output = beta_command(&["found-beta".to_string()], finder_vars(&home))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "the stub beta failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"found-beta\n");
}

#[cfg(unix)]
#[test]
fn run_beta_returns_the_output_path_when_beta_succeeds() {
    let home = home_with_beta_script(SUCCEEDING_BETA);

    let result = block_on(run_beta(&[], "out.mp4", finder_vars(&home), |_| {}));

    assert_eq!(result.unwrap(), "out.mp4");
}

#[cfg(unix)]
#[test]
fn run_beta_reports_the_stderr_of_a_failed_beta() {
    let home = home_with_beta_script(FAILING_BETA);
    let missing = home.path().join("no-such-file");
    let args = vec![missing.to_string_lossy().to_string()];

    let result = block_on(run_beta(&args, "out.mp4", finder_vars(&home), |_| {}));

    let Err(NleError::ExportError(text)) = result else {
        panic!("expected an ExportError, got {result:?}");
    };
    assert!(text.contains("beta export failed"), "got {text}");
    assert!(text.contains("no-such-file"), "got {text}");
}

/// A `beta` that prints one frame line, then waits for the marker file that
/// its first argument names. It gives up after about 20 seconds.
#[cfg(unix)]
const WAITING_BETA: &str = "#!/bin/sh\n\
    echo 'Processed frame 1 / 2'\n\
    i=0; while [ ! -e \"$1\" ]; do i=$((i+1)); \
    if [ \"$i\" -gt 200 ]; then echo 'no progress arrived' >&2; exit 1; fi; \
    sleep 0.1; done; exit 0\n";

#[cfg(unix)]
#[test]
fn run_beta_reports_progress_while_beta_runs() {
    let home = home_with_beta_script(WAITING_BETA);
    let marker = home.path().join("progress-arrived");
    let args = vec![marker.to_string_lossy().to_string()];
    let mut reported = Vec::new();

    let result = block_on(run_beta(&args, "out.mp4", finder_vars(&home), |progress| {
        reported.push(progress);
        fs::write(&marker, b"").unwrap();
    }));

    assert_eq!(result.unwrap(), "out.mp4");
    assert_eq!(reported, [ExportProgress::Frames { done: 1, total: 2 }]);
}

/// The variable that makes the probe test run. It is set only by the outer
/// test, which runs this test binary again.
#[cfg(unix)]
const STDIN_PROBE_VAR: &str = "NLE_STDIN_PROBE";

/// Runs `run_beta` with a `beta` that exits 0 only when its stdin is
/// `/dev/null`. The stdin of `beta` is the stdin of this process unless the
/// code changes it, so only the outer test gives a meaningful answer.
#[cfg(unix)]
#[test]
#[ignore = "the outer test runs it in a re-run of this binary, with a pipe as stdin"]
fn stdin_probe_runs_beta_with_the_stdin_of_this_process() {
    if std::env::var_os(STDIN_PROBE_VAR).is_none() {
        return;
    }
    let home = home_with_beta_script(NULL_STDIN_BETA);

    let result = block_on(run_beta(&[], "out.mp4", finder_vars(&home), |_| {}));

    assert_eq!(result.unwrap(), "out.mp4");
}

/// `cargo test` can run with `/dev/null` as its stdin, and then a test that
/// runs `beta` in this process passes without any fix. So this test runs the
/// probe in a re-run of this binary, with a pipe as its stdin.
#[cfg(unix)]
#[test]
fn run_beta_gives_beta_a_null_stdin_when_the_app_stdin_is_a_pipe() {
    let (_, module) = module_path!().split_once("::").unwrap();
    let name = format!("{module}::stdin_probe_runs_beta_with_the_stdin_of_this_process");

    let output = childenv::fixture_command(std::env::current_exe().unwrap())
        .args([
            name.as_str(),
            "--exact",
            "--ignored",
            "--color",
            "never",
            "--test-threads",
            "1",
        ])
        .env(STDIN_PROBE_VAR, "1")
        .stdin(std::process::Stdio::piped())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the probe failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("1 passed"),
        "the probe did not run\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn beta_command_sets_only_a_path_from_the_cargo_home_and_home_vars() {
    let args = vec!["web".to_string(), "in.cast".to_string()];

    let command = beta_command(
        &args,
        vars(&[
            (PATH_NAME, "/a"),
            (CARGO_HOME_NAME, "/cargo-home"),
            (HOME_NAME, "/home-dir"),
        ]),
    );

    assert_eq!(command.get_program(), "beta");
    assert_eq!(
        command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>(),
        args
    );
    let expected = std::env::join_paths(paths(&[
        "/a",
        "/cargo-home/bin",
        "/home-dir/.cargo/bin",
        HOMEBREW_ARM_BIN,
        HOMEBREW_INTEL_BIN,
    ]))
    .unwrap();
    // `childenv::command` removes each inherited GIT_ variable, and a removal
    // shows as a name with no value. Under a git hook there are some.
    let envs: Vec<_> = command.get_envs().collect();
    let set: Vec<_> = envs.iter().filter(|(_, value)| value.is_some()).collect();
    assert_eq!(set, [&(OsStr::new(PATH_NAME), Some(expected.as_os_str()))]);
    for (name, value) in envs.iter().filter(|(_, value)| value.is_none()) {
        assert!(
            is_git_variable(name),
            "{name:?} is removed ({value:?}) but is not a GIT_ variable"
        );
    }
}

#[test]
fn beta_command_sets_no_path_when_no_path_can_be_built() {
    let command = beta_command(&[], vars(&[(PATH_NAME, "/a"), (CARGO_HOME_NAME, "/x:y")]));

    assert!(
        command.get_envs().all(|(_, value)| value.is_none()),
        "a variable is set: {:?}",
        command.get_envs().collect::<Vec<_>>()
    );
}

#[test]
fn beta_path_lists_app_then_cargo_then_homebrew_dirs() {
    let path = beta_path_env(vars(&[
        (PATH_NAME, "/a:/b"),
        (CARGO_HOME_NAME, "/cargo-home"),
        (HOME_NAME, "/home-dir"),
    ]))
    .unwrap();

    assert_eq!(
        split(&path),
        paths(&[
            "/a",
            "/b",
            "/cargo-home/bin",
            "/home-dir/.cargo/bin",
            HOMEBREW_ARM_BIN,
            HOMEBREW_INTEL_BIN,
        ])
    );
}

#[test]
fn beta_path_adds_no_directory_that_is_already_an_entry() {
    let app_path = format!("/z:{HOMEBREW_INTEL_BIN}:/home-dir/.cargo/bin");
    let path = beta_path_env(vars(&[(PATH_NAME, &app_path), (HOME_NAME, "/home-dir")])).unwrap();

    assert_eq!(
        split(&path),
        paths(&[
            "/z",
            HOMEBREW_INTEL_BIN,
            "/home-dir/.cargo/bin",
            HOMEBREW_ARM_BIN,
        ])
    );
}

#[test]
fn beta_path_with_no_inputs_holds_only_the_homebrew_dirs() {
    let path = beta_path_env(vars(&[])).unwrap();

    assert_eq!(split(&path), paths(HOMEBREW_BIN_DIRS));
}

#[test]
fn beta_path_is_none_when_a_directory_holds_the_separator() {
    let path = beta_path_env(vars(&[(PATH_NAME, "/a"), (CARGO_HOME_NAME, "/x:y")]));

    assert_eq!(path, None);
}
