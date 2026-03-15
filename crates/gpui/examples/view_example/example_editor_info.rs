//! `EditorInfo` — a read-only View over an `ExampleEditor` entity.
//!
//! Demonstrates zero-wiring reactivity: just return the entity from `entity()`,
//! read from it in `render()`, and caching + invalidation happen automatically.
//! No observers, no subscriptions, no manual `cx.notify()`.

use gpui::{App, Entity, IntoViewElement, Window, div, hsla, prelude::*, px};

use crate::example_editor::ExampleEditor;
use crate::example_render_log::RenderLog;

#[derive(Hash, IntoViewElement)]
pub struct EditorInfo {
    editor: Entity<ExampleEditor>,
    render_log: Entity<RenderLog>,
}

impl EditorInfo {
    pub fn new(editor: Entity<ExampleEditor>, render_log: Entity<RenderLog>) -> Self {
        Self { editor, render_log }
    }
}

impl gpui::View for EditorInfo {
    type Entity = ExampleEditor;

    fn entity(&self) -> Option<Entity<ExampleEditor>> {
        Some(self.editor.clone())
    }

    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.render_log.update(cx, |log, _cx| log.log("EditorInfo"));

        let editor = self.editor.read(cx);
        let char_count = editor.content.len();
        let cursor = editor.cursor;
        let is_focused = editor.focus_handle.is_focused(window);

        div()
            .flex()
            .gap(px(8.))
            .text_xs()
            .text_color(hsla(0., 0., 0.45, 1.))
            .child(format!("{char_count} chars"))
            .child("·")
            .child(format!("cursor {cursor}"))
            .child("·")
            .child(if is_focused { "focused" } else { "unfocused" })
    }
}
