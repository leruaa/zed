#![cfg_attr(target_family = "wasm", no_main)]

//! **view_example** — an end-to-end GPUI example demonstrating how Entity,
//! Element, View, and Render compose together to build rich text components.
//!
//! ## Architecture
//!
//! Each module has a focused job:
//!
//! | Module          | Trait         | Job                                                    |
//! |-----------------|---------------|--------------------------------------------------------|
//! | `editor`        | EntityView    | Owns text, cursor, blink; renders via ExampleEditorText |
//! | `input`         | View          | Single-line input with own state + cached editor child  |
//! | `editor_info`   | View          | Read-only stats display; zero-wiring, same editor entity |
//! | `text_area`     | ComponentView | Stateless multi-line wrapper; inner editor caches       |
//! | `main` (here)   | Render        | Root view; creates entities with `use_state`, assembles  |
//!
//! ## Running
//!
//! ```sh
//! cargo run --example view_example -p gpui
//! ```
//!
//! ## Testing
//!
//! ```sh
//! cargo test --example view_example -p gpui
//! ```

mod example_editor;
mod example_editor_info;
mod example_input;
mod example_render_log;
mod example_text_area;

#[cfg(test)]
mod example_tests;

use gpui::{
    App, Bounds, Context, Hsla, KeyBinding, Window, WindowBounds, WindowOptions, actions, div,
    hsla, prelude::*, px, rgb, size,
};
use gpui_platform::application;

use example_editor::ExampleEditor;
use example_editor_info::EditorInfo;
use example_input::{ExampleInput, ExampleInputState};
use example_render_log::{RenderLog, RenderLogPanel};
use example_text_area::ExampleTextArea;

actions!(
    view_example,
    [Backspace, Delete, Left, Right, Home, End, Enter, Quit,]
);

// ---------------------------------------------------------------------------
// ViewExample — the root view using `Render` and `window.use_state()`
// ---------------------------------------------------------------------------

struct ViewExample {
    input_color: Hsla,
    textarea_color: Hsla,
}

impl ViewExample {
    fn new() -> Self {
        Self {
            input_color: hsla(0., 0., 0.1, 1.),
            textarea_color: hsla(250. / 360., 0.7, 0.4, 1.),
        }
    }
}

impl Render for ViewExample {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render_log = window.use_state(cx, |_window, cx| RenderLog::new(cx));
        let input_state = window.use_state(cx, {
            let render_log = render_log.clone();
            move |window, cx| ExampleInputState::new(render_log, window, cx)
        });
        let input_editor = input_state.read(cx).editor.clone();
        let textarea_editor = window.use_state(cx, |window, cx| ExampleEditor::new(window, cx));
        textarea_editor.update(cx, |e, _cx| {
            if e.render_log.is_none() {
                e.render_log = Some(render_log.clone());
            }
        });
        let input_color = self.input_color;
        let textarea_color = self.textarea_color;

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf0f0f0))
            .p(px(24.))
            .gap(px(20.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(hsla(0., 0., 0.3, 1.))
                            .child("Single-line input (View with own state + cached editor)"),
                    )
                    .child(
                        ExampleInput::new(input_state, render_log.clone())
                            .width(px(320.))
                            .color(input_color),
                    )
                    .child(EditorInfo::new(input_editor, render_log.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_sm().text_color(hsla(0., 0., 0.3, 1.)).child(
                        "Multi-line text area (TextArea — same entity type, different View)",
                    ))
                    .child(
                        ExampleTextArea::new(textarea_editor, render_log.clone(), 5)
                            .color(textarea_color),
                    ),
            )
            .child(RenderLogPanel::new(render_log))
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn run_example() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.0), px(500.0)), cx);
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("enter", Enter, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| ViewExample::new()),
        )
        .unwrap();

        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.activate(true);
    });
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    run_example();
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    gpui_platform::web_init();
    run_example();
}
