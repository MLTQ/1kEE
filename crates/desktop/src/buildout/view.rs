use super::detail::{detail, jurisdiction};
use super::{PROJECT_URL, Source};
use crate::model::AppModel;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
pub(super) enum Kind {
    #[default]
    Campus,
    Rule,
    News,
}

#[derive(Default)]
pub struct Browser {
    open: bool,
    pub(super) tab: Kind,
    selected: Option<String>,
    filter: String,
}

pub fn controls(ui: &mut egui::Ui, model: &mut AppModel) {
    if ui
        .checkbox(&mut model.buildout.enabled, "AI data center buildout")
        .on_hover_text("Zack’s Lab · checks daily while enabled")
        .changed()
        && let Err(error) = model.save_settings()
    {
        model.push_log(format!("Buildout setting could not be saved: {error}"));
    }
    let source = &mut model.buildout;
    if source.enabled {
        ui.small(&source.status);
        if let Some(s) = &source.snapshot {
            ui.small(format!(
                "{} campuses · {} rules · {} news",
                s.catalog.campuses.len(),
                s.catalog.rules.len(),
                s.catalog.news.len()
            ));
            ui.horizontal_wrapped(|ui| {
                for stage in [
                    "operational",
                    "construction",
                    "planned",
                    "paused",
                    "cancelled",
                ] {
                    ui.colored_label(super::markers::stage_color(stage), stage);
                }
            });
            ui.small("Filled dot · campus   Ring · approximate   Square · rule");
            ui.small(format!(
                "Last checked · {} UTC",
                crate::event_store::unix_to_datetime_str(s.checked as i64)
            ));
            if ui.button("Browse buildout").clicked() {
                source.browser.open = true;
            }
        }
        ui.hyperlink_to("Zack’s Lab", PROJECT_URL);
    }
}

impl Browser {
    pub(super) fn select(&mut self, tab: Kind, id: &str) {
        self.open = true;
        self.tab = tab;
        self.filter.clear();
        self.selected = Some(id.into());
    }
}

pub fn window(ctx: &egui::Context, source: &mut Source) {
    if !source.enabled || !source.browser.open {
        return;
    }
    let Some(snapshot) = &source.snapshot else {
        return;
    };
    let data = &snapshot.catalog;
    let b = &mut source.browser;
    let mut open = true;
    egui::Window::new("AI data center buildout")
        .open(&mut open)
        .default_size([520.0, 440.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (Kind::Campus, "Campuses"),
                    (Kind::Rule, "Rules"),
                    (Kind::News, "News"),
                ] {
                    if ui.selectable_label(b.tab == tab, label).clicked() {
                        b.tab = tab;
                        b.selected = None;
                        b.filter.clear();
                    }
                }
            });
            ui.separator();
            if b.selected.is_some() {
                if ui.small_button("← All records").clicked() {
                    b.selected = None;
                }
            } else {
                ui.add(egui::TextEdit::singleline(&mut b.filter).hint_text("Search records"));
            }
            egui::ScrollArea::vertical()
                .id_salt((b.tab, &b.selected))
                .show(ui, |ui| {
                    if let Some(id) = b.selected.clone() {
                        detail(ui, b, &id, data);
                    } else {
                        let query = b.filter.to_lowercase();
                        let mut matches = 0;
                        let mut row = |id: &str, title: &str, context: String| {
                            if !title.to_lowercase().contains(&query)
                                && !context.to_lowercase().contains(&query)
                            {
                                return;
                            }
                            matches += 1;
                            ui.vertical(|ui| {
                                if ui.link(title).clicked() {
                                    b.selected = Some(id.to_owned());
                                }
                                ui.small(context);
                            });
                            ui.separator();
                        };
                        match b.tab {
                            Kind::Campus => {
                                for c in &data.campuses {
                                    row(
                                        &c.id,
                                        &c.name,
                                        format!(
                                            "{} · {} · {} · {}",
                                            c.city, c.country, c.operator, c.status
                                        ),
                                    );
                                }
                            }
                            Kind::Rule => {
                                for r in &data.rules {
                                    row(
                                        &r.id,
                                        &r.title,
                                        format!("{} · {} · {}", jurisdiction(r), r.level, r.status),
                                    );
                                }
                            }
                            Kind::News => {
                                for n in &data.news {
                                    row(&n.id, &n.title, n.date.clone());
                                }
                            }
                        }
                        if matches == 0 {
                            ui.label("No matching records");
                        }
                    }
                });
        });
    b.open = open;
}
