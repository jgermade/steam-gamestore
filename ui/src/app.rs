//! The screens: the cover grid, a game's detail, the keyboard, confirmations.
//!
//! Everything is painted directly rather than built from egui's mouse-oriented
//! widgets: focus and scrolling are this module's own state, moved only by
//! [`Nav`] actions, which is what a controller needs.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, TextureHandle, TextureOptions,
    Vec2, pos2, text::LayoutJob, vec2,
};
use gamestore_core::catalog::{Game, Os, ProductId};
use gamestore_core::registry::{Registry, State};
use gamestore_platform_linux::{install, pending};

use crate::covers::{self, Covers};
use crate::input::{Input, Nav};
use crate::jobs::{Jobs, Machine, Progress};
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
    /// Which build each install started this session is of.
    building: HashMap<ProductId, Os>,
    loaded_at: Instant,
    pending: usize,
    notice: Option<(String, Instant)>,
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
        let jobs = Jobs::new(machine.clone(), wake);

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
            building: HashMap::new(),
            loaded_at: Instant::now() - Duration::from_secs(60),
            pending: 0,
            notice: None,
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
        let Some(game) = self.current().cloned() else {
            return;
        };
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
                    Nav::Left | Nav::Up => {
                        self.screen = Screen::Detail {
                            button: button.saturating_sub(1),
                        }
                    }
                    Nav::Right | Nav::Down => {
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
                        Some(DetailAction::Back) | None => self.screen = Screen::Grid,
                    },
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
                if matches!(nav, Nav::Back | Nav::Accept) {
                    self.screen = Screen::Grid;
                }
            }
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
            Nav::Accept if count > 0 => self.screen = Screen::Detail { button: 0 },
            Nav::Search => self.screen = Screen::Keyboard,
            Nav::Action if !self.filter.is_empty() => {
                self.filter.pop();
                self.refilter();
            }
            Nav::Previous | Nav::Next => {
                self.tab = if self.tab == Tab::All {
                    Tab::Installed
                } else {
                    Tab::All
                };
                self.refilter();
            }
            Nav::Menu => self.screen = Screen::Menu { button: 0 },
            // B never closes the store: it clears a search, or shows what is
            // downloading and installing.
            Nav::Back => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.refilter();
                } else {
                    self.screen = Screen::Downloads;
                }
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
            return vec![DetailAction::Restart, DetailAction::Back];
        }
        if progress.is_some_and(Progress::active) {
            return vec![DetailAction::Back];
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
            builds.into_iter().map(DetailAction::Install).collect()
        };
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

        for nav in self.input.poll(&ctx) {
            self.handle(nav, &ctx);
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
        let top = screen.min.y
            + header_height(screen)
            + if self.banner() {
                banner_height(screen)
            } else {
                0.0
            };
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

    fn banner(&self) -> bool {
        self.pending_count() > 0
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

        if self.banner() {
            let band = Rect::from_min_size(
                pos2(screen.min.x, header.max.y),
                vec2(screen.width(), banner_height(screen)),
            );
            painter.rect_filled(band, 0.0, theme::WAITING);
            painter.text(
                band.center(),
                Align2::CENTER_CENTER,
                format!(
                    "{} {} — Start: {}",
                    self.pending_count(),
                    text.pending_banner,
                    text.restart_steam
                ),
                normal.clone(),
                Color32::BLACK,
            );
        }

        // The covers.
        let clip = painter.with_clip_rect(layout.area.expand(layout.gap));
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

        // Hints along the bottom.
        let footer = Rect::from_min_max(
            pos2(screen.min.x, screen.max.y - footer_height(screen)),
            screen.max,
        );
        painter.rect_filled(footer, 0.0, theme::PANEL);
        let hints = [
            ("A", text.select),
            ("Y", text.search),
            ("LB/RB", text.tabs),
            ("B", text.downloads),
            ("Start", text.menu),
        ];
        let mut x = footer.min.x + layout.area.min.x - screen.min.x;
        for (button, label) in hints {
            let chip = painter.layout_no_wrap(button.to_string(), small.clone(), Color32::BLACK);
            let chip_rect = Rect::from_min_size(
                pos2(x, footer.center().y - chip.size().y / 2.0 - 4.0),
                chip.size() + vec2(16.0, 8.0),
            );
            painter.rect_filled(chip_rect, 8.0, theme::TEXT);
            painter.galley(chip_rect.min + vec2(8.0, 4.0), chip, Color32::BLACK);
            let end = painter.text(
                pos2(chip_rect.max.x + 10.0, footer.center().y),
                Align2::LEFT_CENTER,
                label,
                small.clone(),
                theme::DIM,
            );
            x = end.max.x + 36.0;
        }
        painter.text(
            pos2(
                footer.max.x - (layout.area.min.x - screen.min.x),
                footer.center().y,
            ),
            Align2::RIGHT_CENTER,
            format!("{} {}", self.visible.len(), text.results),
            small,
            theme::DIM,
        );
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

    /// What is downloading or installing, and what finished this session.
    fn draw_downloads(&self, painter: &egui::Painter, screen: Rect) {
        let text = self.text;
        painter.rect_filled(screen, 0.0, Color32::from_black_alpha(215));
        let panel = Rect::from_center_size(
            screen.center(),
            vec2(screen.width() * 0.62, screen.height() * 0.74),
        );
        painter.rect_filled(panel, 20.0, theme::PANEL);
        let inner = panel.shrink(screen.height() * 0.04);
        painter.text(
            inner.min,
            Align2::LEFT_TOP,
            text.downloads,
            FontId::proportional(screen.height() * 0.045),
            theme::TEXT,
        );

        // Working ones first, in the order they will run; then what finished.
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

        let font = FontId::proportional(screen.height() * 0.03);
        let small = FontId::proportional(screen.height() * 0.022);
        if rows.is_empty() {
            painter.text(
                inner.center(),
                Align2::CENTER_CENTER,
                text.downloads_empty,
                font,
                theme::DIM,
            );
            return;
        }
        let row_height = screen.height() * 0.1;
        let time = painter.ctx().input(|input| input.time) as f32;
        for (index, (game, progress)) in rows.iter().enumerate() {
            let top = inner.min.y + screen.height() * 0.08 + index as f32 * row_height;
            if top + row_height > inner.max.y {
                break;
            }
            let row = Rect::from_min_size(
                pos2(inner.min.x, top),
                vec2(inner.width(), row_height * 0.86),
            );
            painter.rect_filled(row, 12.0, theme::BACKGROUND);
            let build = match self.building.get(&game.id) {
                Some(Os::Linux) => " · Linux",
                Some(Os::Windows) => " · Windows",
                _ => "",
            };
            painter.text(
                row.min + vec2(20.0, 12.0),
                Align2::LEFT_TOP,
                format!("{}{build}", game.title),
                font.clone(),
                theme::TEXT,
            );
            let (label, colour) = self.badge(&game.id).unwrap_or_default();
            painter.text(
                pos2(row.max.x - 20.0, row.min.y + 12.0),
                Align2::RIGHT_TOP,
                label,
                small.clone(),
                colour,
            );
            if let Progress::Failed(reason) = progress {
                let mut job = LayoutJob::simple(
                    reason.clone(),
                    small.clone(),
                    theme::FAILED,
                    row.width() - 40.0,
                );
                job.wrap.max_rows = 1;
                job.wrap.overflow_character = Some('…');
                painter.galley(
                    pos2(row.min.x + 20.0, row.max.y - 34.0),
                    painter.layout_job(job),
                    theme::FAILED,
                );
            }
            let bar = Rect::from_min_max(
                pos2(row.min.x + 20.0, row.max.y - 18.0),
                pos2(row.max.x - 20.0, row.max.y - 8.0),
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
        painter.rect_filled(screen, 0.0, theme::BACKGROUND.gamma_multiply(0.96));

        let margin = screen.width() * 0.07;
        let cover_height = screen.height() * 0.62;
        let cover = Rect::from_min_size(
            pos2(screen.min.x + margin, screen.min.y + screen.height() * 0.16),
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

        let left = cover.max.x + margin * 0.6;
        let width = screen.max.x - margin - left;
        let mut y = cover.min.y;
        let write = |message: String, size: f32, colour: Color32, y: &mut f32| {
            let mut job = LayoutJob::simple(
                message,
                FontId::proportional(screen.height() * size),
                colour,
                width,
            );
            job.wrap.max_rows = 6;
            let galley = painter.layout_job(job);
            let height = galley.size().y;
            painter.galley(pos2(left, *y), galley, colour);
            *y += height + screen.height() * 0.02;
        };

        write(game.title.clone(), 0.055, theme::TEXT, &mut y);
        let known = self.known.get(&game.id).cloned().unwrap_or_default();
        if let Some((label, colour)) = self.badge(&game.id) {
            write(label, 0.032, colour, &mut y);
        }
        match self.progress.get(&game.id) {
            Some(Progress::Done { pending: true }) => write(
                text.in_steam_after_restart.to_string(),
                0.03,
                theme::TEXT,
                &mut y,
            ),
            _ if self.installed(&game.id) => {
                write(text.play_in_steam.to_string(), 0.03, theme::TEXT, &mut y);
                let build = match known.platform {
                    Some(Os::Linux) => text.linux_build,
                    _ => text.windows_build,
                };
                write(build.to_string(), 0.025, theme::DIM, &mut y);
                if let Some(build) = &known.build {
                    write(
                        format!("{}: {build}", text.version),
                        0.025,
                        theme::DIM,
                        &mut y,
                    );
                }
                if let Some(path) = &known.install_path {
                    write(
                        format!("{}: {path}", text.install_path),
                        0.025,
                        theme::DIM,
                        &mut y,
                    );
                }
            }
            _ => {}
        }
        if !game.has_windows_build() {
            write(text.not_windows.to_string(), 0.03, theme::FAILED, &mut y);
        }
        if let Some(reason) = self.failure(&game.id) {
            write(reason, 0.025, theme::FAILED, &mut y);
        }

        // The actions.
        let font = FontId::proportional(screen.height() * 0.035);
        let mut x = left;
        let y = cover.max.y - screen.height() * 0.08;
        for (index, action) in self.detail_buttons().iter().enumerate() {
            let label = match action {
                DetailAction::Install(_)
                    if self.failure(&game.id).is_some() && install::builds(game).len() == 1 =>
                {
                    text.retry
                }
                DetailAction::Install(Os::Linux) if install::builds(game).len() > 1 => {
                    text.install_linux
                }
                DetailAction::Install(Os::Windows) if install::builds(game).len() > 1 => {
                    text.install_windows
                }
                DetailAction::Install(_) => text.install,
                DetailAction::Switch(Os::Linux) => text.switch_linux,
                DetailAction::Switch(_) => text.switch_windows,
                DetailAction::Restart => text.restart_steam,
                DetailAction::Back => text.back,
            };
            x = draw_button(painter, pos2(x, y), label, &font, index == button)
                .max
                .x
                + 24.0;
        }
    }
}

fn draw_button(
    painter: &egui::Painter,
    at: Pos2,
    label: &str,
    font: &FontId,
    focused: bool,
) -> Rect {
    let galley = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::BLACK);
    let rect = Rect::from_min_size(at, galley.size() + vec2(64.0, 32.0));
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
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font.clone(),
        if focused { Color32::BLACK } else { theme::TEXT },
    );
    rect
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

fn banner_height(screen: Rect) -> f32 {
    screen.height() * 0.05
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
