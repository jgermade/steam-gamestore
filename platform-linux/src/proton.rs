//! Which Proton to install and run games with: one Steam already manages.
//!
//! The project never ships or manages its own Proton (`03-platform-linux.md`,
//! `compat`). A compatibility tool has two identities: the internal name Steam
//! keeps in `CompatToolMapping` (`proton_11`), and the directory holding its
//! `proton` script (`steamapps/common/Proton 11.0`). The mapping needs the first
//! and running an installer needs the second, so this module finds both.
//!
//! Custom tools under `compatibilitytools.d` declare their name in
//! `compatibilitytool.vdf`. Valve's own do not declare it anywhere on disk; Steam
//! knows it from its app info. [`official_name`] derives it from the directory
//! name the way Steam's names have always followed it, and a wrong guess shows up
//! as a tool that is not found rather than as a wrong mapping.

use std::path::{Path, PathBuf};

use gamestore_core::text_vdf::{self, Node};
use gamestore_core::{Error, Result};

/// A compatibility tool that can run Windows games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonTool {
    /// The internal name, as `CompatToolMapping` spells it.
    pub name: String,
    /// The directory with the `proton` script in it.
    pub dir: PathBuf,
}

impl ProtonTool {
    /// The script that `run` is passed to.
    pub fn script(&self) -> PathBuf {
        self.dir.join("proton")
    }
}

/// `config/config.vdf`, where Steam keeps `CompatToolMapping`.
pub fn config_vdf(steam_root: &Path) -> PathBuf {
    steam_root.join("config/config.vdf")
}

/// The key path of `CompatToolMapping` inside `config.vdf`.
pub const MAPPING_PATH: [&str; 5] = [
    "InstallConfigStore",
    "Software",
    "Valve",
    "Steam",
    "CompatToolMapping",
];

/// Every Steam library on the machine: the root itself and whatever
/// `steamapps/libraryfolders.vdf` lists, without duplicates.
pub fn library_folders(steam_root: &Path) -> Vec<PathBuf> {
    let mut folders = vec![steam_root.to_path_buf()];
    let listed = std::fs::read_to_string(steam_root.join("steamapps/libraryfolders.vdf"))
        .ok()
        .and_then(|text| text_vdf::parse(&text).ok());
    if let Some(entries) = listed
        .as_ref()
        .and_then(|document| document.get("libraryfolders"))
        .and_then(Node::entries)
    {
        for (_, library) in entries {
            if let Some(path) = library.get("path").and_then(Node::as_str) {
                let path = PathBuf::from(path);
                let same = |known: &PathBuf| {
                    known == &path
                        || known
                            .canonicalize()
                            .ok()
                            .is_some_and(|known| path.canonicalize().ok().as_ref() == Some(&known))
                };
                if !folders.iter().any(same) {
                    folders.push(path);
                }
            }
        }
    }
    folders
}

/// The internal name of one of Valve's Proton builds, from its directory name.
pub fn official_name(dir_name: &str) -> Option<String> {
    match dir_name {
        "Proton - Experimental" => return Some("proton_experimental".to_string()),
        "Proton Hotfix" => return Some("proton_hotfix".to_string()),
        _ => {}
    }
    let version = dir_name.strip_prefix("Proton ")?;
    let version = version.split_whitespace().next()?;
    let (major, minor) = version.split_once('.')?;
    let major: u32 = major.parse().ok()?;
    minor.parse::<u32>().ok()?;
    // From 7.0 on Steam names a branch by its major version alone; before that
    // the minor version was part of the name (`proton_63`, `proton_513`).
    Some(if major >= 7 {
        format!("proton_{major}")
    } else {
        format!("proton_{major}{minor}")
    })
}

/// Every Proton Steam could map a game to, official and custom.
pub fn installed(steam_root: &Path) -> Vec<ProtonTool> {
    let mut tools = Vec::new();

    for library in library_folders(steam_root) {
        let Ok(dirs) = std::fs::read_dir(library.join("steamapps/common")) else {
            continue;
        };
        for dir in dirs.flatten() {
            let path = dir.path();
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(official_name)
            else {
                continue;
            };
            if path.join("proton").is_file() {
                tools.push(ProtonTool { name, dir: path });
            }
        }
    }

    for base in [
        steam_root.join("compatibilitytools.d"),
        PathBuf::from("/usr/share/steam/compatibilitytools.d"),
    ] {
        let Ok(dirs) = std::fs::read_dir(&base) else {
            continue;
        };
        for dir in dirs.flatten() {
            tools.extend(custom_tools(&dir.path()));
        }
    }

    tools
}

/// The tools a `compatibilitytool.vdf` in `dir` declares.
fn custom_tools(dir: &Path) -> Vec<ProtonTool> {
    let Some(document) = std::fs::read_to_string(dir.join("compatibilitytool.vdf"))
        .ok()
        .and_then(|text| text_vdf::parse(&text).ok())
    else {
        return Vec::new();
    };
    let Some(declared) = document
        .path(&["compatibilitytools", "compat_tools"])
        .and_then(Node::entries)
    else {
        return Vec::new();
    };
    declared
        .iter()
        .filter_map(|(name, tool)| {
            let install_path = tool.get("install_path").and_then(Node::as_str)?;
            let tool_dir = dir.join(install_path);
            tool_dir.join("proton").is_file().then(|| ProtonTool {
                name: name.clone(),
                dir: tool_dir,
            })
        })
        .collect()
}

