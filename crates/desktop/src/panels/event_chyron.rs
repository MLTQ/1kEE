//! Five independent, readable news crawls for Follow Events.
use crate::model::{EventRecord, EventSeverity};
use crate::theme;
use egui::{Rect, Vec2};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

const ROWS: usize = 5;
const ROW_HEIGHT: f32 = 26.0;
const PADDING: f32 = 8.0;
const SCROLL_SPEED: f64 = 42.0;
const START_HOLD: f64 = 2.0;
const END_HOLD: f64 = 2.0;
const MIN_READ_TIME: f64 = 10.0;
pub(super) const HEIGHT: f32 = ROWS as f32 * ROW_HEIGHT + PADDING * 2.0;

fn state_id() -> egui::Id {
    egui::Id::new("follow_event_chyron")
}

#[derive(Clone)]
struct Story {
    id: String,
    title: String,
    summary: String,
    severity: EventSeverity,
}

impl Story {
    fn from_event(event: &EventRecord) -> Self {
        let clean = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        Self {
            id: event.id.clone(),
            title: clean(&event.title),
            summary: clean(&event.summary),
            severity: event.severity,
        }
    }

    fn same_text(&self, other: &Self) -> bool {
        self.title == other.title
            && self.summary == other.summary
            && self.severity == other.severity
    }

    fn layout(&self, painter: &egui::Painter) -> Arc<egui::Galley> {
        let mut job = egui::text::LayoutJob {
            break_on_newline: false,
            ..Default::default()
        };
        let style = painter.ctx().style();
        let mut append = |text: &str, color| {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::proportional(15.0),
                    color,
                    ..Default::default()
                },
            );
        };
        append(&self.title, style.visuals.strong_text_color());
        append(" - ", theme::text_muted());
        append(self.severity.label(), self.severity.color());
        if !self.summary.is_empty() {
            append(" - ", theme::text_muted());
            append(
                &self.summary,
                style.visuals.strong_text_color().gamma_multiply(0.88),
            );
        }
        painter.layout_job(job)
    }
}

#[derive(Clone)]
struct Row {
    story: Story,
    started: f64,
}

impl Row {
    fn timing(&self, now: f64, text_width: f32, viewport_width: f32) -> (f32, bool) {
        let overflow = (text_width - viewport_width).max(0.0);
        let elapsed = (now - self.started).max(0.0);
        let travel = ((elapsed - START_HOLD).max(0.0) * SCROLL_SPEED).min(overflow as f64);
        let duration = (START_HOLD + overflow as f64 / SCROLL_SPEED + END_HOLD).max(MIN_READ_TIME);
        (travel as f32, elapsed >= duration)
    }
}

#[derive(Clone, Default)]
struct State {
    rows: [Option<Row>; ROWS],
    cursor: Option<String>,
    recent: VecDeque<String>,
    last_selected: Option<String>,
    retry_at: f64,
}

