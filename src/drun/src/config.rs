//! Which Docker hosts `drun` uses, and how long it waits for each.
//!
//! The hosts come from the `DRUN_CONTEXTS` environment variable when it is set
//! and not empty, and from the `contexts` key of the config file when it is
//! not. The config file is `$XDG_CONFIG_HOME/drun/config.toml`, or
//! `~/.config/drun/config.toml` when `XDG_CONFIG_HOME` does not hold an
//! absolute path.

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::host::{ContextName, InvalidContextName};

/// The environment variable that names the hosts, separated by commas.
pub const CONTEXTS_VARIABLE: &str = "DRUN_CONTEXTS";

/// The name of the config file, in the `drun` directory of the config home.
const CONFIG_FILE_NAME: &str = "config.toml";

/// How long `drun` waits for a host when the config file does not say.
const DEFAULT_HOST_TIMEOUT: Duration = Duration::from_secs(10);

/// The hosts and the time limit that `drun` works with.
#[derive(Debug)]
pub struct Config {
    contexts: Vec<ContextName>,
    host_timeout: Duration,
}

/// The parts of the process environment that decide the configuration.
///
/// The tests give their own values, so no test changes the environment of the
/// test process, which other tests share.
#[derive(Debug, Default)]
pub struct Environment {
    /// The value of `DRUN_CONTEXTS`.
    pub contexts: Option<OsString>,
    /// The value of `XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<OsString>,
    /// The value of `HOME`.
    pub home: Option<OsString>,
}

/// The keys of the config file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    contexts: Option<Vec<String>>,
    host_timeout_secs: Option<u64>,
}

