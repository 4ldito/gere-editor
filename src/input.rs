use gpui::{div, prelude::*, px, rgb, HighlightStyle, KeyDownEvent, Pixels, StyledText};

use crate::{FG, MUTED};

#[derive(Default)]
pub(super) struct SingleLineInput {
    pub(super) text: String,
    pub(super) cursor: usize,
    pub(super) anchor: Option<usize>,
}

impl SingleLineInput {
    pub(super) fn set_text(&mut self, text: String) {
        self.cursor = text.len();
        self.anchor = None;
        self.text = text;
    }

    pub(super) fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub(super) fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    fn move_to(&mut self, target: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = target;
    }

    pub(super) fn replace_selection(&mut self, insert: &str) -> bool {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        if range.is_empty() && insert.is_empty() {
            return false;
        }
        self.text.replace_range(range.clone(), insert);
        self.cursor = range.start + insert.len();
        self.anchor = None;
        true
    }

    fn word_left(&self) -> usize {
        let mut at = self.cursor;
        while at > 0 {
            let (start, ch) = self.text[..at].char_indices().next_back().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            at = start;
        }
        let is_word = self.text[..at]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        while at > 0 {
            let (start, ch) = self.text[..at].char_indices().next_back().unwrap();
            if (ch.is_alphanumeric() || ch == '_') != is_word || ch.is_whitespace() {
                break;
            }
            at = start;
        }
        at
    }

