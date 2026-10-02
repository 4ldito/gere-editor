//! Visual primitives shared by Git and historical OpenCode side-by-side diffs.
use gpui::{div, prelude::*, px, rgb, App, MouseButton, MouseDownEvent, Window};

pub(super) fn split_panes(
    left: impl IntoElement,
    right: impl IntoElement,
    dragging: bool,
    on_drag: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    div()
        .flex()
        .w_full()
        .h_full()
        .child(left)
        .child(
            div()
                .w(px(6.))
                .h_full()
                .bg(rgb(if dragging { 0x61afef } else { 0x3e4451 }))
                .cursor(gpui::CursorStyle::ResizeColumn)
                .hover(|style| style.bg(rgb(0x61afef)))
                .on_mouse_down(MouseButton::Left, on_drag),
        )
        .child(right)
}

pub(super) fn ghost(gere: bool) -> u32 {
    if gere {
        0x1b222b
    } else {
        0x30363c
    }
}

pub(super) fn pane_background(gere: bool, background: u32, changed: bool, original: bool) -> u32 {
    if changed {
        if original {
            if gere {
                0x302126
            } else {
                0x3b292c
            }
        } else if gere {
            0x1a302b
        } else {
            0x26392f
        }
    } else {
        background
    }
}

pub(super) fn changed_text(gere: bool, original: bool) -> u32 {
    if original {
        if gere {
            0x4a2930
        } else {
            0x703839
        }
    } else if gere {
        0x1e4437
    } else {
        0x345f42
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_diff_sources_share_git_change_colors() {
        assert_eq!(pane_background(false, 0x21252b, true, true), 0x3b292c);
        assert_eq!(pane_background(false, 0x21252b, true, false), 0x26392f);
        assert_eq!(pane_background(true, 0x141920, false, false), 0x141920);
        assert_eq!(changed_text(false, true), 0x703839);
        assert_eq!(changed_text(false, false), 0x345f42);
    }
}