/// Why `drun` has no usable configuration.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// Neither `DRUN_CONTEXTS` nor the config file names a host.
    #[error(
        "no hosts are set. Set {CONTEXTS_VARIABLE}=box-a,box-b, or write \
         contexts = [\"box-a\", \"box-b\"] in {}. The command `docker context ls` \
         shows the Docker contexts on this computer",
        describe_path(.path.as_deref())
    )]
    NoContexts {
        /// Where `drun` looked for the config file.
        path: Option<PathBuf>,
    },
    /// A host is not a Docker context name.
    #[error(transparent)]
    InvalidName(#[from] InvalidContextName),
    /// The same host is in the list more than one time.
    #[error("the host \"{0}\" is in the list more than one time")]
    Duplicate(ContextName),
    /// The config file sets a time limit of zero.
    #[error("host_timeout_secs in {} must be 1 or more", .0.display())]
    ZeroTimeout(PathBuf),
    /// The config file exists, but `drun` cannot read it.
    #[error("cannot read {}: {message}", .path.display())]
    Read {
        /// The config file.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
    /// The config file is not correct TOML, or it has a key that `drun` does
    /// not know.
    #[error("{} is not a correct config file: {message}", .path.display())]
    Parse {
        /// The config file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// `DRUN_CONTEXTS` holds bytes that are not UTF-8.
    #[error("{CONTEXTS_VARIABLE} is not valid UTF-8")]
    VariableNotUnicode,
}

/// Words for the path of the config file, for an error message.
fn describe_path(path: Option<&Path>) -> String {
    match path {
        Some(path) => path.display().to_string(),
        None => "the config file (HOME is not set)".to_owned(),
    }
}

impl Environment {
    /// Reads the three variables from the environment of this process.
    pub fn from_process() -> Self {
        Self {
            contexts: std::env::var_os(CONTEXTS_VARIABLE),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            home: std::env::var_os("HOME"),
        }
    }

    /// The path of the config file.
    ///
    /// It is under `XDG_CONFIG_HOME` when that variable holds an absolute
    /// path, as the XDG base directory rules say, and under `~/.config` when
    /// it does not. It is `None` when neither variable gives a directory.
    pub fn config_path(&self) -> Option<PathBuf> {
        let xdg = self
            .xdg_config_home
            .as_deref()
            .map(Path::new)
            .filter(|directory| directory.is_absolute());
        let base = if let Some(directory) = xdg {
            directory.to_path_buf()
        } else {
            let home = self.home.as_deref().filter(|home| !home.is_empty())?;
            Path::new(home).join(".config")
        };
        Some(base.join("drun").join(CONFIG_FILE_NAME))
    }
}

impl Config {
    /// Reads the configuration that `environment` points at.
    ///
    /// # Errors
    ///
    /// Returns a [`LoadError`] when no host is set, when a host is not a
    /// Docker context name or is in the list two times, or when the config
    /// file exists and is not correct.
    pub fn load(environment: &Environment) -> Result<Self, LoadError> {
        let path = environment.config_path();
        let file = match &path {
            Some(path) => read_file(path)?,
            None => None,
        };
        let names: Vec<String> = match environment
            .contexts
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            Some(value) => {
                let text = value.to_str().ok_or(LoadError::VariableNotUnicode)?;
                text.split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            None => file
                .as_ref()
                .and_then(|file| file.contexts.clone())
                .unwrap_or_default(),
        };
        if names.is_empty() {
            return Err(LoadError::NoContexts { path });
        }
        let mut contexts: Vec<ContextName> = Vec::with_capacity(names.len());
        for name in &names {
            let context = ContextName::parse(name)?;
            if contexts.contains(&context) {
                return Err(LoadError::Duplicate(context));
            }
            contexts.push(context);
        }
        let host_timeout = match (file.and_then(|file| file.host_timeout_secs), path) {
            (None, _) => DEFAULT_HOST_TIMEOUT,
            (Some(0), Some(path)) => return Err(LoadError::ZeroTimeout(path)),
            (Some(seconds), _) => Duration::from_secs(seconds),
        };
        Ok(Self {
            contexts,
            host_timeout,
        })
    }

    /// The hosts, in the order of the configuration.
    pub fn contexts(&self) -> &[ContextName] {
        &self.contexts
    }

    /// The longest time to wait for one host to answer.
    pub fn host_timeout(&self) -> Duration {
        self.host_timeout
    }
}

/// Reads the config file at `path`. A file that does not exist is `None`, so
/// `DRUN_CONTEXTS` alone is enough.
///
/// # Errors
///
/// Returns a [`LoadError`] when the file exists and cannot be read, or is not
/// a correct config file.
fn read_file(path: &Path) -> Result<Option<File>, LoadError> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .map_err(|error| LoadError::Parse {
                path: path.to_owned(),
                message: error.message().to_owned(),
            }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(LoadError::Read {
            path: path.to_owned(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment with a home directory of `home` and nothing more.
    fn environment_with_home(home: &Path) -> Environment {
        Environment {
            home: Some(home.as_os_str().to_owned()),
            ..Environment::default()
        }
    }

    /// Writes `text` as the config file under `home/.config`.
    fn write_config(home: &Path, text: &str) -> PathBuf {
        let directory = home.join(".config").join("drun");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE_NAME);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn names(config: &Config) -> Vec<&str> {
        config.contexts().iter().map(ContextName::as_str).collect()
    }

    #[test]
    fn the_config_file_is_under_xdg_config_home_when_it_is_absolute() {
        let environment = Environment {
            xdg_config_home: Some("/xdg".into()),
            home: Some("/home/me".into()),
            ..Environment::default()
        };
        assert_eq!(
            environment.config_path(),
            Some(PathBuf::from("/xdg/drun/config.toml"))
        );
    }

    #[test]
    fn a_relative_xdg_config_home_is_ignored() {
        let environment = Environment {
            xdg_config_home: Some("relative".into()),
            home: Some("/home/me".into()),
            ..Environment::default()
        };
        assert_eq!(
            environment.config_path(),
            Some(PathBuf::from("/home/me/.config/drun/config.toml"))
        );
    }

    #[test]
    fn without_a_home_there_is_no_config_file() {
        assert_eq!(Environment::default().config_path(), None);
        let empty_home = Environment {
            home: Some(OsString::new()),
            ..Environment::default()
        };
        assert_eq!(empty_home.config_path(), None);
    }

    #[test]
    fn the_config_file_names_the_hosts_and_the_time_limit() {
        let home = tempfile::tempdir().unwrap();
        write_config(
            home.path(),
            "contexts = [\"box-a\", \"box-b\"]\nhost_timeout_secs = 3\n",
        );
        let config = Config::load(&environment_with_home(home.path())).unwrap();
        assert_eq!(names(&config), ["box-a", "box-b"]);
        assert_eq!(config.host_timeout(), Duration::from_secs(3));
    }

    #[test]
    fn the_time_limit_is_ten_seconds_when_the_file_does_not_say() {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "contexts = [\"box-a\"]\n");
        let config = Config::load(&environment_with_home(home.path())).unwrap();
        assert_eq!(config.host_timeout(), Duration::from_secs(10));
    }

    #[test]
    fn the_variable_overrides_the_hosts_of_the_file_and_keeps_its_time_limit() {
        let home = tempfile::tempdir().unwrap();
        write_config(
            home.path(),
            "contexts = [\"box-a\"]\nhost_timeout_secs = 4\n",
        );
        let environment = Environment {
            contexts: Some(" box-b , box-c, ".into()),
            ..environment_with_home(home.path())
        };
        let config = Config::load(&environment).unwrap();
        assert_eq!(names(&config), ["box-b", "box-c"]);
        assert_eq!(config.host_timeout(), Duration::from_secs(4));
    }

    #[test]
    fn the_variable_works_without_a_config_file() {
        let home = tempfile::tempdir().unwrap();
        let environment = Environment {
            contexts: Some("box-a".into()),
            ..environment_with_home(home.path())
        };
        assert_eq!(names(&Config::load(&environment).unwrap()), ["box-a"]);
    }

    #[test]
    fn an_empty_variable_falls_back_to_the_file() {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "contexts = [\"box-a\"]\n");
        let environment = Environment {
            contexts: Some(OsString::new()),
            ..environment_with_home(home.path())
        };
        assert_eq!(names(&Config::load(&environment).unwrap()), ["box-a"]);
    }

    #[test]
    fn no_hosts_is_an_error_that_names_the_config_file() {
        let home = tempfile::tempdir().unwrap();
        let error = Config::load(&environment_with_home(home.path())).unwrap_err();
        let expected_path = home.path().join(".config/drun/config.toml");
        assert_eq!(
            error,
            LoadError::NoContexts {
                path: Some(expected_path.clone())
            }
        );
        let message = error.to_string();
        assert!(message.contains(CONTEXTS_VARIABLE), "{message}");
        assert!(
            message.contains(&expected_path.display().to_string()),
            "{message}"
        );
    }

    #[test]
    fn an_empty_list_of_hosts_is_no_hosts() {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "contexts = []\n");
        assert!(matches!(
            Config::load(&environment_with_home(home.path())),
            Err(LoadError::NoContexts { .. })
        ));
    }

    #[test]
    fn a_host_that_is_not_a_context_name_is_refused() {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "contexts = [\"box-a\", \"-x\"]\n");
        assert_eq!(
            Config::load(&environment_with_home(home.path())).unwrap_err(),
            LoadError::InvalidName(InvalidContextName("-x".to_owned()))
        );
    }

    #[test]
    fn a_host_in_the_list_two_times_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let environment = Environment {
            contexts: Some("box-a,box-b,box-a".into()),
            ..environment_with_home(home.path())
        };
        assert_eq!(
            Config::load(&environment).unwrap_err(),
            LoadError::Duplicate(ContextName::parse("box-a").unwrap())
        );
    }

    #[test]
    fn a_time_limit_of_zero_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let path = write_config(
            home.path(),
            "contexts = [\"box-a\"]\nhost_timeout_secs = 0\n",
        );
        assert_eq!(
            Config::load(&environment_with_home(home.path())).unwrap_err(),
            LoadError::ZeroTimeout(path)
        );
    }

    #[test]
    fn an_unknown_key_is_refused_so_a_typo_is_not_ignored() {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "context = [\"box-a\"]\n");
        assert!(matches!(
            Config::load(&environment_with_home(home.path())),
            Err(LoadError::Parse { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_variable_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStringExt;
        let home = tempfile::tempdir().unwrap();
        let environment = Environment {
            contexts: Some(OsString::from_vec(vec![0x62, 0xff])),
            ..environment_with_home(home.path())
        };
        assert_eq!(
            Config::load(&environment).unwrap_err(),
            LoadError::VariableNotUnicode
        );
    }
}