    fn word_right(&self) -> usize {
        let mut at = self.cursor;
        let is_word = self.text[at..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        while at < self.text.len() {
            let ch = self.text[at..].chars().next().unwrap();
            if (ch.is_alphanumeric() || ch == '_') != is_word || ch.is_whitespace() {
                break;
            }
            at += ch.len_utf8();
        }
        while at < self.text.len() {
            let ch = self.text[at..].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            at += ch.len_utf8();
        }
        at
    }

    // Returns true only when the text changes, so callers can avoid redundant searches.
    pub(super) fn handle(&mut self, event: &KeyDownEvent, paste: Option<&str>) -> bool {
        let key = &event.keystroke;
        let secondary = key.modifiers.secondary();
        if secondary && key.key == "a" {
            self.anchor = Some(0);
            self.cursor = self.text.len();
        } else if secondary && key.key == "v" {
            if let Some(paste) = paste {
                return self.replace_selection(&paste.replace(['\n', '\r'], " "));
            }
        } else if key.key == "left" || key.key == "right" || key.key == "home" || key.key == "end" {
            let target = match key.key.as_str() {
                "home" => 0,
                "end" => self.text.len(),
                "left" if secondary => self
                    .selection()
                    .filter(|_| !key.modifiers.shift)
                    .map_or_else(|| self.word_left(), |selection| selection.start),
                "right" if secondary => self
                    .selection()
                    .filter(|_| !key.modifiers.shift)
                    .map_or_else(|| self.word_right(), |selection| selection.end),
                "left" => self
                    .selection()
                    .filter(|_| !key.modifiers.shift)
                    .map_or_else(
                        || {
                            self.text[..self.cursor]
                                .char_indices()
                                .next_back()
                                .map_or(0, |(at, _)| at)
                        },
                        |selection| selection.start,
                    ),
                _ => self
                    .selection()
                    .filter(|_| !key.modifiers.shift)
                    .map_or_else(
                        || {
                            self.text[self.cursor..]
                                .chars()
                                .next()
                                .map_or(self.cursor, |ch| self.cursor + ch.len_utf8())
                        },
                        |selection| selection.end,
                    ),
            };
            self.move_to(target, key.modifiers.shift);
        } else if key.key == "backspace" || key.key == "delete" {
            if self.selection().is_some() {
                return self.replace_selection("");
            }
            let target = if key.key == "backspace" {
                if secondary {
                    self.word_left()
                } else {
                    self.text[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(at, _)| at)
                }
            } else if secondary {
                self.word_right()
            } else {
                self.text[self.cursor..]
                    .chars()
                    .next()
                    .map_or(self.cursor, |ch| self.cursor + ch.len_utf8())
            };
            self.anchor = Some(target);
            return self.replace_selection("");
        }
        false
    }

    pub(super) fn display(&self, width: usize) -> (StyledText, usize) {
        let width = width.max(3);
        let boundaries: Vec<_> = self
            .text
            .char_indices()
            .map(|(at, _)| at)
            .chain(std::iter::once(self.text.len()))
            .collect();
        let count = boundaries.len() - 1;
        let cursor = self.text[..self.cursor].chars().count();
        // Reserve space for both overflow markers so the caret remains in view.
        let visible_width = if count > width { width - 2 } else { width };
        let start = cursor
            .saturating_sub(visible_width / 2)
            .min(count.saturating_sub(visible_width));
        let end = (start + visible_width).min(count);
        let prefix = if start > 0 { "…" } else { "" };
        let suffix = if end < count { "…" } else { "" };
        let visible = format!(
            "{prefix}{}{suffix}",
            &self.text[boundaries[start]..boundaries[end]]
        );
        let mut styled = StyledText::new(visible);
        if let Some(selection) = self.selection() {
            let from = selection.start.max(boundaries[start]);
            let to = selection.end.min(boundaries[end]);
            if from < to {
                let mut style = HighlightStyle::default();
                style.background_color = Some(rgb(0x3e4451).into());
                styled = styled.with_highlights(vec![(
                    prefix.len() + from - boundaries[start]..prefix.len() + to - boundaries[start],
                    style,
                )]);
            }
        }
        (styled, prefix.chars().count() + cursor - start)
    }
}

pub(super) fn sidebar_input_columns(sidebar: Pixels, reserved: f32, cell_width: Pixels) -> usize {
    ((f32::from(sidebar) - reserved) / f32::from(cell_width))
        .floor()
        .max(3.) as usize
}

pub(super) fn input_view(
    input: &SingleLineInput,
    placeholder: &'static str,
    focused: bool,
    caret_visible: bool,
    width: usize,
    cell_width: Pixels,
    background: u32,
    compact: bool,
) -> gpui::Stateful<gpui::Div> {
    let (display, cursor) = input.display(width);
    div()
        .id(placeholder)
        .relative()
        .px_2()
        .py_1()
        .when(compact, |view| view.h(px(21.)).py_0().flex().items_center())
        .overflow_hidden()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if focused { 0x61afef } else { 0x3e4451 }))
        .bg(rgb(background))
        .text_size(px(14.))
        .text_color(rgb(if input.text.is_empty() { MUTED } else { FG }))
        .cursor_text()
        .on_hover(|hovered, window, _| {
            if *hovered {
                window.refresh();
            }
        })
        .child(if input.text.is_empty() && !focused {
            StyledText::new(placeholder.to_string())
        } else {
            display
        })
        .when(caret_visible, |view| {
            view.child(
                div()
                    .absolute()
                    .left(px(8.) + cell_width * cursor)
                    .top(px(if compact { 2. } else { 6. }))
                    .w(px(1.5))
                    .h(px(17.))
                    .bg(rgb(0x61afef)),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_input_keeps_caret_within_the_visible_columns() {
        let mut input = SingleLineInput::default();
        input.set_text("fixed bug with a key, fixed bug with cursor icon, ".into());
        for width in [4, 12, 21, 27] {
            let (_, caret) = input.display(width);
            assert!(caret < width, "caret {caret} outside {width} columns");
        }
        input.move_to(0, false);
        assert!(input.display(12).1 < 12);
        input.move_to("fixed bug with a key".len(), false);
        assert!(input.display(12).1 < 12);
    }

    #[test]
    fn query_editing_replaces_selection_and_deletes_unicode_words() {
        let mut input = SingleLineInput::default();
        input.set_text("uno árbol 🙂".into());
        input.cursor = "uno árbol".len();
        assert_eq!(input.word_left(), 4);
        input.anchor = Some(input.word_left());
        assert!(input.replace_selection("otro"));
        assert_eq!(input.text, "uno otro 🙂");
        assert_eq!(input.cursor, "uno otro".len());
        input.anchor = Some(0);
        input.cursor = input.text.len();
        assert!(input.replace_selection("hola"));
        assert_eq!(input.text, "hola");
    }

    #[test]
    fn printable_keys_wait_for_native_text_input_instead_of_inserting_twice() {
        let mut input = SingleLineInput::default();
        let key = gpui::Keystroke {
            key: "a".into(),
            key_char: Some("a".into()),
            modifiers: Default::default(),
        };
        assert!(!input.handle(
            &KeyDownEvent {
                keystroke: key,
                is_held: false
            },
            None
        ));
        assert!(input.text.is_empty());
        assert!(input.replace_selection("a"));
        assert_eq!(input.text, "a");
    }
}
