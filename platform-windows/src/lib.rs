//! Windows side of gamestore: no Proton, no prefix, and later the native
//! installer runner and launch wrapper.
//!
//! The crate is plain path and environment logic so that `cargo build
//! --workspace` stays green on Linux too; nothing here calls a Windows API. The
//! registry lookup for the Steam root belongs to the `wvdf` task, and until then
//! `STEAM_ROOT` covers non-default installations.

use std::ffi::OsString;
use std::path::PathBuf;

use gamestore_core::platform::{STEAM_ROOT_ENV, first_existing};
use gamestore_core::{Error, Platform, Result};

/// Environment variables holding the Program Files directories, most specific
/// first: 32-bit Steam is the default install even on 64-bit Windows.
const PROGRAM_FILES_ENV: [&str; 2] = ["ProgramFiles(x86)", "ProgramFiles"];

/// Steam on Windows.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsPlatform;

impl Platform for WindowsPlatform {
    fn id(&self) -> &'static str {
        "windows"
    }

    fn steam_root(&self) -> Result<PathBuf> {
        steam_root_from(|key| std::env::var_os(key))
    }

    fn uses_proton(&self) -> bool {
        false
    }
}

/// Resolve the Steam root, letting `STEAM_ROOT` win when it is set.
pub fn steam_root_from<F>(lookup: F) -> Result<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(root) = lookup(STEAM_ROOT_ENV) {
        return Ok(PathBuf::from(root));
    }

    let candidates = PROGRAM_FILES_ENV
        .iter()
        .filter_map(|key| lookup(key))
        .map(|dir| PathBuf::from(dir).join("Steam"));

    first_existing(candidates).ok_or(Error::SteamNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_files_x86_is_tried_before_program_files() {
        let root = tempfile::tempdir().unwrap();
        let x86 = root.path().join("Program Files (x86)");
        let native = root.path().join("Program Files");
        std::fs::create_dir_all(x86.join("Steam")).unwrap();
        std::fs::create_dir_all(native.join("Steam")).unwrap();

        let found = steam_root_from(|key| match key {
            "ProgramFiles(x86)" => Some(x86.clone().into_os_string()),
            "ProgramFiles" => Some(native.clone().into_os_string()),
            _ => None,
        })
        .unwrap();

        assert_eq!(found, x86.join("Steam"));
    }

    #[test]
    fn steam_root_env_overrides_program_files() {
        let found =
            steam_root_from(|key| (key == STEAM_ROOT_ENV).then(|| OsString::from(r"D:\Steam")))
                .unwrap();

        assert_eq!(found, PathBuf::from(r"D:\Steam"));
    }

    #[test]
    fn no_steam_anywhere_is_an_error() {
        let error = steam_root_from(|_| None).unwrap_err();

        assert!(matches!(error, Error::SteamNotFound), "{error}");
    }

    #[test]
    fn windows_runs_games_natively() {
        assert!(!WindowsPlatform.uses_proton());
        assert_eq!(WindowsPlatform.id(), "windows");
    }
}
