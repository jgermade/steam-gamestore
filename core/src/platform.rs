//! What genuinely differs between Linux and Windows.
//!
//! The `platform-*` crates stay thin on purpose: they own Steam and Proton
//! integration, nothing else. Anything both platforms do the same way belongs in
//! this crate instead.

use std::path::PathBuf;

use crate::{Error, Paths, Result};

/// A host platform gamestore can install games on.
pub trait Platform {
    /// Short identifier used in logs and in `gamestore info`.
    fn id(&self) -> &'static str;

    /// Root of the Steam installation this platform should write shortcuts into.
    fn steam_root(&self) -> Result<PathBuf>;

    /// Whether games run through the Proton that Steam manages.
    fn uses_proton(&self) -> bool;

    /// Where installed games go by default.
    fn default_install_root(&self, paths: &Paths) -> PathBuf {
        paths.install_root()
    }
}

/// Stand-in for platforms with no Steam integration, so the workspace still
/// builds (and says something useful) on, for example, macOS.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedPlatform;

impl Platform for UnsupportedPlatform {
    fn id(&self) -> &'static str {
        "unsupported"
    }

    fn steam_root(&self) -> Result<PathBuf> {
        Err(Error::Unsupported("Steam integration"))
    }

    fn uses_proton(&self) -> bool {
        false
    }
}

/// Environment variable pointing at a Steam installation, for setups the
/// platform crates cannot guess.
pub const STEAM_ROOT_ENV: &str = "STEAM_ROOT";

/// Return the first candidate that exists on disk.
pub fn first_existing<I>(candidates: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    candidates.into_iter().find(|path| path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_existing_picks_the_first_directory_that_is_there() {
        let temp = tempfile::tempdir().unwrap();
        let present = temp.path().join("steam");
        std::fs::create_dir(&present).unwrap();

        let found = first_existing(vec![temp.path().join("missing"), present.clone()]);

        assert_eq!(found, Some(present));
    }

    #[test]
    fn first_existing_is_none_when_nothing_matches() {
        assert_eq!(first_existing(vec![PathBuf::from("/nope/nothing")]), None);
    }

    #[test]
    fn the_unsupported_platform_explains_itself() {
        let error = UnsupportedPlatform.steam_root().unwrap_err();

        assert!(matches!(error, Error::Unsupported(_)), "{error}");
    }
}
