//! Settings cache status uses the renderer's read-only cache/recovery rules.
use crate::panels::world_map::{
    layer_snapshot::Snapshot,
    srtm_focus_cache::diagnostics::{self, Cache, Original, Recovery},
};
use crate::theme;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    root: Option<PathBuf>,
    generation: u64,
}
#[derive(Default)]
struct State {
    snapshot: Snapshot<Key, Vec<Cache>>,
    refresh: Option<Instant>,
    generation: u64,
}
impl State {
    fn key(&mut self, root: Option<&std::path::Path>) -> Key {
        // Do not invalidate a slow scan each time the refresh interval passes.
        if self.snapshot.loading_state() != (0, 1, 0)
            && self
                .refresh
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
        {
            self.generation = self.generation.wrapping_add(1);
            self.refresh = Some(Instant::now());
        }
        Key {
            root: root.map(ToOwned::to_owned),
            generation: self.generation,
        }
    }
}
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<State>>);

pub(super) fn render(ui: &mut egui::Ui, root: Option<&std::path::Path>) {
    let shared = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Shared>(egui::Id::new("settings-contour-cache-status"))
            .clone()
    });
    let (result, loading) = {
        let mut state = shared.0.lock().unwrap();
        let key = state.key(root);
        let result = state.snapshot.get(
            key,
            "settings-contour-cache",
            |a, b| a.root == b.root,
            |key| Some(diagnostics::load(key.root.as_deref())),
        );
        (result, state.snapshot.loading_state() != (1, 0, 0))
    };
    ui.ctx().request_repaint_after(if loading {
        Duration::from_millis(150)
    } else {
        Duration::from_secs(5)
    });
    if let Some(caches) = result {
        for cache in caches.iter() {
            paint_cache(ui, cache);
        }
        ui.collapsing("Contour cache locations and details", |ui| {
            ui.small("Original database totals exclude separately stored recovered tiles. Recovery checks are bounded; they do not verify every tile.");
            for cache in caches.iter() {
                ui.strong(cache.label);
                ui.label(format!("Original: {}", cache.path.display()));
                ui.label(format!("Recovery: {}", cache.recovery_root.display()));
                if let Original::Ready(levels) = &cache.original {
                    ui.small(levels.iter().map(|l| format!("z{}: {} tiles / {} lines",l.zoom,l.tiles,l.lines)).collect::<Vec<_>>().join(" · "));
                }
                match &cache.recovery {
                    Recovery::Available {sample} => {ui.small(format!("Readable recovered tile manifest: {}",sample.display()));}
                    Recovery::Unreadable(error) => {ui.label(format!("Recovery storage could not be read: {error}"));}
                    _ => {}
                }
            }
        });
        if loading {
            ui.small("Refreshing contour cache status…");
        }
    } else {
        ui.colored_label(theme::text_muted(), "Checking contour caches…");
    }
}

fn paint_cache(ui: &mut egui::Ui, cache: &Cache) {
    let good = egui::Color32::from_rgb(80, 200, 100);
    let warn = egui::Color32::from_rgb(220, 160, 40);
    let recovered = matches!(cache.recovery, Recovery::Available { .. });
    let (text, color) = match &cache.original {
        Original::Ready(levels) => {
            let tiles: i64 = levels.iter().map(|l| l.tiles).sum();
            let lines: i64 = levels.iter().map(|l| l.lines).sum();
            (
                format!(
                    "{}: {tiles} original tiles · {lines} contour lines{}",
                    cache.label,
                    if recovered {
                        " · recovered tiles also available"
                    } else {
                        ""
                    }
                ),
                if tiles > 0 || recovered {
                    good
                } else {
                    theme::text_muted()
                },
            )
        }
        _ if recovered => (format!("{}: recovered tiles available", cache.label), good),
        Original::Missing => (
            format!("{}: no original cache yet", cache.label),
            theme::text_muted(),
        ),
        Original::Unreadable(_) => (
            format!("{}: original database unreadable", cache.label),
            warn,
        ),
    };
    ui.colored_label(color, text);
    if let Original::Unreadable(error) = &cache.original {
        ui.colored_label(warn, format!("Original cache totals unavailable: {error}"));
    }
    if matches!(cache.recovery, Recovery::Present) {
        ui.colored_label(
            theme::text_muted(),
            "Recovery storage found; no completed tile verified in this check.",
        );
    }
    if let Recovery::Unreadable(error) = &cache.recovery {
        ui.colored_label(warn, format!("Recovery storage could not be read: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contour_cache_status_refresh_does_not_discard_a_slow_scan() {
        let mut state = State::default();
        let key = state.key(Some(std::path::Path::new("first-drive")));
        let (release, wait) = std::sync::mpsc::channel();
        state.snapshot.get(
            key.clone(),
            "slow-cache-status-test",
            |a, b| a.root == b.root,
            move |_| {
                wait.recv().unwrap();
                Some(vec![])
            },
        );
        state.refresh = Some(Instant::now() - Duration::from_secs(10));
        assert_eq!(state.key(Some(std::path::Path::new("first-drive"))), key);
        assert_ne!(state.key(Some(std::path::Path::new("second-drive"))), key);
        release.send(()).unwrap();
    }
    fn text(cache: &Cache) -> String {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| paint_cache(ui, cache));
        });
        output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn contour_cache_status_distinguishes_recovery_errors_and_genuine_empty_tiles() {
        let mut cache = Cache {
            label: "SRTM contours",
            path: "base.sqlite".into(),
            original: Original::Unreadable("database disk image is malformed".into()),
            recovery_root: "repairs".into(),
            recovery: Recovery::Available {
                sample: "repairs/tile.sqlite".into(),
            },
        };
        let painted = text(&cache);
        assert!(painted.contains("recovered tiles available"));
        assert!(painted.contains("totals unavailable: database disk image is malformed"));
        assert!(!painted.contains("0 original tiles"));
        assert!(!painted.contains("wrong file"));
        cache.original = Original::Ready(vec![diagnostics::Level {
            zoom: 0,
            tiles: 1,
            lines: 0,
        }]);
        cache.recovery = Recovery::Absent;
        let empty = text(&cache);
        assert!(empty.contains("1 original tiles · 0 contour lines"));
        assert!(!empty.contains("failed") && !empty.contains("unreadable"));
    }
}
