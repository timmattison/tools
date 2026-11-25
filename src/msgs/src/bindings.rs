//! The TypeScript types of the IPC boundary, generated from the Rust types.
//!
//! `frontend/src/bindings.ts` is the generated file. The test
//! `bindings_match_the_rust_types` fails when the file differs from what the
//! Rust types generate, so the two cannot drift apart without an error.

use std::path::{Path, PathBuf};

use specta::TypeCollection;
use specta_typescript::{BigIntExportBehavior, Typescript};

use crate::state::CacheStatus;
use crate::types::{AttachmentInfo, Conversation, DbStatus, ExportResult, Message, SearchResult};

const BINDINGS_FILE: &str = "frontend/src/bindings.ts";
const REGENERATE_COMMAND: &str = "cargo test -p msgs -- --ignored regenerate_bindings";
const HEADER: &str = "// Generated from the Rust types by `cargo test -p msgs -- --ignored regenerate_bindings`.\n// Do not edit by hand.";

fn bindings_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(BINDINGS_FILE)
}

/// The text of `bindings.ts`: the header and each type that crosses the IPC
/// boundary.
fn typescript_bindings() -> String {
    let mut types = TypeCollection::default();
    types
        .register::<Conversation>()
        .register::<Message>()
        .register::<AttachmentInfo>()
        .register::<SearchResult>()
        .register::<ExportResult>()
        .register::<DbStatus>()
        .register::<CacheStatus>();
    Typescript::new()
        .bigint(BigIntExportBehavior::Number)
        .header(HEADER)
        .export(&types)
        .unwrap_or_else(|e| panic!("cannot export the TypeScript types: {e}"))
}

#[test]
fn bindings_match_the_rust_types() {
    let path = bindings_path();
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}. Run `{REGENERATE_COMMAND}`",
            path.display()
        )
    });
    assert!(
        committed == typescript_bindings(),
        "{} differs from the Rust types. Run `{REGENERATE_COMMAND}`",
        path.display()
    );
}

#[test]
#[ignore = "writes frontend/src/bindings.ts"]
fn regenerate_bindings() {
    std::fs::write(bindings_path(), typescript_bindings()).expect("cannot write bindings.ts");
}