/// The tool Steam applies to everything without a mapping of its own: the
/// `"0"` entry of `CompatToolMapping`, which is what "Enable Steam Play for all
/// other titles" writes.
pub fn default_name(steam_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_vdf(steam_root)).ok()?;
    let document = text_vdf::parse(&text).ok()?;
    document
        .path(&MAPPING_PATH)?
        .path(&["0", "name"])?
        .as_str()
        .filter(|name| !name.is_empty())
}

/// The tool to use: `configured` when it is set, else Steam's own default, else
/// the newest numbered official Proton.
pub fn choose(steam_root: &Path, configured: Option<&str>) -> Result<ProtonTool> {
    let tools = installed(steam_root);
    let listed = || {
        if tools.is_empty() {
            "none is installed; install one from Steam's library (search for Proton)".to_string()
        } else {
            let names: Vec<_> = tools.iter().map(|tool| tool.name.as_str()).collect();
            format!("installed: {}", names.join(", "))
        }
    };

    let wanted = configured
        .map(str::to_string)
        .or_else(|| default_name(steam_root));
    if let Some(wanted) = wanted {
        return tools
            .iter()
            .find(|tool| tool.name == wanted)
            .cloned()
            .ok_or_else(|| {
                Error::Steam(format!(
                    "the compatibility tool {wanted} is not installed; {}",
                    listed()
                ))
            });
    }

    tools
        .iter()
        .filter_map(|tool| {
            let number: u32 = tool.name.strip_prefix("proton_")?.parse().ok()?;
            Some((number, tool))
        })
        .max_by_key(|(number, _)| *number)
        .map(|(_, tool)| tool.clone())
        .ok_or_else(|| Error::Steam(format!("no Proton to install with; {}", listed())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proton_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("steamapps/common").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proton"), "#!/usr/bin/env python3\n").unwrap();
        dir
    }

    fn write_default(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            config_vdf(root),
            format!(
                "\"InstallConfigStore\"\n{{\n\t\"Software\"\n\t{{\n\t\t\"Valve\"\n\t\t{{\n\t\t\t\"Steam\"\n\t\t\t{{\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{{\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"name\"\t\t\"{name}\"\n\t\t\t\t\t}}\n\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t}}\n}}\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn official_names_follow_the_directory_names() {
        assert_eq!(official_name("Proton 11.0").as_deref(), Some("proton_11"));
        assert_eq!(
            official_name("Proton 9.0 (Beta)").as_deref(),
            Some("proton_9")
        );
        assert_eq!(official_name("Proton 6.3").as_deref(), Some("proton_63"));
        assert_eq!(official_name("Proton 5.13").as_deref(), Some("proton_513"));
        assert_eq!(
            official_name("Proton - Experimental").as_deref(),
            Some("proton_experimental")
        );
        assert_eq!(official_name("Proton EasyAntiCheat Runtime"), None);
        assert_eq!(official_name("SteamLinuxRuntime_4"), None);
    }

    #[test]
    fn steams_default_wins_over_the_newest() {
        let root = tempfile::tempdir().unwrap();
        proton_dir(root.path(), "Proton 11.0");
        let experimental = proton_dir(root.path(), "Proton - Experimental");
        write_default(root.path(), "proton_experimental");

        let tool = choose(root.path(), None).unwrap();

        assert_eq!(tool.name, "proton_experimental");
        assert_eq!(tool.dir, experimental);
    }

    #[test]
    fn without_a_default_the_newest_numbered_proton_is_used() {
        let root = tempfile::tempdir().unwrap();
        proton_dir(root.path(), "Proton 9.0 (Beta)");
        proton_dir(root.path(), "Proton 11.0");
        proton_dir(root.path(), "Proton - Experimental");

        assert_eq!(choose(root.path(), None).unwrap().name, "proton_11");
    }

    #[test]
    fn a_configured_tool_that_is_missing_lists_what_is_there() {
        let root = tempfile::tempdir().unwrap();
        proton_dir(root.path(), "Proton 11.0");

        let error = choose(root.path(), Some("GE-Proton10-1")).unwrap_err();

        assert!(error.to_string().contains("GE-Proton10-1"), "{error}");
        assert!(error.to_string().contains("proton_11"), "{error}");
    }

    #[test]
    fn custom_tools_declare_their_own_name() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("compatibilitytools.d/GE-Proton10-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("proton"), "").unwrap();
        std::fs::write(
            dir.join("compatibilitytool.vdf"),
            "\"compatibilitytools\"\n{\n  \"compat_tools\"\n  {\n    \"GE-Proton10-1\"\n    {\n      \"install_path\" \".\"\n      \"display_name\" \"GE-Proton10-1\"\n    }\n  }\n}\n",
        )
        .unwrap();

        let tool = choose(root.path(), Some("GE-Proton10-1")).unwrap();

        assert_eq!(tool.dir, dir.join("."));
    }

    #[test]
    fn libraries_elsewhere_are_searched_too() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("steamapps")).unwrap();
        std::fs::write(
            root.path().join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                root.path().display(),
                other.path().display()
            ),
        )
        .unwrap();
        proton_dir(other.path(), "Proton 11.0");

        assert_eq!(library_folders(root.path()).len(), 2);
        assert_eq!(choose(root.path(), None).unwrap().name, "proton_11");
    }
}
