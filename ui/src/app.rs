//! The screens: the cover grid, a game's detail, the keyboard, confirmations.
//!
//! Everything is painted directly rather than built from egui's mouse-oriented
//! widgets: focus and scrolling are this module's own state, moved only by
//! [`Nav`] actions, which is what a controller needs.

use std::collections::{HashMap, HashSet};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, TextureHandle, TextureOptions,
    Vec2, pos2, text::LayoutJob, vec2,
};
use gamestore_core::artwork::{Art, Choice, Kind};
use gamestore_core::catalog::{Game, Os, ProductId};
use gamestore_core::details::{Details, Storage, human_size};
use gamestore_core::registry::{Registry, State};
use gamestore_platform_linux::{install, pending};

use crate::covers::{self, Covers};
use crate::icons;
use crate::input::{Input, Nav};
use crate::jobs::{Jobs, Machine, Progress};
use crate::media::{Answer, Apply, Media};
use crate::osk::{self, Keyboard};
use crate::text::Text;
use crate::theme;

/// What the registry says about a game, kept between reloads.
#[derive(Debug, Clone, Default)]
struct Known {
    state: Option<State>,
    failure: Option<String>,
    install_path: Option<String>,
    build: Option<String>,
    platform: Option<Os>,
    /// The game's Steam tile.
    appid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    All,
    Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Grid,
    Detail {
        button: usize,
    },
    Keyboard,
    ConfirmRestart {
        button: usize,
    },
    ConfirmQuit {
        button: usize,
    },
    /// What is downloading or installing, and what finished this session.
    Downloads,
    /// Start: restart Steam, or quit.
    Menu {
        button: usize,
    },
    /// Choosing the focused game's artwork from SteamGridDB.
    Art,
}

/// The artwork picker's state: one kind at a time, its images in a row.
struct Picker {
    id: ProductId,
    /// Index into [`Kind::ALL`].
    kind: usize,
    /// What SteamGridDB has of each kind, once asked.
    choices: HashMap<Kind, Result<Vec<Choice>, String>>,
    previews: HashMap<(Kind, usize), Option<TextureHandle>>,
    asked: HashSet<(Kind, usize)>,
    /// The image in focus for each kind.
    focus: HashMap<Kind, usize>,
    /// The images picked so far, by URL.
    picked: HashMap<Kind, String>,
    /// Being put in place: its kind and address.
    applying: Option<(Kind, String)>,
}

impl Picker {
    fn kind(&self) -> Kind {
        Kind::ALL[self.kind]
    }

    fn focused(&self) -> usize {
        self.focus.get(&self.kind()).copied().unwrap_or(0)
    }

    fn count(&self) -> usize {
        self.choices
            .get(&self.kind())
            .and_then(|found| found.as_ref().ok())
            .map_or(0, Vec::len)
    }
}

pub struct App {
    machine: Machine,
    text: &'static Text,
    games: Vec<Game>,
    /// Folded titles, for search.
    keys: Vec<String>,
    known: HashMap<ProductId, Known>,
    progress: HashMap<ProductId, Progress>,
    filter: String,
    tab: Tab,
    visible: Vec<usize>,
    selected: usize,
    columns: usize,
    scroll: f32,
    screen: Screen,
    keyboard: Keyboard,
    textures: HashMap<ProductId, Option<TextureHandle>>,
    input: Input,
    covers: Covers,
    jobs: Jobs,
    media: Media,
    details: HashMap<ProductId, Details>,
    /// Details asked for, so each game is asked once.
    asked: HashSet<ProductId>,
    /// The current game's screenshots: requested, and the ones that arrived.
    shots_asked: HashSet<usize>,
    shots: HashMap<usize, Option<TextureHandle>>,
    shots_of: Option<ProductId>,
    shot: usize,
    /// Where the screenshot column is scrolled to, easing towards `shot`.
    shot_scroll: f32,
    disk: HashMap<ProductId, u64>,
    storage: Option<Option<Storage>>,
    storage_asked: Option<Instant>,
    /// A video playing in mpv, to stop it from the controller.
    video: Arc<Mutex<Option<Child>>>,
    /// Which build each install started this session is of.
    building: HashMap<ProductId, Os>,
    loaded_at: Instant,
    pending: usize,
    notice: Option<(String, Instant)>,
    picker: Option<Picker>,
    /// The row in focus in the downloads panel.
    downloads_focus: usize,
    /// The grid's focused game and since when, to fetch its details before it
    /// is opened.
    dwell: Option<(ProductId, Instant)>,
}

impl App {
    pub fn new(ctx: &egui::Context, machine: Machine, mut games: Vec<Game>) -> Self {
        games.retain(|game| !install::builds(game).is_empty());
        games.sort_by_key(|game| fold(&game.title));
        let keys = games.iter().map(|game| fold(&game.title)).collect();

        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let covers = Covers::new(&machine.paths.cache_dir, wake.clone());
        for game in &games {
            covers.request(game.id.clone());
        }
        let jobs = Jobs::new(machine.clone(), wake.clone());
        let media = Media::new(machine.clone(), wake);

        let mut app = Self {
            machine,
            text: crate::text::current(),
            games,
            keys,
            known: HashMap::new(),
            progress: HashMap::new(),
            filter: String::new(),
            tab: Tab::All,
            visible: Vec::new(),
            selected: 0,
            columns: 6,
            scroll: 0.0,
            screen: Screen::Grid,
            keyboard: Keyboard::default(),
            textures: HashMap::new(),
            input: Input::new(),
            covers,
            jobs,
            media,
            details: HashMap::new(),
            asked: HashSet::new(),
            shots_asked: HashSet::new(),
            shots: HashMap::new(),
            shots_of: None,
            shot: 0,
            shot_scroll: 0.0,
            disk: HashMap::new(),
            storage: None,
            storage_asked: None,
            video: Arc::new(Mutex::new(None)),
            building: HashMap::new(),
            loaded_at: Instant::now() - Duration::from_secs(60),
            pending: 0,
            notice: None,
            picker: None,
            downloads_focus: 0,
            dwell: None,
        };
        app.reload();
        app.refilter();
        app
    }

    /// Re-read the registry and the pending queue: installs from the CLI, or from
    /// an earlier run of the UI, show up without restarting it.
    fn reload(&mut self) {
        self.loaded_at = Instant::now();
        if let Ok(registry) = Registry::load(&self.machine.paths) {
            self.known = registry
                .entries()
                .map(|entry| {
                    (
                        entry.id.clone(),
                        Known {
                            state: Some(entry.state),
                            failure: entry.failure.clone(),
                            install_path: entry
                                .install_path
                                .as_ref()
                                .map(|path| path.display().to_string()),
                            build: entry.build.clone().filter(|build| !build.is_empty()),
                            platform: entry.platform,
                            appid: entry.steam_appid,
                        },
                    )
                })
                .collect();
        }
        self.pending = pending::list(&self.machine.paths).len();
    }

    fn installed(&self, id: &ProductId) -> bool {
        self.known
            .get(id)
            .and_then(|known| known.state)
            .is_some_and(State::is_playable)
    }

    fn refilter(&mut self) {
        let words: Vec<String> = fold(&self.filter)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let current = self.current().map(|game| game.id.clone());
        self.visible = (0..self.games.len())
            .filter(|&index| {
                words
                    .iter()
                    .all(|word| self.keys[index].contains(word.as_str()))
            })
            .filter(|&index| self.tab == Tab::All || self.installed(&self.games[index].id))
            .collect();
        // Keep the same game focused when it is still there.
        self.selected = current
            .and_then(|id| {
                self.visible
                    .iter()
                    .position(|&index| self.games[index].id == id)
            })
            .unwrap_or(0);
    }

    fn current(&self) -> Option<&Game> {
        self.visible
            .get(self.selected)
            .map(|&index| &self.games[index])
    }

    fn any_active(&self) -> bool {
        self.progress.values().any(Progress::active)
    }

    fn notify(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
    }

    fn start_install(&mut self, os: Os) {
        if let Some(game) = self.current().cloned() {
            self.install_game(game, os);
        }
    }

    fn install_game(&mut self, game: Game, os: Os) {
        if !install::builds(&game).contains(&os) {
            return;
        }
        if self.progress.get(&game.id).is_some_and(Progress::active) {
            return;
        }
        self.progress.insert(game.id.clone(), Progress::Queued);
        self.building.insert(game.id.clone(), os);
        self.jobs.install(game, os);
    }

    /// The downloads panel's rows: working ones first, in the order they will
    /// run; then failures, which can be retried; then what finished.
    fn download_rows(&self) -> Vec<(&Game, &Progress)> {
        let mut rows: Vec<(&Game, &Progress)> = self
            .games
            .iter()
            .filter_map(|game| self.progress.get(&game.id).map(|progress| (game, progress)))
            .collect();
        rows.sort_by_key(|(_, progress)| match progress {
            Progress::Downloading { .. } | Progress::Installing | Progress::Finishing => 0,
            Progress::Queued => 1,
            Progress::Failed(_) => 2,
            Progress::Done { .. } => 3,
        });
        rows
    }

