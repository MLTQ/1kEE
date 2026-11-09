use super::{Feed, Product, cache, provider, window::Window};
use crate::{panels::world_map::satellite_layer::SatelliteLayer, satellite_image::SatelliteImage};
use std::{
    collections::VecDeque,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) fn fetch(
    product: Product,
    stamp: &str,
    window: Window,
    cache_path: &Path,
) -> Result<SatelliteImage, String> {
    let client = provider::client()?;
    let [w, h] = window.size();
    let mut pixels = image::RgbaImage::new(w, h);
    for (offset, bounds, size) in window.segments() {
        let key = format!("{}|{stamp}|{bounds:?}|{size:?}", product.id());
        let cached = cache::detail_png(cache_path, &key)
            .and_then(|png| provider::decode_png(&png, size).ok());
        let part = match cached {
            Some(image) => image,
            None => {
                let png = provider::fetch_map(&client, product, stamp, bounds, size)?;
                let image = provider::decode_png(&png, size)?;
                // Read-only or full storage still allows on-screen refinement.
                let _ = cache::store_detail(cache_path, &key, &png);
                image
            }
        }
        .into_rgba8();
        image::imageops::replace(&mut pixels, &part, i64::from(offset), 0);
    }
    // All-transparent polar windows are valid for infrared.
    Ok(SatelliteImage {
        bounds: window.bounds(),
        pixels: egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], pixels.as_raw()),
    })
}

type Worker = (
    String,
    Window,
    mpsc::Receiver<Result<SatelliteImage, String>>,
);

#[derive(Default)]
pub(super) struct Detail {
    stamp: Option<String>,
    loaded: VecDeque<(Window, SatelliteLayer)>,
    target: Option<(Window, Instant)>,
    worker: Option<Worker>,
    failure: Option<(Window, Instant, String)>,
    pub status: String,
}

impl Detail {
    fn poll(&mut self, ctx: &egui::Context, product: Product) {
        let Some((stamp, window, rx)) = &self.worker else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Detail worker stopped".into()),
        };
        if self.stamp.as_ref() != Some(stamp) {
            self.worker = None;
            return;
        }
        match result {
            Ok(image) => {
                self.loaded.push_front((
                    *window,
                    SatelliteLayer::new(ctx, product.label().into(), image),
                ));
                self.loaded.truncate(3);
                self.failure = None;
            }
            Err(e) => self.failure = Some((*window, Instant::now() + Duration::from_secs(60), e)),
        }
        self.worker = None;
    }

    pub fn select(
        &mut self,
        ctx: &egui::Context,
        feed: &Feed,
        stamp: &str,
        cache_path: &Path,
        visible: Option<[f32; 4]>,
        display_pixels: f32,
    ) -> Option<&SatelliteLayer> {
        let product = feed.product;
        if self.stamp.as_deref() != Some(stamp) {
            self.stamp = Some(stamp.to_owned());
            self.loaded.clear();
            self.target = None;
            self.failure = None;
        }
        self.poll(ctx, product);
        self.status.clear();
        let Some((visible, planned)) = visible.and_then(|b| {
            Window::plan(
                product,
                b,
                display_pixels,
                ctx.input(|i| i.max_texture_side),
            )
            .map(|plan| (b, plan))
        }) else {
            self.target = None;
            return None;
        };
        let now = Instant::now();
        let (wanted, since) = *self.target.get_or_insert((planned, now));
        let (wanted, since) = if wanted.columns == planned.columns && wanted.contains(visible) {
            (wanted, since)
        } else {
            self.target = Some((planned, now));
            (planned, now)
        };
        let best = self
            .loaded
            .iter()
            .enumerate()
            .filter(|(_, (window, _))| window.contains(visible))
            .max_by_key(|(_, (window, _))| window.columns)
            .map(|(i, _)| i);
        if best.is_none_or(|i| self.loaded[i].0.columns < wanted.columns) {
            self.request(ctx, product, stamp, cache_path, (wanted, since));
        }
        let (window, layer) = &mut self.loaded[best?];
        if self.status.is_empty() {
            self.status = format!(
                "Detail · {:.2} km/pixel at equator",
                40075.0 / window.columns as f64
            );
        }
        layer.set_opacity(feed.options.opacity);
        Some(layer)
    }

    fn request(
        &mut self,
        ctx: &egui::Context,
        product: Product,
        stamp: &str,
        cache_path: &Path,
        (wanted, since): (Window, Instant),
    ) {
        let now = Instant::now();
        if let Some((_, _, error)) = self
            .failure
            .as_ref()
            .filter(|(w, retry, _)| *w == wanted && now < *retry)
        {
            self.status = format!("Detail unavailable · {error}");
            return;
        }
        self.status.push_str("Loading finer imagery…");
        if self.worker.is_some() {
            return;
        }
        let wait = Duration::from_millis(250).saturating_sub(now - since);
        if !wait.is_zero() {
            ctx.request_repaint_after(wait);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        let requested = stamp.to_owned();
        let path = cache_path.to_owned();
        match std::thread::Builder::new()
            .name("weather-detail".into())
            .spawn(move || {
                let _ = tx.send(fetch(product, &requested, wanted, &path));
                wake.request_repaint();
            }) {
            Ok(_) => self.worker = Some((stamp.to_owned(), wanted, rx)),
            Err(e) => self.failure = Some((wanted, now + Duration::from_secs(60), e.to_string())),
        }
    }
}
