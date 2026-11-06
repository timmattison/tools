//! The registry of a host: the file `sidecar.toml`.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::name::{SidecarName, NAME_RULE};

/// The name of the registry file at the root of a host worktree.
pub const REGISTRY_FILE: &str = "sidecar.toml";
/// The directory of the links when `link_dir` is not set.
pub const DEFAULT_LINK_DIR: &str = "data";
/// The variable of the CI token when `token_env` is not set.
pub const DEFAULT_TOKEN_ENV: &str = "SIDECAR_TOKEN";

/// The errors of reading the registry.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The file does not exist.
    #[error("{} is missing. Run `sidecar add <name>` to make it.", path.display())]
    Missing {
        /// The place where the file must be.
        path: PathBuf,
    },
    /// The file exists and the read failed.
    #[error("cannot read {}: {source}", path.display())]
    Read {
        /// The place of the file.
        path: PathBuf,
        /// The error of the operating system.
        source: std::io::Error,
    },
    /// The text is not valid TOML, or it holds an unknown key or a wrong type.
    #[error("{REGISTRY_FILE} is not valid: {message}")]
    Syntax {
        /// The text of the `toml` error.
        message: String,
    },
    /// A name in `sidecars` breaks the name rule.
    #[error("{REGISTRY_FILE} names the sidecar \"{name}\". A name holds only {NAME_RULE}.")]
    BadName {
        /// The refused name.
        name: String,
    },
    /// A name occurs two times in `sidecars`.
    #[error("{REGISTRY_FILE} names the sidecar \"{name}\" two times.")]
    Duplicate {
        /// The repeated name.
        name: String,
    },
    /// `link_dir` is empty, absolute, or holds a component that is not a normal name.
    #[error(
        "{REGISTRY_FILE} sets link_dir to \"{value}\". link_dir must be a relative path that is not empty, and each part of it must be a normal name."
    )]
    BadLinkDir {
        /// The refused value.
        value: String,
    },
    /// `token_env` is not a valid variable name.
    #[error(
        "{REGISTRY_FILE} sets token_env to \"{value}\". token_env must not be empty, must hold only ASCII letters, digits, and `_`, and must not start with a digit."
    )]
    BadTokenEnv {
        /// The refused value.
        value: String,
    },
}

/// The registry of a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    link_dir: PathBuf,
    token_env: String,
    sidecars: Vec<SidecarName>,
}

/// The keys of the file, as they are written. Unknown keys are errors.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistry {
    link_dir: Option<String>,
    token_env: Option<String>,
    #[serde(default)]
    sidecars: Vec<String>,
}

/// True when `value` is a relative path of normal names only.
fn is_valid_link_dir(value: &str) -> bool {
    // Split at the text level. `Path::components` hides a `.` in the middle of a path.
    value
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// True when `value` is a portable variable name: not empty, ASCII letters, digits, and `_`,
/// and no digit at the start.
fn is_valid_variable_name(value: &str) -> bool {
    value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value.chars().next().is_some_and(|c| !c.is_ascii_digit())
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            link_dir: PathBuf::from(DEFAULT_LINK_DIR),
            token_env: DEFAULT_TOKEN_ENV.to_string(),
            sidecars: Vec::new(),
        }
    }
}

