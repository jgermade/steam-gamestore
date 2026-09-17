//! Linux side of gamestore: the Steam installation, and later the `shortcuts.vdf`
//! writer, the `CompatToolMapping` writer and the generated launch wrappers.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use gamestore_core::platform::{STEAM_ROOT_ENV, first_existing};
use gamestore_core::{Error, Platform, Result};

/// Steam on Linux, native or Flatpak.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn id(&self) -> &'static str {
        "linux"
    }

    fn steam_root(&self) -> Result<PathBuf> {
        let home = BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .ok_or(Error::MissingDirectory("home"))?;

        steam_root_in(&home, |key| std::env::var_os(key))
    }

    fn uses_proton(&self) -> bool {
        true
    }
}

/// Steam locations to try, in order, under `home`.
pub fn steam_root_candidates(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".steam/root"),
        // Flatpak Steam, which Bazzite installs by default.
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
}

/// Resolve the Steam root under `home`, letting `STEAM_ROOT` win when it is set.
pub fn steam_root_in<F>(home: &Path, lookup: F) -> Result<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(root) = lookup(STEAM_ROOT_ENV) {
        return Ok(PathBuf::from(root));
    }

    first_existing(steam_root_candidates(home)).ok_or(Error::SteamNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn the_first_existing_candidate_wins() {
        let home = tempfile::tempdir().unwrap();
        let flatpak = home
            .path()
            .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        let native = home.path().join(".local/share/Steam");
        std::fs::create_dir_all(&flatpak).unwrap();
        std::fs::create_dir_all(&native).unwrap();

        assert_eq!(steam_root_in(home.path(), no_env).unwrap(), native);
    }

    #[test]
    fn steam_root_env_overrides_the_candidates() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".steam/steam")).unwrap();

        let root = steam_root_in(home.path(), |key| {
            (key == STEAM_ROOT_ENV).then(|| OsString::from("/elsewhere/Steam"))
        })
        .unwrap();

        assert_eq!(root, PathBuf::from("/elsewhere/Steam"));
    }

    #[test]
    fn no_steam_anywhere_is_an_error_that_names_the_way_out() {
        let home = tempfile::tempdir().unwrap();

        let error = steam_root_in(home.path(), no_env).unwrap_err();

        assert!(matches!(error, Error::SteamNotFound), "{error}");
        assert!(error.to_string().contains(STEAM_ROOT_ENV));
    }

    #[test]
    fn linux_runs_games_through_proton() {
        assert!(LinuxPlatform.uses_proton());
        assert_eq!(LinuxPlatform.id(), "linux");
    }
}
