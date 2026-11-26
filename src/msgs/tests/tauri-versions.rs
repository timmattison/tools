//! The Tauri CLI refuses `cargo tauri build` when a Tauri crate and its npm
//! package differ in the major or the minor version. This test holds the
//! lockfiles of msgs to that rule: `Cargo.lock` at the repository root, and
//! `frontend/pnpm-lock.yaml`.
//!
//! The pairs are the ones the CLI compares: `tauri` with `@tauri-apps/api`, and
//! `tauri-plugin-<x>` with `@tauri-apps/plugin-<x>`.

use std::collections::BTreeMap;

use yaml_rust2::{Yaml, YamlLoader};

/// Name of the Tauri crate, and of the npm package that goes with it.
const TAURI_CRATE: &str = "tauri";
const TAURI_NPM: &str = "@tauri-apps/api";
/// Prefix of the crate name of a Tauri plugin, and of its npm package name.
const PLUGIN_CRATE_PREFIX: &str = "tauri-plugin-";
const PLUGIN_NPM_PREFIX: &str = "@tauri-apps/plugin-";
/// Name of the package under test in `Cargo.lock`.
const MSGS_PACKAGE: &str = "msgs";

/// The version of each Tauri crate that msgs depends on, by crate name.
fn crate_versions(cargo_lock: &str) -> BTreeMap<String, String> {
    let lock: toml::Table = cargo_lock.parse().expect("Cargo.lock is valid TOML");
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .expect("Cargo.lock has a package list");
    let name_of = |package: &toml::Value| {
        package
            .get("name")
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    };
    let msgs: Vec<&toml::Value> = packages
        .iter()
        .filter(|package| name_of(package).as_deref() == Some(MSGS_PACKAGE))
        .collect();
    assert_eq!(msgs.len(), 1, "Cargo.lock has one package named msgs");
    let dependencies = msgs
        .first()
        .and_then(|package| package.get("dependencies"))
        .and_then(toml::Value::as_array)
        .expect("the msgs package has dependencies");

    let mut versions = BTreeMap::new();
    for entry in dependencies {
        let entry = entry.as_str().expect("a dependency entry is a string");
        let mut words = entry.split(' ');
        let name = words.next().expect("a dependency entry has a name");
        if name != TAURI_CRATE && !name.starts_with(PLUGIN_CRATE_PREFIX) {
            continue;
        }
        let version = match words.next() {
            Some(version) => version.to_owned(),
            None => {
                let same_name: Vec<&toml::Value> = packages
                    .iter()
                    .filter(|package| name_of(package).as_deref() == Some(name))
                    .collect();
                assert_eq!(
                    same_name.len(),
                    1,
                    "Cargo.lock has more than one package named {name}, \
                     and the msgs entry does not say which"
                );
                same_name
                    .first()
                    .and_then(|package| package.get("version"))
                    .and_then(toml::Value::as_str)
                    .expect("a package has a version")
                    .to_owned()
            }
        };
        versions.insert(name.to_owned(), version);
    }
    versions
}

/// The version of each `@tauri-apps/*` package in the root importer of a
/// `pnpm-lock.yaml`, by package name.
fn npm_versions(pnpm_lock: &str) -> BTreeMap<String, String> {
    let documents = YamlLoader::load_from_str(pnpm_lock).expect("pnpm-lock.yaml is valid YAML");
    let document = documents.first().expect("pnpm-lock.yaml has a document");
    let importer = &document["importers"]["."];
    assert!(
        !importer.is_badvalue(),
        "pnpm-lock.yaml has an importers section for ."
    );
    let mut versions = BTreeMap::new();
    for section in ["dependencies", "devDependencies"] {
        let Yaml::Hash(entries) = &importer[section] else {
            continue;
        };
        for (name, entry) in entries {
            let name = name.as_str().expect("a package name is a string");
            if !name.starts_with("@tauri-apps/") {
                continue;
            }
            let version = entry["version"]
                .as_str()
                .expect("a locked package has a version");
            let version = version.split('(').next().unwrap_or(version);
            versions.insert(name.to_owned(), version.to_owned());
        }
    }
    versions
}

/// The npm package that goes with a Tauri crate, if the CLI compares one.
fn npm_name(crate_name: &str) -> Option<String> {
    if crate_name == TAURI_CRATE {
        return Some(TAURI_NPM.to_owned());
    }
    crate_name
        .strip_prefix(PLUGIN_CRATE_PREFIX)
        .map(|plugin| format!("{PLUGIN_NPM_PREFIX}{plugin}"))
}

/// The major and the minor version of `version`.
fn major_minor(version: &str) -> (u64, u64) {
    let mut parts = version.split('.');
    let mut next = || {
        parts
            .next()
            .and_then(|part| part.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("`{version}` is not a version with a major and a minor"))
    };
    let major = next();
    let minor = next();
    (major, minor)
}

/// Each pair of a Tauri crate and its npm package that differs in the major or
/// the minor version, written as `<crate> <version> : <npm> <version>`.
fn mismatches(cargo_lock: &str, pnpm_lock: &str) -> Vec<String> {
    let crates = crate_versions(cargo_lock);
    let npm = npm_versions(pnpm_lock);
    assert!(
        crates.contains_key(TAURI_CRATE) && npm.contains_key(TAURI_NPM),
        "the pair {TAURI_CRATE} / {TAURI_NPM} must be in both lockfiles, \
         or the comparison proves nothing"
    );
    let mut found = Vec::new();
    for (name, version) in &crates {
        let Some(npm_package) = npm_name(name) else {
            continue;
        };
        let Some(npm_version) = npm.get(&npm_package) else {
            continue;
        };
        if major_minor(version) != major_minor(npm_version) {
            found.push(format!("{name} {version} : {npm_package} {npm_version}"));
        }
    }
    found
}

#[test]
fn each_tauri_crate_agrees_with_its_npm_package() {
    let cargo_lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
            .expect("read Cargo.lock");
    let pnpm_lock = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/frontend/pnpm-lock.yaml"
    ))
    .expect("read pnpm-lock.yaml");
    let found = mismatches(&cargo_lock, &pnpm_lock);
    assert!(
        found.is_empty(),
        "The Tauri CLI refuses a build for these pairs:\n{}\n\
         Pin the npm package in src/msgs/frontend/package.json to the line of \
         the crate, or change the Tauri block of the root Cargo.toml.",
        found.join("\n")
    );
}

#[test]
fn a_minor_difference_is_a_mismatch_and_a_patch_difference_is_not() {
    let cargo_lock = r#"
[[package]]
name = "msgs"
version = "0.1.0"
dependencies = ["serde", "tauri", "tauri-plugin-dialog"]

[[package]]
name = "tauri"
version = "2.9.5"

[[package]]
name = "tauri-plugin-dialog"
version = "2.6.0"
"#;
    let pnpm_lock = |api: &str| {
        format!(
            "lockfileVersion: '9.0'\nimporters:\n  .:\n    dependencies:\n      '@tauri-apps/api':\n        specifier: ^2\n        version: {api}\n      '@tauri-apps/plugin-dialog':\n        specifier: ^2.6.0\n        version: 2.6.0(foo@1.0.0)\n"
        )
    };
    assert_eq!(
        mismatches(cargo_lock, &pnpm_lock("2.10.1")),
        ["tauri 2.9.5 : @tauri-apps/api 2.10.1"]
    );
    assert!(mismatches(cargo_lock, &pnpm_lock("2.9.1")).is_empty());
}
