//! `inno`: run a GOG installer silently through Proton.
//!
//! What `RECORD/2026-10-06.manual-install-prototype.completed.md` found by hand:
//!
//! - the installer has to be given by absolute path, or Proton's `steam.exe`
//!   cannot find it and exits 1 without a word;
//! - Wine needs a display even for a silent install, and a shell outside the
//!   gamescope session has none, which also exits 1 without a word;
//! - the game is installed outside the prefix, into gamestore's install root,
//!   with a throwaway prefix. Steam makes the real one at first launch, under
//!   the shortcut's appid, which is not known until the exe is.
//!
//! Success is judged by InnoSetup's own log, not only the exit status: Proton
//! passes the installer's status through, but a log that says
//! `Installation process succeeded.` is the stronger evidence.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gamestore_core::{Error, Result};

use crate::proton::ProtonTool;

/// What InnoSetup writes to its log when it is done and happy.
const SUCCESS: &str = "Installation process succeeded.";

/// The display a gamescope session runs its Xwayland on.
const FALLBACK_DISPLAY: &str = ":0";

/// One silent install.
#[derive(Debug, Clone)]
pub struct Setup<'a> {
    pub proton: &'a ProtonTool,
    pub steam_root: &'a Path,
    /// The `setup_*.exe`, with its `.bin` parts next to it.
    pub installer: &'a Path,
    /// Where the game goes.
    pub target: &'a Path,
    /// A prefix to install with. It is created, and it can be deleted afterwards.
    pub prefix: &'a Path,
    /// Where InnoSetup's log goes.
    pub log: &'a Path,
    /// How long before an installer that is not finishing is assumed to be stuck
    /// on a dialog nobody can see.
    pub timeout: Duration,
}

/// A Linux path as Wine sees it, through the `Z:` drive that maps `/`.
pub fn wine_path(path: &Path) -> String {
    format!("Z:{}", path.display()).replace('/', "\\")
}

/// The arguments that make an InnoSetup installer silent.
pub fn arguments(setup: &Setup, installer: &Path) -> Vec<String> {
    vec![
        "run".to_string(),
        installer.display().to_string(),
        "/VERYSILENT".to_string(),
        "/SUPPRESSMSGBOXES".to_string(),
        "/NORESTART".to_string(),
        "/SP-".to_string(),
        "/NOICONS".to_string(),
        format!("/DIR={}", wine_path(setup.target)),
        format!("/LOG={}", wine_path(setup.log)),
    ]
}

/// Run the installer and wait for it, within `setup.timeout`.
pub fn install(setup: &Setup) -> Result<()> {
    let installer = setup.installer.canonicalize().map_err(|error| {
        Error::io(
            format!("finding the installer {}", setup.installer.display()),
            error,
        )
    })?;
    for dir in [setup.prefix, setup.target] {
        std::fs::create_dir_all(dir)
            .map_err(|error| Error::io(format!("creating {}", dir.display()), error))?;
    }
    if let Some(parent) = setup.log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::io(format!("creating {}", parent.display()), error))?;
    }
    // A log from an earlier attempt would make a silent failure look like success.
    let _ = std::fs::remove_file(setup.log);

    let mut command = Command::new(setup.proton.script());
    command
        .args(arguments(setup, &installer))
        .env("STEAM_COMPAT_DATA_PATH", setup.prefix)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", setup.steam_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, so a stuck install can be stopped together with
        // the wineserver and helpers it started.
        .process_group(0);
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        command.env("DISPLAY", FALLBACK_DISPLAY);
    }

    let mut child = command.spawn().map_err(|error| {
        Error::io(
            format!("starting {}", setup.proton.script().display()),
            error,
        )
    })?;

    let deadline = Instant::now() + setup.timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| Error::io("waiting for the installer", error))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            stop_group(child.id());
            let _ = child.wait();
            return Err(Error::Install(format!(
                "the installer was still running after {} minutes and was stopped; it is \
                 probably waiting on a dialog that cannot be seen. Its log is {}",
                setup.timeout.as_secs() / 60,
                setup.log.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(250));
    };

    let log = std::fs::read(setup.log)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    if status.success() && log.contains(SUCCESS) {
        return Ok(());
    }

    let reason = if log.is_empty() {
        "it wrote no log, so it probably never started (is there a display?)".to_string()
    } else {
        let tail: Vec<_> = log.lines().rev().take(3).collect();
        format!(
            "its log ends with: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
        )
    };
    Err(Error::Install(format!(
        "the installer failed ({status}); {reason}. Full log: {}",
        setup.log.display()
    )))
}

