use crate::station_replay::{
    Pick, StationId,
    waveform::{self, Request, Seismogram, Trace},
};
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, pos2, vec2};

pub(crate) fn station_click(
    painter: &egui::Painter,
    pos: Pos2,
    station: &StationId,
    arrival: Option<(&Pick, &str)>,
) {
    let ctx = painter.ctx();
    if ctx.layer_id_at(pos) == Some(painter.layer_id())
        && ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary))
    {
        let request = Request {
            station: station.clone(),
            arrival: arrival.map(|(pick, event)| (pick.clone(), event.to_owned())),
        };
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("station-seismogram-request"), Some(request)));
        ctx.request_repaint();
    }
}

pub(crate) fn render(ctx: &egui::Context, chart: &mut Seismogram) {
    let picked = ctx
        .data_mut(|d| d.remove_temp::<Option<Request>>(egui::Id::new("station-seismogram-request")))
        .flatten();
    if let Some(req) = picked {
        chart.select(req);
    }
    let now = chrono::Utc::now().timestamp_millis();
    chart.tick(ctx, now);
    if !chart.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Seismogram")
        .id(egui::Id::new("station-seismogram"))
        .open(&mut open)
        .resizable(true)
        .default_size(vec2(460., 240.))
        .min_width(300.)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let input = ui.add(
                    egui::TextEdit::singleline(&mut chart.entry)
                        .desired_width(115.)
                        .hint_text("NET.STA")
                        .char_limit(25),
                );
                let id = waveform::station(&chart.entry);
                let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui
                    .add_enabled(id.is_some(), egui::Button::new("Watch live"))
                    .clicked()
                    || enter)
                    && let Some(id) = id
                {
                    chart.live(id);
                }
                if chart.request.is_some() && ui.small_button("Reload").clicked() {
                    chart.retry();
                }
            });
            let Some(req) = &chart.request else {
                ui.small(
                    "Enter a network and station code, or right-click a station on the globe.",
                );
                return;
            };
            if let Some((pick, event)) = &req.arrival {
                ui.label(format!(
                    "{} · {} · {} at {} UTC",
                    req.station,
                    event,
                    pick.phase,
                    utc(pick.time_ms)
                ));
            } else {
                ui.label(format!("{} · live vertical motion", req.station));
            }
            if !chart.traces.is_empty() {
                egui::ComboBox::from_id_salt("seismogram-channel")
                    .selected_text(&chart.channel)
                    .show_ui(ui, |ui| {
                        for name in chart.traces.keys() {
                            if ui
                                .selectable_value(&mut chart.channel, name.clone(), name)
                                .clicked()
                            {
                                chart.channel_pinned = true;
                            }
                        }
                    });
            }
            let (from, to) = req.range(now);
            if let Some(trace) = chart
                .traces
                .get(&chart.channel)
                .filter(|t| !t.bins.is_empty())
            {
                plot(
                    ui,
                    trace,
                    from,
                    to,
                    req.arrival.as_ref().map(|(p, _)| p.time_ms),
                );
            }
            ui.small(&chart.status);
        });
    chart.open = open;
    if !open {
        chart.tick(ctx, now);
    }
}

fn utc(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .format("%H:%M:%S")
        .to_string()
}

fn plot(ui: &mut egui::Ui, trace: &Trace, from: i64, to: i64, pick: Option<i64>) {
    ui.small("Vertical amplitude (instrument counts)");
    let size = vec2(
        ui.available_width(),
        (ui.available_height() - 24.).clamp(115., 580.),
    );
    let (response, paint) = ui.allocate_painter(size, egui::Sense::hover());
    let r = response.rect;
    let axes = Rect::from_min_max(r.min + vec2(62., 8.), r.max - vec2(4., 21.));
    if axes.width() <= 1. {
        return;
    }
    let span = trace.bins.range(from.div_euclid(50)..=to.div_euclid(50));
    let (mut lo, mut hi) = span.clone().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(a, b), (_, (low, high))| (a.min(low.1), b.max(high.1)),
    );
    if !lo.is_finite() {
        return;
    }
    if lo == hi {
        lo -= 1.;
        hi += 1.;
    }
    let x = |t| axes.left() + ((t - from) as f64 / (to - from) as f64) as f32 * axes.width();
    let y = |v: f64| axes.bottom() - ((v - lo) / (hi - lo)) as f32 * axes.height();
    let ink = ui.visuals().text_color();
    let grid = Stroke::new(1_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let font = FontId::monospace(10.);
    for val in [lo, (lo + hi) * 0.5, hi] {
        let yy = y(val);
        paint.line_segment([pos2(axes.left(), yy), pos2(axes.right(), yy)], grid);
        paint.text(
            pos2(axes.left() - 4., yy),
            Align2::RIGHT_CENTER,
            format!("{val:.0}"),
            font.clone(),
            ink,
        );
    }
    for (t, align) in [
        (from, Align2::LEFT_TOP),
        ((from + to) / 2, Align2::CENTER_TOP),
        (to, Align2::RIGHT_TOP),
    ] {
        paint.text(
            pos2(x(t), axes.bottom() + 4.),
            align,
            format!("{}Z", utc(t)),
            font.clone(),
            ink,
        );
    }
    let line = Stroke::new(1.25_f32, Color32::from_rgb(91, 216, 206));
    let inside = paint.with_clip_rect(axes);
    let mut points: Vec<Pos2> = Vec::new();
    let mut last = None;
    let gap = 125_f64.max(2500. / trace.rate) as i64;
    for (_, &(a, b)) in span {
        let (first, second) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        if last.is_some_and(|t| first.0 - t > gap) && !points.is_empty() {
            inside.add(egui::Shape::line(std::mem::take(&mut points), line));
        }
        points.push(pos2(x(first.0), y(first.1)));
        if first != second {
            points.push(pos2(x(second.0), y(second.1)));
        }
        last = Some(second.0);
    }
    if !points.is_empty() {
        inside.add(egui::Shape::line(points, line));
    }
    if let Some(t) = pick {
        let xx = x(t);
        inside.line_segment(
            [pos2(xx, axes.top()), pos2(xx, axes.bottom())],
            Stroke::new(1_f32, Color32::GOLD),
        );
        inside.text(
            pos2(xx + 3., axes.top()),
            Align2::LEFT_TOP,
            "Arrival",
            font,
            Color32::GOLD,
        );
    }
}
