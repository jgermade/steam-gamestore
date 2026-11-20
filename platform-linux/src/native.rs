//! Native Linux builds from GOG.
//!
//! GOG's Linux installers are a MojoSetup shell script with a zip archive
//! appended. Everything the game needs is under `data/noarch/` in that zip:
//! `start.sh`, which sets the game up and runs it, `gameinfo` (name, version,
//! build, language, id, one per line), the game itself under `game/`, and GOG's
//! helper scripts under `support/`. So installing is extracting that directory:
//! no installer runs, nothing needs a display, and there is no Proton.
//!
//! `unzip` reads the zip past the script in front of it, warning about the extra
//! bytes, which is expected and not an error.

use std::path::{Path, PathBuf};
use std::process::Command;

use gamestore_core::installer::PlayTask;
use gamestore_core::{Error, Result};

const PAYLOAD: &str = "data/noarch";

/// Extract `installer` into `target`, by way of `work`.
pub fn install(installer: &Path, target: &Path, work: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work)
        .map_err(|error| Error::io(format!("creating {}", work.display()), error))?;

    let output = Command::new("unzip")
        .arg("-qq")
        .arg("-o")
        .arg(installer)
        .arg(format!("{PAYLOAD}/*"))
        .arg("-d")
        .arg(work)
        .output()
        .map_err(|error| Error::io("running unzip (is it installed?)", error))?;
    // 0 is clean; 1 is "warnings", which the script in front of the zip causes.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(Error::Install(format!(
            "unzip could not extract {}: {}",
            installer.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let payload = work.join(PAYLOAD);
    if !payload.join("start.sh").is_file() {
        return Err(Error::Install(format!(
            "{} has no {PAYLOAD}/start.sh; it does not look like a GOG Linux installer",
            installer.display()
        )));
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::io(format!("creating {}", parent.display()), error))?;
    }
    if target.exists() {
        std::fs::remove_dir_all(target)
            .map_err(|error| Error::io(format!("replacing {}", target.display()), error))?;
    }
    std::fs::rename(&payload, target)
        .map_err(|error| Error::io(format!("moving the game into {}", target.display()), error))?;
    let _ = std::fs::remove_dir_all(work);

    make_executable(&target.join("start.sh"))
}

/// How an extracted native game starts, when `target` holds one.
pub fn play_task(target: &Path) -> Option<PlayTask> {
    let start = target.join("start.sh");
    start.is_file().then(|| PlayTask {
        exe: start,
        arguments: String::new(),
        working_dir: target.to_path_buf(),
    })
}

/// The version line of `gameinfo`.
pub fn version(target: &Path) -> Option<String> {
    let info = std::fs::read_to_string(target.join("gameinfo")).ok()?;
    info.lines().nth(1).map(|line| line.trim().to_string())
}

/// GOG's icon for the game, when the installer shipped one.
pub fn icon(target: &Path) -> Option<PathBuf> {
    let icon = target.join("support/icon.png");
    icon.is_file().then_some(icon)
}

fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| Error::io(format!("reading {}", path.display()), error))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|error| Error::io(format!("making {} executable", path.display()), error))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GOG-shaped installer: a script with a zip appended, built with `zip`.
    fn fake_installer(dir: &Path) -> Option<PathBuf> {
        let tree = dir.join("tree/data/noarch");
        std::fs::create_dir_all(tree.join("game")).unwrap();
        std::fs::create_dir_all(tree.join("support")).unwrap();
        std::fs::write(tree.join("start.sh"), "#!/bin/bash\n").unwrap();
        std::fs::write(tree.join("gameinfo"), "Game\n1.2.3\n42\nen-US\n7\n").unwrap();
        std::fs::write(tree.join("game/run.sh"), "").unwrap();
        let zip = dir.join("payload.zip");
        let made = Command::new("zip")
            .current_dir(dir.join("tree"))
            .args(["-qr", zip.to_str().unwrap(), "data"])
            .status()
            .ok()?;
        if !made.success() {
            return None;
        }
        let mut installer = b"#!/bin/sh\n# MojoSetup stand-in\nexit 0\n".to_vec();
        installer.extend(std::fs::read(&zip).unwrap());
        let path = dir.join("game_1_2_3.sh");
        std::fs::write(&path, installer).unwrap();
        Some(path)
    }

    #[test]
    fn the_payload_becomes_the_install_directory() {
        let temp = tempfile::tempdir().unwrap();
        // Without `zip` there is nothing to build the fixture with; the real
        // installer was checked on the mini PC instead.
        let Some(installer) = fake_installer(temp.path()) else {
            return;
        };
        let target = temp.path().join("games/Game (Linux)");

        install(&installer, &target, &temp.path().join("work")).unwrap();

        let task = play_task(&target).unwrap();
        assert_eq!(task.exe, target.join("start.sh"));
        assert_eq!(version(&target).as_deref(), Some("1.2.3"));
        assert!(target.join("game/run.sh").is_file());
        assert!(!temp.path().join("work").exists());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target.join("start.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }

    #[test]
    fn something_else_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let bogus = temp.path().join("not-an-installer.sh");
        std::fs::write(&bogus, "#!/bin/sh\n").unwrap();

        assert!(
            install(
                &bogus,
                &temp.path().join("target"),
                &temp.path().join("work")
            )
            .is_err()
        );
    }
}
