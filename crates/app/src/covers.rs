//! Album art: downloaded, decoded and cached in memory, plus the colour seed it gives the theme.
//!
//! Network and decoding happen on background threads with a small pool of downloads at a time;
//! when an image is ready the cache notifies its observers once per batch, which redraw with the
//! real cover instead of the placeholder.
//!
//! Two sizes are kept: a thumbnail for rows, the queue and the mini player ([`THUMB_PX`]) and the
//! big art for cards and headers ([`COVER_PX`]). The cache is an LRU bounded by bytes, so a long
//! session cannot grow it without limit.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context, Hsla, RenderImage, hsla};
use image::imageops::FilterType;

/// Big art: cards, headers and the playing sheet.
pub const COVER_PX: u32 = 440;
/// Small art: list rows, the queue and the mini player.
pub const THUMB_PX: u32 = 96;

/// How much decoded art stays in memory before the least recently used entry is dropped. A big
/// cover is about 774 KB and a thumbnail about 37 KB, so this is a few dozen of both.
const CACHE_BYTES: usize = 192 * 1024 * 1024;
/// Hard cap on entries, for pages that ask for hundreds of covers.
const CACHE_ENTRIES: usize = 1200;
/// Art drawn this recently is on screen (or just was): evicting it would make it download and
/// fade in again on the next frame, over and over. The cache runs over budget instead.
const ON_SCREEN: Duration = Duration::from_secs(10);
/// Downloads at once. A home page asks for dozens of covers in one frame and those downloads
/// share the background executor with the search, the lyrics and the album page.
const MAX_IN_FLIGHT: usize = 5;
/// Arrivals closer than this are announced together, so a page is not rebuilt once per cover.
const NOTIFY_BATCH: Duration = Duration::from_millis(90);

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) atic-play/0.1";

/// Which of the two decoded sizes to ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtSize {
    /// 96 px: list rows, the queue, the mini player.
    Thumb,
    /// 440 px: cards in the carousels and grids, headers and the playing sheet.
    Large,
}

impl ArtSize {
    fn px(self) -> u32 {
        match self {
            ArtSize::Thumb => THUMB_PX,
            ArtSize::Large => COVER_PX,
        }
    }

    fn bytes(self) -> usize {
        let side = self.px() as usize;
        side * side * 4
    }
}

#[derive(Debug)]
struct Entry {
    image: Option<Arc<RenderImage>>,
    bytes: usize,
    /// When it was last drawn; the smallest one goes first when the cache is full.
    used: Cell<u64>,
    /// The same, as a time, to keep what is on screen.
    seen: Cell<Instant>,
}

pub struct CoverCache {
    agent: ureq::Agent,
    large: HashMap<String, Entry>,
    thumbs: HashMap<String, Entry>,
    seeds: HashMap<String, Hsla>,
    pending_large: HashSet<String>,
    pending_thumbs: HashSet<String>,
    /// Covers that are waiting for a free download slot, oldest first.
    waiting: VecDeque<(String, ArtSize)>,
    in_flight: usize,
    bytes: usize,
    use_tick: Cell<u64>,
    notified: bool,
}

