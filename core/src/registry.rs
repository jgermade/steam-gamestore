//! What is installed on this machine, and how far along.
//!
//! The catalog says what the account *owns*; it cannot say what is on the disk.
//! The grid's badge, the uninstall path and the launch wrapper all need that
//! second answer, and this is where it lives.
//!
//! Unlike the catalog, **this is not derived data**. A lost catalog costs one
//! refresh; a lost registry means gamestore no longer knows what it installed, and
//! the games become files nobody owns. That difference decides two things here: a
//! registry that cannot be parsed is an error rather than a fresh start, and every
//! write goes through [`crate::atomic`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::catalog::ProductId;
use crate::{Error, Paths, Result};

/// How long a claim on a transient entry is believed without other evidence.
///
/// The liveness check is what normally clears a dead claim; this is the backstop
/// for platforms where there is no cheap one. Generous on purpose: a real download
/// over a slow line can take hours, and clearing a live one is worse than leaving a
/// dead one an extra day.
pub const CLAIM_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Where a game has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Owned, nothing on disk.
    NotInstalled,
    /// Asked for, not started.
    Queued,
    /// Fetching files.
    Downloading,
    /// Running the installer.
    Installing,
    /// Playable.
    Installed,
    /// Playable, but GOG has a newer build.
    UpdateAvailable,
    /// The last attempt failed; `failure` says why.
    Failed,
}

impl State {
    /// Whether this state means "a process is working on it right now".
    ///
    /// These are the states that cannot survive the process that set them: nothing
    /// is downloading if nothing is running.
    pub fn is_transient(self) -> bool {
        matches!(self, State::Queued | State::Downloading | State::Installing)
    }

    /// Whether the game can be launched.
    pub fn is_playable(self) -> bool {
        matches!(self, State::Installed | State::UpdateAvailable)
    }
}

/// Who is working on a transient entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// Process id that claimed it.
    pub pid: u32,
    /// When it was claimed.
    pub at: SystemTime,
}

/// One game's row in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The GOG product id, which is also the map key.
    pub id: ProductId,
    /// Where it has got to.
    pub state: State,
    /// Installed build or version, once there is one.
    #[serde(default)]
    pub build: Option<String>,
    /// Where the files are.
    #[serde(default)]
    pub install_path: Option<PathBuf>,
    /// The generated launch wrapper the Steam shortcut points at.
    #[serde(default)]
    pub wrapper_path: Option<PathBuf>,
    /// The Steam appid of the shortcut, so uninstall can find it again.
    #[serde(default)]
    pub steam_appid: Option<u32>,
    /// Last successful launch, for "last played" ordering.
    #[serde(default)]
    pub last_played: Option<SystemTime>,
    /// Why the last attempt failed, when it did.
    #[serde(default)]
    pub failure: Option<String>,
    /// Set while a process is working on it.
    #[serde(default)]
    pub claim: Option<Claim>,
}

impl Entry {
    /// A row for a game nothing has been done to yet.
    pub fn new(id: ProductId) -> Self {
        Self {
            id,
            state: State::NotInstalled,
            build: None,
            install_path: None,
            wrapper_path: None,
            steam_appid: None,
            last_played: None,
            failure: None,
            claim: None,
        }
    }
}

/// Everything gamestore has installed, or tried to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    entries: BTreeMap<ProductId, Entry>,
}

impl Registry {
    /// Where the registry file lives.
    pub fn path(paths: &Paths) -> PathBuf {
        paths.data_dir.join("registry.json")
    }

