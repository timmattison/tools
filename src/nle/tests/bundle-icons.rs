//! Guard: the icons that `Dioxus.toml` names for the bundle.
//!
//! `dx bundle` packs the app with the icons that `bundle.icon` names. A missing
//! file fails only at bundle time, which is late, and the message is not about
//! the config. These tests read the same list, so they fail first and name the
//! file that is absent.

use std::path::{Path, PathBuf};

/// The directory that holds `Dioxus.toml`, which every icon path is relative to.
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The entries of `bundle.icon` in `dioxus_toml` that are not a file under
/// `crate_dir`, in the order the config states them.
///
/// Returns an error when the text is not TOML, or when it has no `bundle.icon`
/// array, or when the array is empty, or when an entry is not a string. An
/// empty list would bundle an app that carries no icon of its own.
fn missing_icons(crate_dir: &Path, dioxus_toml: &str) -> Result<Vec<String>, String> {
    let config: toml::Table = dioxus_toml
        .parse()
        .map_err(|e| format!("cannot parse the config: {e}"))?;
    let icons = config
        .get("bundle")
        .and_then(|bundle| bundle.get("icon"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "the config has no bundle.icon array".to_string())?;
    if icons.is_empty() {
        return Err("bundle.icon is empty, so the bundle ships no icon".to_string());
    }
    let mut missing = Vec::new();
    for entry in icons {
        let icon = entry
            .as_str()
            .ok_or_else(|| format!("bundle.icon holds {entry}, which is not a string"))?;
        if !crate_dir.join(icon).is_file() {
            missing.push(icon.to_string());
        }
    }
    Ok(missing)
}

#[test]
fn the_real_config_names_icons_and_each_one_exists() {
    let path = crate_dir().join("Dioxus.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let missing =
        missing_icons(&crate_dir(), &text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        missing.is_empty(),
        "{} names icons that are not files under {}: {missing:?}",
        path.display(),
        crate_dir().display()
    );
}

#[test]
fn a_missing_icon_is_reported_by_name() {
    let config = r#"
        [bundle]
        icon = ["icons/icon.png", "icons/no-such-icon.png"]
    "#;
    let missing = missing_icons(&crate_dir(), config).unwrap();
    assert_eq!(missing, ["icons/no-such-icon.png"]);
}

#[test]
fn a_config_with_no_bundle_icon_is_an_error() {
    let config = "[bundle]\nidentifier = \"com.example.app\"\n";
    let error = missing_icons(&crate_dir(), config).unwrap_err();
    assert!(error.contains("bundle.icon"), "got {error}");
}

#[test]
fn an_empty_bundle_icon_list_is_an_error() {
    let error = missing_icons(&crate_dir(), "[bundle]\nicon = []\n").unwrap_err();
    assert!(error.contains("empty"), "got {error}");
}