    /// The failed install in focus in the downloads panel, with the build to retry.
    fn focused_failure(&self) -> Option<(Game, Os)> {
        let (game, progress) = self.download_rows().get(self.downloads_focus).copied()?;
        if !matches!(progress, Progress::Failed(_)) {
            return None;
        }
        let os = self
            .building
            .get(&game.id)
            .copied()
            .or_else(|| install::preferred(game))?;
        Some((game.clone(), os))
    }

    /// The build a retry of `game` installs: the one that failed this session,
    /// else the only one there is.
    fn retry_build(&self, game: &Game) -> Option<Os> {
        self.failure(&game.id)?;
        self.building.get(&game.id).copied().or_else(|| {
            let builds = install::builds(game);
            (builds.len() == 1).then(|| builds[0])
        })
    }

    /// Open the artwork picker for the focused game, asking SteamGridDB for every
    /// kind at once.
    fn open_art(&mut self) {
        let Some(game) = self.current().cloned() else {
            return;
        };
        if self.machine.config.steamgriddb.api_key.is_none() {
            self.notify(self.text.art_needs_key);
            return;
        }
        let picked = gamestore_core::artwork::chosen(&self.machine.paths, &game.id)
            .into_iter()
            .map(|art| (art.kind, art.url))
            .collect();
        let reuse = self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.id == game.id);
        if !reuse {
            for kind in Kind::ALL {
                self.media
                    .choices(game.id.clone(), game.title.clone(), kind);
            }
            self.picker = Some(Picker {
                id: game.id.clone(),
                kind: 0,
                choices: HashMap::new(),
                previews: HashMap::new(),
                asked: HashSet::new(),
                focus: HashMap::new(),
                picked,
                applying: None,
            });
        }
        self.screen = Screen::Art;
    }

    /// Ask for the previews around the focused one, once each.
    fn want_previews(&mut self) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let kind = picker.kind();
        let Some(Ok(found)) = picker.choices.get(&kind) else {
            return;
        };
        let focused = picker.focused();
        let wanted: Vec<(usize, String)> = (focused.saturating_sub(3)
            ..(focused + 6).min(found.len()))
            .map(|index| (index, found[index].thumb.clone()))
            .collect();
        for (index, url) in wanted {
            if picker.asked.insert((kind, index)) {
                self.media.preview(picker.id.clone(), kind, index, url);
            }
        }
    }

    fn handle_art(&mut self, nav: Nav) {
        let Some(picker) = self.picker.as_mut() else {
            self.screen = Screen::Grid;
            return;
        };
        let count = picker.count();
        let kind = picker.kind();
        match nav {
            Nav::Left => {
                let focus = picker.focus.entry(kind).or_insert(0);
                *focus = focus.saturating_sub(1);
            }
            Nav::Right => {
                let focus = picker.focus.entry(kind).or_insert(0);
                *focus = (*focus + 1).min(count.saturating_sub(1));
            }
            Nav::Up | Nav::Previous => picker.kind = picker.kind.saturating_sub(1),
            Nav::Down | Nav::Next => picker.kind = (picker.kind + 1).min(Kind::ALL.len() - 1),
            Nav::Accept if picker.applying.is_none() && count > 0 => {
                let Some(Ok(found)) = picker.choices.get(&kind) else {
                    return;
                };
                let url = found[picker.focused()].url.clone();
                picker.applying = Some((kind, url.clone()));
                let id = picker.id.clone();
                let appid = self.known.get(&id).and_then(|known| known.appid);
                self.media.apply(Apply {
                    id,
                    art: Art { kind, url },
                    appid,
                });
            }
            Nav::Back => self.screen = Screen::Detail { button: 0 },
            _ => {}
        }
    }

    fn current_details(&self) -> Option<&Details> {
        self.current().and_then(|game| self.details.get(&game.id))
    }

    /// Show the focused game's detail, asking for what it needs.
    fn open_detail(&mut self) {
        let Some(game) = self.current().cloned() else {
            return;
        };
        self.screen = Screen::Detail { button: 0 };
        if self.asked.insert(game.id.clone()) {
            self.media.details(game.id.clone());
        }
        if self.shots_of.as_ref() != Some(&game.id) {
            // Only one game's screenshots are kept, to keep video memory small.
            self.shots.clear();
            self.shots_asked.clear();
            self.shots_of = Some(game.id.clone());
            self.shot = 0;
            self.shot_scroll = 0.0;
        }
        if let Some(path) = self
            .known
            .get(&game.id)
            .filter(|known| known.state.is_some_and(State::is_playable))
            .and_then(|known| known.install_path.clone())
        {
            self.media.disk_usage(game.id.clone(), path.into());
        }
    }

    fn open_downloads(&mut self) {
        self.screen = Screen::Downloads;
        self.downloads_focus = 0;
        let stale = self
            .storage_asked
            .is_none_or(|at| at.elapsed() > Duration::from_secs(30));
        if stale {
            self.storage_asked = Some(Instant::now());
            self.media
                .storage(gamestore_platform_linux::proton::library_folders(
                    &self.machine.steam_root,
                ));
        }
    }

    /// Ask for the screenshot on show and the next one, once each.
    fn want_screenshots(&mut self) {
        let Some(game) = self.current().cloned() else {
            return;
        };
        let Some(details) = self.details.get(&game.id) else {
            return;
        };
        let count = details.screenshots.len();
        let previous = (self.shot + count.max(1) - 1) % count.max(1);
        let next = |step: usize| (self.shot + step) % count.max(1);
        for index in [self.shot, next(1), next(2), previous] {
            if index < count && self.shots_asked.insert(index) {
                self.media.screenshot(
                    game.id.clone(),
                    index,
                    details.screenshots[index].full.clone(),
                );
            }
        }
    }

    fn video_playing(&self) -> bool {
        let mut slot = self.video.lock().expect("not poisoned");
        match slot.as_mut().map(Child::try_wait) {
            Some(Ok(None)) => true,
            Some(_) => {
                *slot = None;
                false
            }
            None => false,
        }
    }

    fn stop_video(&self) {
        if let Some(mut child) = self.video.lock().expect("not poisoned").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Play the game's first video in mpv, fullscreen, through yt-dlp. While it
    /// plays the UI ignores the controller except B, which stops it.
    fn play_video(&mut self) {
        let Some(url) = self
            .current_details()
            .and_then(|d| d.videos.first())
            .cloned()
        else {
            return;
        };
        let has = |tool: &str| {
            std::process::Command::new("sh")
                .args(["-c", &format!("command -v {tool}")])
                .stdout(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        if !has("yt-dlp") || !has("mpv") {
            self.notify(self.text.video_needs);
            return;
        }
        let child = std::process::Command::new("mpv")
            .args([
                "--fs",
                "--really-quiet",
                "--ytdl-format=bestvideo[height<=?1080]+bestaudio/best",
                &url,
            ])
            .stdin(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(child) => *self.video.lock().expect("not poisoned") = Some(child),
            Err(error) => self.notify(error.to_string()),
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        if self.any_active() {
            self.screen = Screen::ConfirmQuit { button: 1 };
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn restart_steam(&mut self, ctx: &egui::Context) {
        if !pending::has_steam_unit() {
            self.notify(self.text.no_unit);
            return;
        }
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            });
        if let (Some(config_home), Ok(binary)) = (config_home, std::env::current_exe())
            && !pending::hook_installed(&config_home)
            && let Err(error) = pending::hook(&config_home, &binary, false)
        {
            self.notify(error.to_string());
            return;
        }
        match pending::restart_steam() {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.notify(error.to_string()),
        }
    }

    fn handle(&mut self, nav: Nav, ctx: &egui::Context) {
        match self.screen {
            Screen::Grid => self.handle_grid(nav, ctx),
            Screen::Keyboard => {
                let mut filter = std::mem::take(&mut self.filter);
                let outcome = self.keyboard.handle(nav, &mut filter);
                self.filter = filter;
                self.refilter();
                if outcome == osk::Outcome::Done {
                    self.screen = Screen::Grid;
                }
            }
            Screen::Detail { button } => {
                let buttons = self.detail_buttons().len();
                match nav {
                    Nav::Left => {
                        self.screen = Screen::Detail {
                            button: button.saturating_sub(1),
                        }
                    }
                    Nav::Right => {
                        self.screen = Screen::Detail {
                            button: (button + 1).min(buttons.saturating_sub(1)),
                        }
                    }
                    Nav::Accept => match self.detail_buttons().get(button).copied() {
                        Some(DetailAction::Install(os)) => {
                            self.start_install(os);
                            self.screen = Screen::Grid;
                        }
                        Some(DetailAction::Switch(os)) => {
                            self.start_install(os);
                            self.screen = Screen::Grid;
                        }
                        Some(DetailAction::Restart) => {
                            self.screen = Screen::ConfirmRestart { button: 0 }
                        }
                        Some(DetailAction::Art) => self.open_art(),
                        Some(DetailAction::Back) | None => self.screen = Screen::Grid,
                    },
                    // The screenshots run top to bottom, so up and down go through
                    // them; the shoulders do too. They stop at the ends rather
                    // than wrap, which a column makes obvious.
                    Nav::Up | Nav::Down | Nav::Previous | Nav::Next => {
                        let count = self.current_details().map_or(0, |d| d.screenshots.len());
                        if matches!(nav, Nav::Down | Nav::Next) {
                            self.shot = (self.shot + 1).min(count.saturating_sub(1));
                        } else {
                            self.shot = self.shot.saturating_sub(1);
                        }
                    }
                    Nav::Search => self.play_video(),
                    Nav::Back => self.screen = Screen::Grid,
                    _ => {}
                }
            }
            Screen::ConfirmRestart { button } => match nav {
                Nav::Left | Nav::Right | Nav::Up | Nav::Down => {
                    self.screen = Screen::ConfirmRestart { button: 1 - button }
                }
                Nav::Accept if button == 0 => {
                    self.screen = Screen::Grid;
                    self.restart_steam(ctx);
                }
                Nav::Accept | Nav::Back => self.screen = Screen::Grid,
                _ => {}
            },
            Screen::Downloads => {
                let rows = self.download_rows().len();
                match nav {
                    Nav::Up => self.downloads_focus = self.downloads_focus.saturating_sub(1),
                    Nav::Down => {
                        self.downloads_focus =
                            (self.downloads_focus + 1).min(rows.saturating_sub(1))
                    }
                    Nav::Accept => {
                        if let Some((game, os)) = self.focused_failure() {
                            self.install_game(game, os);
                        }
                    }
                    Nav::Back | Nav::Action => self.screen = Screen::Grid,
                    _ => {}
                }
            }
            Screen::Art => self.handle_art(nav),
            Screen::Menu { button } => match nav {
                Nav::Up | Nav::Left => {
                    self.screen = Screen::Menu {
                        button: button.saturating_sub(1),
                    }
                }
                Nav::Down | Nav::Right => {
                    self.screen = Screen::Menu {
                        button: (button + 1).min(2),
                    }
                }
                Nav::Accept => match button {
                    0 => {
                        self.screen = Screen::Grid;
                        self.restart_steam(ctx);
                    }
                    1 => {
                        self.screen = Screen::Grid;
                        self.quit(ctx);
                    }
                    _ => self.screen = Screen::Grid,
                },
                Nav::Back | Nav::Menu => self.screen = Screen::Grid,
                _ => {}
            },
            Screen::ConfirmQuit { button } => match nav {
                Nav::Left | Nav::Right | Nav::Up | Nav::Down => {
                    self.screen = Screen::ConfirmQuit { button: 1 - button }
                }
                Nav::Accept if button == 0 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Nav::Accept | Nav::Back => self.screen = Screen::Grid,
                _ => {}
            },
        }
    }

    fn handle_grid(&mut self, nav: Nav, _ctx: &egui::Context) {
        let count = self.visible.len();
        let columns = self.columns.max(1);
        match nav {
            Nav::Left => self.selected = self.selected.saturating_sub(1),
            Nav::Right if count > 0 => self.selected = (self.selected + 1).min(count - 1),
            Nav::Up => self.selected = self.selected.saturating_sub(columns.min(self.selected)),
            Nav::Down if count > 0 => {
                if self.selected + columns < count {
                    self.selected += columns;
                } else if self.selected / columns < (count - 1) / columns {
                    // The row below is shorter: land on its last cover.
                    self.selected = count - 1;
                }
            }
            Nav::Accept if count > 0 => self.open_detail(),
            Nav::Search => self.screen = Screen::Keyboard,
            Nav::Action => self.open_downloads(),
            Nav::Previous | Nav::Next => {
                self.tab = if self.tab == Tab::All {
                    Tab::Installed
                } else {
                    Tab::All
                };
                self.refilter();
            }
            Nav::Menu => self.screen = Screen::Menu { button: 1 },
            // B is "back" everywhere; on the grid that is clearing a search. It
            // never closes the store: Start's menu does.
            Nav::Back if !self.filter.is_empty() => {
                self.filter.clear();
                self.refilter();
            }
            _ => {}
        }
    }

    fn detail_buttons(&self) -> Vec<DetailAction> {
        let Some(game) = self.current() else {
            return vec![DetailAction::Back];
        };
        let progress = self.progress.get(&game.id);
        if progress.is_some_and(|p| matches!(p, Progress::Done { pending: true })) {
            return vec![DetailAction::Restart, DetailAction::Art, DetailAction::Back];
        }
        if progress.is_some_and(Progress::active) {
            return vec![DetailAction::Art, DetailAction::Back];
        }
        let builds = install::builds(game);
        let mut actions: Vec<DetailAction> = if self.installed(&game.id) {
            // Installed: offer the other build, when there is one.
            let current = self.known.get(&game.id).and_then(|known| known.platform);
            builds
                .into_iter()
                .filter(|os| Some(*os) != current.or(Some(Os::Windows)))
                .map(DetailAction::Switch)
                .collect()
        } else {
            // A failed build first, so A on the first button retries it.
            let retry = self.retry_build(game);
            let mut builds = builds;
            builds.sort_by_key(|os| Some(*os) != retry);
            builds.into_iter().map(DetailAction::Install).collect()
        };
        actions.push(DetailAction::Art);
        actions.push(DetailAction::Back);
        actions
    }

    /// The badge for a game: what it says and its colour.
    fn badge(&self, id: &ProductId) -> Option<(String, Color32)> {
        let text = self.text;
        if let Some(progress) = self.progress.get(id) {
            return Some(match progress {
                Progress::Queued => (text.queued.to_string(), theme::DIM),
                Progress::Downloading { fraction, .. } => (
                    format!("{} {:.0}%", text.downloading, fraction * 100.0),
                    theme::WORKING,
                ),
                Progress::Installing => (text.installing.to_string(), theme::WORKING),
                Progress::Finishing => (text.finishing.to_string(), theme::WORKING),
                Progress::Done { pending: true } => {
                    (text.restart_needed.to_string(), theme::WAITING)
                }
                Progress::Done { pending: false } => (text.installed.to_string(), theme::INSTALLED),
                Progress::Failed(_) => (text.failed.to_string(), theme::FAILED),
            });
        }
        match self.known.get(id).and_then(|known| known.state)? {
            State::Installed | State::UpdateAvailable => {
                Some((text.installed.to_string(), theme::INSTALLED))
            }
            State::Failed => Some((text.failed.to_string(), theme::FAILED)),
            // A transient state with no live progress here is another process's
            // install, or one interrupted; the registry's reconcile sorts out which.
            State::Queued | State::Downloading | State::Installing => {
                Some((text.installing.to_string(), theme::WORKING))
            }
            _ => None,
        }
    }

    fn failure(&self, id: &ProductId) -> Option<String> {
        match self.progress.get(id) {
            Some(Progress::Failed(reason)) => Some(reason.clone()),
            Some(_) => None,
            None => self
                .known
                .get(id)
                .filter(|known| known.state == Some(State::Failed))
                .and_then(|known| known.failure.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailAction {
    Install(Os),
    /// Install the other build of an installed game; its tile replaces the old one.
    Switch(Os),
    Restart,
    /// Choose the game's artwork.
    Art,
    Back,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        for (id, image) in self.covers.ready(8) {
            let texture =
                image.map(|image| ctx.load_texture(id.to_string(), image, TextureOptions::LINEAR));
            self.textures.insert(id, texture);
        }
        for (id, progress) in self.jobs.updates() {
            let finished = !progress.active();
            self.progress.insert(id, progress);
            if finished {
                self.reload();
            }
        }
        if self.loaded_at.elapsed() > Duration::from_secs(3) {
            self.reload();
        }
        for answer in self.media.answers() {
            match answer {
                Answer::Details(id, details) => {
                    self.details.insert(id, details);
                }
                Answer::Screenshot(id, index, image) => {
                    if self.shots_of.as_ref() == Some(&id) {
                        let texture = image.map(|image| {
                            ctx.load_texture(
                                format!("{id}-shot-{index}"),
                                image,
                                TextureOptions::LINEAR,
                            )
                        });
                        self.shots.insert(index, texture);
                    }
                }
                Answer::DiskUsage(id, bytes) => {
                    self.disk.insert(id, bytes);
                }
                Answer::Storage(storage) => self.storage = Some(storage),
                Answer::Choices(id, kind, found) => {
                    if let Some(picker) = self.picker.as_mut().filter(|p| p.id == id) {
                        picker.choices.insert(kind, found);
                    }
                }
                Answer::Preview(id, kind, index, image) => {
                    if let Some(picker) = self.picker.as_mut().filter(|p| p.id == id) {
                        let texture = image.map(|image| {
                            ctx.load_texture(
                                format!("{id}-art-{kind:?}-{index}"),
                                image,
                                TextureOptions::LINEAR,
                            )
                        });
                        picker.previews.insert((kind, index), texture);
                    }
                }
                Answer::Applied(id, kind, result) => {
                    let url = self
                        .picker
                        .as_mut()
                        .filter(|picker| picker.id == id)
                        .and_then(|picker| picker.applying.take())
                        .map(|(_, url)| url);
                    match result {
                        Ok(cover) => {
                            if let (Some(picker), Some(url)) = (self.picker.as_mut(), url) {
                                picker.picked.insert(kind, url);
                            }
                            if let Some(cover) = cover {
                                let texture =
                                    ctx.load_texture(id.to_string(), cover, TextureOptions::LINEAR);
                                self.textures.insert(id.clone(), Some(texture));
                            }
                            let has_tile = self.known.get(&id).is_some_and(|k| k.appid.is_some());
                            self.notify(if has_tile {
                                self.text.art_saved
                            } else {
                                self.text.art_saved_store
                            });
                        }
                        Err(error) => {
                            self.notify(format!("{}: {error}", self.text.art_failed));
                        }
                    }
                }
            }
        }
        if self.screen == Screen::Art {
            self.want_previews();
        }
        // A cover looked at for a moment is likely to be opened: fetch its
        // details now, so the detail view has them when it opens.
        if self.screen == Screen::Grid
            && let Some(id) = self.current().map(|game| game.id.clone())
        {
            match &self.dwell {
                Some((dwelling, since)) if *dwelling == id => {
                    if since.elapsed() > Duration::from_millis(400) && self.asked.insert(id.clone())
                    {
                        self.media.details(id);
                    }
                }
                _ => self.dwell = Some((id, Instant::now())),
            }
        }
        if matches!(self.screen, Screen::Detail { .. }) {
            self.want_screenshots();
        }
        self.shot_scroll += (self.shot as f32 - self.shot_scroll) * 0.3;
        if (self.shot as f32 - self.shot_scroll).abs() < 0.01 {
            self.shot_scroll = self.shot as f32;
        }

        // A physical keyboard types straight into the search, from any screen
        // where that makes sense.
        if matches!(self.screen, Screen::Grid | Screen::Keyboard) {
            let typed: String = ctx.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            });
            if !typed.is_empty() {
                self.filter.push_str(&typed);
                self.refilter();
            }
        }

        let navs = self.input.poll(&ctx);
        if self.video_playing() {
            // mpv has the screen; only B means something here: stop it.
            if navs.contains(&Nav::Back) {
                self.stop_video();
            }
        } else {
            for nav in navs {
                self.handle(nav, &ctx);
            }
        }

        let screen = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(screen, 0.0, theme::BACKGROUND);
        self.draw_grid(&painter, screen);
        match self.screen {
            Screen::Keyboard => {
                let text = self.text;
                self.keyboard
                    .show(&painter, screen, [text.space, text.delete, text.done]);
            }
            Screen::Detail { button } => self.draw_detail(&painter, screen, button),
            Screen::ConfirmRestart { button } => {
                let text = self.text;
                draw_confirm(
                    &painter,
                    screen,
                    text.restart_title,
                    text.restart_body,
                    [text.restart_confirm, text.cancel],
                    button,
                )
            }
            Screen::ConfirmQuit { button } => {
                let text = self.text;
                draw_confirm(
                    &painter,
                    screen,
                    text.quit_title,
                    text.quit_body,
                    [text.quit, text.cancel],
                    button,
                )
            }
            Screen::Downloads => self.draw_downloads(&painter, screen),
            Screen::Art => self.draw_art(&painter, screen),
            Screen::Menu { button } => {
                let text = self.text;
                draw_menu(
                    &painter,
                    screen,
                    text.menu,
                    [text.restart_steam, text.quit_store, text.cancel],
                    button,
                );
            }
            Screen::Grid => {}
        }
        self.draw_footer(&painter, screen);
        if let Some((message, at)) = &self.notice {
            if at.elapsed() < Duration::from_secs(8) {
                draw_notice(&painter, screen, message);
            } else {
                self.notice = None;
            }
        }

        // gilrs does not wake the UI, so it is polled: quickly while something
        // moves, at a still responsive pace when nothing does.
        let busy = self.input.busy()
            || (self.shot as f32 - self.shot_scroll).abs() > 0.01
            || self.any_active()
            || (self.scroll - self.scroll_target(screen)).abs() > 0.5;
        ctx.request_repaint_after(Duration::from_millis(if busy { 16 } else { 33 }));
    }
}

/// Layout of the grid inside the screen.
struct Layout {
    area: Rect,
    cover: Vec2,
    cell: Vec2,
    gap: f32,
    columns: usize,
}

impl App {
    fn layout(&self, screen: Rect) -> Layout {
        let margin = (screen.width() * 0.035).max(24.0);
        // Room above the first row for the focused cover, which is drawn larger.
        let top = screen.min.y + header_height(screen) + (screen.width() * 0.018).max(14.0);
        let bottom = screen.max.y - footer_height(screen);
        let area = Rect::from_min_max(
            pos2(screen.min.x + margin, top),
            pos2(screen.max.x - margin, bottom),
        );
        let gap = (screen.width() * 0.018).max(14.0);
        let target = (screen.width() / 8.5).max(150.0);
        let columns = (((area.width() + gap) / (target + gap)).floor() as usize).max(3);
        let width = (area.width() - gap * (columns as f32 - 1.0)) / columns as f32;
        let cover = vec2(width, width * covers::HEIGHT as f32 / covers::WIDTH as f32);
        let title = screen.height() * 0.06;
        Layout {
            area,
            cover,
            cell: vec2(width + gap, cover.y + title + gap),
            gap,
            columns,
        }
    }

    fn pending_count(&self) -> usize {
        let fresh = self
            .progress
            .values()
            .filter(|p| matches!(p, Progress::Done { pending: true }))
            .count();
        self.pending.max(fresh)
    }

    fn scroll_target(&self, screen: Rect) -> f32 {
        let layout = self.layout(screen);
        let row = (self.selected / layout.columns.max(1)) as f32;
        let top = row * layout.cell.y;
        let bottom = top + layout.cell.y - layout.gap;
        let height = layout.area.height();
        let mut target = self.scroll;
        // Keep a little of the next row in view, so there is visibly more.
        let lead = layout.cell.y * 0.35;
        if top - lead < target {
            target = (top - lead).max(0.0);
        }
        if bottom + lead > target + height {
            target = bottom + lead - height;
        }
        target.max(0.0)
    }

    fn draw_grid(&mut self, painter: &egui::Painter, screen: Rect) {
        let text = self.text;
        let layout = self.layout(screen);
        self.columns = layout.columns;

        // Ease towards the focused row.
        let target = self.scroll_target(screen);
        self.scroll += (target - self.scroll) * 0.3;
        if (target - self.scroll).abs() < 0.5 {
            self.scroll = target;
        }

        let header = Rect::from_min_size(screen.min, vec2(screen.width(), header_height(screen)));
        let big = FontId::proportional(screen.height() * 0.045);
        let normal = FontId::proportional(screen.height() * 0.028);
        let small = FontId::proportional(screen.height() * 0.022);
        painter.text(
            pos2(layout.area.min.x, header.center().y),
            Align2::LEFT_CENTER,
            "GOG",
            big.clone(),
            theme::ACCENT,
        );

        // The search box: what is typed, or how to start typing.
        let search = Rect::from_center_size(
            header.center(),
            vec2(screen.width() * 0.36, header.height() * 0.56),
        );
        painter.rect_filled(search, 12.0, theme::PANEL);
        if self.screen == Screen::Keyboard {
            painter.rect_stroke(
                search,
                12.0,
                Stroke::new(3.0, theme::ACCENT),
                StrokeKind::Outside,
            );
        }
        let (shown, colour) = if self.filter.is_empty() {
            (format!("Y  {}", text.search_hint), theme::DIM)
        } else {
            (format!("{}▏", self.filter), theme::TEXT)
        };
        painter.text(
            pos2(search.min.x + 20.0, search.center().y),
            Align2::LEFT_CENTER,
            shown,
            normal.clone(),
            colour,
        );

        // Tabs on the right, switched with the shoulder buttons.
        let installed_count = self
            .games
            .iter()
            .filter(|game| self.installed(&game.id))
            .count();
        let labels = [
            (format!("{} ({})", text.all, self.games.len()), Tab::All),
            (
                format!("{} ({installed_count})", text.installed),
                Tab::Installed,
            ),
        ];
        let mut x = layout.area.max.x;
        for (label, tab) in labels.iter().rev() {
            let galley = painter.layout_no_wrap(label.clone(), normal.clone(), theme::TEXT);
            let rect = Rect::from_min_size(
                pos2(
                    x - galley.size().x - 28.0,
                    header.center().y - galley.size().y / 2.0 - 8.0,
                ),
                galley.size() + vec2(28.0, 16.0),
            );
            if *tab == self.tab {
                painter.rect_filled(rect, 20.0, theme::ACCENT);
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                normal.clone(),
                if *tab == self.tab {
                    Color32::BLACK
                } else {
                    theme::DIM
                },
            );
            x = rect.min.x - 14.0;
        }

        // The covers.
        // Clipped exactly at the header and the footer, never over them.
        let clip = painter.with_clip_rect(Rect::from_min_max(
            pos2(screen.min.x, screen.min.y + header_height(screen)),
            pos2(screen.max.x, screen.max.y - footer_height(screen)),
        ));
        if self.visible.is_empty() {
            painter.text(
                layout.area.center(),
                Align2::CENTER_CENTER,
                text.no_results,
                big,
                theme::DIM,
            );
        }
        let first_row = (self.scroll / layout.cell.y).floor().max(0.0) as usize;
        let rows_shown = (layout.area.height() / layout.cell.y).ceil() as usize + 2;
        let time = painter.ctx().input(|input| input.time) as f32;
        for row in first_row..first_row + rows_shown {
            for column in 0..layout.columns {
                let slot = row * layout.columns + column;
                let Some(&index) = self.visible.get(slot) else {
                    break;
                };
                let game = &self.games[index];
                let origin = layout.area.min
                    + vec2(
                        column as f32 * layout.cell.x,
                        row as f32 * layout.cell.y - self.scroll,
                    );
                let focused = slot == self.selected && self.screen != Screen::Keyboard;
                let mut rect = Rect::from_min_size(origin, layout.cover);
                if focused {
                    rect = rect.expand(layout.cover.x * 0.04);
                }
                self.draw_cover(&clip, rect, game, focused, time, &small);

                // The title under the cover, two lines at most.
                let mut job = LayoutJob::simple(
                    game.title.clone(),
                    small.clone(),
                    if focused { theme::TEXT } else { theme::DIM },
                    layout.cover.x,
                );
                job.wrap.max_rows = 2;
                job.wrap.overflow_character = Some('…');
                let galley = clip.layout_job(job);
                clip.galley(
                    pos2(origin.x, origin.y + layout.cover.y + layout.gap * 0.6),
                    galley,
                    theme::TEXT,
                );
            }
        }
    }

    fn draw_cover(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        game: &Game,
        focused: bool,
        time: f32,
        font: &FontId,
    ) {
        let radius = rect.width() * 0.04;
        match self.textures.get(&game.id) {
            Some(Some(texture)) => {
                painter.image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {
                // No cover (yet): the title, centred, so the slot still says what it is.
                painter.rect_filled(rect, radius, theme::PLACEHOLDER);
                let mut job = LayoutJob::simple(
                    game.title.clone(),
                    font.clone(),
                    theme::TEXT,
                    rect.width() * 0.85,
                );
                job.halign = egui::Align::Center;
                job.wrap.max_rows = 4;
                job.wrap.overflow_character = Some('…');
                let galley = painter.layout_job(job);
                painter.galley(
                    pos2(rect.center().x, rect.center().y - galley.size().y / 2.0),
                    galley,
                    theme::TEXT,
                );
            }
        }

        if let Some((label, colour)) = self.badge(&game.id) {
            let galley = painter.layout_no_wrap(label, font.clone(), Color32::BLACK);
            let badge = Rect::from_min_size(
                rect.min + vec2(10.0, 10.0),
                galley.size() + vec2(18.0, 10.0),
            );
            painter.rect_filled(badge, 8.0, colour);
            painter.galley(badge.min + vec2(9.0, 5.0), galley, Color32::BLACK);
        }

        // Progress along the bottom edge, on a solid track so it reads over any art.
        if let Some(progress) = self.progress.get(&game.id) {
            let height = (rect.height() * 0.035).max(10.0);
            let track = Rect::from_min_max(pos2(rect.min.x, rect.max.y - height), rect.max);
            match progress {
                Progress::Downloading { fraction, .. } => {
                    painter.rect_filled(track, 0.0, theme::TRACK);
                    let filled = Rect::from_min_max(
                        track.min,
                        pos2(
                            track.min.x + track.width() * fraction.clamp(0.0, 1.0),
                            track.max.y,
                        ),
                    );
                    painter.rect_filled(filled, 0.0, theme::WORKING);
                }
                Progress::Installing | Progress::Finishing | Progress::Queued => {
                    // No percentage to show, so none is invented: a moving segment.
                    painter.rect_filled(track, 0.0, theme::TRACK);
                    if *progress != Progress::Queued {
                        let span = track.width() * 0.3;
                        let phase = (time * 0.6).fract();
                        let start = track.min.x - span + phase * (track.width() + span);
                        let segment = Rect::from_min_max(
                            pos2(start.max(track.min.x), track.min.y),
                            pos2((start + span).min(track.max.x), track.max.y),
                        );
                        painter.rect_filled(segment, 0.0, theme::WORKING);
                    }
                }
                _ => {}
            }
        }

        if focused {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(rect.width() * 0.025, theme::ACCENT),
                StrokeKind::Outside,
            );
        }
    }

    /// What is downloading or installing, what finished this session, and how
    /// the disk is used.
    fn draw_downloads(&self, painter: &egui::Painter, screen: Rect) {
        let text = self.text;
        painter.rect_filled(screen, 0.0, Color32::from_black_alpha(215));
        let panel = Rect::from_center_size(
            screen.center(),
            vec2(screen.width() * 0.66, screen.height() * 0.8),
        );
        painter.rect_filled(panel, 20.0, theme::PANEL);
        let inner = panel.shrink(screen.height() * 0.035);
        let title = FontId::proportional(screen.height() * 0.042);
        let font = FontId::proportional(screen.height() * 0.028);
        let small = FontId::proportional(screen.height() * 0.021);

        // Storage first: whether there is room is the question before installing.
        painter.text(
            inner.min,
            Align2::LEFT_TOP,
            text.storage,
            title.clone(),
            theme::TEXT,
        );
        let bar = Rect::from_min_size(
            inner.min + vec2(0.0, screen.height() * 0.065),
            vec2(inner.width(), screen.height() * 0.032),
        );
        match &self.storage {
            Some(Some(storage)) => draw_storage(painter, bar, storage, text, &small),
            Some(None) => {}
            None => {
                painter.rect_filled(bar, 8.0, theme::TRACK);
                painter.text(
                    bar.center(),
                    Align2::CENTER_CENTER,
                    text.measuring,
                    small.clone(),
                    theme::DIM,
                );
            }
        }

        let list_top = bar.max.y + screen.height() * 0.085;
        painter.text(
            pos2(inner.min.x, list_top),
            Align2::LEFT_TOP,
            text.downloads,
            title,
            theme::TEXT,
        );

        let rows = self.download_rows();
        let top = list_top + screen.height() * 0.06;
        if rows.is_empty() {
            painter.text(
                pos2(inner.center().x, (top + inner.max.y) / 2.0),
                Align2::CENTER_CENTER,
                text.downloads_empty,
                font,
                theme::DIM,
            );
            return;
        }
        let row_height = screen.height() * 0.105;
        let time = painter.ctx().input(|input| input.time) as f32;
        // Scrolled so the focused row is always in view.
        let fits = (((inner.max.y - top) / row_height).floor() as usize).max(1);
        let first = (self.downloads_focus + 1).saturating_sub(fits);
        for (index, (game, progress)) in rows.iter().enumerate().skip(first).take(fits) {
            let y = top + (index - first) as f32 * row_height;
            let row =
                Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), row_height * 0.88));
            painter.rect_filled(row, 12.0, theme::BACKGROUND);
            let focused = index == self.downloads_focus;
            if focused {
                painter.rect_stroke(
                    row,
                    12.0,
                    Stroke::new(3.0, theme::ACCENT),
                    StrokeKind::Outside,
                );
            }

            // The cover, small, so the row is recognised before it is read.
            let cover_height = row.height() - 16.0;
            let cover = Rect::from_min_size(
                row.min + vec2(8.0, 8.0),
                vec2(
                    cover_height * covers::WIDTH as f32 / covers::HEIGHT as f32,
                    cover_height,
                ),
            );
            match self.textures.get(&game.id) {
                Some(Some(texture)) => {
                    painter.image(
                        texture.id(),
                        cover,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                _ => {
                    painter.rect_filled(cover, 6.0, theme::PLACEHOLDER);
                }
            }

            let left = cover.max.x + 18.0;
            let mut name_end = painter
                .text(
                    pos2(left, row.min.y + 12.0),
                    Align2::LEFT_TOP,
                    &game.title,
                    font.clone(),
                    theme::TEXT,
                )
                .max
                .x;
            if let Some(os) = self.building.get(&game.id) {
                let size = font.size;
                let icon =
                    Rect::from_min_size(pos2(name_end + 14.0, row.min.y + 14.0), vec2(size, size));
                icons::os(painter, icon, *os);
                name_end = icon.max.x;
            }
            let _ = name_end;
            let (label, colour) = self.badge(&game.id).unwrap_or_default();
            painter.text(
                pos2(row.max.x - 20.0, row.min.y + 14.0),
                Align2::RIGHT_TOP,
                label,
                small.clone(),
                colour,
            );
            if let Progress::Failed(reason) = progress {
                // On the focused failure, how to try again, at the row's right.
                let mut reason_right = row.max.x - 20.0;
                if focused {
                    let style = self.input.style();
                    let at = pos2(row.max.x - 20.0, row.max.y - 24.0);
                    let label =
                        painter.layout_no_wrap(text.retry.to_string(), small.clone(), theme::TEXT);
                    let label_left = at.x - label.size().x;
                    painter.galley(
                        pos2(label_left, at.y - label.size().y / 2.0),
                        label,
                        theme::TEXT,
                    );
                    let chip_width = small.size + 8.0;
                    let chip = icons::button(
                        painter,
                        pos2(label_left - 10.0 - chip_width, at.y),
                        "A",
                        style,
                        &small,
                    );
                    reason_right = chip.min.x - 20.0;
                }
                let mut job = LayoutJob::simple(
                    reason.clone(),
                    small.clone(),
                    theme::FAILED,
                    reason_right - left,
                );
                job.wrap.max_rows = 1;
                job.wrap.overflow_character = Some('…');
                painter.galley(
                    pos2(left, row.max.y - 36.0),
                    painter.layout_job(job),
                    theme::FAILED,
                );
            }
            let bar = Rect::from_min_max(
                pos2(left, row.max.y - 20.0),
                pos2(row.max.x - 20.0, row.max.y - 10.0),
            );
            match progress {
                Progress::Downloading { fraction, .. } => {
                    painter.rect_filled(bar, 4.0, theme::TRACK);
                    painter.rect_filled(
                        Rect::from_min_max(
                            bar.min,
                            pos2(
                                bar.min.x + bar.width() * fraction.clamp(0.0, 1.0),
                                bar.max.y,
                            ),
                        ),
                        4.0,
                        theme::WORKING,
                    );
                }
                Progress::Installing | Progress::Finishing => {
                    painter.rect_filled(bar, 4.0, theme::TRACK);
                    let span = bar.width() * 0.3;
                    let start = bar.min.x - span + (time * 0.6).fract() * (bar.width() + span);
                    painter.rect_filled(
                        Rect::from_min_max(
                            pos2(start.max(bar.min.x), bar.min.y),
                            pos2((start + span).min(bar.max.x), bar.max.y),
                        ),
                        4.0,
                        theme::WORKING,
                    );
                }
                _ => {}
            }
        }
    }

    fn draw_detail(&self, painter: &egui::Painter, screen: Rect, button: usize) {
        let Some(game) = self.current() else { return };
        let text = self.text;
        // Everything but the footer, which stays the same on every screen.
        let body = Rect::from_min_max(
            screen.min,
            pos2(screen.max.x, screen.max.y - footer_height(screen)),
        );
        painter.rect_filled(body, 0.0, theme::BACKGROUND);
        let details = self.details.get(&game.id);

        let margin = screen.width() * 0.045;
        let gap = margin * 0.6;
        let top = screen.min.y + screen.height() * 0.08;

        // Right: the screenshots, a column scrolled with up and down. Everything
        // else stays left of it, clipped there, so nothing ever runs under it.
        let shot_width = screen.width() * 0.40;
        let shot = Rect::from_center_size(
            pos2(screen.max.x - margin - shot_width / 2.0, body.center().y),
            vec2(shot_width, shot_width * 9.0 / 16.0),
        );
        let column_left = shot.min.x - gap;

        // Left: the cover. Middle: what the game is.
        let cover_height = screen.height() * 0.5;
        let cover = Rect::from_min_size(
            pos2(screen.min.x + margin, top),
            vec2(
                cover_height * covers::WIDTH as f32 / covers::HEIGHT as f32,
                cover_height,
            ),
        );
        self.draw_cover(
            painter,
            cover,
            game,
            false,
            0.0,
            &FontId::proportional(screen.height() * 0.025),
        );

        let left = cover.max.x + gap;
        let width = (column_left - left).max(1.0);
        let info = painter.with_clip_rect(Rect::from_min_max(
            pos2(left, body.min.y),
            pos2(column_left, cover.max.y),
        ));
        let mut y = top;
        let write = |message: String, size: f32, colour: Color32, rows: usize, y: &mut f32| {
            let mut job = LayoutJob::simple(
                message,
                FontId::proportional(screen.height() * size),
                colour,
                width,
            );
            job.wrap.max_rows = rows;
            job.wrap.overflow_character = Some('…');
            let galley = info.layout_job(job);
            let height = galley.size().y;
            info.galley(pos2(left, *y), galley, colour);
            *y += height + screen.height() * 0.016;
        };

        write(game.title.clone(), 0.046, theme::TEXT, 3, &mut y);
        let known = self.known.get(&game.id).cloned().unwrap_or_default();
        if let Some((label, colour)) = self.badge(&game.id) {
            write(label, 0.03, colour, 1, &mut y);
        }

        // Each build GOG has, with its icon and sizes, wrapped to the space there is.
        let line = screen.height() * 0.03;
        for os in install::builds(game) {
            let icon = Rect::from_min_size(pos2(left, y), vec2(line, line));
            icons::os(&info, icon, os);
            let installed_here =
                self.installed(&game.id) && known.platform.unwrap_or(Os::Windows) == os;
            let name = match os {
                Os::Linux => text.linux_build,
                _ => text.windows_build,
            };
            let mut description = name.to_string();
            if let Some(bytes) = details.and_then(|details| details.size(os)) {
                description.push_str(&format!("\n{} {}", text.download_size, human_size(bytes)));
            }
            if installed_here && let Some(bytes) = self.disk.get(&game.id) {
                description.push_str(&format!("  ·  {} {}", text.on_disk, human_size(*bytes)));
            }
            let colour = if installed_here {
                theme::TEXT
            } else {
                theme::DIM
            };
            let text_left = icon.max.x + 14.0;
            let mut job = LayoutJob::simple(
                description,
                FontId::proportional(screen.height() * 0.023),
                colour,
                (column_left - text_left).max(1.0),
            );
            job.wrap.max_rows = 3;
            let galley = info.layout_job(job);
            let height = galley.size().y.max(line);
            info.galley(pos2(text_left, y), galley, colour);
            y += height + screen.height() * 0.016;
        }

        if let Some(Progress::Done { pending: true }) = self.progress.get(&game.id) {
            write(
                text.in_steam_after_restart.to_string(),
                0.024,
                theme::TEXT,
                2,
                &mut y,
            );
        } else if self.installed(&game.id) {
            write(
                text.play_in_steam.to_string(),
                0.024,
                theme::TEXT,
                2,
                &mut y,
            );
            if let Some(build) = &known.build {
                write(
                    format!("{}: {build}", text.version),
                    0.021,
                    theme::DIM,
                    1,
                    &mut y,
                );
            }
        }
        if let Some(reason) = self.failure(&game.id) {
            write(reason, 0.021, theme::FAILED, 3, &mut y);
        }

        // The actions, under the cover, wrapping onto a second row rather than
        // running under the screenshots.
        let font = FontId::proportional(screen.height() * 0.03);
        let retry = self.retry_build(game);
        let mut x = cover.min.x;
        let mut row_y = cover.max.y + screen.height() * 0.045;
        let row_gap = 20.0;
        for (index, action) in self.detail_buttons().iter().enumerate() {
            let (label, icon) = match action {
                DetailAction::Install(os) if retry == Some(*os) => (text.retry, Some(*os)),
                DetailAction::Install(os) => (text.install, Some(*os)),
                DetailAction::Switch(os) => (
                    match os {
                        Os::Linux => text.switch_linux,
                        _ => text.switch_windows,
                    },
                    Some(*os),
                ),
                DetailAction::Restart => (text.restart_steam, None),
                DetailAction::Art => (text.artwork, None),
                DetailAction::Back => (text.back, None),
            };
            let size = button_size(painter, label, icon, &font);
            if x > cover.min.x && x + size.x > column_left {
                x = cover.min.x;
                row_y += size.y + row_gap;
            }
            x = draw_button(painter, pos2(x, row_y), label, icon, &font, index == button)
                .max
                .x
                + 24.0;
        }

        // Screenshots: the one in focus in the middle, its neighbours above and
        // below, dimmed and cut by the screen's edges, so it reads as a column.
        let count = details.map_or(0, |details| details.screenshots.len());
        let column = Rect::from_min_max(
            pos2(shot.min.x - 8.0, body.min.y),
            pos2(shot.max.x + 8.0, body.max.y),
        );
        let clip = painter.with_clip_rect(column);
        let step = shot.height() + screen.height() * 0.03;
        let first = (self.shot_scroll.floor() as i64 - 2).max(0) as usize;
        for index in first..count.min(first + 5) {
            let offset = (index as f32 - self.shot_scroll) * step;
            let rect = shot.translate(vec2(0.0, offset));
            let focused = index == self.shot;
            let tint = if focused {
                Color32::WHITE
            } else {
                Color32::from_gray(90)
            };
            match self.shots.get(&index) {
                Some(Some(texture)) => {
                    clip.image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        tint,
                    );
                }
                _ => {
                    clip.rect_filled(rect, 14.0, theme::TRACK);
                }
            }
            if focused && count > 1 {
                clip.text(
                    rect.right_bottom() + vec2(-16.0, -14.0),
                    Align2::RIGHT_BOTTOM,
                    format!("{}/{count}", index + 1),
                    FontId::proportional(screen.height() * 0.024),
                    Color32::WHITE,
                );
            }
        }
        if count == 0 {
            painter.rect_filled(shot, 14.0, theme::TRACK);
            if details.is_none() {
                painter.text(
                    shot.center(),
                    Align2::CENTER_CENTER,
                    text.measuring,
                    FontId::proportional(screen.height() * 0.024),
                    theme::DIM,
                );
            }
        }
    }

    /// The artwork picker: the kinds as tabs, the images of one kind in a row,
    /// the focused one in the middle.
    fn draw_art(&self, painter: &egui::Painter, screen: Rect) {
        let Some(picker) = &self.picker else { return };
        let Some(game) = self.games.iter().find(|game| game.id == picker.id) else {
            return;
        };
        let text = self.text;
        let body = Rect::from_min_max(
            screen.min,
            pos2(screen.max.x, screen.max.y - footer_height(screen)),
        );
        painter.rect_filled(body, 0.0, theme::BACKGROUND);
        let margin = (screen.width() * 0.045).max(24.0);
        let title = FontId::proportional(screen.height() * 0.042);
        let normal = FontId::proportional(screen.height() * 0.028);
        let small = FontId::proportional(screen.height() * 0.022);

        let mut job = LayoutJob::simple(
            format!("{} · {}", text.artwork, game.title),
            title,
            theme::TEXT,
            body.width() - margin * 2.0,
        );
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        let heading = painter.layout_job(job);
        let heading_height = heading.size().y;
        painter.galley(
            pos2(body.min.x + margin, body.min.y + margin * 0.6),
            heading,
            theme::TEXT,
        );

        // The kinds, as tabs.
        let tabs_y = body.min.y + margin * 0.6 + heading_height + screen.height() * 0.035;
        let mut x = body.min.x + margin;
        for (index, kind) in Kind::ALL.iter().enumerate() {
            let label = match kind {
                Kind::Portrait => text.art_portrait,
                Kind::Wide => text.art_wide,
                Kind::Hero => text.art_hero,
                Kind::Logo => text.art_logo,
            };
            let galley = painter.layout_no_wrap(label.to_string(), normal.clone(), theme::TEXT);
            let rect = Rect::from_min_size(pos2(x, tabs_y), galley.size() + vec2(32.0, 16.0));
            let current = index == picker.kind;
            if current {
                painter.rect_filled(rect, 20.0, theme::ACCENT);
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                normal.clone(),
                if current { Color32::BLACK } else { theme::DIM },
            );
            x = rect.max.x + 14.0;
        }

        // The images, each in a box of its kind's shape.
        let kind = picker.kind();
        let aspect = match kind {
            Kind::Portrait => 2.0 / 3.0,
            Kind::Wide => 920.0 / 430.0,
            Kind::Hero => 1920.0 / 620.0,
            Kind::Logo => 16.0 / 9.0,
        };
        let row_top = tabs_y + screen.height() * 0.1;
        let row_bottom = body.max.y - screen.height() * 0.08;
        let mut height = (row_bottom - row_top).min(screen.height() * 0.52);
        let mut box_width = height * aspect;
        if box_width > screen.width() * 0.42 {
            box_width = screen.width() * 0.42;
            height = box_width / aspect;
        }
        let center = pos2(body.center().x, (row_top + row_bottom) / 2.0);
        let status = |message: &str, colour: Color32| {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                message,
                normal.clone(),
                colour,
            );
        };
        let found = match picker.choices.get(&kind) {
            None => return status(text.art_loading, theme::DIM),
            Some(Err(error)) if error.is_empty() => return status(text.art_needs_key, theme::DIM),
            Some(Err(error)) => return status(error, theme::FAILED),
            Some(Ok(found)) if found.is_empty() => return status(text.art_none, theme::DIM),
            Some(Ok(found)) => found,
        };
        let focused = picker.focused();
        let step = box_width + screen.width() * 0.025;
        let row = painter.with_clip_rect(Rect::from_min_max(
            pos2(body.min.x, row_top - 20.0),
            pos2(body.max.x, row_bottom + 20.0),
        ));
        let first = focused.saturating_sub(3);
        for (index, choice) in found.iter().enumerate().skip(first).take(7) {
            let slot = Rect::from_center_size(
                center + vec2((index as f32 - focused as f32) * step, 0.0),
                vec2(box_width, height),
            );
            let is_focused = index == focused;
            row.rect_filled(slot, 12.0, theme::PANEL);
            if let Some(Some(texture)) = picker.previews.get(&(kind, index)) {
                // Fitted inside the box, keeping the image's own shape.
                let [w, h] = texture.size();
                let scale = (slot.width() / w as f32).min(slot.height() / h as f32);
                let image = Rect::from_center_size(slot.center(), vec2(w as f32, h as f32) * scale);
                row.image(
                    texture.id(),
                    image,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    if is_focused {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(110)
                    },
                );
            }
            if picker.picked.get(&kind) == Some(&choice.url) {
                let galley =
                    row.layout_no_wrap(text.art_current.to_string(), small.clone(), Color32::BLACK);
                let badge = Rect::from_min_size(
                    slot.min + vec2(10.0, 10.0),
                    galley.size() + vec2(18.0, 10.0),
                );
                row.rect_filled(badge, 8.0, theme::INSTALLED);
                row.galley(badge.min + vec2(9.0, 5.0), galley, Color32::BLACK);
            }
            if is_focused {
                let busy = picker
                    .applying
                    .as_ref()
                    .is_some_and(|(applying, url)| *applying == kind && *url == choice.url);
                row.rect_stroke(
                    slot,
                    12.0,
                    Stroke::new(4.0, if busy { theme::WORKING } else { theme::ACCENT }),
                    StrokeKind::Outside,
                );
            }
        }
        painter.text(
            pos2(center.x, row_bottom + screen.height() * 0.035),
            Align2::CENTER_CENTER,
            format!("{}/{}", focused + 1, found.len()),
            small,
            theme::DIM,
        );
    }

    /// The footer: the same bar on every screen, saying what the buttons do there.
    fn draw_footer(&self, painter: &egui::Painter, screen: Rect) {
        let text = self.text;
        let footer = Rect::from_min_max(
            pos2(screen.min.x, screen.max.y - footer_height(screen)),
            screen.max,
        );
        painter.rect_filled(footer, 0.0, theme::PANEL);
        let small = FontId::proportional(screen.height() * 0.022);
        let margin = (screen.width() * 0.035).max(24.0);

        let details = self.current_details();
        let hints: Vec<(&str, &str)> = if self.video_playing() {
            vec![("B", text.stop_video)]
        } else {
            match self.screen {
                Screen::Grid => {
                    let mut hints = vec![
                        ("A", text.select),
                        ("Y", text.search),
                        ("X", text.downloads),
                        ("LB/RB", text.tabs),
                        ("Start", text.menu),
                    ];
                    if !self.filter.is_empty() {
                        hints.push(("B", text.back));
                    }
                    hints
                }
                Screen::Detail { .. } => {
                    let mut hints = vec![("◀▶", text.options), ("A", text.accept)];
                    if details.is_some_and(|d| d.screenshots.len() > 1) {
                        hints.insert(1, ("▲▼", text.screenshots));
                    }
                    if details.is_some_and(|d| !d.videos.is_empty()) {
                        hints.push(("Y", text.video));
                    }
                    hints.push(("B", text.back));
                    hints
                }
                Screen::Keyboard => vec![
                    ("A", text.type_key),
                    ("X", text.delete),
                    ("Y", text.space),
                    ("B", text.done),
                ],
                Screen::Downloads => {
                    let mut hints = Vec::new();
                    if self.download_rows().len() > 1 {
                        hints.push(("▲▼", text.downloads));
                    }
                    if self.focused_failure().is_some() {
                        hints.push(("A", text.retry));
                    }
                    hints.push(("B", text.back));
                    hints
                }
                Screen::Art => vec![
                    ("◀▶", text.artwork),
                    ("LB/RB", text.art_kinds),
                    ("A", text.art_pick),
                    ("B", text.back),
                ],
                Screen::Menu { .. }
                | Screen::ConfirmRestart { .. }
                | Screen::ConfirmQuit { .. } => {
                    vec![("A", text.accept), ("B", text.cancel)]
                }
            }
        };
        let style = self.input.style();
        let mut x = footer.min.x + margin;
        for (button, label) in hints {
            let chip = icons::button(painter, pos2(x, footer.center().y), button, style, &small);
            let end = painter.text(
                pos2(chip.max.x + 10.0, footer.center().y),
                Align2::LEFT_CENTER,
                label,
                small.clone(),
                theme::DIM,
            );
            x = end.max.x + 36.0;
        }

        // On the right: games waiting for Steam to restart, or the count.
        let right = footer.max.x - margin;
        if self.pending_count() > 0 {
            let label = format!(
                "{} {} · Start: {}",
                self.pending_count(),
                text.ready,
                text.restart_steam
            );
            let galley = painter.layout_no_wrap(label, small.clone(), Color32::BLACK);
            let rect = Rect::from_min_size(
                pos2(
                    right - galley.size().x - 32.0,
                    footer.center().y - galley.size().y / 2.0 - 8.0,
                ),
                galley.size() + vec2(32.0, 16.0),
            );
            painter.rect_filled(rect, 20.0, theme::WAITING);
            painter.galley(rect.min + vec2(16.0, 8.0), galley, Color32::BLACK);
        } else if self.screen == Screen::Grid {
            painter.text(
                pos2(right, footer.center().y),
                Align2::RIGHT_CENTER,
                format!("{} {}", self.visible.len(), text.results),
                small,
                theme::DIM,
            );
        }
    }
}

/// The space [`draw_button`] takes for `label`.
fn button_size(painter: &egui::Painter, label: &str, icon: Option<Os>, font: &FontId) -> Vec2 {
    let galley = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::BLACK);
    let icon_room = if icon.is_some() {
        galley.size().y + 14.0
    } else {
        0.0
    };
    galley.size() + vec2(64.0 + icon_room, 32.0)
}

