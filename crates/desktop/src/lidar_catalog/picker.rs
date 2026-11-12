use super::{
    directory::{self, Directory, Entry},
    *,
};

#[derive(Default)]
struct Listings {
    data: Option<Directory>,
    job: Option<ReadJob<Directory>>,
    error: String,
}

#[derive(Default)]
pub(super) struct Picker {
    world: Listings,
    near: Listings,
    nearby: bool,
    opened_at: Option<GeoPoint>,
    open_button: Option<egui::Id>,
    near_center: Option<GeoPoint>,
    group: Option<String>,
    place: Option<String>,
    filter: String,
    matches: Vec<usize>,
}

impl Picker {
    pub(super) fn close(&mut self) {
        self.open_button = None;
        self.opened_at = None;
        self.world.error.clear();
        self.near.error.clear();
    }
    pub(super) fn invalidate(&mut self) {
        self.world = Listings::default();
        self.near = Listings::default();
        self.clear_filter();
    }
    fn clear_filter(&mut self) {
        self.filter.clear();
        self.matches.clear();
    }

    fn menu(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        at: GeoPoint,
        selected: Option<&Survey>,
    ) -> Option<Entry> {
        let at = *self.opened_at.get_or_insert(at);
        ui.horizontal(|ui| {
            for (near, label) in [(false, "Worldwide"), (true, "At map center")] {
                if ui.selectable_label(self.nearby == near, label).clicked() && self.nearby != near
                {
                    self.nearby = near;
                    self.group = None;
                    self.place = None;
                    self.clear_filter();
                }
            }
        });
        ui.separator();
        if self.nearby && self.near_center != Some(at) {
            self.near_center = Some(at);
            self.near = Listings::default();
            self.group = None;
            self.place = None;
            self.clear_filter();
        }
        let nearby = self.nearby;
        let list = if nearby {
            &mut self.near
        } else {
            &mut self.world
        };
        if let Some(done) = completed(&mut list.job) {
            match done {
                Ok(data) => list.data = Some(data),
                Err(error) => list.error = error,
            }
        }
        let Some(groups) = &list.data else {
            if list.error.is_empty() {
                if list.job.is_none() {
                    let path = path.to_owned();
                    list.job = Some(read_in_background(ui.ctx(), move || {
                        if nearby {
                            directory::at_center(&path, at)
                        } else {
                            directory::worldwide(&path)
                        }
                    }));
                }
                ui.small("Finding surveys…");
            } else {
                ui.small(&list.error);
            }
            return None;
        };
        if groups.is_empty() {
            ui.small(if nearby {
                "No surveys cover the map center"
            } else {
                "No mapped surveys in the catalog"
            });
            return None;
        }
        let group = self
            .group
            .as_deref()
            .and_then(|key| groups.get_key_value(key));
        let Some((&group, places)) = group else {
            for &name in groups.keys() {
                if ui.button(name).clicked() {
                    self.group = Some(name.into());
                    self.place = None;
                }
            }
            return None;
        };
        let place = self
            .place
            .as_deref()
            .and_then(|key| places.get_key_value(key));
        let Some((&place, entries)) = place else {
            if ui.small_button("‹ All regions").clicked() {
                self.group = None;
            }
            ui.strong(group);
            egui::ScrollArea::vertical()
                .max_height(280.)
                .show(ui, |ui| {
                    for &name in places.keys() {
                        if ui.button(name).clicked() {
                            self.place = Some(name.into());
                            self.filter.clear();
                            self.matches.clear();
                        }
                    }
                });
            return None;
        };
        if ui.small_button(format!("‹ {group}")).clicked() {
            self.place = None;
        }
        ui.strong(place);
        if entries.len() > 12
            && ui
                .add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter surveys"))
                .changed()
        {
            let query = self.filter.trim().to_lowercase();
            self.matches = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.label.to_lowercase().contains(&query) || e.id.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect();
        }
        let filtered = !self.filter.trim().is_empty();
        let count = if filtered {
            self.matches.len()
        } else {
            entries.len()
        };
        if count == 0 {
            ui.small("No matching surveys");
        }
        let height = ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + 2. * ui.spacing().button_padding.y,
        );
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt((nearby, group, place, &self.filter))
            .max_height(280.)
            .show_rows(ui, height, count, |ui, range| {
                for i in range {
                    let entry = &entries[if filtered { self.matches[i] } else { i }];
                    let active = selected.is_some_and(|s| {
                        s.id == entry.id
                            || (s.id.starts_with("LINZ:")
                                && s.id.ends_with('/')
                                && entry.id.starts_with(&s.id))
                    });
                    if ui
                        .selectable_label(active, &entry.label)
                        .on_hover_text(&entry.label)
                        .clicked()
                    {
                        chosen = Some(entry.clone());
                        ui.close_menu();
                    }
                }
            });
        chosen
    }
}

pub(crate) fn show(ui: &mut egui::Ui, model: &mut AppModel) {
    if !model.lidar.enabled || model.active_body != ActiveBody::Earth {
        return;
    }
    ui.horizontal(|ui| {
        button(ui, model, "Lidar surveys ▾");
        let b = &model.lidar;
        if b.selection_job.is_some() {
            ui.small("Opening survey…");
        } else if !b.search_error.is_empty() {
            ui.small(&b.search_error);
        } else if let Some(s) = &b.selected {
            ui.add(egui::Label::new(&s.name).truncate())
                .on_hover_text(&s.name);
        }
    });
}

pub(super) fn button(ui: &mut egui::Ui, model: &mut AppModel, title: &'static str) {
    let at = center(model);
    let b = &mut model.lidar;
    let id = ui.make_persistent_id(title);
    let mut visible = false;
    ui.menu_button(title, |ui| {
        visible = true;
        if b.picker.open_button != Some(id) {
            b.picker.opened_at = Some(at);
            b.picker.open_button = Some(id);
        }
        ui.set_width((ui.ctx().screen_rect().width() - 24.).clamp(1., 360.));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        if ui
            .selectable_label(b.selected.is_none(), "Automatic")
            .clicked()
        {
            b.selection_job = None;
            b.selected = None;
            b.footprint = None;
            ui.close_menu();
        }
        if let Some(entry) = b.picker.menu(ui, &b.path, at, b.selected.as_ref()) {
            b.enabled = true;
            b.search_error.clear();
            if let Some(survey) = entry.local {
                b.selection_job = None;
                b.selected = Some(survey);
                b.footprint = None;
            } else {
                let path = b.path.clone();
                b.selection_job = Some(read_in_background(ui.ctx(), move || {
                    directory::open(&path, &entry.id)
                }));
            }
        }
    });
    if !visible && b.picker.open_button == Some(id) {
        b.picker.close();
    }
}