/// Stop a process group: politely, then not.
fn stop_group(pid: u32) {
    let group = format!("-{pid}");
    for signal in ["-TERM", "-KILL"] {
        let _ = Command::new("kill")
            .args([signal, "--", &group])
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Remove a throwaway prefix. A failure to remove it is reported, not fatal: it is
/// disk space, not correctness.
pub fn discard_prefix(prefix: &Path) -> Option<PathBuf> {
    std::fs::remove_dir_all(prefix)
        .err()
        .filter(|error| error.kind() != std::io::ErrorKind::NotFound)
        .map(|_| prefix.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn linux_paths_go_through_the_z_drive() {
        assert_eq!(
            wine_path(Path::new("/home/joshua/games/Xeno Crisis")),
            "Z:\\home\\joshua\\games\\Xeno Crisis"
        );
    }

    /// A stand-in for the `proton` script: it records its arguments and environment,
    /// and writes the log the way InnoSetup would, or not.
    fn fake_proton(dir: &Path, body: &str) -> ProtonTool {
        let script = dir.join("proton");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        ProtonTool {
            name: "proton_test".to_string(),
            dir: dir.to_path_buf(),
        }
    }

    struct Fixture {
        temp: tempfile::TempDir,
        proton: ProtonTool,
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(temp.path().join("setup.exe"), "MZ").unwrap();
            let tools = temp.path().join("tool");
            std::fs::create_dir_all(&tools).unwrap();
            let proton = fake_proton(&tools, body);
            Self { temp, proton }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.temp.path().join(name)
        }

        fn run(&self, timeout: Duration) -> Result<()> {
            let (installer, target, prefix, log, steam) = (
                self.path("setup.exe"),
                self.path("games/Game"),
                self.path("prefix"),
                self.path("logs/install.log"),
                self.path("steam"),
            );
            install(&Setup {
                proton: &self.proton,
                steam_root: &steam,
                installer: &installer,
                target: &target,
                prefix: &prefix,
                log: &log,
                timeout,
            })
        }
    }

    /// Turn the `/LOG=Z:\…` argument back into a Linux path inside the script.
    const LOG_FROM_ARGS: &str = r#"for a in "$@"; do case "$a" in /LOG=*) log=$(printf '%s' "${a#/LOG=Z:}" | tr '\\' /);; esac; done"#;

    #[test]
    fn a_successful_install_passes_the_silent_flags_and_the_prefix() {
        let fixture = Fixture::new(&format!(
            r#"{LOG_FROM_ARGS}
printf '%s\n' "$@" > "$(dirname "$0")/args"
printf '%s\n' "$STEAM_COMPAT_DATA_PATH" > "$(dirname "$0")/env"
printf 'Installation process succeeded.\n' > "$log""#
        ));

        fixture.run(Duration::from_secs(10)).unwrap();

        let args = std::fs::read_to_string(fixture.path("tool/args")).unwrap();
        let args: Vec<_> = args.lines().collect();
        assert_eq!(args[0], "run");
        assert!(Path::new(args[1]).is_absolute(), "{args:?}");
        assert!(args.contains(&"/VERYSILENT"), "{args:?}");
        assert!(args.iter().any(|a| a.starts_with("/DIR=Z:\\")), "{args:?}");
        let env = std::fs::read_to_string(fixture.path("tool/env")).unwrap();
        assert_eq!(env.trim(), fixture.path("prefix").display().to_string());
    }

    #[test]
    fn a_zero_exit_without_the_success_line_is_a_failure() {
        let fixture = Fixture::new(&format!(
            "{LOG_FROM_ARGS}\nprintf 'Something went sideways.\\n' > \"$log\""
        ));

        let error = fixture.run(Duration::from_secs(10)).unwrap_err();

        assert!(
            error.to_string().contains("Something went sideways"),
            "{error}"
        );
    }

    #[test]
    fn no_log_at_all_points_at_the_display() {
        let fixture = Fixture::new("exit 1");

        let error = fixture.run(Duration::from_secs(10)).unwrap_err();

        assert!(error.to_string().contains("display"), "{error}");
    }

    #[test]
    fn a_stuck_installer_is_stopped_at_the_deadline() {
        let fixture = Fixture::new("sleep 60");
        let started = Instant::now();

        let error = fixture.run(Duration::from_millis(300)).unwrap_err();

        assert!(error.to_string().contains("dialog"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
