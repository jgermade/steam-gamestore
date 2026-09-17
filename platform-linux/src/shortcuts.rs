//! Finding and rewriting each Steam user's `shortcuts.vdf`.
//!
//! The parsing and the editing live in `core`; what is here is the part that knows
//! Steam's directory layout — which users exist, where their file is, and how to
//! replace it without losing the original.
//!
//! **Steam owns this file while it runs.** It reads `shortcuts.vdf` at startup and
//! writes it back on exit, so a change made underneath a running client is
//! discarded when that client quits. `03-platform-linux.md` lists detecting a
//! running Steam under `compat`; until that exists, callers should assume a write
//! only sticks while Steam is stopped, and the UI has to say so.

use std::path::{Path, PathBuf};

use gamestore_core::atomic::{self, Access};
use gamestore_core::shortcuts::Shortcuts;
use gamestore_core::{Error, Result};

/// Suffix of the untouched copy taken before the first write.
pub const BACKUP_SUFFIX: &str = ".gamestore-backup";

/// One Steam account with a local profile on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamUser {
    /// The account id, which is the directory name under `userdata/`.
    pub id: u32,
    /// `<steam root>/userdata/<id>`.
    pub dir: PathBuf,
}

impl SteamUser {
    /// `<steam root>/userdata/<id>/config`.
    pub fn config_dir(&self) -> PathBuf {
        self.dir.join("config")
    }

    /// The shortcuts file, whether or not it exists yet.
    pub fn shortcuts_file(&self) -> PathBuf {
        self.config_dir().join("shortcuts.vdf")
    }

    /// Where Steam keeps tile artwork for this user.
    pub fn grid_dir(&self) -> PathBuf {
        self.config_dir().join("grid")
    }

    /// The copy taken before gamestore first wrote the shortcuts file.
    pub fn shortcuts_backup(&self) -> PathBuf {
        let mut name = std::ffi::OsString::from("shortcuts.vdf");
        name.push(BACKUP_SUFFIX);

        self.config_dir().join(name)
    }
}

/// Every Steam account with a profile under `<steam root>/userdata`.
///
/// Sorted by id so the order does not depend on the filesystem. `userdata/0` and
/// `userdata/anonymous` are Steam's own placeholders, not people, and are skipped —
/// writing a tile into them would create a shortcut nobody can see.
///
/// Which of these to write into is open question 5: the mini PC is shared with the
/// kids, and "install for everyone" and "install for me" are different answers.
/// This returns all of them and leaves the choice to the caller.
pub fn steam_users(steam_root: &Path) -> Result<Vec<SteamUser>> {
    let userdata = steam_root.join("userdata");
    let entries = match std::fs::read_dir(&userdata) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::io(format!("reading {}", userdata.display()), error));
        }
    };

    let mut users = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| Error::io(format!("listing {}", userdata.display()), error))?;
        if !entry.path().is_dir() {
            continue;
        }

        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|id| *id != 0)
        else {
            continue;
        };

        users.push(SteamUser {
            id,
            dir: entry.path(),
        });
    }
    users.sort_by_key(|user| user.id);

    Ok(users)
}

