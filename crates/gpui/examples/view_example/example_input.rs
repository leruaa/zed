//! The `ExampleInput` view — a single-line text input component.
//!
//! Composes `ExampleEditorText` inside a styled container with focus ring, border,
//! and action handlers. Implements the `View` trait backed by its own
//! `ExampleInputState` entity, giving it an independent caching boundary
//! from both its parent and the inner editor.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, BoxShadow, Context, CursorStyle, Entity, FocusHandle, Hsla,
    IntoViewElement, Pixels, SharedString, StyleRefinement, Subscription, ViewElement, Window,
    bounce, div, ease_in_out, hsla, point, prelude::*, px, white,
};

use crate::example_editor::ExampleEditor;
use crate::example_render_log::RenderLog;
use crate::{Backspace, Delete, End, Enter, Home, Left, Right};

pub struct ExampleInputState {
    pub editor: Entity<ExampleEditor>,
    focus_handle: FocusHandle,
    is_focused: bool,
    flash_count: usize,
    _subscriptions: Vec<Subscription>,
}

impl ExampleInputState {
    pub fn new(render_log: Entity<RenderLog>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| ExampleEditor::new(window, cx));
        editor.update(cx, |e, _cx| {
            e.render_log = Some(render_log);
        });
        let focus_handle = editor.read(cx).focus_handle.clone();

        let focus_sub = cx.on_focus(&focus_handle, window, |this, _window, cx| {
            this.is_focused = true;
            cx.notify();
        });
        let blur_sub = cx.on_blur(&focus_handle, window, |this, _window, cx| {
            this.is_focused = false;
            cx.notify();
        });

        Self {
            editor,
            focus_handle,
            is_focused: false,
            flash_count: 0,
            _subscriptions: vec![focus_sub, blur_sub],
        }
    }
}

#[derive(Hash, IntoViewElement)]
pub struct ExampleInput {
    state: Entity<ExampleInputState>,
    render_log: Entity<RenderLog>,
    width: Option<Pixels>,
    color: Option<Hsla>,
}

impl ExampleInput {
    pub fn new(state: Entity<ExampleInputState>, render_log: Entity<RenderLog>) -> Self {
        Self {
            state,
            render_log,
            width: None,
            color: None,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl gpui::View for ExampleInput {
    type Entity = ExampleInputState;

    fn entity(&self) -> Option<Entity<ExampleInputState>> {
        Some(self.state.clone())
    }

    fn cache_style(&mut self, _window: &mut Window, _cx: &mut App) -> Option<StyleRefinement> {
        let mut style = StyleRefinement::default();
        if let Some(w) = self.width {
            style.size.width = Some(w.into());
        }
        style.size.height = Some(px(36.).into());
        Some(style)
    }

    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.render_log
            .update(cx, |log, _cx| log.log("ExampleInput"));

        let input_state = self.state.read(cx);
        let count = input_state.flash_count;
        let editor = input_state.editor.clone();
        let focus_handle = input_state.focus_handle.clone();
        let is_focused = input_state.is_focused;
        let text_color = self.color.unwrap_or(hsla(0., 0., 0.1, 1.));
        let box_width = self.width.unwrap_or(px(300.));
        let state = self.state;

        let focused_border = hsla(220. / 360., 0.8, 0.5, 1.);
        let unfocused_border = hsla(0., 0., 0.75, 1.);
        let normal_border = if is_focused {
            focused_border
        } else {
            unfocused_border
        };
        let highlight_border = hsla(140. / 360., 0.8, 0.5, 1.);

        let base = div()
            .id("input")
            .key_context("TextInput")
            .track_focus(&focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action({
                let editor = editor.clone();
                move |action: &Backspace, _window, cx| {
                    editor.update(cx, |state, cx| state.backspace(action, _window, cx));
                }
            })
            .on_action({
                let editor = editor.clone();
                move |action: &Delete, _window, cx| {
                    editor.update(cx, |state, cx| state.delete(action, _window, cx));
                }
            })
            .on_action({
                let editor = editor.clone();
                move |action: &Left, _window, cx| {
                    editor.update(cx, |state, cx| state.left(action, _window, cx));
                }
            })
            .on_action({
                let editor = editor.clone();
                move |action: &Right, _window, cx| {
                    editor.update(cx, |state, cx| state.right(action, _window, cx));
                }
            })
            .on_action({
                let editor = editor.clone();
                move |action: &Home, _window, cx| {
                    editor.update(cx, |state, cx| state.home(action, _window, cx));
                }
            })
            .on_action({
                let editor = editor.clone();
                move |action: &End, _window, cx| {
                    editor.update(cx, |state, cx| state.end(action, _window, cx));
                }
            })
            .on_action({
                move |_: &Enter, _window, cx| {
                    state.update(cx, |state, cx| {
                        state.flash_count += 1;
                        cx.notify();
                    });
                }
            })
            .w(box_width)
            .h(px(36.))
            .px(px(8.))
            .bg(white())
            .border_1()
            .border_color(normal_border)
            .when(is_focused, |this| {
                this.shadow(vec![BoxShadow {
                    color: hsla(220. / 360., 0.8, 0.5, 0.3),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(4.),
                    spread_radius: px(1.),
                }])
            })
            .rounded(px(4.))
            .overflow_hidden()
            .flex()
            .items_center()
            .line_height(px(20.))
            .text_size(px(14.))
            .text_color(text_color)
            .child(ViewElement::new(editor));

        if count > 0 {
            base.with_animation(
                SharedString::from(format!("enter-bounce-{count}")),
                Animation::new(Duration::from_millis(300)).with_easing(bounce(ease_in_out)),
                move |this, delta| {
                    let h = normal_border.h + (highlight_border.h - normal_border.h) * delta;
                    let s = normal_border.s + (highlight_border.s - normal_border.s) * delta;
                    let l = normal_border.l + (highlight_border.l - normal_border.l) * delta;
                    this.border_color(hsla(h, s, l, 1.0))
                },
            )
            .into_any_element()
        } else {
            base.into_any_element()
        }
    }
}