    /// Read the registry. A file that is not there yet is an empty registry.
    ///
    /// A file that is there but cannot be parsed is an **error**: it is the only
    /// record of what is installed, and starting fresh would orphan every
    /// installed game silently. The message points at the file so it can be moved
    /// aside deliberately.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = Self::path(paths);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                Error::Registry(format!(
                    "{} could not be read ({error}); it is the only record of what is \
                     installed, so it is not being replaced — move it aside to start over",
                    path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::io(format!("reading {}", path.display()), error)),
        }
    }

    /// Write the registry.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(|error| {
            Error::Registry(format!("the registry could not be written ({error})"))
        })?;

        crate::atomic::write(
            &Self::path(paths),
            text.as_bytes(),
            crate::atomic::Access::Default,
        )
    }

    /// How many games have a row.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been installed or attempted.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// One game's row.
    pub fn get(&self, id: &ProductId) -> Option<&Entry> {
        self.entries.get(id)
    }

    /// Every row, ordered by product id.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// The state of a game, `NotInstalled` when it has no row at all.
    pub fn state_of(&self, id: &ProductId) -> State {
        self.get(id)
            .map_or(State::NotInstalled, |entry| entry.state)
    }

    /// Change a row, creating it when it is not there.
    pub fn update(&mut self, id: &ProductId, change: impl FnOnce(&mut Entry)) -> &Entry {
        let entry = self
            .entries
            .entry(id.clone())
            .or_insert_with(|| Entry::new(id.clone()));
        change(entry);

        entry
    }

    /// Take ownership of a game for a transient operation.
    ///
    /// The claim is what makes a crash recoverable: it records which process is
    /// working on the entry, so a later run can tell "being installed right now"
    /// from "was being installed when the machine was switched off".
    pub fn claim(&mut self, id: &ProductId, state: State, pid: u32, now: SystemTime) {
        debug_assert!(state.is_transient(), "only transient states are claimed");
        self.update(id, |entry| {
            entry.state = state;
            entry.failure = None;
            entry.claim = Some(Claim { pid, at: now });
        });
    }

    /// Record a finished installation.
    pub fn mark_installed(
        &mut self,
        id: &ProductId,
        build: impl Into<String>,
        install_path: PathBuf,
        steam_appid: u32,
    ) {
        self.update(id, |entry| {
            entry.state = State::Installed;
            entry.build = Some(build.into());
            entry.install_path = Some(install_path);
            entry.steam_appid = Some(steam_appid);
            entry.failure = None;
            entry.claim = None;
        });
    }

    /// Record a failed attempt, keeping what is known so far.
    pub fn mark_failed(&mut self, id: &ProductId, reason: impl Into<String>) {
        self.update(id, |entry| {
            entry.state = State::Failed;
            entry.failure = Some(reason.into());
            entry.claim = None;
        });
    }

    /// Record a successful launch.
    pub fn record_launch(&mut self, id: &ProductId, now: SystemTime) {
        self.update(id, |entry| entry.last_played = Some(now));
    }

    /// Forget a game entirely, after `uninst` has removed everything.
    pub fn remove(&mut self, id: &ProductId) -> bool {
        self.entries.remove(id).is_some()
    }

    /// Clear claims left behind by a process that is no longer running.
    ///
    /// This is what keeps a machine switched off mid-install from leaving a tile
    /// stuck on "Installing" forever. An entry whose claim is dead goes to
    /// [`State::Failed`] with a reason, which is a state the UI offers a retry from
    /// — the roadmap's "resumable or clearable, not permanently stuck".
    ///
    /// A claim is dead when the process is not running, or when it is older than
    /// [`CLAIM_MAX_AGE`]. The age is the backstop for platforms with no cheap
    /// liveness check; on Linux `is_alive` answers it directly.
    ///
    /// Returns the games that were reset, so the caller can say what it did.
    pub fn reconcile_with(
        &mut self,
        is_alive: impl Fn(u32) -> bool,
        now: SystemTime,
    ) -> Vec<ProductId> {
        let mut reset = Vec::new();

        for entry in self.entries.values_mut() {
            if !entry.state.is_transient() {
                continue;
            }

            let dead = match &entry.claim {
                // A transient state with no claim at all cannot be owned by
                // anything: whatever set it never recorded itself, and the only
                // safe reading is that it is over.
                None => true,
                Some(claim) => {
                    let too_old = now
                        .duration_since(claim.at)
                        .map(|age| age > CLAIM_MAX_AGE)
                        .unwrap_or(false);

                    !is_alive(claim.pid) || too_old
                }
            };

            if dead {
                entry.state = State::Failed;
                entry.failure = Some(
                    "interrupted before it finished — the process that started it is \
                     gone; start it again"
                        .to_string(),
                );
                entry.claim = None;
                reset.push(entry.id.clone());
            }
        }

        reset
    }

    /// [`Registry::reconcile_with`] using this platform's liveness check.
    pub fn reconcile(&mut self, now: SystemTime) -> Vec<ProductId> {
        self.reconcile_with(pid_is_alive, now)
    }
}

