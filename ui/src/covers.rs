//! Cover images for the grid, fetched and decoded off the UI thread.
//!
//! Covers are GOG's vertical covers from gamesdb, cached under
//! `<cache>/covers/<id>.jpg` so the grid works offline once they have been seen. A
//! game without one is remembered as `<id>.none` and drawn as a placeholder. The
//! grid never waits on any of this: a cover appears when it is ready.
//!
//! An installed game shows its Steam portrait instead, the image in Steam's
//! `grid` directory, so the store and Steam's library show the same cover;
//! that image is `core::artwork`'s job.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::ColorImage;
use gamestore_core::catalog::ProductId;
use gamestore_core::http::{DownloadClient, Fetch, HttpClient, UreqClient};

/// Thumbnails are decoded at this size: plenty for a TV grid, and 385 of them fit
/// in memory comfortably.
pub const WIDTH: u32 = 300;
pub const HEIGHT: u32 = 423;

const WORKERS: usize = 4;

pub struct Covers {
    requests: Sender<(ProductId, Option<PathBuf>)>,
    results: Receiver<(ProductId, Option<ColorImage>)>,
}

impl Covers {
    pub fn new(cache_dir: &Path, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (requests, queue) = channel::<(ProductId, Option<PathBuf>)>();
        let (done, results) = channel();
        let queue = Arc::new(Mutex::new(queue));
        let wake = Arc::new(wake);
        let dir = cache_dir.join("covers");
        let _ = std::fs::create_dir_all(&dir);

        for _ in 0..WORKERS {
            let (queue, done, wake, dir) = (queue.clone(), done.clone(), wake.clone(), dir.clone());
            std::thread::spawn(move || {
                let http = UreqClient::new();
                let cdn = DownloadClient::new();
                loop {
                    let next = queue.lock().ok().and_then(|queue| queue.recv().ok());
                    let Some((id, steam)) = next else { break };
                    let image = steam
                        .and_then(|file| std::fs::read(file).ok())
                        .and_then(|bytes| decode(&bytes))
                        .or_else(|| load(&http, &cdn, &dir, &id));
                    if done.send((id, image)).is_err() {
                        break;
                    }
                    wake();
                }
            });
        }

        Self { requests, results }
    }

    /// Ask for a cover: `steam`'s image when there is one, else the store's.
    /// Asking again replaces the cover when it arrives.
    pub fn request(&self, id: ProductId, steam: Option<PathBuf>) {
        let _ = self.requests.send((id, steam));
    }

    /// Covers that are ready, at most `limit` of them, so uploading textures never
    /// stalls a frame.
    pub fn ready(&self, limit: usize) -> Vec<(ProductId, Option<ColorImage>)> {
        self.results.try_iter().take(limit).collect()
    }
}

fn load(http: &UreqClient, cdn: &DownloadClient, dir: &Path, id: &ProductId) -> Option<ColorImage> {
    let file = dir.join(format!("{id}.jpg"));
    let none = dir.join(format!("{id}.none"));
    if none.exists() {
        return None;
    }
    if !file.exists() {
        match cover_url(http, id) {
            Ok(Some(url)) => {
                let bytes = fetch(cdn, &url)?;
                let _ = std::fs::write(&file, &bytes);
            }
            Ok(None) => {
                let _ = std::fs::write(&none, "");
                return None;
            }
            // A network failure is not remembered: next time may work.
            Err(()) => return None,
        }
    }
    decode(&std::fs::read(&file).ok()?)
}

/// The vertical cover's address from gamesdb, `Ok(None)` when it has none.
fn cover_url(http: &UreqClient, id: &ProductId) -> Result<Option<String>, ()> {
    let response = http
        .get(&format!(
            "https://gamesdb.gog.com/platforms/gog/external_releases/{id}"
        ))
        .map_err(|_| ())?;
    if response.status == 404 {
        return Ok(None);
    }
    if !response.is_success() {
        return Err(());
    }
    let parsed: serde_json::Value = serde_json::from_str(&response.body).map_err(|_| ())?;
    Ok(parsed
        .pointer("/game/vertical_cover/url_format")
        .and_then(|url| url.as_str())
        .map(|format| format.replace("{formatter}", "").replace("{ext}", "jpg")))
}

fn fetch(cdn: &DownloadClient, url: &str) -> Option<Vec<u8>> {
    let (status, mut body) = cdn.open(url, 0).ok()?;
    if status != 200 {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut body, &mut bytes).ok()?;
    Some(bytes)
}

/// Decode and fit into the thumbnail size.
pub fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image.resize_to_fill(WIDTH, HEIGHT, image::imageops::FilterType::Triangle);
    let rgba = image.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}
