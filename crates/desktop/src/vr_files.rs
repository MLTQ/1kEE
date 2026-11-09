//! Controller-only browsing of local layers. Directory I/O stays off the UI thread.
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

type DirectoryEntries = std::io::Result<Vec<(bool, PathBuf)>>;

#[derive(Default)]
pub(super) struct Browser {
    directory: Option<PathBuf>,
    entries: Vec<(bool, PathBuf)>,
    pending: Option<mpsc::Receiver<DirectoryEntries>>,
    error: Option<String>,
    page: usize,
}
fn entries(path: &Path) -> DirectoryEntries {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        let is_dir = kind.is_dir() || (kind.is_symlink() && path.is_dir());
        if is_dir
            || path
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|ext| {
                    ["geojson", "json", "kml", "kmz", "tif", "tiff"]
                        .iter()
                        .any(|v| ext.eq_ignore_ascii_case(v))
                })
        {
            files.push((is_dir, path));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(files)
}
impl Browser {
    fn open(&mut self, directory: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.entries.clear();
        self.page = 0;
        self.error = None;
        self.directory = Some(directory.clone());
        match std::thread::Builder::new()
            .name("vr-layer-files".into())
            .spawn(move || {
                let _ = tx.send(entries(&directory));
            }) {
            Ok(_) => self.pending = Some(rx),
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, root: Option<&Path>) -> Option<PathBuf> {
        let Some(directory) = self.directory.clone() else {
            if ui.button("Browse files on wrist").clicked() {
                self.open(
                    root.map(Path::to_owned)
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
                );
            }
            return None;
        };
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(list) => self.entries = list,
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("Directory reader stopped".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        ui.horizontal(|ui| {
            if let Some(up) = directory.parent()
                && ui.button("Up").clicked()
            {
                self.open(up.to_owned());
            }
            if ui.button("Close browser").clicked() {
                *self = Self::default();
            }
        });
        if self.directory.as_ref() != Some(&directory) {
            return None;
        }
        ui.label(directory.display().to_string());
        if self.pending.is_some() {
            ui.label("Reading folder…");
        }
        if let Some(err) = &self.error {
            ui.label(err);
        }
        if self.pending.is_none() && self.error.is_none() && self.entries.is_empty() {
            ui.label("No folders or supported layers here.");
        }
        const PAGE: usize = 12;
        let mut selected = None;
        for (is_dir, path) in self.entries.iter().skip(self.page * PAGE).take(PAGE) {
            let name = path.file_name()?.to_string_lossy();
            if ui
                .button(if *is_dir {
                    format!("{name}/")
                } else {
                    name.into_owned()
                })
                .clicked()
            {
                selected = Some((*is_dir, path.clone()));
            }
        }
        if self.entries.len() > PAGE {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.page > 0, egui::Button::new("Previous"))
                    .clicked()
                {
                    self.page -= 1;
                }
                ui.label(format!(
                    "{} / {}",
                    self.page + 1,
                    self.entries.len().div_ceil(PAGE)
                ));
                if ui
                    .add_enabled(
                        (self.page + 1) * PAGE < self.entries.len(),
                        egui::Button::new("Next"),
                    )
                    .clicked()
                {
                    self.page += 1;
                }
            });
        }
        if let Some((folder, path)) = selected {
            if folder {
                self.open(path);
            } else {
                *self = Self::default();
                return Some(path);
            }
        }
        None
    }
}
