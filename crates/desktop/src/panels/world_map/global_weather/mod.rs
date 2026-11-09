//! Automatically refreshed global weather imagery for the desktop Earth scenes.
mod cache;
mod provider;
#[cfg(test)]
mod tests;

use super::satellite_layer::SatelliteLayer;
use crate::{
    model::{ActiveBody, AppModel},
    satellite_image::SatelliteImage,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Product {
    TrueColor,
    Infrared,
}

impl Product {
    fn id(self) -> &'static str {
        match self {
            Self::TrueColor => "viirs-noaa20",
            Self::Infrared => "eumetsat-ring-ir108",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::TrueColor => "Daily true color",
            Self::Infrared => "Three-hourly infrared",
        }
    }
    fn recheck(self) -> Duration {
        Duration::from_secs(match self {
            Self::TrueColor => 3600,
            Self::Infrared => 900,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct LayerOptions {
    enabled: bool,
    opacity: u8,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    true_color: LayerOptions,
    infrared: LayerOptions,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            true_color: LayerOptions {
                enabled: false,
                opacity: 100,
            },
            infrared: LayerOptions {
                enabled: false,
                opacity: 55,
            },
        }
    }
}

enum Message {
    Frame {
        stamp: String,
        image: SatelliteImage,
        cache_warning: Option<String>,
    },
    Done(Result<(), String>),
}

struct Feed {
    product: Product,
    options: LayerOptions,
    stamp: Option<String>,
    image: Option<SatelliteLayer>,
    status: String,
    cache_warning: Option<String>,
    worker: Option<mpsc::Receiver<Message>>,
    due: Instant,
}

impl Feed {
    fn new(product: Product, options: LayerOptions) -> Self {
        Self {
            product,
            options,
            stamp: None,
            image: None,
            status: String::new(),
            cache_warning: None,
            worker: None,
            due: Instant::now(),
        }
    }

    fn finish(&mut self, result: Result<(), String>) {
        self.worker = None;
        let delay = match result {
            Ok(()) => {
                self.status.clear();
                self.product.recheck()
            }
            Err(error) => {
                self.status = format!("{error} · retry in 5 min");
                Duration::from_secs(300)
            }
        };
        self.due = Instant::now() + delay;
    }

    fn tick(&mut self, ctx: &egui::Context, earth: bool, cache_path: &Path) {
        while let Some(rx) = &self.worker {
            match rx.try_recv() {
                Ok(Message::Frame {
                    stamp,
                    image,
                    cache_warning,
                }) => {
                    self.image = Some(SatelliteLayer::new(ctx, self.product.label().into(), image));
                    self.stamp = Some(stamp);
                    self.cache_warning = cache_warning;
                }
                Ok(Message::Done(result)) => self.finish(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.finish(Err("Imagery worker stopped".into()))
                }
            }
        }
        if let Some(image) = &mut self.image {
            image.set_opacity(self.options.opacity);
        }
        if !earth || !self.options.enabled || self.worker.is_some() || Instant::now() < self.due {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let product = self.product;
        let previous = self.stamp.clone();
        let path = cache_path.to_owned();
        let max_side = ctx.input(|i| i.max_texture_side);
        let wake = ctx.clone();
        self.worker = Some(rx);
        self.status = "Checking for updates…".into();
        if let Err(e) = std::thread::Builder::new()
            .name(product.id().into())
            .spawn(move || {
                let result = update(product, previous, &path, max_side, |message| {
                    let _ = tx.send(message);
                    wake.request_repaint();
                });
                let _ = tx.send(Message::Done(result));
                wake.request_repaint();
            })
        {
            self.finish(Err(e.to_string()));
        }
    }
}

fn update(
    product: Product,
    mut previous: Option<String>,
    path: &Path,
    max_side: usize,
    mut publish: impl FnMut(Message),
) -> Result<(), String> {
    if previous.is_none()
        && let Some((stamp, bytes)) = cache::read(path, product)
        && let Ok(image) = provider::decode(product, &bytes, max_side)
    {
        previous = Some(stamp.clone());
        publish(Message::Frame {
            stamp,
            image,
            cache_warning: None,
        });
    }
    let client = provider::client()?;
    let stamp = provider::latest(&client, product)?;
    if previous.as_ref().is_some_and(|old| old >= &stamp) {
        return Ok(());
    }
    let png = provider::fetch_png(&client, product, &stamp)?;
    let image = provider::decode(product, &png, max_side)?;
    let cache_warning = cache::write(path, product, &stamp, &png)
        .err()
        .map(|e| format!("Image available for this session only: {e}"));
    publish(Message::Frame {
        stamp,
        image,
        cache_warning,
    });
    Ok(())
}

pub(crate) struct GlobalWeather {
    feeds: [Feed; 2],
    cache_path: PathBuf,
}

pub(crate) fn cache_path(data_root: Option<&str>, asset_root: Option<&Path>) -> PathBuf {
    data_root
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| asset_root.unwrap_or_else(|| Path::new(".")).join("Data"))
        .join("satellite/global-weather.sqlite")
}

impl GlobalWeather {
    pub(crate) fn new(settings: Settings, cache_path: PathBuf) -> Self {
        Self {
            cache_path,
            feeds: [
                Feed::new(Product::TrueColor, settings.true_color),
                Feed::new(Product::Infrared, settings.infrared),
            ],
        }
    }
    pub(crate) fn settings(&self) -> Settings {
        Settings {
            true_color: self.feeds[0].options,
            infrared: self.feeds[1].options,
        }
    }
    pub(crate) fn set_settings(&mut self, settings: Settings) {
        self.feeds[0].options = settings.true_color;
        self.feeds[1].options = settings.infrared;
    }
    pub(crate) fn set_cache_path(&mut self, path: PathBuf) {
        if self.cache_path != path {
            *self = Self::new(self.settings(), path);
        }
    }
    pub(crate) fn tick(&mut self, ctx: &egui::Context, body: ActiveBody) {
        for feed in &mut self.feeds {
            feed.tick(ctx, body == ActiveBody::Earth, &self.cache_path);
        }
    }
    pub(super) fn images(&self) -> impl Iterator<Item = &SatelliteLayer> {
        self.feeds
            .iter()
            .filter(|f| f.options.enabled)
            .filter_map(|f| f.image.as_ref())
    }
}

pub(crate) fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    let mut save = false;
    ui.add_enabled_ui(model.active_body == ActiveBody::Earth, |ui| {
        for feed in &mut model.global_weather.feeds {
            ui.push_id(feed.product.id(), |ui| {
                save |= ui
                    .checkbox(&mut feed.options.enabled, feed.product.label())
                    .changed();
                if !feed.options.enabled {
                    return;
                }
                let attribution = match feed.product {
                    Product::TrueColor => "NASA GIBS · NOAA-20 VIIRS",
                    Product::Infrared => "EUMETSAT · Geostationary Ring",
                };
                ui.small(attribution);
                if let Some(stamp) = &feed.stamp {
                    ui.small(format!(
                        "Showing {} UTC",
                        stamp.trim_end_matches('Z').replace('T', " ")
                    ));
                }
                if let Some(warning) = &feed.cache_warning {
                    ui.small(warning);
                }
                if !feed.status.is_empty() {
                    ui.small(&feed.status);
                }
                let slider = ui.add(
                    egui::Slider::new(&mut feed.options.opacity, 0..=100)
                        .suffix("%")
                        .text("Opacity"),
                );
                save |= slider.drag_stopped() || (slider.changed() && !slider.dragged());
                if ui
                    .add_enabled(
                        feed.worker.is_none(),
                        egui::Button::new("Check now").small(),
                    )
                    .clicked()
                {
                    feed.due = Instant::now();
                }
            });
        }
    })
    .response
    .on_disabled_hover_text("Earth imagery");
    if save && let Err(e) = model.save_settings() {
        model.push_log(format!("Weather layer settings: {e}"));
    }
}
