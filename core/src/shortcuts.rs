//! The shortcuts gamestore adds to `shortcuts.vdf`, on top of the raw VDF tree.
//!
//! [`crate::vdf`] guarantees nothing is lost from the file; this module is what
//! guarantees nothing is lost from an *entry*. Updating a shortcut reads the entry
//! that is already there and changes only the fields gamestore owns, so a tag the
//! user added in Big Picture, or a `LaunchOptions` they edited by hand, survives a
//! reinstall.

use crate::steam;
use crate::vdf::Value;
use crate::{Error, Result};

/// The key the shortcut list lives under.
const SHORTCUTS: &str = "shortcuts";

/// The fields gamestore sets on a shortcut it owns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shortcut {
    /// Display name, as the tile shows it.
    pub app_name: String,
    /// Executable path, unquoted. It is stored quoted; see [`steam::quote_exe`].
    pub exe: String,
    /// Working directory, unquoted.
    pub start_dir: String,
    /// Arguments appended after the executable.
    pub launch_options: String,
    /// Icon path, unquoted. Empty means Steam picks one.
    pub icon: String,
}

impl Shortcut {
    /// The executable as it is stored in the file, and as the appid is derived.
    pub fn stored_exe(&self) -> String {
        steam::quote_exe(&self.exe)
    }

    /// The appid Steam derives for this shortcut.
    pub fn appid(&self) -> u32 {
        steam::shortcut_appid(&self.stored_exe(), &self.app_name)
    }
}

/// A parsed `shortcuts.vdf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcuts {
    document: Value,
}

impl Shortcuts {
    /// Parse a `shortcuts.vdf`.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let document = crate::vdf::parse(bytes)?;
        if document.get(SHORTCUTS).is_none() {
            return Err(Error::Vdf(format!(
                "this does not look like a shortcuts.vdf: it has no {SHORTCUTS:?} map"
            )));
        }

        Ok(Self { document })
    }

    /// The document Steam writes when there are no shortcuts at all.
    pub fn empty() -> Self {
        Self {
            document: Value::Map(vec![(SHORTCUTS.to_string(), Value::map())]),
        }
    }

    /// Parse a `shortcuts.vdf`, treating an empty file as no shortcuts.
    ///
    /// Steam does not always create the file, and a zero-length one turns up after
    /// an interrupted write; neither is a reason to refuse to add a game.
    pub fn parse_or_empty(bytes: &[u8]) -> Result<Self> {
        if bytes.iter().all(|byte| *byte == 0) && bytes.len() < 2 {
            return Ok(Self::empty());
        }

        Self::parse(bytes)
    }

    /// Serialize back to `shortcuts.vdf` bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        crate::vdf::write(&self.document)
    }

    fn list(&self) -> &[(String, Value)] {
        self.document
            .get(SHORTCUTS)
            .and_then(Value::entries)
            .unwrap_or(&[])
    }

    fn list_mut(&mut self) -> &mut Vec<(String, Value)> {
        self.document
            .entries_mut()
            .expect("the root is a map")
            .iter_mut()
            .find(|(key, _)| key.eq_ignore_ascii_case(SHORTCUTS))
            .map(|(_, value)| value)
            .and_then(Value::entries_mut)
            .expect("the shortcuts map is present, checked on construction")
    }

    /// How many shortcuts the file holds, gamestore's and everybody else's.
    pub fn len(&self) -> usize {
        self.list().len()
    }

    /// Whether there are no shortcuts at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The appid of every shortcut in the file, in file order.
    ///
    /// The stored `appid` field is preferred over deriving one, since an entry
    /// another tool wrote may not derive to what it stored.
    pub fn appids(&self) -> Vec<u32> {
        self.list()
            .iter()
            .map(|(_, entry)| stored_appid(entry))
            .collect()
    }

    /// The display name of the shortcut with this appid.
    pub fn name_of(&self, appid: u32) -> Option<&str> {
        self.find(appid).and_then(|entry| entry.get_str("AppName"))
    }

    fn find(&self, appid: u32) -> Option<&Value> {
        self.list()
            .iter()
            .find(|(_, entry)| stored_appid(entry) == appid)
            .map(|(_, entry)| entry)
    }

    /// Add `shortcut`, or update the one already there with the same appid.
    ///
    /// Idempotent: running it twice leaves the file as it was after the first run.
    /// Fields gamestore does not own are left exactly as they were found, which is
    /// what makes a reinstall non-destructive.
    pub fn upsert(&mut self, shortcut: &Shortcut) -> Result<u32> {
        let appid = shortcut.appid();
        let position = self
            .list()
            .iter()
            .position(|(_, entry)| stored_appid(entry) == appid);

        match position {
            Some(index) => {
                let entry = &mut self.list_mut()[index].1;
                apply(entry, shortcut, appid);
            }
            None => {
                let mut entry = Value::map();
                fill_defaults(&mut entry);
                apply(&mut entry, shortcut, appid);
                let key = self.len().to_string();
                self.list_mut().push((key, entry));
            }
        }

        Ok(appid)
    }

    /// Remove the shortcut with this appid. Returns whether there was one.
    ///
    /// The indices are renumbered afterwards: Steam expects them contiguous from
    /// zero, and a gap makes it stop reading at the hole — which would look exactly
    /// like uninstalling one game deleting every game after it.
    pub fn remove(&mut self, appid: u32) -> bool {
        let before = self.len();
        self.list_mut()
            .retain(|(_, entry)| stored_appid(entry) != appid);

        let removed = self.len() != before;
        if removed {
            for (index, (key, _)) in self.list_mut().iter_mut().enumerate() {
                *key = index.to_string();
            }
        }

        removed
    }
}