impl CoverCache {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::new_with_defaults(),
            large: HashMap::new(),
            thumbs: HashMap::new(),
            seeds: HashMap::new(),
            pending_large: HashSet::new(),
            pending_thumbs: HashSet::new(),
            waiting: VecDeque::new(),
            in_flight: 0,
            bytes: 0,
            use_tick: Cell::new(0),
            notified: false,
        }
    }

    fn map(&self, size: ArtSize) -> &HashMap<String, Entry> {
        match size {
            ArtSize::Thumb => &self.thumbs,
            ArtSize::Large => &self.large,
        }
    }

    fn map_mut(&mut self, size: ArtSize) -> &mut HashMap<String, Entry> {
        match size {
            ArtSize::Thumb => &mut self.thumbs,
            ArtSize::Large => &mut self.large,
        }
    }

    fn pending(&self, size: ArtSize) -> &HashSet<String> {
        match size {
            ArtSize::Thumb => &self.pending_thumbs,
            ArtSize::Large => &self.pending_large,
        }
    }

    fn pending_mut(&mut self, size: ArtSize) -> &mut HashSet<String> {
        match size {
            ArtSize::Thumb => &mut self.pending_thumbs,
            ArtSize::Large => &mut self.pending_large,
        }
    }

    /// Touches the entry so the LRU keeps it a while longer.
    fn use_entry(&self, entry: &Entry) -> Option<Arc<RenderImage>> {
        let tick = self.use_tick.get() + 1;
        self.use_tick.set(tick);
        entry.used.set(tick);
        entry.seen.set(Instant::now());
        entry.image.clone()
    }

    /// The image if it is already here; never starts a download.
    pub fn peek(&self, url: Option<&str>) -> Option<Arc<RenderImage>> {
        self.peek_size(url, ArtSize::Large)
    }

    /// The image of `size` if it is already here; never starts a download.
    pub fn peek_size(&self, url: Option<&str>, size: ArtSize) -> Option<Arc<RenderImage>> {
        let url = url?;
        self.map(size).get(url).and_then(|entry| self.use_entry(entry))
    }

    /// The colour the theme takes from this cover, once it is decoded.
    pub fn seed(&self, url: Option<&str>) -> Option<Hsla> {
        url.and_then(|url| self.seeds.get(url).copied())
    }

    /// The big cover for `url`, starting the download the first time it is asked for.
    pub fn image(&mut self, url: Option<&str>, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.image_size(url, ArtSize::Large, cx)
    }

    /// The cover of `size` for `url`, starting the download the first time it is asked for.
    pub fn image_size(
        &mut self,
        url: Option<&str>,
        size: ArtSize,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let url = url?;
        if let Some(cached) = self.peek_size(Some(url), size) {
            return Some(cached);
        }
        self.request(url, size, cx);
        None
    }

    fn request(&mut self, url: &str, size: ArtSize, cx: &mut Context<Self>) {
        if self.pending(size).contains(url) || self.map(size).contains_key(url) {
            return;
        }
        self.pending_mut(size).insert(url.to_string());
        self.waiting.push_back((url.to_string(), size));
        self.pump(cx);
    }

    /// Starts downloads while there is room; the rest wait their turn.
    fn pump(&mut self, cx: &mut Context<Self>) {
        while self.in_flight < MAX_IN_FLIGHT {
            let Some((url, size)) = self.waiting.pop_front() else {
                break;
            };
            self.in_flight += 1;
            let agent = self.agent.clone();
            let fetched = url.clone();
            cx.spawn(async move |this, cx| {
                let download = {
                    let agent = agent.clone();
                    let url = fetched.clone();
                    async move { fetch(&agent, &url) }
                };
                let bytes = cx.background_spawn(download).await;
                let decoded = match bytes {
                    Ok(bytes) => {
                        let decode = async move { decode(&bytes, size) };
                        cx.background_spawn(decode).await
                    }
                    Err(err) => Err(err),
                };
                this.update(cx, |cache, cx| {
                    cache.in_flight = cache.in_flight.saturating_sub(1);
                    cache.pending_mut(size).remove(&url);
                    cache.store(&url, size, decoded);
                    // The slot that just freed may belong to a cover that is still waiting.
                    cache.pump(cx);
                    cache.notify_soon(cx);
                })
                .ok();
            })
            .detach();
        }
    }

    fn store(&mut self, url: &str, size: ArtSize, decoded: Result<(RenderImage, Hsla), String>) {
        let (image, bytes) = match decoded {
            Ok((image, seed)) => {
                self.seeds.entry(url.to_string()).or_insert(seed);
                (Some(Arc::new(image)), size.bytes())
            }
            Err(err) => {
                log::warn!("cover {url}: {err}");
                (None, 0)
            }
        };
        // The cover that just arrived is the freshest thing in the cache.
        let tick = self.use_tick.get() + 1;
        self.use_tick.set(tick);
        let entry = Entry {
            image,
            bytes,
            used: Cell::new(tick),
            seen: Cell::new(Instant::now()),
        };
        if let Some(old) = self.map_mut(size).insert(url.to_string(), entry) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes += bytes;
        self.trim();
    }

    /// Drops the least recently drawn entries until the cache fits.
    fn trim(&mut self) {
        while self.bytes > CACHE_BYTES || self.large.len() + self.thumbs.len() > CACHE_ENTRIES {
            let victim = [ArtSize::Large, ArtSize::Thumb]
                .into_iter()
                .filter_map(|size| self.oldest(size).map(|(url, used)| (url, size, used)))
                .min_by_key(|(_, _, used)| *used);
            let Some((url, size, _)) = victim else {
                break;
            };
            if let Some(entry) = self.map_mut(size).remove(&url) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
            if !self.large.contains_key(&url) && !self.thumbs.contains_key(&url) {
                // The seed is tiny, but a page that saw these covers is gone too.
                self.seeds.remove(&url);
            }
        }
    }

    /// The entry drawn least recently in `size`.
    fn oldest(&self, size: ArtSize) -> Option<(String, u64)> {
        self.map(size)
            .iter()
            .filter(|(_, entry)| entry.image.is_some() && entry.seen.get().elapsed() > ON_SCREEN)
            .min_by_key(|(_, entry)| entry.used.get())
            .map(|(url, entry)| (url.clone(), entry.used.get()))
    }

    /// Notifies the observers at most once per [`NOTIFY_BATCH`], however many covers land.
    fn notify_soon(&mut self, cx: &mut Context<Self>) {
        if self.notified {
            return;
        }
        self.notified = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTIFY_BATCH).await;
            this.update(cx, |cache, cx| {
                cache.notified = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn fetch(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|err| err.to_string())?;
    let bytes = response
        .body_mut()
        .read_to_vec()
        .map_err(|err| err.to_string())?;
    if bytes.is_empty() {
        return Err("la respuesta vino vacía".into());
    }
    Ok(bytes)
}

/// Center-crops to a square, resizes to the size asked for and prepares it for GPUI's sprite
/// atlas.
fn decode(bytes: &[u8], size: ArtSize) -> Result<(RenderImage, Hsla), String> {
    let image = image::load_from_memory(bytes).map_err(|err| err.to_string())?;
    let seed = seed_of(&image);
    let side = size.px();
    let crop = image.width().min(image.height()).max(1);
    let x = (image.width() - crop) / 2;
    let y = (image.height() - crop) / 2;
    let square = image
        .crop_imm(x, y, crop, crop)
        .resize_exact(side, side, FilterType::Triangle)
        .to_rgba8();
    // GPUI hands the bytes to the platform as BGRA, so swap red and blue here.
    let mut data = square.into_raw();
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(side, side, data)
        .ok_or("no se pudo armar la portada")?;
    Ok((RenderImage::new(vec![image::Frame::new(buffer)]), seed))
}

/// The most common saturated colour of the cover, softened for use as a theme seed.
fn seed_of(image: &image::DynamicImage) -> Hsla {
    let small = image.thumbnail(48, 48).to_rgba8();
    let mut buckets = [[0f32; 4]; 24];
    for pixel in small.pixels() {
        let [r, g, b, a] = pixel.0;
        if a < 128 {
            continue;
        }
        let (h, s, l) = rgb_to_hsl(r, g, b);
        if s < 0.12 || !(0.06..0.96).contains(&l) {
            continue;
        }
        let weight = s * (1. - (l - 0.5).abs() * 1.2).max(0.05);
        let bucket = &mut buckets[((h * 24.) as usize).min(23)];
        bucket[0] += h * weight;
        bucket[1] += s * weight;
        bucket[2] += l * weight;
        bucket[3] += weight;
    }
    let best = buckets
        .iter()
        .max_by(|a, b| a[3].total_cmp(&b[3]))
        .copied()
        .unwrap_or([0.; 4]);
    if best[3] <= 0.001 {
        return hsla(0.6, 0.4, 0.5, 1.);
    }
    hsla(
        best[0] / best[3],
        (best[1] / best[3]).clamp(0.3, 0.9),
        (best[2] / best[3]).clamp(0.35, 0.68),
        1.,
    )
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255., g as f32 / 255., b as f32 / 255.);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.;
    if (max - min).abs() < f32::EPSILON {
        return (0., 0., l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2. - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d + if g < b { 6. } else { 0. }) / 6.
    } else if max == g {
        ((b - r) / d + 2.) / 6.
    } else {
        ((r - g) / d + 4.) / 6.
    };
    (h, s, l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_of_primaries() {
        let (h, s, l) = rgb_to_hsl(255, 0, 0);
        assert!((h - 0.).abs() < 1e-4 && (s - 1.).abs() < 1e-4 && (l - 0.5).abs() < 1e-4);
        let (h, s, _) = rgb_to_hsl(128, 128, 128);
        assert_eq!((h, s), (0., 0.));
    }

    #[test]
    fn grey_covers_fall_back_to_a_seed() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([120, 120, 120, 255]),
        ));
        let seed = seed_of(&image);
        assert!(seed.s > 0.);
    }

    #[test]
    fn the_two_sizes_fit_their_purpose() {
        assert!(THUMB_PX <= 128);
        assert_eq!(COVER_PX, ArtSize::Large.px());
        assert_eq!(ArtSize::Thumb.bytes(), 96 * 96 * 4);
        // The byte budget holds dozens of big covers, not hundreds.
        assert!(CACHE_BYTES / ArtSize::Large.bytes() >= 32);
    }
}