impl State {
    fn next(&mut self, pool: &[&EventRecord], selected: Option<&str>) -> Option<Story> {
        let candidates: Vec<_> = pool
            .iter()
            .copied()
            .filter(|event| {
                let story = Story::from_event(event);
                !self
                    .rows
                    .iter()
                    .flatten()
                    .any(|row| row.story.id == event.id || row.story.same_text(&story))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let priority = selected
            .filter(|id| self.last_selected.as_deref() != Some(*id))
            .and_then(|id| candidates.iter().find(|event| event.id == id).copied());
        let start = pool
            .iter()
            .position(|event| Some(event.id.as_str()) == self.cursor.as_deref())
            .map_or(0, |index| (index + 1) % pool.len());
        let ordered = || {
            pool.iter()
                .cycle()
                .skip(start)
                .take(pool.len())
                .copied()
                .filter(|event| candidates.iter().any(|candidate| candidate.id == event.id))
        };
        let is_priority = priority.is_some();
        let event = priority
            .or_else(|| ordered().find(|event| !self.recent.contains(&event.id)))
            .or_else(|| ordered().next())?;
        if Some(event.id.as_str()) == selected {
            self.last_selected = selected.map(str::to_owned);
        }
        // Inserting the followed story must not skip the leading visible
        // stories or reset the ordinary crawl's coverage cursor.
        if !is_priority {
            self.cursor = Some(event.id.clone());
        }
        self.recent.push_back(event.id.clone());
        if self.recent.len() > 20 {
            self.recent.pop_front();
        }
        Some(Story::from_event(event))
    }
}

/// Existing rows finish their full summary even if the camera/poll changes.
pub(super) fn draw<'a>(
    painter: &egui::Painter,
    viewport: Rect,
    events: impl Iterator<Item = &'a EventRecord>,
    selected: Option<&str>,
    visible: impl Iterator<Item = &'a str>,
) -> Option<Rect> {
    let rect = Rect::from_min_size(
        egui::pos2(viewport.left() + 12.0, viewport.bottom() - HEIGHT - 12.0),
        egui::vec2(viewport.width() - 24.0, HEIGHT),
    );
    if rect.width() < 100.0 || !viewport.contains_rect(rect) {
        return None;
    }
    let now = painter.ctx().input(|input| input.time);
    let mut state = painter
        .ctx()
        .data_mut(|data| data.remove_temp::<State>(state_id()).unwrap_or_default());
    if state
        .rows
        .iter()
        .flatten()
        .any(|row| Some(row.story.id.as_str()) == selected)
    {
        state.last_selected = selected.map(str::to_owned);
    }
    let width = rect.width() - PADDING * 2.0 - 16.0;
    let mut galleys: [Option<Arc<egui::Galley>>; ROWS] = Default::default();
    for (index, row) in state.rows.iter_mut().enumerate() {
        if let Some(active) = row {
            let galley = active.story.layout(painter);
            if active.timing(now, galley.size().x, width).1 {
                *row = None;
            } else {
                galleys[index] = Some(galley);
            }
        }
    }
    if state.rows.iter().any(Option::is_none) && now >= state.retry_at {
        state.retry_at = now + 1.0;
        let visible: HashSet<_> = visible.collect();
        let mut ids = HashSet::new();
        let mut pool: Vec<_> = events
            .filter(|event| {
                event.id.starts_with("factal-")
                    && event.factal_brief.is_some()
                    && ids.insert(event.id.as_str())
            })
            .collect();
        pool.sort_by(|a, b| {
            visible
                .contains(b.id.as_str())
                .cmp(&visible.contains(a.id.as_str()))
                .then_with(|| priority(b).cmp(&priority(a)))
                .then_with(|| b.occurred_at.cmp(&a.occurred_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        for (index, galley) in galleys.iter_mut().enumerate() {
            if state.rows[index].is_none() {
                let Some(story) = state.next(&pool, selected) else {
                    break;
                };
                *galley = Some(story.layout(painter));
                state.rows[index] = Some(Row {
                    story,
                    started: now,
                });
            }
        }
    }
    if state.rows.iter().all(Option::is_none) {
        return None;
    }
    let painter = painter.with_clip_rect(viewport);
    painter.rect(
        rect,
        8.0,
        theme::panel_fill(248),
        egui::Stroke::new(1.0, theme::panel_stroke()),
        egui::StrokeKind::Inside,
    );
    for (index, (row, galley)) in state.rows.iter().zip(galleys).enumerate() {
        let Some((row, galley)) = row.as_ref().zip(galley) else {
            continue;
        };
        let line = Rect::from_min_size(
            rect.min + egui::vec2(PADDING, PADDING + index as f32 * ROW_HEIGHT),
            egui::vec2(rect.width() - PADDING * 2.0, ROW_HEIGHT),
        );
        if selected == Some(row.story.id.as_str()) {
            painter.rect_filled(line, 3.0, row.story.severity.color().gamma_multiply(0.10));
        }
        painter.line_segment(
            [
                line.left_top() + egui::vec2(1.5, 5.0),
                line.left_bottom() + egui::vec2(1.5, -5.0),
            ],
            egui::Stroke::new(3.0, row.story.severity.color()),
        );
        let text_rect = line.shrink2(Vec2::new(8.0, 0.0));
        let offset = row.timing(now, galley.size().x, text_rect.width()).0;
        painter.with_clip_rect(text_rect).galley(
            egui::pos2(
                text_rect.left() - offset,
                text_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            painter.ctx().style().visuals.text_color(),
        );
    }
    painter
        .ctx()
        .data_mut(|data| data.insert_temp(state_id(), state));
    painter
        .ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    Some(rect)
}

fn priority(event: &EventRecord) -> i64 {
    event
        .factal_brief
        .as_ref()
        .and_then(|brief| brief.severity_value)
        .unwrap_or(match event.severity {
            EventSeverity::Critical => 4,
            EventSeverity::Elevated => 2,
            EventSeverity::Advisory => 1,
        })
}

pub(super) fn clear(ctx: &egui::Context) {
    ctx.data_mut(|data| data.remove::<State>(state_id()));
}

#[cfg(test)]
#[path = "event_chyron_tests.rs"]
mod tests;