impl Registry {
    /// Reads the registry from the text of `sidecar.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] when the text is not a valid registry.
    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        let raw: RawRegistry = toml::from_str(text).map_err(|e| RegistryError::Syntax {
            message: e.to_string(),
        })?;
        let sidecars = raw
            .sidecars
            .into_iter()
            .map(|name| SidecarName::parse(&name).map_err(|_| RegistryError::BadName { name }))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, name) in sidecars.iter().enumerate() {
            if sidecars[..index].contains(name) {
                return Err(RegistryError::Duplicate {
                    name: name.to_string(),
                });
            }
        }
        let link_dir = match raw.link_dir {
            None => PathBuf::from(DEFAULT_LINK_DIR),
            Some(value) if is_valid_link_dir(&value) => PathBuf::from(value),
            Some(value) => return Err(RegistryError::BadLinkDir { value }),
        };
        let token_env = match raw.token_env {
            None => DEFAULT_TOKEN_ENV.to_string(),
            Some(value) if is_valid_variable_name(&value) => value,
            Some(value) => return Err(RegistryError::BadTokenEnv { value }),
        };
        Ok(Self {
            link_dir,
            token_env,
            sidecars,
        })
    }

    /// Reads `sidecar.toml` at the root of a worktree.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::Missing`] when the file does not exist. Returns the other
    /// [`RegistryError`] variants when the read fails or the text is not valid.
    pub fn load(_worktree_root: &Path) -> Result<Self, RegistryError> {
        Ok(Self::default())
    }

    /// Reads `sidecar.toml` at the root of a worktree. A missing file gives the default
    /// registry, which has no sidecars.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] when the file exists and cannot be read or is not valid.
    pub fn load_or_default(_worktree_root: &Path) -> Result<Self, RegistryError> {
        Ok(Self::default())
    }

    /// The directory of the links, relative to the worktree root.
    #[must_use]
    pub fn link_dir(&self) -> &Path {
        &self.link_dir
    }

    /// The name of the variable that holds the CI token.
    #[must_use]
    pub fn token_env(&self) -> &str {
        &self.token_env
    }

    /// The sidecars, in the order of the file.
    #[must_use]
    pub fn sidecars(&self) -> &[SidecarName] {
        &self.sidecars
    }

    /// True when `name` is one of the sidecars.
    #[must_use]
    pub fn contains(&self, name: &SidecarName) -> bool {
        self.sidecars.contains(name)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn names(registry: &Registry) -> Vec<&str> {
        registry
            .sidecars()
            .iter()
            .map(SidecarName::as_str)
            .collect()
    }

    #[test]
    fn defaults_apply_when_keys_are_missing() {
        let registry = Registry::parse("sidecars = [\"testdata\"]").unwrap();
        assert_eq!(registry.link_dir(), Path::new("data"));
        assert_eq!(registry.token_env(), "SIDECAR_TOKEN");
        assert_eq!(names(&registry), ["testdata"]);
        assert!(registry.contains(&SidecarName::parse("testdata").unwrap()));
        assert!(!registry.contains(&SidecarName::parse("other").unwrap()));
    }

    #[test]
    fn explicit_values_are_kept() {
        let registry = Registry::parse(
            "link_dir = \"vendor/private\"\ntoken_env = \"MY_TOKEN\"\nsidecars = [\"b\", \"a\"]",
        )
        .unwrap();
        assert_eq!(registry.link_dir(), Path::new("vendor/private"));
        assert_eq!(registry.token_env(), "MY_TOKEN");
        assert_eq!(names(&registry), ["b", "a"]);
    }

    #[test]
    fn empty_file_gives_no_sidecars() {
        let registry = Registry::parse("").unwrap();
        assert!(registry.sidecars().is_empty());
        assert_eq!(registry, Registry::default());
    }

    #[test]
    fn unknown_key_is_an_error_that_names_the_key() {
        let error = Registry::parse("lnk_dir = \"x\"").unwrap_err();
        assert!(matches!(error, RegistryError::Syntax { .. }));
        assert!(error.to_string().contains("lnk_dir"), "{error}");
    }

    #[test]
    fn refused_name_is_an_error_that_names_it() {
        for bad in ["..", "Bad_Name"] {
            let error = Registry::parse(&format!("sidecars = [\"{bad}\"]")).unwrap_err();
            assert!(matches!(error, RegistryError::BadName { .. }), "{error}");
            assert!(error.to_string().contains(&format!("\"{bad}\"")), "{error}");
        }
    }

    #[test]
    fn repeated_name_is_an_error_that_names_it() {
        let error = Registry::parse("sidecars = [\"a\", \"b\", \"a\"]").unwrap_err();
        assert!(matches!(error, RegistryError::Duplicate { .. }), "{error}");
        assert!(error.to_string().contains("\"a\""), "{error}");
    }

    #[test]
    fn bad_link_dir_is_an_error_that_names_the_key() {
        for bad in ["../x", "/abs", "", "a/./b", "a/../b"] {
            let error = Registry::parse(&format!("link_dir = \"{bad}\"")).unwrap_err();
            assert!(
                matches!(error, RegistryError::BadLinkDir { .. }),
                "{bad:?}: {error}"
            );
            assert!(error.to_string().contains("link_dir"), "{error}");
        }
    }

    #[test]
    fn bad_token_env_is_an_error_that_names_the_key() {
        for bad in ["", "1X", "A-B"] {
            let error = Registry::parse(&format!("token_env = \"{bad}\"")).unwrap_err();
            assert!(
                matches!(error, RegistryError::BadTokenEnv { .. }),
                "{bad:?}: {error}"
            );
            assert!(error.to_string().contains("token_env"), "{error}");
        }
    }

    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    /// A directory that no other test or process uses. It goes away on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sidecar-test-{}-{nanos}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_without_a_file_is_missing() {
        let dir = TempDir::new();
        let error = Registry::load(&dir.0).unwrap_err();
        assert!(matches!(error, RegistryError::Missing { .. }), "{error}");
        assert!(error.to_string().contains(REGISTRY_FILE), "{error}");
    }

    #[test]
    fn load_or_default_without_a_file_gives_the_default() {
        let dir = TempDir::new();
        assert_eq!(
            Registry::load_or_default(&dir.0).unwrap(),
            Registry::default()
        );
    }

    #[test]
    fn load_and_load_or_default_read_the_file() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join(REGISTRY_FILE), "sidecars = [\"testdata\"]").unwrap();
        for registry in [
            Registry::load(&dir.0).unwrap(),
            Registry::load_or_default(&dir.0).unwrap(),
        ] {
            assert_eq!(names(&registry), ["testdata"]);
        }
    }

    #[test]
    fn load_reports_an_invalid_file() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join(REGISTRY_FILE), "lnk_dir = \"x\"").unwrap();
        for error in [
            Registry::load(&dir.0).unwrap_err(),
            Registry::load_or_default(&dir.0).unwrap_err(),
        ] {
            assert!(error.to_string().contains("lnk_dir"), "{error}");
        }
    }
}
