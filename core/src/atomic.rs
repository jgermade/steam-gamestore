//! Writing a file so that an interrupted write never leaves a damaged one.
//!
//! The target machine is a console: it gets switched off at the wall, mid-write as
//! easily as any other moment. Every file gamestore owns — the stored session, the
//! installed-games registry, Steam's `shortcuts.vdf` — is one whose truncation
//! would be worse than its absence, so they all go through here: the bytes land in
//! a temporary file next to the target, are flushed to disk, and only then replace
//! it with a rename.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Who should be able to read the file afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Only this user (`0600`, in a `0700` directory). For anything holding
    /// credentials.
    Private,
    /// Whatever the umask says. For files that are not ours alone, such as the
    /// ones Steam also reads and writes.
    Default,
}

/// Write `contents` to `path`, atomically.
pub fn write(path: &Path, contents: &[u8], access: Access) -> Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| Error::io_path(path, "has no parent directory"))?;
    create_dir(parent, access)?;

    let temporary = temporary_beside(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if access == Access::Private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&temporary)
        .map_err(|error| Error::io(format!("creating {}", temporary.display()), error))?;

    // `mode` above only applies when the file is created, so a temporary left by an
    // earlier crash would keep whatever permissions it had. Set them either way.
    #[cfg(unix)]
    if access == Access::Private {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                Error::io(
                    format!("setting permissions on {}", temporary.display()),
                    error,
                )
            })?;
    }

    let written = file.write_all(contents).and_then(|()| file.sync_all());
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::io(format!("writing {}", temporary.display()), error));
    }
    drop(file);

    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::io(
            format!("replacing {} with {}", path.display(), temporary.display()),
            error,
        )
    })
}

/// Create a directory, private when the file going into it is.
pub fn create_dir(dir: &Path, access: Access) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;

    #[cfg(unix)]
    if access == Access::Private {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
            Error::io(format!("setting permissions on {}", dir.display()), error)
        })?;
    }

    // Off unix there are no modes to set, and the directory a file goes into is
    // already inside the user's own profile.
    #[cfg(not(unix))]
    let _ = access;

    Ok(())
}

/// The temporary file used while writing `path`. Beside it on purpose: a rename is
/// only atomic within one filesystem, and the system temporary directory is
/// routinely on another.
fn temporary_beside(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".gamestore-tmp");

    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn writing_creates_the_directory_and_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/deeper/file.bin");

        write(&path, b"hello", Access::Default).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn writing_replaces_what_was_there_and_leaves_no_temporary() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.bin");
        write(&path, b"first", Access::Default).unwrap();

        write(&path, b"second", Access::Default).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            1,
            "the temporary file must not be left behind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_readable_only_by_this_user() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("secrets/file.bin");

        write(&path, b"token", Access::Private).unwrap();

        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(mode_of(path.parent().unwrap()), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_leftover_does_not_survive_a_private_write() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.bin");
        write(&path, b"first", Access::Private).unwrap();
        let leftover = temporary_beside(&path);
        std::fs::write(&leftover, b"stale").unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o644)).unwrap();

        write(&path, b"second", Access::Private).unwrap();

        assert_eq!(mode_of(&path), 0o600);
        assert!(!leftover.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_default_write_does_not_tighten_a_directory_steam_also_uses() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        write(&dir.join("shortcuts.vdf"), b"x", Access::Default).unwrap();

        assert_eq!(mode_of(&dir), 0o755, "we do not own Steam's directories");
    }
}
