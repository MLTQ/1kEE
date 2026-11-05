use super::{
    catalog::{Catalog, Citation},
    view::{Browser, Kind},
};

pub(super) fn jurisdiction(r: &super::catalog::Rule) -> String {
    [
        r.locality.as_deref(),
        r.region.as_deref(),
        Some(r.country.as_str()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ")
}

fn references(ui: &mut egui::Ui, citations: &[Citation]) {
    for c in citations {
        if let Ok(url) = reqwest::Url::parse(&c.url)
            && matches!(url.scheme(), "https" | "http")
            && let Some(host) = url.host_str()
        {
            ui.hyperlink_to(host, &c.url);
        }
    }
}

pub(super) fn detail(ui: &mut egui::Ui, browser: &mut Browser, id: &str, data: &Catalog) {
    match browser.tab {
        Kind::Campus => {
            if let Some(c) = data.campuses.iter().find(|c| c.id == id) {
                ui.heading(&c.name);
                ui.label(format!("{} · {} · {}", c.operator, c.status, c.country));
                if let Some(region) = &c.region {
                    ui.label(region);
                }
                if !c.tenants.is_empty() {
                    ui.label(format!("Tenants · {}", c.tenants.join(", ")));
                }
                ui.label(format!(
                    "{} · {}-level location · {} confidence",
                    c.city, c.precision, c.confidence
                ));
                for (label, mw) in [
                    ("Operational", c.mw.operational),
                    ("Under construction", c.mw.construction),
                    ("Planned", c.mw.planned),
                ] {
                    if let Some(mw) = mw {
                        ui.label(format!("{label} · {mw} MW"));
                    }
                }
                if [c.mw.operational, c.mw.construction, c.mw.planned]
                    .iter()
                    .all(Option::is_none)
                {
                    ui.label("Capacity undisclosed");
                }
                if let Some(date) = &c.first_power {
                    ui.label(format!("First power · {date}"));
                }
                if let Some(date) = &c.full_build {
                    ui.label(format!("Full build · {date}"));
                }
                if let Some(notes) = &c.notes {
                    ui.label(notes);
                }
                publisher_link(ui, "site", &c.id);
                ui.small(format!("Verified by Zack’s Lab · {}", c.last_verified));
                references(ui, &c.sources);
                return;
            }
        }
        Kind::Rule => {
            if let Some(r) = data.rules.iter().find(|r| r.id == id) {
                ui.heading(&r.title);
                ui.label(format!("{} · {} · {}", jurisdiction(r), r.level, r.status));
                ui.label(format!("{} · severity {} · {}", r.kind, r.severity, r.date));
                if let Some(date) = &r.effective {
                    ui.label(format!("Effective · {date}"));
                }
                if let Some(date) = &r.expires {
                    ui.label(format!("Expires · {date}"));
                }
                ui.label(&r.summary);
                publisher_link(ui, "reg", &r.id);
                ui.small(format!("Verified by Zack’s Lab · {}", r.last_verified));
                references(ui, &r.sources);
                return;
            }
        }
        Kind::News => {
            if let Some(n) = data.news.iter().find(|n| n.id == id) {
                ui.heading(&n.title);
                ui.small(&n.date);
                ui.label(&n.summary);
                for site in &n.sites {
                    if let Some(c) = data.campuses.iter().find(|c| c.id == *site)
                        && ui.link(&c.name).clicked()
                    {
                        browser.select(Kind::Campus, site);
                    }
                }
                for rule in &n.rules {
                    if let Some(r) = data.rules.iter().find(|r| r.id == *rule)
                        && ui.link(&r.title).clicked()
                    {
                        browser.select(Kind::Rule, rule);
                    }
                }
                references(ui, &n.sources);
                return;
            }
        }
    }
    ui.label("This record is no longer in the current buildout.");
}

fn publisher_link(ui: &mut egui::Ui, kind: &str, id: &str) {
    let mut url = reqwest::Url::parse(super::PROJECT_URL).expect("static publisher URL");
    url.query_pairs_mut()
        .append_pair("pick", &format!("{kind}:{id}"));
    ui.hyperlink_to("View on Zack’s Lab", url.as_str());
}
