//! In-editor find state and navigation.
use super::*;

pub(super) fn matching_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let query_len = query.chars().count();
    if query_len == 0 {
        return Vec::new();
    }
    let query = query.to_lowercase();
    let mut boundaries: Vec<_> = text.char_indices().map(|(index, _)| index).collect();
    boundaries.push(text.len());
    let mut matches = Vec::new();
    let mut index = 0;
    while index + query_len < boundaries.len() {
        let start = boundaries[index];
        let end = boundaries[index + query_len];
        if text[start..end].to_lowercase() == query {
            matches.push(start..end);
            index += query_len;
        } else {
            index += 1;
        }
    }
    matches
}

impl Reviewer {
    pub(super) fn open_find(&mut self, cx: &mut Context<Self>) {
        if let Some(selected) = self
            .active
            .and_then(|index| self.tabs.get(index))
            .and_then(|tab| tab.buffer.selected_text())
            .filter(|selected| !selected.contains('\n'))
        {
            self.find_query.set_text(selected.to_owned());
        }
        self.find_open = true;
        self.find_has_focus = true;
        self.search_focused = false;
        self.find_query.cursor = self.find_query.text.len();
        self.find_query.anchor = None;
        self.cursor_blink_visible = true;
        self.update_find();
        if let Some(index) = self.active {
            self.ensure_editor_cursor_visible(index);
        }
        cx.notify();
    }

    pub(super) fn close_find(&mut self) {
        self.find_open = false;
        self.find_has_focus = false;
        self.find_matches.clear();
        self.find_active = None;
        self.cursor_blink_visible = true;
    }

    pub(super) fn update_find(&mut self) {
        self.refresh_find_matches();
        if let (Some(index), Some(match_index)) = (self.active, self.find_active) {
            self.tabs[index]
                .buffer
                .set_selection(self.find_matches[match_index].clone());
        } else if let Some(index) = self.active {
            self.tabs[index].buffer.clear_selection();
        }
    }

    pub(super) fn refresh_find_matches(&mut self) {
        self.find_matches = self
            .active
            .and_then(|index| self.tabs.get(index))
            .map(|tab| matching_ranges(tab.buffer.text(), &self.find_query.text))
            .unwrap_or_default();
        let existing_selection = self
            .active
            .and_then(|index| self.tabs.get(index))
            .and_then(|tab| tab.buffer.selection_range());
        self.find_active = if self.find_matches.is_empty() {
            None
        } else {
            existing_selection
                .and_then(|selection| {
                    self.find_matches
                        .iter()
                        .position(|range| *range == selection)
                })
                .or(Some(0))
        };
    }

    pub(super) fn move_find(&mut self, backwards: bool) {
        if self.find_matches.is_empty() {
            self.find_active = None;
            return;
        }
        let len = self.find_matches.len();
        let current = self
            .find_active
            .unwrap_or(if backwards { 0 } else { len - 1 });
        let next = if backwards {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        self.find_active = Some(next);
        if let Some(index) = self.active {
            self.tabs[index]
                .buffer
                .set_selection(self.find_matches[next].clone());
            self.ensure_editor_cursor_visible(index);
        }
    }

    pub(super) fn on_find_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let modifiers = key.modifiers;
        self.cursor_blink_visible = true;
        if key.key == "escape" {
            self.close_find();
            cx.notify();
            return;
        }
        if modifiers.secondary() && key.key == "s" {
            self.save_active(cx);
            return;
        }
        if key.key == "enter" {
            self.move_find(modifiers.shift);
            cx.notify();
            return;
        }
        let changed = Self::edit_input(&mut self.find_query, event, cx);
        if changed {
            self.update_find();
        }
        cx.notify();
    }
}