/// Read a user's shortcuts. A file that is not there yet means no shortcuts, which
/// is the normal state of an account that has never added one.
pub fn read(user: &SteamUser) -> Result<Shortcuts> {
    let path = user.shortcuts_file();
    match std::fs::read(&path) {
        Ok(bytes) => Shortcuts::parse_or_empty(&bytes)
            .map_err(|error| Error::Vdf(format!("{} could not be read: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Shortcuts::empty()),
        Err(error) => Err(Error::io(format!("reading {}", path.display()), error)),
    }
}

/// Replace a user's shortcuts, keeping an untouched copy of what was there first.
///
/// The backup is taken once and never overwritten: its job is to preserve the file
/// as it was before gamestore ever touched it, so a later bug cannot launder itself
/// into the only copy by backing up its own output.
pub fn save(user: &SteamUser, shortcuts: &Shortcuts) -> Result<()> {
    let path = user.shortcuts_file();
    let bytes = shortcuts.to_bytes()?;

    back_up(&path, &user.shortcuts_backup())?;
    atomic::create_dir(&user.config_dir(), Access::Default)?;
    atomic::write(&path, &bytes, Access::Default)
}

fn back_up(path: &Path, backup: &Path) -> Result<()> {
    if backup.exists() || !path.exists() {
        return Ok(());
    }

    std::fs::copy(path, backup)
        .map(|_| ())
        .map_err(|error| Error::io(format!("backing up {} ", path.display()), error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamestore_core::shortcuts::Shortcut;

    fn steam_with_users(ids: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for id in ids {
            std::fs::create_dir_all(root.path().join("userdata").join(id).join("config")).unwrap();
        }

        root
    }

    fn a_shortcut(name: &str) -> Shortcut {
        Shortcut {
            app_name: name.to_string(),
            exe: format!("/games/{name}/{name}.exe"),
            start_dir: format!("/games/{name}"),
            launch_options: String::new(),
            icon: String::new(),
        }
    }

    #[test]
    fn every_real_user_is_found_and_the_placeholders_are_not() {
        let root = steam_with_users(&["1234", "0", "anonymous", "77"]);

        let users = steam_users(root.path()).unwrap();

        assert_eq!(
            users.iter().map(|user| user.id).collect::<Vec<_>>(),
            vec![77, 1234],
            "real accounts only, in a stable order"
        );
    }

    #[test]
    fn no_userdata_directory_is_no_users_rather_than_an_error() {
        let root = tempfile::tempdir().unwrap();

        assert!(steam_users(root.path()).unwrap().is_empty());
    }

    #[test]
    fn the_paths_follow_steams_layout() {
        let root = steam_with_users(&["1234"]);
        let user = &steam_users(root.path()).unwrap()[0];

        assert!(
            user.shortcuts_file()
                .ends_with("userdata/1234/config/shortcuts.vdf")
        );
        assert!(user.grid_dir().ends_with("userdata/1234/config/grid"));
        assert!(
            user.shortcuts_backup()
                .ends_with("userdata/1234/config/shortcuts.vdf.gamestore-backup")
        );
    }

    #[test]
    fn a_user_who_never_added_a_shortcut_reads_as_empty() {
        let root = steam_with_users(&["1234"]);
        let user = &steam_users(root.path()).unwrap()[0];

        assert!(read(user).unwrap().is_empty());
    }

    #[test]
    fn saving_and_reading_back_returns_the_same_shortcuts() {
        let root = steam_with_users(&["1234"]);
        let user = &steam_users(root.path()).unwrap()[0];
        let mut shortcuts = Shortcuts::empty();
        let appid = shortcuts.upsert(&a_shortcut("Bastion")).unwrap();

        save(user, &shortcuts).unwrap();

        let back = read(user).unwrap();
        assert_eq!(back.appids(), vec![appid]);
        assert_eq!(back.name_of(appid), Some("Bastion"));
    }

    #[test]
    fn the_first_save_preserves_the_original_and_later_ones_do_not_overwrite_it() {
        let root = steam_with_users(&["1234"]);
        let user = &steam_users(root.path()).unwrap()[0];

        // A file Steam wrote, with a shortcut gamestore knows nothing about.
        let mut original = Shortcuts::empty();
        original.upsert(&a_shortcut("Theirs")).unwrap();
        let original_bytes = original.to_bytes().unwrap();
        std::fs::write(user.shortcuts_file(), &original_bytes).unwrap();

        let mut changed = read(user).unwrap();
        changed.upsert(&a_shortcut("Ours")).unwrap();
        save(user, &changed).unwrap();
        let mut changed_again = read(user).unwrap();
        changed_again.upsert(&a_shortcut("Another")).unwrap();
        save(user, &changed_again).unwrap();

        assert_eq!(
            std::fs::read(user.shortcuts_backup()).unwrap(),
            original_bytes,
            "the backup has to stay the file as it was before gamestore touched it"
        );
        assert_eq!(read(user).unwrap().len(), 3);
    }

    #[test]
    fn saving_into_a_config_directory_that_does_not_exist_yet_works() {
        let root = tempfile::tempdir().unwrap();
        let user = SteamUser {
            id: 1234,
            dir: root.path().join("userdata/1234"),
        };
        let mut shortcuts = Shortcuts::empty();
        shortcuts.upsert(&a_shortcut("Bastion")).unwrap();

        save(&user, &shortcuts).unwrap();

        assert!(user.shortcuts_file().exists());
        assert!(
            !user.shortcuts_backup().exists(),
            "there was nothing to back up"
        );
    }

    #[test]
    fn a_corrupt_file_names_itself_rather_than_being_silently_replaced() {
        let root = steam_with_users(&["1234"]);
        let user = &steam_users(root.path()).unwrap()[0];
        std::fs::write(user.shortcuts_file(), b"\x03garbage").unwrap();

        let error = read(user).unwrap_err();

        assert!(error.to_string().contains("shortcuts.vdf"), "{error}");
    }

    #[test]
    fn a_foreign_shortcut_survives_a_save_that_adds_and_removes_ours() {
        let root = steam_with_users(&["1234"]);
        let user = &steam_users(root.path()).unwrap()[0];
        let mut theirs = Shortcuts::empty();
        theirs.upsert(&a_shortcut("Theirs")).unwrap();
        save(user, &theirs).unwrap();
        let before = std::fs::read(user.shortcuts_file()).unwrap();

        let mut ours = read(user).unwrap();
        let appid = ours.upsert(&a_shortcut("Ours")).unwrap();
        save(user, &ours).unwrap();
        let mut without = read(user).unwrap();
        assert!(without.remove(appid));
        save(user, &without).unwrap();

        assert_eq!(std::fs::read(user.shortcuts_file()).unwrap(), before);
    }
}
