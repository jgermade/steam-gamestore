//! Per-game corrections to how GOG says a game starts.
//!
//! `goggame-<id>.info` names what GOG's own client launches, and for some games
//! that is not the game but a settings launcher built for a mouse: a window that
//! cannot be used from a controller under gamescope, and that may not even work
//! under Wine. For those, the game's real executable is started directly, with
//! the arguments the launcher would have passed it.
//!
//! This is the same idea as the per-game fixes Lutris and Heroic keep. Each entry
//! here was found on real hardware and says how; nothing is added on a guess.

use std::path::Path;

use crate::catalog::ProductId;
use crate::installer::{PlayTask, resolve_case};

/// Where a fixed game takes its arguments from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arguments {
    /// A `key=value` line in an ini file in the install directory, the way the
    /// launcher stores the options it passes.
    Ini {
        file: &'static str,
        key: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
struct Fix {
    id: &'static str,
    exe: &'static str,
    arguments: Arguments,
}

const FIXES: &[Fix] = &[
    // Moto Racer: GOG's MotoRacer.exe is "Moto Racer Launcher for GOG.com", a .NET
    // 4.8 settings window that throws "invalid window handle" under Proton and
    // closes. game.EXE is the game, and the launcher passes it the `Params` line
    // of motoracer.ini. Found on the mini PC, 2026-10-06.
    Fix {
        id: "1207658895",
        exe: "game.EXE",
        arguments: Arguments::Ini {
            file: "motoracer.ini",
            key: "Params",
        },
    },
];

/// `task` corrected for `id`, when there is a known fix and the files it expects
/// are there; otherwise `task` unchanged.
pub fn apply(id: &ProductId, install_dir: &Path, task: PlayTask) -> PlayTask {
    let Some(fix) = FIXES.iter().find(|fix| fix.id == id.as_str()) else {
        return task;
    };
    let exe = resolve_case(install_dir, fix.exe);
    if !exe.is_file() {
        return task;
    }
    let arguments = match fix.arguments {
        Arguments::Ini { file, key } => {
            ini_value(&resolve_case(install_dir, file), key).unwrap_or_default()
        }
    };
    PlayTask {
        exe,
        arguments,
        working_dir: install_dir.to_path_buf(),
    }
}

fn ini_value(path: &Path, key: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    String::from_utf8_lossy(&bytes).lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case(key)
            .then(|| value.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(dir: &Path) -> PlayTask {
        PlayTask {
            exe: dir.join("MotoRacer.exe"),
            arguments: String::new(),
            working_dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn moto_racer_skips_its_launcher_and_takes_the_launchers_arguments() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("game.EXE"), "MZ").unwrap();
        std::fs::write(
            temp.path().join("motoracer.ini"),
            "[Settings]\r\nD3D=true\r\nParams=-DoRealLock -NoDither -FrameRateMax60\r\n",
        )
        .unwrap();

        let fixed = apply(&"1207658895".into(), temp.path(), task(temp.path()));

        assert_eq!(fixed.exe, temp.path().join("game.EXE"));
        assert_eq!(fixed.arguments, "-DoRealLock -NoDither -FrameRateMax60");
    }

    #[test]
    fn without_the_expected_files_the_task_is_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let unchanged = apply(&"1207658895".into(), temp.path(), task(temp.path()));
        assert_eq!(unchanged, task(temp.path()));

        std::fs::write(temp.path().join("game.EXE"), "MZ").unwrap();
        let other = apply(&"1".into(), temp.path(), task(temp.path()));
        assert_eq!(other, task(temp.path()));
    }
}