/// The appid an entry declares, falling back to deriving it from its own fields
/// when the field is missing.
fn stored_appid(entry: &Value) -> u32 {
    if let Some(stored) = entry.get_i32("appid") {
        // Steam stores it as a signed 32-bit field, so the high bit reads negative.
        return stored as u32;
    }

    steam::shortcut_appid(
        entry.get_str("Exe").unwrap_or_default(),
        entry.get_str("AppName").unwrap_or_default(),
    )
}

/// Write the fields gamestore owns, leaving every other key untouched.
fn apply(entry: &mut Value, shortcut: &Shortcut, appid: u32) {
    entry.set("appid", Value::Int32(appid as i32));
    entry.set("AppName", Value::String(shortcut.app_name.clone()));
    entry.set("Exe", Value::String(shortcut.stored_exe()));
    entry.set(
        "StartDir",
        Value::String(steam::quote_exe(&shortcut.start_dir)),
    );
    entry.set("icon", Value::String(shortcut.icon.clone()));
    entry.set(
        "LaunchOptions",
        Value::String(shortcut.launch_options.clone()),
    );
}

/// The fields Steam writes on a shortcut it creates itself, in its order. Only
/// used for a brand new entry; an existing one keeps whatever it has.
fn fill_defaults(entry: &mut Value) {
    for (key, value) in [
        ("appid", Value::Int32(0)),
        ("AppName", Value::String(String::new())),
        ("Exe", Value::String(String::new())),
        ("StartDir", Value::String(String::new())),
        ("icon", Value::String(String::new())),
        ("ShortcutPath", Value::String(String::new())),
        ("LaunchOptions", Value::String(String::new())),
        ("IsHidden", Value::Int32(0)),
        ("AllowDesktopConfig", Value::Int32(1)),
        ("AllowOverlay", Value::Int32(1)),
        ("OpenVR", Value::Int32(0)),
        ("Devkit", Value::Int32(0)),
        ("DevkitGameID", Value::String(String::new())),
        ("DevkitOverrideAppID", Value::Int32(0)),
        ("LastPlayTime", Value::Int32(0)),
        ("FlatpakAppID", Value::String(String::new())),
    ] {
        entry.set(key, value);
    }
    entry.set("tags", Value::map());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_shortcut(name: &str) -> Shortcut {
        Shortcut {
            app_name: name.to_string(),
            exe: format!("/games/{name}/{name}.exe"),
            start_dir: format!("/games/{name}"),
            launch_options: String::new(),
            icon: String::new(),
        }
    }

    /// A file with one entry that gamestore did not write, carrying a field it
    /// does not manage and a tag the user added.
    fn foreign_file() -> Shortcuts {
        let mut shortcuts = Shortcuts::empty();
        let mut entry = Value::map();
        fill_defaults(&mut entry);
        entry.set("appid", Value::Int32(0x1234_5678));
        entry.set("AppName", Value::String("Someone Else's Game".to_string()));
        entry.set("Exe", Value::String("\"/elsewhere/other.exe\"".to_string()));
        entry.set("SomethingValveAddedLater", Value::UInt64(u64::MAX));
        let mut tags = Value::map();
        tags.set("0", Value::String("favourite".to_string()));
        entry.set("tags", tags);
        shortcuts.list_mut().push(("0".to_string(), entry));

        shortcuts
    }

    #[test]
    fn a_new_shortcut_is_appended_and_gets_its_derived_appid() {
        let mut shortcuts = Shortcuts::empty();
        let bastion = a_shortcut("Bastion");

        let appid = shortcuts.upsert(&bastion).unwrap();

        assert_eq!(appid, bastion.appid());
        assert_eq!(shortcuts.len(), 1);
        assert_eq!(shortcuts.appids(), vec![appid]);
        assert_eq!(shortcuts.name_of(appid), Some("Bastion"));
    }

    #[test]
    fn upserting_twice_changes_nothing_the_second_time() {
        let mut shortcuts = Shortcuts::empty();
        let bastion = a_shortcut("Bastion");

        shortcuts.upsert(&bastion).unwrap();
        let after_first = shortcuts.to_bytes().unwrap();
        shortcuts.upsert(&bastion).unwrap();

        assert_eq!(
            shortcuts.len(),
            1,
            "a second upsert must not add a duplicate"
        );
        assert_eq!(shortcuts.to_bytes().unwrap(), after_first);
    }

    #[test]
    fn updating_a_shortcut_keeps_fields_gamestore_does_not_own() {
        // The reinstall case: the user tagged the game and set launch options in
        // Big Picture, and reinstalling must not throw that away.
        let mut shortcuts = Shortcuts::empty();
        let bastion = a_shortcut("Bastion");
        let appid = shortcuts.upsert(&bastion).unwrap();

        let entry = &mut shortcuts.list_mut()[0].1;
        let mut tags = Value::map();
        tags.set("0", Value::String("favourite".to_string()));
        entry.set("tags", tags);
        entry.set("AllowOverlay", Value::Int32(0));
        entry.set("SomethingValveAddedLater", Value::UInt64(7));

        shortcuts.upsert(&bastion).unwrap();

        let entry = shortcuts.find(appid).unwrap();
        assert_eq!(entry.get("tags").unwrap().get_str("0"), Some("favourite"));
        assert_eq!(entry.get_i32("AllowOverlay"), Some(0));
        assert_eq!(
            entry.get("SomethingValveAddedLater"),
            Some(&Value::UInt64(7))
        );
    }

    #[test]
    fn a_foreign_entry_survives_adding_and_removing_our_own() {
        let mut shortcuts = foreign_file();
        let before = shortcuts.to_bytes().unwrap();
        let bastion = a_shortcut("Bastion");

        let appid = shortcuts.upsert(&bastion).unwrap();
        assert_eq!(shortcuts.len(), 2);
        assert!(shortcuts.remove(appid));

        assert_eq!(
            shortcuts.to_bytes().unwrap(),
            before,
            "adding and removing our shortcut has to leave the file as it was"
        );
    }

    #[test]
    fn removing_renumbers_so_steam_does_not_stop_at_the_hole() {
        let mut shortcuts = Shortcuts::empty();
        let first = shortcuts.upsert(&a_shortcut("First")).unwrap();
        shortcuts.upsert(&a_shortcut("Second")).unwrap();
        shortcuts.upsert(&a_shortcut("Third")).unwrap();

        assert!(shortcuts.remove(first));

        let keys: Vec<&str> = shortcuts
            .list()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["0", "1"],
            "indices must stay contiguous from zero"
        );
    }

    #[test]
    fn removing_something_absent_says_so_and_changes_nothing() {
        let mut shortcuts = foreign_file();
        let before = shortcuts.to_bytes().unwrap();

        assert!(!shortcuts.remove(a_shortcut("Never Installed").appid()));

        assert_eq!(shortcuts.to_bytes().unwrap(), before);
    }

    #[test]
    fn a_new_entry_carries_the_fields_steam_writes_itself() {
        let mut shortcuts = Shortcuts::empty();
        let appid = shortcuts.upsert(&a_shortcut("Bastion")).unwrap();

        let entry = shortcuts.find(appid).unwrap();

        assert_eq!(entry.get_i32("IsHidden"), Some(0));
        assert_eq!(entry.get_i32("AllowOverlay"), Some(1));
        assert_eq!(entry.get_str("ShortcutPath"), Some(""));
        assert!(entry.get("tags").is_some());
    }

    #[test]
    fn the_stored_exe_is_quoted_and_the_appid_matches_it() {
        let mut shortcuts = Shortcuts::empty();
        let bastion = a_shortcut("Bastion");
        let appid = shortcuts.upsert(&bastion).unwrap();

        let entry = shortcuts.find(appid).unwrap();

        assert_eq!(entry.get_str("Exe"), Some("\"/games/Bastion/Bastion.exe\""));
        assert_eq!(
            appid,
            steam::shortcut_appid("\"/games/Bastion/Bastion.exe\"", "Bastion"),
            "the appid has to be derived from the string actually stored"
        );
    }

    #[test]
    fn the_stored_appid_field_wins_over_a_derived_one() {
        // Another tool may store an appid that does not derive from its own fields;
        // removing it has to key on what is stored, or uninstall misses it.
        let shortcuts = foreign_file();

        assert_eq!(shortcuts.appids(), vec![0x1234_5678]);
    }

    #[test]
    fn a_round_trip_through_bytes_preserves_everything() {
        let mut shortcuts = foreign_file();
        shortcuts.upsert(&a_shortcut("Bastion")).unwrap();
        let bytes = shortcuts.to_bytes().unwrap();

        let reparsed = Shortcuts::parse(&bytes).unwrap();

        assert_eq!(reparsed, shortcuts);
        assert_eq!(reparsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn an_empty_or_missing_file_is_treated_as_no_shortcuts() {
        assert!(Shortcuts::parse_or_empty(&[]).unwrap().is_empty());
        assert!(Shortcuts::parse_or_empty(&[0]).unwrap().is_empty());
    }

    #[test]
    fn something_that_is_not_a_shortcuts_file_is_refused() {
        let other = Value::Map(vec![("config".to_string(), Value::map())]);
        let bytes = crate::vdf::write(&other).unwrap();

        let error = Shortcuts::parse(&bytes).unwrap_err();

        assert!(error.to_string().contains("shortcuts.vdf"), "{error}");
    }
}