fn draw_button(
    painter: &egui::Painter,
    at: Pos2,
    label: &str,
    icon: Option<Os>,
    font: &FontId,
    focused: bool,
) -> Rect {
    let galley = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::BLACK);
    let icon_size = galley.size().y;
    let icon_room = if icon.is_some() {
        icon_size + 14.0
    } else {
        0.0
    };
    let rect = Rect::from_min_size(at, button_size(painter, label, icon, font));
    painter.rect_filled(rect, 14.0, if focused { theme::ACCENT } else { theme::KEY });
    if focused {
        painter.rect_stroke(
            rect,
            14.0,
            Stroke::new(3.0, Color32::WHITE),
            StrokeKind::Outside,
        );
    }
    let content = rect.min + vec2(32.0, 16.0);
    if let Some(os) = icon {
        icons::os(
            painter,
            Rect::from_min_size(content, vec2(icon_size, icon_size)),
            os,
        );
    }
    painter.galley(
        content + vec2(icon_room, 0.0),
        galley,
        if focused { Color32::BLACK } else { theme::TEXT },
    );
    rect
}

/// The disk as one bar: system and other, Steam, GOG, free, each labelled.
fn draw_storage(painter: &egui::Painter, bar: Rect, storage: &Storage, text: &Text, font: &FontId) {
    painter.rect_filled(bar, 8.0, theme::TRACK);
    let total = storage.total.max(1) as f32;
    let parts = [
        (text.storage_other, storage.other(), theme::DIM),
        ("Steam", storage.steam, Color32::from_rgb(70, 140, 230)),
        ("GOG", storage.gog, theme::ACCENT),
    ];
    let mut x = bar.min.x;
    for (_, bytes, colour) in parts {
        let width = bar.width() * bytes as f32 / total;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(x, bar.min.y),
                pos2((x + width).min(bar.max.x), bar.max.y),
            ),
            0.0,
            colour,
        );
        x += width;
    }
    let mut x = bar.min.x;
    let y = bar.max.y + 22.0;
    for (label, bytes, colour) in
        parts
            .into_iter()
            .chain([(text.storage_free, storage.free, theme::TRACK)])
    {
        painter.rect_filled(
            Rect::from_min_size(pos2(x, y - 8.0), vec2(16.0, 16.0)),
            4.0,
            colour,
        );
        let end = painter.text(
            pos2(x + 24.0, y),
            Align2::LEFT_CENTER,
            format!("{label} {}", human_size(bytes)),
            font.clone(),
            theme::TEXT,
        );
        x = end.max.x + 30.0;
    }
}

