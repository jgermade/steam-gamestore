//! Everything a detail view or the downloads panel shows that takes a while to
//! get: a game's details from GOG, its screenshots, the artwork to choose from,
//! the space it takes, and how the disk is used.
//!
//! Two lanes, so the slow kind never holds up the quick one: the network lane has
//! a few threads, because a detail view wants its details and two or three
//! screenshots at once; the disk lane has one, because measuring a 35 GB game
//! walks every file in it and doing two of those at once only makes both slower.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::ColorImage;
use gamestore_core::artwork::{self, Art, Choice, Kind};
use gamestore_core::catalog::ProductId;
use gamestore_core::details::{self, Details, Storage};
use gamestore_core::http::{DownloadClient, Fetch, UreqClient};

use crate::covers;
use crate::jobs::Machine;

/// Screenshots are decoded at this width: sharp on a 1080p TV at the size the
/// detail view draws them, without holding full images in memory.
const SCREENSHOT_WIDTH: u32 = 1280;

/// Artwork previews fit in a square this size.
const PREVIEW: u32 = 480;

/// How many images of a kind the artwork picker offers.
const CHOICES: usize = 30;

const NETWORK_WORKERS: usize = 3;

#[derive(Debug, Clone)]
enum Request {
    Details(ProductId),
    Screenshot(ProductId, usize, String),
    Choices(ProductId, String, Kind),
    Preview(ProductId, Kind, usize, String),
    Apply(Apply),
    DiskUsage(ProductId, PathBuf),
    Storage(Vec<PathBuf>),
}

/// An image picked for a game: where it goes.
#[derive(Debug, Clone)]
pub struct Apply {
    pub id: ProductId,
    pub art: Art,
    /// The game's Steam tile, when it has one.
    pub appid: Option<u32>,
}

/// What came back.
pub enum Answer {
    Details(ProductId, Details),
    Screenshot(ProductId, usize, Option<ColorImage>),
    Choices(ProductId, Kind, Result<Vec<Choice>, String>),
    Preview(ProductId, Kind, usize, Option<ColorImage>),
    /// An image was put in place; for a portrait, the grid's new cover too.
    Applied(ProductId, Kind, Result<Option<ColorImage>, String>),
    DiskUsage(ProductId, u64),
    Storage(Option<Storage>),
}

pub struct Media {
    network: Sender<Request>,
    disk: Sender<Request>,
    answers: Receiver<Answer>,
}

impl Media {
    pub fn new(machine: Machine, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (answer, answers) = channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let network = lane(NETWORK_WORKERS, &machine, &answer, &wake);
        let disk = lane(1, &machine, &answer, &wake);
        Self {
            network,
            disk,
            answers,
        }
    }

    pub fn details(&self, id: ProductId) {
        let _ = self.network.send(Request::Details(id));
    }

    pub fn screenshot(&self, id: ProductId, index: usize, url: String) {
        let _ = self.network.send(Request::Screenshot(id, index, url));
    }

    /// The images SteamGridDB has of `kind` for the game.
    pub fn choices(&self, id: ProductId, title: String, kind: Kind) {
        let _ = self.network.send(Request::Choices(id, title, kind));
    }

    pub fn preview(&self, id: ProductId, kind: Kind, index: usize, url: String) {
        let _ = self.network.send(Request::Preview(id, kind, index, url));
    }

    pub fn apply(&self, apply: Apply) {
        let _ = self.network.send(Request::Apply(apply));
    }

    pub fn disk_usage(&self, id: ProductId, path: PathBuf) {
        let _ = self.disk.send(Request::DiskUsage(id, path));
    }

    pub fn storage(&self, libraries: Vec<PathBuf>) {
        let _ = self.disk.send(Request::Storage(libraries));
    }

    pub fn answers(&self) -> Vec<Answer> {
        self.answers.try_iter().collect()
    }
}

/// `workers` threads taking requests from one queue, in order.
fn lane(
    workers: usize,
    machine: &Machine,
    answer: &Sender<Answer>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) -> Sender<Request> {
    let (requests, queue) = channel::<Request>();
    let queue = Arc::new(Mutex::new(queue));
    for _ in 0..workers {
        let (queue, answer, wake, machine) =
            (queue.clone(), answer.clone(), wake.clone(), machine.clone());
        std::thread::spawn(move || {
            let http = UreqClient::new();
            let cdn = DownloadClient::new();
            loop {
                let next = queue.lock().ok().and_then(|queue| queue.recv().ok());
                let Some(request) = next else { break };
                if let Some(reply) = serve(&machine, &http, &cdn, request) {
                    if answer.send(reply).is_err() {
                        break;
                    }
                    wake();
                }
            }
        });
    }
    requests
}

