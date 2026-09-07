//! Guard: the icons that `tauri::generate_context!` reads at compile time.
//!
//! The macro reads an icon off disk while it expands, so a missing file is a
//! compile error and not a run-time error. The message it gives names the file
//! and nothing else:
//!
//! ```text
//! error: proc macro panicked
//!   = help: message: failed to open icon .../src/nle/icons/icon.png:
//!           No such file or directory (os error 2)
//! ```
//!
//! The path in that message is a default. `find_icon` in `tauri-codegen` takes
//! the first entry of `bundle.icon` with the extension it wants, and falls back
//! to `icons/icon.png` when the list holds none. An empty list thus points the
//! build at a file that no configuration names. These tests obey the same rule,
//! so they fail before the build does, and they say which file is absent.

use std::path::{Path, PathBuf};

/// The path `find_icon` falls back to when `bundle.icon` names no candidate.
const DEFAULT_ICON: &str = "icons/icon.png";

/// The directory that holds `tauri.conf.json`, which every icon path is relative to.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `bundle.icon` list, in the order the configuration states it.
fn bundle_icons() -> Vec<String> {
    let path = crate_dir().join("tauri.conf.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let config: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
    config["bundle"]["icon"]
        .as_array()
        .unwrap_or_else(|| panic!("{} has no bundle.icon array", path.display()))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("bundle.icon holds {entry}, which is not a string"))
                .to_string()
        })
        .collect()
}

/// The rule `find_icon` obeys: the first entry with the extension, else the default.
fn resolve(extension: &str) -> PathBuf {
    let chosen = bundle_icons()
        .into_iter()
        .find(|icon| icon.ends_with(extension))
        .unwrap_or_else(|| DEFAULT_ICON.to_string());
    crate_dir().join(chosen)
}

/// Say which file is absent, and why the build needs it.
fn assert_exists(path: &Path, purpose: &str) {
    assert!(
        path.exists(),
        "the {purpose} is {}, which does not exist. \
         Tauri reads every icon it names off disk, so the build or the bundle fails.",
        path.display()
    );
}

#[test]
fn the_window_icon_exists() {
    assert_exists(&resolve(".png"), "window icon");
}

#[test]
fn the_macos_application_icon_exists() {
    // A macOS development build takes the first `.icns` entry, and falls back to
    // the same rule as the window icon.
    let icns = bundle_icons().into_iter().find(|icon| icon.ends_with(".icns"));
    let path = match icns {
        Some(icon) => crate_dir().join(icon),
        None => resolve(".png"),
    };
    assert_exists(&path, "macOS application icon");
}

#[test]
fn every_icon_the_configuration_names_exists() {
    for icon in bundle_icons() {
        assert_exists(&crate_dir().join(&icon), &format!("bundle icon `{icon}`"));
    }
}

#[test]
fn the_configuration_names_the_bundle_icons() {
    // The fallback covers the window icon and nothing else. A bundle needs the
    // `.icns` and the `.ico` that only this list can name, so an empty list
    // builds an application that carries no icon of its own.
    let icons = bundle_icons();
    assert!(
        !icons.is_empty(),
        "bundle.icon in {} is empty, so the bundle ships no icon",
        crate_dir().join("tauri.conf.json").display()
    );
}