fn draw_menu(painter: &egui::Painter, screen: Rect, title: &str, items: [&str; 3], focused: usize) {
    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(200));
    let font = FontId::proportional(screen.height() * 0.035);
    let panel = Rect::from_center_size(
        screen.center(),
        vec2(screen.width() * 0.34, screen.height() * 0.5),
    );
    painter.rect_filled(panel, 20.0, theme::PANEL);
    let inner = panel.shrink(screen.height() * 0.04);
    painter.text(
        inner.min,
        Align2::LEFT_TOP,
        title,
        FontId::proportional(screen.height() * 0.045),
        theme::TEXT,
    );
    let height = screen.height() * 0.085;
    for (index, label) in items.iter().enumerate() {
        let rect = Rect::from_min_size(
            pos2(
                inner.min.x,
                inner.min.y + screen.height() * 0.09 + index as f32 * (height + 14.0),
            ),
            vec2(inner.width(), height),
        );
        let focused = index == focused;
        painter.rect_filled(rect, 14.0, if focused { theme::ACCENT } else { theme::KEY });
        if focused {
            painter.rect_stroke(
                rect,
                14.0,
                Stroke::new(3.0, Color32::WHITE),
                StrokeKind::Outside,
            );
        }
        painter.text(
            rect.left_center() + vec2(28.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            font.clone(),
            if focused { Color32::BLACK } else { theme::TEXT },
        );
    }
}