fn serve(
    machine: &Machine,
    http: &UreqClient,
    cdn: &DownloadClient,
    request: Request,
) -> Option<Answer> {
    match request {
        Request::Details(id) => {
            let found = details::cached(&machine.paths, &id).or_else(|| {
                let token = (machine.token)().ok()?;
                details::fetch(http, &token, &machine.paths, &id).ok()
            });
            found.map(|found| Answer::Details(id, found))
        }
        Request::Screenshot(id, index, url) => {
            let image = cache_file(&machine.paths.cache_dir, "screenshots", &url)
                .and_then(|file| cached_bytes(cdn, &file, &url))
                .and_then(|bytes| screenshot(&bytes));
            Some(Answer::Screenshot(id, index, image))
        }
        Request::Choices(id, title, kind) => {
            let found = match machine.config.steamgriddb.api_key.as_deref() {
                Some(key) => artwork::choices(http, key, &id, &title, kind, CHOICES)
                    .map_err(|error| error.to_string()),
                None => Err(String::new()),
            };
            Some(Answer::Choices(id, kind, found))
        }
        Request::Preview(id, kind, index, url) => {
            let image = cache_file(&machine.paths.cache_dir, "artwork", &url)
                .and_then(|file| cached_bytes(cdn, &file, &url))
                .and_then(|bytes| preview(&bytes));
            Some(Answer::Preview(id, kind, index, image))
        }
        Request::Apply(apply) => Some(Answer::Applied(
            apply.id.clone(),
            apply.art.kind,
            put_in_place(machine, cdn, &apply).map_err(|error| error.to_string()),
        )),
        Request::DiskUsage(id, path) => Some(Answer::DiskUsage(id, details::disk_usage(&path))),
        Request::Storage(libraries) => Some(Answer::Storage(details::storage(
            &machine.config.install_root(&machine.paths),
            &machine.paths.cache_dir,
            &libraries,
        ))),
    }
}

/// Remember the pick, write it into every Steam account's artwork when the game
/// has a tile, and make a portrait the grid's own cover as well.
fn put_in_place(
    machine: &Machine,
    cdn: &DownloadClient,
    apply: &Apply,
) -> gamestore_core::Result<Option<ColorImage>> {
    artwork::remember(&machine.paths, &apply.id, &apply.art)?;
    let mut in_steam = None;
    if let Some(appid) = apply.appid {
        let dirs: Vec<PathBuf> = machine.users.iter().map(|user| user.grid_dir()).collect();
        let written = artwork::save(cdn, std::slice::from_ref(&apply.art), appid, &dirs)?;
        in_steam = written.into_iter().next();
    }
    if apply.art.kind != Kind::Portrait {
        return Ok(None);
    }
    // A game with a tile shows Steam's portrait, badge and all.
    if let Some(file) = in_steam {
        return Ok(std::fs::read(file)
            .ok()
            .and_then(|bytes| covers::decode(&bytes)));
    }
    let bytes = cache_file(&machine.paths.cache_dir, "artwork", &apply.art.url)
        .and_then(|file| cached_bytes(cdn, &file, &apply.art.url));
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let covers = machine.paths.cache_dir.join("covers");
    let _ = std::fs::create_dir_all(&covers);
    let _ = std::fs::remove_file(covers.join(format!("{}.none", apply.id)));
    // The image crate reads the format from the bytes, so a PNG kept under `.jpg`
    // still decodes.
    let _ = std::fs::write(covers.join(format!("{}.jpg", apply.id)), &bytes);
    Ok(covers::decode(&bytes))
}

/// `<cache>/<dir>/<the URL's file name>`.
fn cache_file(cache_dir: &Path, dir: &str, url: &str) -> Option<PathBuf> {
    let name = url.rsplit('/').next()?.split('?').next()?;
    (!name.is_empty()).then(|| cache_dir.join(dir).join(name))
}

/// The bytes at `url`, from `file` when they were fetched before.
fn cached_bytes(cdn: &DownloadClient, file: &Path, url: &str) -> Option<Vec<u8>> {
    if let Ok(bytes) = std::fs::read(file) {
        return Some(bytes);
    }
    let (status, mut body) = cdn.open(url, 0).ok()?;
    if status != 200 {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut body, &mut bytes).ok()?;
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(file, &bytes);
    Some(bytes)
}

/// A screenshot decoded at [`SCREENSHOT_WIDTH`].
fn screenshot(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let height = image.height() * SCREENSHOT_WIDTH / image.width().max(1);
    let image = image.resize_exact(
        SCREENSHOT_WIDTH,
        height,
        image::imageops::FilterType::Triangle,
    );
    Some(colour_image(&image))
}

/// An artwork preview, fitted inside [`PREVIEW`] and keeping its shape.
fn preview(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = if image.width() > PREVIEW || image.height() > PREVIEW {
        image.resize(PREVIEW, PREVIEW, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    Some(colour_image(&image))
}

fn colour_image(image: &image::DynamicImage) -> ColorImage {
    let rgba = image.to_rgba8();
    ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    )
}
