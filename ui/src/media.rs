//! Everything a detail view or the downloads panel shows that takes a while to
//! get: a game's details from GOG, its screenshots, the space it takes, and how
//! the disk is used. One background thread does it all, in order, so a burst of
//! requests from scrolling through details never runs ahead of the network.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::ColorImage;
use gamestore_core::catalog::ProductId;
use gamestore_core::details::{self, Details, Storage};
use gamestore_core::http::{DownloadClient, Fetch, UreqClient};

use crate::jobs::Machine;

/// Screenshots are decoded at this width: sharp on a 1080p TV at the size the
/// detail view draws them, without holding full images in memory.
const SCREENSHOT_WIDTH: u32 = 1280;

#[derive(Debug, Clone)]
enum Request {
    Details(ProductId),
    Screenshot(ProductId, usize, String),
    DiskUsage(ProductId, PathBuf),
    Storage(Vec<PathBuf>),
}

/// What came back.
pub enum Answer {
    Details(ProductId, Details),
    Screenshot(ProductId, usize, Option<ColorImage>),
    DiskUsage(ProductId, u64),
    Storage(Option<Storage>),
}

pub struct Media {
    requests: Sender<Request>,
    answers: Receiver<Answer>,
}

impl Media {
    pub fn new(machine: Machine, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (requests, queue) = channel::<Request>();
        let (answer, answers) = channel();
        std::thread::spawn(move || {
            let http = UreqClient::new();
            let cdn = DownloadClient::new();
            for request in queue {
                let reply = match request {
                    Request::Details(id) => {
                        let found = details::cached(&machine.paths, &id).or_else(|| {
                            let token = (machine.token)().ok()?;
                            details::fetch(&http, &token, &machine.paths, &id).ok()
                        });
                        found.map(|found| Answer::Details(id, found))
                    }
                    Request::Screenshot(id, index, url) => {
                        let image = screenshot(&cdn, &machine, &url);
                        Some(Answer::Screenshot(id, index, image))
                    }
                    Request::DiskUsage(id, path) => {
                        Some(Answer::DiskUsage(id, details::disk_usage(&path)))
                    }
                    Request::Storage(libraries) => Some(Answer::Storage(details::storage(
                        &machine.config.install_root(&machine.paths),
                        &machine.paths.cache_dir,
                        &libraries,
                    ))),
                };
                if let Some(reply) = reply {
                    if answer.send(reply).is_err() {
                        break;
                    }
                    wake();
                }
            }
        });
        Self { requests, answers }
    }

    pub fn details(&self, id: ProductId) {
        let _ = self.requests.send(Request::Details(id));
    }

    pub fn screenshot(&self, id: ProductId, index: usize, url: String) {
        let _ = self.requests.send(Request::Screenshot(id, index, url));
    }

    pub fn disk_usage(&self, id: ProductId, path: PathBuf) {
        let _ = self.requests.send(Request::DiskUsage(id, path));
    }

    pub fn storage(&self, libraries: Vec<PathBuf>) {
        let _ = self.requests.send(Request::Storage(libraries));
    }

    pub fn answers(&self) -> Vec<Answer> {
        self.answers.try_iter().collect()
    }
}

/// A screenshot, from the cache or GOG's CDN, decoded at [`SCREENSHOT_WIDTH`].
fn screenshot(cdn: &DownloadClient, machine: &Machine, url: &str) -> Option<ColorImage> {
    let name = url.rsplit('/').next()?.split('?').next()?;
    let file = machine.paths.cache_dir.join("screenshots").join(name);
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(_) => {
            let (status, mut body) = cdn.open(url, 0).ok()?;
            if status != 200 {
                return None;
            }
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut body, &mut bytes).ok()?;
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&file, &bytes);
            bytes
        }
    };
    let image = image::load_from_memory(&bytes).ok()?;
    let height = image.height() * SCREENSHOT_WIDTH / image.width().max(1);
    let image = image.resize_exact(
        SCREENSHOT_WIDTH,
        height,
        image::imageops::FilterType::Triangle,
    );
    let rgba = image.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}