fn draw_confirm(
    painter: &egui::Painter,
    screen: Rect,
    title: &str,
    body: &str,
    buttons: [&str; 2],
    focused: usize,
) {
    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(200));
    let panel = Rect::from_center_size(
        screen.center(),
        vec2(screen.width() * 0.5, screen.height() * 0.42),
    );
    painter.rect_filled(panel, 20.0, theme::PANEL);
    let inner = panel.shrink(screen.height() * 0.04);
    painter.text(
        inner.min,
        Align2::LEFT_TOP,
        title,
        FontId::proportional(screen.height() * 0.045),
        theme::TEXT,
    );
    let mut job = LayoutJob::simple(
        body.to_string(),
        FontId::proportional(screen.height() * 0.028),
        theme::DIM,
        inner.width(),
    );
    job.wrap.max_rows = 5;
    let galley = painter.layout_job(job);
    painter.galley(
        inner.min + vec2(0.0, screen.height() * 0.08),
        galley,
        theme::DIM,
    );
    let font = FontId::proportional(screen.height() * 0.032);
    let mut x = inner.min.x;
    for (index, label) in buttons.iter().enumerate() {
        x = draw_button(
            painter,
            pos2(x, inner.max.y - screen.height() * 0.07),
            label,
            None,
            &font,
            index == focused,
        )
        .max
        .x + 24.0;
    }
}

