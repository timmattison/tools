//! Guard: the frontend `pnpm-workspace.yaml` must approve every build script it names.
//!
//! `pnpm` refuses to install when a dependency has a build script that is neither
//! approved nor rejected. It exits with `ERR_PNPM_IGNORED_BUILDS`, and every `pnpm`
//! script in the package fails with it, because `pnpm run` does a dependency check
//! first. That failure stops `pnpm dev`, which stops the Tauri `beforeDevCommand`,
//! which stops `scripts/run-nle.sh`.
//!
//! `pnpm` writes the `allowBuilds` key with the placeholder text
//! "set this to true or false" when it finds such a dependency. A person must then
//! replace the placeholder with a boolean. These tests fail when nobody did.

use std::fs;
use std::path::PathBuf;

use yaml_rust2::{Yaml, YamlLoader};

/// The name of the key that holds the decision for each build script.
const ALLOW_BUILDS_KEY: &str = "allowBuilds";

/// The name of the key that `pnpm` wrote before `allowBuilds` existed.
const IGNORED_BUILT_DEPENDENCIES_KEY: &str = "ignoredBuiltDependencies";

/// Read and parse the frontend `pnpm-workspace.yaml`.
fn workspace_config() -> Yaml {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("frontend")
        .join("pnpm-workspace.yaml");
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let documents = YamlLoader::load_from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
    assert_eq!(
        documents.len(),
        1,
        "{} must hold exactly one YAML document",
        path.display()
    );
    documents.into_iter().next().expect("one document")
}

#[test]
fn every_allow_builds_entry_holds_a_boolean() {
    let config = workspace_config();
    let Some(entries) = config[ALLOW_BUILDS_KEY].as_hash() else {
        // No `allowBuilds` key at all means nothing waits for a decision.
        return;
    };

    for (package, decision) in entries {
        let package = package.as_str().unwrap_or("<non-string key>");
        assert!(
            decision.as_bool().is_some(),
            "pnpm-workspace.yaml leaves the build script of `{package}` undecided \
             (its allowBuilds value is {decision:?}, not true or false). \
             pnpm then fails every command in the frontend with ERR_PNPM_IGNORED_BUILDS, \
             which breaks scripts/run-nle.sh."
        );
    }
}

#[test]
fn no_package_is_both_allowed_and_ignored() {
    let config = workspace_config();
    let Some(ignored) = config[IGNORED_BUILT_DEPENDENCIES_KEY].as_vec() else {
        return;
    };
    let allowed = config[ALLOW_BUILDS_KEY].as_hash();

    for package in ignored {
        let package = package.as_str().unwrap_or("<non-string entry>");
        let allowed_here = allowed
            .is_some_and(|entries| entries.iter().any(|(key, _)| key.as_str() == Some(package)));
        assert!(
            !allowed_here,
            "pnpm-workspace.yaml names `{package}` under both {ALLOW_BUILDS_KEY} and \
             {IGNORED_BUILT_DEPENDENCIES_KEY}. Keep one decision, not two."
        );
    }
}
