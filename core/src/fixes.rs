//! Per-game corrections to how GOG says a game starts.
//!
//! `goggame-<id>.info` names what GOG's own client launches, and for some games
//! that is not the game but a settings launcher built for a mouse: a window that
//! cannot be used from a controller under gamescope, and that may not even work
//! under Wine. For those, the game's real executable is started directly, with
//! the arguments the launcher would have passed it.
//!
//! Some of those launchers also switch the display to 16-bit colour first, which
//! late-90s games insist on. [`DEPTH16`] does that one job: a 2 KB Windows program
//! (`assets/windows/depth16.c`) that switches, starts the game, waits for it and
//! switches back. Under Wine the switch is emulated inside the game's prefix; the
//! real display never changes.
//!
//! This is the same idea as the per-game fixes Lutris and Heroic keep. Each entry
//! here was found on real hardware and says how; nothing is added on a guess.

use std::path::Path;

use crate::catalog::ProductId;
use crate::installer::{PlayTask, resolve_case};

/// Where a fixed game takes its arguments from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arguments {
    /// `before`, then a `key=value` line in an ini file in the install
    /// directory, the way the launcher stores the options it passes.
    Ini {
        before: &'static str,
        file: &'static str,
        key: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
struct Fix {
    id: &'static str,
    exe: &'static str,
    arguments: Arguments,
    /// Start it through [`DEPTH16`].
    depth16: bool,
}

/// The 16-bit colour launcher, built by `assets/windows/build.sh`.
pub const DEPTH16: &[u8] = include_bytes!("../../assets/windows/depth16.exe");

/// The name it is written under in a game's install directory.
pub const DEPTH16_NAME: &str = "gamestore-depth16.exe";

const FIXES: &[Fix] = &[
    // Moto Racer: GOG's MotoRacer.exe is "Moto Racer Launcher for GOG.com", a
    // settings window that throws "invalid window handle" under Proton and closes.
    // game.EXE is the game. The launcher passes it the `Params` line of
    // motoracer.ini, after the switches its D3D and ZBuffer settings stand for, and
    // `-fullscreen`; its own help text lists the switches. And it switches the
    // display to 16-bit colour first: game.EXE checks the desktop's depth on start
    // and stops at "16 bit screen mode required" otherwise, fullscreen or not.
    // Found on the mini PC, 2026-10-06.
    Fix {
        id: "1207658895",
        exe: "game.EXE",
        arguments: Arguments::Ini {
            before: "-fullscreen -D3D -ZBuffer",
            file: "motoracer.ini",
            key: "Params",
        },
        depth16: true,
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
        Arguments::Ini { before, file, key } => {
            let from_ini = ini_value(&resolve_case(install_dir, file), key).unwrap_or_default();
            format!("{before} {from_ini}").trim().to_string()
        }
    };
    let working_dir = install_dir.to_path_buf();
    if fix.depth16 {
        let launcher = install_dir.join(DEPTH16_NAME);
        let current = std::fs::read(&launcher).ok();
        if current.as_deref() == Some(DEPTH16) || std::fs::write(&launcher, DEPTH16).is_ok() {
            let name = exe
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            return PlayTask {
                exe: launcher,
                arguments: format!("\"{name}\" {arguments}").trim().to_string(),
                working_dir,
            };
        }
    }
    PlayTask {
        exe,
        arguments,
        working_dir,
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

        assert_eq!(fixed.exe, temp.path().join(DEPTH16_NAME));
        assert_eq!(
            fixed.arguments,
            "\"game.EXE\" -fullscreen -D3D -ZBuffer -DoRealLock -NoDither -FrameRateMax60"
        );
        assert_eq!(std::fs::read(&fixed.exe).unwrap(), DEPTH16);
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
