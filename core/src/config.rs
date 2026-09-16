//! Configuration file and the per-platform directories it lives in.
//!
//! XDG directories on Linux, `%APPDATA%` on Windows. Every directory can be
//! overridden with an environment variable, which is what the tests use and what
//! lets a shared machine point one Steam user at its own state.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{APP_NAME, Error, Result};

/// Environment variable overriding the configuration directory.
pub const CONFIG_DIR_ENV: &str = "GAMESTORE_CONFIG_DIR";
/// Environment variable overriding the data directory.
pub const DATA_DIR_ENV: &str = "GAMESTORE_DATA_DIR";
/// Environment variable overriding the cache directory.
pub const CACHE_DIR_ENV: &str = "GAMESTORE_CACHE_DIR";

/// Where gamestore keeps its configuration, state and cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Holds `config.toml`.
    pub config_dir: PathBuf,
    /// Holds installed games, launch wrappers and per-game logs.
    pub data_dir: PathBuf,
    /// Holds the catalog cache and downloaded installers.
    pub cache_dir: PathBuf,
}

impl Paths {
    /// Resolve the directories from the environment and the platform defaults.
    pub fn resolve() -> Result<Self> {
        Self::resolve_with(|key| std::env::var_os(key))
    }

    /// Same as [`Paths::resolve`] with an explicit environment lookup, so tests do
    /// not have to mutate the process environment.
    pub fn resolve_with<F>(lookup: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let dirs = ProjectDirs::from("", "", APP_NAME);
        let dir = |env: &str, default: Option<&Path>, what: &'static str| -> Result<PathBuf> {
            match lookup(env) {
                Some(value) => Ok(PathBuf::from(value)),
                None => default
                    .map(Path::to_path_buf)
                    .ok_or(Error::MissingDirectory(what)),
            }
        };

        Ok(Self {
            config_dir: dir(
                CONFIG_DIR_ENV,
                dirs.as_ref().map(ProjectDirs::config_dir),
                "configuration",
            )?,
            data_dir: dir(
                DATA_DIR_ENV,
                dirs.as_ref().map(ProjectDirs::data_dir),
                "data",
            )?,
            cache_dir: dir(
                CACHE_DIR_ENV,
                dirs.as_ref().map(ProjectDirs::cache_dir),
                "cache",
            )?,
        })
    }

    /// The configuration file, whether or not it exists yet.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Where installed games go unless the configuration says otherwise.
    pub fn install_root(&self) -> PathBuf {
        self.data_dir.join("games")
    }

    /// Where per-game launch logs go; Big Picture has no terminal to read.
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}

/// The user's configuration, as read from `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Install games here instead of under the data directory.
    pub install_root: Option<PathBuf>,
    pub download: Download,
    pub ludusavi: Ludusavi,
}

/// Download behaviour. Defaults are deliberately polite: GOG rate-limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Download {
    /// Number of chunks fetched in parallel.
    pub concurrency: usize,
}

/// How to reach Ludusavi, which handles save backup and restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ludusavi {
    /// Binary name or absolute path; resolved on `PATH` when it is a bare name.
    pub binary: PathBuf,
}

impl Default for Download {
    fn default() -> Self {
        Self { concurrency: 4 }
    }
}

impl Default for Ludusavi {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("ludusavi"),
        }
    }
}

impl Config {
    /// Read the configuration file, falling back to the defaults when it is absent.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml(&text, &path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::io(format!("reading {}", path.display()), error)),
        }
    }

    /// Parse a configuration from TOML; `path` is only used in error messages.
    pub fn from_toml(text: &str, path: &Path) -> Result<Self> {
        toml::from_str(text).map_err(|source| Error::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Write the configuration, creating the configuration directory if needed.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        std::fs::create_dir_all(&paths.config_dir).map_err(|error| {
            Error::io(format!("creating {}", paths.config_dir.display()), error)
        })?;
        let path = paths.config_file();
        std::fs::write(&path, self.to_toml()?)
            .map_err(|error| Error::io(format!("writing {}", path.display()), error))
    }

    /// Render the configuration as TOML.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Where games are installed, honouring `install_root`.
    pub fn install_root(&self, paths: &Paths) -> PathBuf {
        self.install_root
            .clone()
            .unwrap_or_else(|| paths.install_root())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(dir: &Path) -> impl Fn(&str) -> Option<OsString> + '_ {
        move |key| match key {
            CONFIG_DIR_ENV => Some(dir.join("config").into()),
            DATA_DIR_ENV => Some(dir.join("data").into()),
            CACHE_DIR_ENV => Some(dir.join("cache").into()),
            _ => None,
        }
    }

    #[test]
    fn environment_overrides_win_over_platform_defaults() {
        let root = Path::new("/somewhere");
        let paths = Paths::resolve_with(overrides(root)).unwrap();

        assert_eq!(paths.config_dir, root.join("config"));
        assert_eq!(paths.config_file(), root.join("config").join("config.toml"));
        assert_eq!(paths.install_root(), root.join("data").join("games"));
        assert_eq!(paths.cache_dir, root.join("cache"));
    }

    #[test]
    fn missing_config_file_yields_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::resolve_with(overrides(temp.path())).unwrap();

        assert_eq!(Config::load(&paths).unwrap(), Config::default());
    }

    #[test]
    fn config_survives_a_save_and_load_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::resolve_with(overrides(temp.path())).unwrap();
        let config = Config {
            install_root: Some(PathBuf::from("/games")),
            download: Download { concurrency: 8 },
            ludusavi: Ludusavi {
                binary: PathBuf::from("/usr/bin/ludusavi"),
            },
        };

        config.save(&paths).unwrap();

        assert_eq!(Config::load(&paths).unwrap(), config);
        assert_eq!(config.install_root(&paths), PathBuf::from("/games"));
    }

    #[test]
    fn partial_config_keeps_the_defaults_for_what_it_omits() {
        let config =
            Config::from_toml("[download]\nconcurrency = 2\n", Path::new("config.toml")).unwrap();

        assert_eq!(config.download.concurrency, 2);
        assert_eq!(config.ludusavi, Ludusavi::default());
    }

    #[test]
    fn unknown_keys_are_rejected_instead_of_silently_ignored() {
        let error = Config::from_toml("concurrency = 2\n", Path::new("config.toml")).unwrap_err();

        assert!(matches!(error, Error::ConfigParse { .. }), "{error}");
    }
}