/// Whether a process is still running.
///
/// On Linux this is `/proc/<pid>` and it is exact. Everywhere else there is no
/// check available without pulling in a dependency for it, so it answers "alive"
/// and [`CLAIM_MAX_AGE`] becomes the thing that eventually clears a dead claim.
/// Answering "alive" is the conservative direction: the cost of being wrong is a
/// tile that stays busy for a day, where the other way round is clobbering a
/// download that is still running.
pub fn pid_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: SystemTime = SystemTime::UNIX_EPOCH;

    fn paths_in(dir: &std::path::Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            data_dir: dir.join("data"),
            cache_dir: dir.join("cache"),
        }
    }

    fn bastion() -> ProductId {
        ProductId::new("1207658930")
    }

    fn alive(_: u32) -> bool {
        true
    }

    fn dead(_: u32) -> bool {
        false
    }

    #[test]
    fn a_game_with_no_row_is_not_installed() {
        let registry = Registry::default();

        assert_eq!(registry.state_of(&bastion()), State::NotInstalled);
        assert!(registry.is_empty());
    }

    #[test]
    fn an_install_records_everything_uninstall_will_need() {
        let mut registry = Registry::default();

        registry.mark_installed(
            &bastion(),
            "build-42",
            PathBuf::from("/games/Bastion"),
            4057670083,
        );

        let entry = registry.get(&bastion()).unwrap();
        assert_eq!(entry.state, State::Installed);
        assert_eq!(entry.build.as_deref(), Some("build-42"));
        assert_eq!(entry.install_path, Some(PathBuf::from("/games/Bastion")));
        assert_eq!(entry.steam_appid, Some(4057670083));
        assert!(entry.state.is_playable());
    }

    #[test]
    fn a_claim_survives_only_as_long_as_the_process_that_made_it() {
        let mut registry = Registry::default();
        registry.claim(&bastion(), State::Downloading, 4321, T0);

        // While the process is alive, nothing is touched.
        assert!(registry.reconcile_with(alive, T0).is_empty());
        assert_eq!(registry.state_of(&bastion()), State::Downloading);

        // Once it is gone, the entry becomes something the user can act on.
        let reset = registry.reconcile_with(dead, T0);

        assert_eq!(reset, vec![bastion()]);
        assert_eq!(registry.state_of(&bastion()), State::Failed);
        let entry = registry.get(&bastion()).unwrap();
        assert!(entry.failure.as_ref().unwrap().contains("interrupted"));
        assert_eq!(entry.claim, None);
    }

    #[test]
    fn a_stale_claim_is_cleared_even_when_the_pid_looks_alive() {
        // The backstop for platforms with no liveness check: a pid is reused, or
        // the check always answers yes, and the claim is a day old.
        let mut registry = Registry::default();
        registry.claim(&bastion(), State::Installing, 4321, T0);

        let reset = registry.reconcile_with(alive, T0 + CLAIM_MAX_AGE + Duration::from_secs(1));

        assert_eq!(reset, vec![bastion()]);
        assert_eq!(registry.state_of(&bastion()), State::Failed);
    }

    #[test]
    fn a_transient_state_with_no_claim_is_treated_as_interrupted() {
        let mut registry = Registry::default();
        registry.update(&bastion(), |entry| entry.state = State::Installing);

        assert_eq!(registry.reconcile_with(alive, T0), vec![bastion()]);
        assert_eq!(registry.state_of(&bastion()), State::Failed);
    }

    #[test]
    fn reconciling_never_touches_a_settled_entry() {
        let mut registry = Registry::default();
        registry.mark_installed(&bastion(), "b", PathBuf::from("/games/B"), 1);
        let other = ProductId::new("2");
        registry.mark_failed(&other, "the installer ignored /VERYSILENT");
        let before = registry.clone();

        assert!(
            registry
                .reconcile_with(dead, T0 + CLAIM_MAX_AGE * 10)
                .is_empty()
        );

        assert_eq!(registry, before);
    }

    #[test]
    fn claiming_again_clears_a_previous_failure() {
        let mut registry = Registry::default();
        registry.mark_failed(&bastion(), "no space left on device");

        registry.claim(&bastion(), State::Downloading, 99, T0);

        let entry = registry.get(&bastion()).unwrap();
        assert_eq!(entry.state, State::Downloading);
        assert_eq!(entry.failure, None, "a retry must not show the old reason");
    }

    #[test]
    fn a_launch_is_recorded_without_disturbing_the_state() {
        let mut registry = Registry::default();
        registry.mark_installed(&bastion(), "b", PathBuf::from("/games/B"), 1);

        registry.record_launch(&bastion(), T0 + Duration::from_secs(99));

        let entry = registry.get(&bastion()).unwrap();
        assert_eq!(entry.last_played, Some(T0 + Duration::from_secs(99)));
        assert_eq!(entry.state, State::Installed);
    }

    #[test]
    fn removing_forgets_the_game_and_says_whether_there_was_one() {
        let mut registry = Registry::default();
        registry.mark_installed(&bastion(), "b", PathBuf::from("/games/B"), 1);

        assert!(registry.remove(&bastion()));
        assert!(!registry.remove(&bastion()));
        assert!(registry.is_empty());
    }

    #[test]
    fn the_registry_round_trips_through_its_file() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        let mut registry = Registry::default();
        registry.mark_installed(
            &bastion(),
            "build-42",
            PathBuf::from("/games/B"),
            4057670083,
        );
        registry.record_launch(&bastion(), T0 + Duration::from_secs(5));
        registry.claim(&ProductId::new("2"), State::Downloading, 7, T0);

        registry.save(&paths).unwrap();

        assert_eq!(Registry::load(&paths).unwrap(), registry);
    }

    #[test]
    fn no_registry_file_is_an_empty_registry() {
        let temp = tempfile::tempdir().unwrap();

        assert!(Registry::load(&paths_in(temp.path())).unwrap().is_empty());
    }

    #[test]
    fn a_corrupt_registry_refuses_rather_than_orphaning_every_install() {
        // The opposite of the catalog on purpose: this is not derived data, and
        // quietly starting fresh would leave installed games nobody owns.
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(Registry::path(&paths), "{ not json").unwrap();

        let error = Registry::load(&paths).unwrap_err();

        assert!(error.to_string().contains("registry.json"), "{error}");
        assert!(error.to_string().contains("move it aside"), "{error}");
    }

    #[test]
    fn a_registry_written_by_an_older_version_still_loads() {
        // Every optional field defaulted, so adding one later does not invalidate
        // what is already on disk.
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_in(temp.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(
            Registry::path(&paths),
            r#"{"entries":{"1207658930":{"id":"1207658930","state":"installed"}}}"#,
        )
        .unwrap();

        let registry = Registry::load(&paths).unwrap();

        assert_eq!(registry.state_of(&bastion()), State::Installed);
        assert_eq!(registry.get(&bastion()).unwrap().build, None);
    }

    #[test]
    fn this_process_counts_as_alive() {
        assert!(pid_is_alive(std::process::id()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_pid_that_is_not_running_is_seen_as_dead_on_linux() {
        // pid 0 is never a real process in /proc.
        assert!(!pid_is_alive(0));
    }
}
