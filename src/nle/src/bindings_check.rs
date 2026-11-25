//! Golden-file check for the ts-rs bindings in `frontend/src/bindings/`.
//!
//! The types in `types.rs` keep `export_to` and drop `export`, so no
//! generated test writes the binding files. These tests compare instead.
//! `NLE_UPDATE_BINDINGS=1 cargo test -p nle` writes the files.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use ts_rs::TS;

use crate::types::{
    Event, EventType, ExportFormat, ExportOptions, LoadedRecording, Recording, RecordingMetadata,
};

/// Name of the variable that makes the golden test write the files.
const UPDATE_VAR: &str = "NLE_UPDATE_BINDINGS";
/// The bindings directory, relative to the crate root.
const BINDINGS_DIR: &str = "frontend/src/bindings";
/// The barrel file that is kept by hand.
const INDEX_FILE: &str = "index.ts";
/// Extension of a binding file.
const TS_EXTENSION: &str = "ts";
/// What to run to regenerate the bindings.
const REGENERATE_HINT: &str =
    "run NLE_UPDATE_BINDINGS=1 cargo test -p nle to regenerate the bindings";

fn bindings_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BINDINGS_DIR)
}

fn rendered<T: TS + 'static>() -> (String, String) {
    let text = T::export_to_string().expect("ts-rs renders the type");
    (T::ident(), text)
}

/// The one list of exported types: the identifier and the rendered text.
fn rendered_types() -> Vec<(String, String)> {
    vec![
        rendered::<Recording>(),
        rendered::<Event>(),
        rendered::<EventType>(),
        rendered::<ExportOptions>(),
        rendered::<ExportFormat>(),
        rendered::<LoadedRecording>(),
        rendered::<RecordingMetadata>(),
    ]
}

#[test]
fn committed_bindings_match_the_rust_types() {
    let update = std::env::var(UPDATE_VAR).is_ok_and(|v| v == "1");
    for (ident, text) in rendered_types() {
        let path = bindings_dir().join(format!("{ident}.{TS_EXTENSION}"));
        if update {
            fs::write(&path, &text).expect("write the binding file");
            continue;
        }
        let committed = fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            committed,
            text,
            "{} differs from the Rust type {ident}: {REGENERATE_HINT}",
            path.display()
        );
    }
}

#[test]
fn bindings_index_names_each_generated_file() {
    let dir = bindings_dir();
    let listed: BTreeSet<String> = rendered_types()
        .into_iter()
        .map(|(ident, _)| ident)
        .collect();
    let on_disk: BTreeSet<String> = fs::read_dir(&dir)
        .expect("read the bindings directory")
        .map(|entry| entry.expect("read a directory entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == TS_EXTENSION))
        .filter(|path| path.file_name().is_some_and(|n| n != INDEX_FILE))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(on_disk, listed, "binding files and exported types differ");

    let index = fs::read_to_string(dir.join(INDEX_FILE)).expect("read index.ts");
    let lines: BTreeSet<&str> = index.lines().collect();
    for ident in &listed {
        let line = format!("export type {{ {ident} }} from \"./{ident}\";");
        assert!(
            lines.contains(line.as_str()),
            "{INDEX_FILE} lacks the line: {line}"
        );
    }
}
