//! `RenderLog` — a diagnostic panel that records which components re-render
//! and when, letting you observe GPUI's caching behaviour in real time.

use std::time::Instant;

use gpui::{App, Context, Entity, IntoViewElement, Window, div, hsla, prelude::*, px};

// ---------------------------------------------------------------------------
// RenderLog entity
// ---------------------------------------------------------------------------

pub struct RenderLog {
    entries: Vec<RenderLogEntry>,
    start_time: Instant,
}

struct RenderLogEntry {
    component: &'static str,
    timestamp: Instant,
}

impl RenderLog {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            entries: Vec::new(),
            start_time: Instant::now(),
        }
    }

    /// Record that `component` rendered. Does **not** call `cx.notify()` — the
    /// panel updates passively the next time its parent re-renders, which avoids
    /// an infinite invalidation loop.
    pub fn log(&mut self, component: &'static str) {
        self.entries.push(RenderLogEntry {
            component,
            timestamp: Instant::now(),
        });
        if self.entries.len() > 50 {
            self.entries.drain(0..self.entries.len() - 50);
        }
    }
}

// ---------------------------------------------------------------------------
// RenderLogPanel — stateless ComponentView that displays the log
// ---------------------------------------------------------------------------

#[derive(Hash, IntoViewElement)]
pub struct RenderLogPanel {
    log: Entity<RenderLog>,
}

impl RenderLogPanel {
    pub fn new(log: Entity<RenderLog>) -> Self {
        Self { log }
    }
}

impl gpui::ComponentView for RenderLogPanel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let log = self.log.read(cx);
        let start = log.start_time;

        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(8.))
            .bg(hsla(0., 0., 0.12, 1.))
            .rounded(px(4.))
            .max_h(px(180.))
            .overflow_hidden()
            .child(
                div()
                    .text_xs()
                    .text_color(hsla(0., 0., 0.55, 1.))
                    .mb(px(4.))
                    .child("Render log (most recent 20)"),
            )
            .children(
                log.entries
                    .iter()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(|entry| {
                        let elapsed = entry.timestamp.duration_since(start);
                        let secs = elapsed.as_secs_f64();
                        div()
                            .text_xs()
                            .text_color(hsla(120. / 360., 0.7, 0.65, 1.))
                            .child(format!("{:<20} +{:.1}s", entry.component, secs))
                    }),
            )
    }
}