fn draw_notice(painter: &egui::Painter, screen: Rect, message: &str) {
    let font = FontId::proportional(screen.height() * 0.026);
    let mut job = LayoutJob::simple(message.to_string(), font, theme::TEXT, screen.width() * 0.6);
    job.wrap.max_rows = 3;
    let galley = painter.layout_job(job);
    let rect = Rect::from_center_size(
        pos2(
            screen.center().x,
            screen.max.y - footer_height(screen) - galley.size().y - 40.0,
        ),
        galley.size() + vec2(48.0, 28.0),
    );
    painter.rect_filled(rect, 14.0, theme::FAILED);
    painter.galley(rect.min + vec2(24.0, 14.0), galley, theme::TEXT);
}

fn header_height(screen: Rect) -> f32 {
    screen.height() * 0.11
}

fn footer_height(screen: Rect) -> f32 {
    screen.height() * 0.075
}

/// A title as search compares it: lowercase, without accents or the marks GOG
/// puts in titles, punctuation as spaces.
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => Some('a'),
            'é' | 'è' | 'ë' | 'ê' => Some('e'),
            'í' | 'ì' | 'ï' | 'î' => Some('i'),
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' | 'ø' => Some('o'),
            'ú' | 'ù' | 'ü' | 'û' => Some('u'),
            'ñ' => Some('n'),
            'ç' => Some('c'),
            '™' | '®' | '©' => None,
            c if c.is_alphanumeric() || c == ' ' => Some(c),
            '&' => Some('&'),
            _ => Some(' '),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ignores_case_accents_and_marks() {
        assert_eq!(
            fold("Legacy of Kain™ Soul Reaver 1&2"),
            "legacy of kain soul reaver 1&2"
        );
        assert_eq!(fold("Tomb Raider: Anniversary"), "tomb raider  anniversary");
        assert!(fold("Pokémon Ñandú").contains("pokemon nandu"));
    }
}
