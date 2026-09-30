//! Keyboard handling for the editable and original panes.
use super::*;

// Cursor movement is not a text edit: it must never trigger full parsing or lint.
fn navigate_editor(
    buffer: &mut buffer::EditorBuffer,
    key: &str,
    secondary: bool,
    alt: bool,
    shift: bool,
) -> Option<bool> {
    Some(match key {
        "left" if secondary => buffer.move_word_left(shift),
        "right" if secondary => buffer.move_word_right(shift),
        "home" if secondary => buffer.move_document_start(shift),
        "end" if secondary => buffer.move_document_end(shift),
        "left" => buffer.move_left(shift),
        "right" => buffer.move_right(shift),
        "up" if !alt => buffer.move_up(shift),
        "down" if !alt => buffer.move_down(shift),
        "home" => buffer.move_home(shift),
        "end" => buffer.move_end(shift),
        _ => return None,
    })
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn repeating_arrows_moves_the_cursor_without_editing() {
        let mut buffer = buffer::EditorBuffer::new("abc\ndef");
        for (key, expected) in [
            ("right", buffer::Position { line: 0, column: 1 }),
            ("down", buffer::Position { line: 1, column: 1 }),
            ("left", buffer::Position { line: 1, column: 0 }),
            ("up", buffer::Position { line: 0, column: 0 }),
        ] {
            assert_eq!(
                navigate_editor(&mut buffer, key, false, false, false),
                Some(true)
            );
            assert_eq!(buffer.cursor_position(), expected);
            assert_eq!(buffer.text(), "abc\ndef");
        }
        assert_eq!(
            navigate_editor(&mut buffer, "left", false, false, false),
            Some(false)
        );
        assert_eq!(
            navigate_editor(&mut buffer, "down", false, true, false),
            None
        );
        assert_eq!(
            navigate_editor(&mut buffer, "right", false, false, true),
            Some(true)
        );
        assert_eq!(buffer.selected_text(), Some("a"));
    }
}

impl Reviewer {
    pub(super) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        if key.modifiers.alt && !key.modifiers.control && !key.modifiers.platform {
            let target = match key.key.as_str() {
                "1" => Some(Sidebar::Files),
                "2" => Some(Sidebar::Search),
                "3" => Some(Sidebar::Git),
                _ => None,
            };
            if let Some(target) = target {
                if !self.settings_open
                    && !self.confirm_discard_all
                    && self.confirm_delete.is_none()
                    && self.confirm_discard.is_none()
                {
                    self.toggle_sidebar(target, window, cx);
                }
                return;
            }
        }
        if self.file_edit.is_some() {
            if key.key == "escape" {
                self.file_edit = None;
                cx.notify();
            } else if key.key == "enter" {
                self.finish_file_edit(cx);
            } else {
                Self::edit_input(&mut self.file_name, event, cx);
                cx.notify();
            }
            return;
        }
        if self.confirm_discard_all {
            if key.key == "enter" {
                self.confirm_discard_all(cx);
            } else if key.key == "escape" {
                self.confirm_discard_all = false;
                cx.notify();
            }
            return;
        }
        if self.confirm_delete.is_some() {
            if key.key == "enter" {
                self.delete_selected_file(cx);
            } else if key.key == "escape" {
                self.confirm_delete = None;
                cx.notify();
            }
            return;
        }
        if self.confirm_discard.is_some() {
            if key.key == "enter" {
                self.action("discard", cx);
            } else if key.key == "escape" {
                self.confirm_discard = None;
                cx.notify();
            }
            return;
        }
        if key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.shift
            && key.key == "j"
            && !self.settings_open
            && !self.palette_open
        {
            self.toggle_terminal(window, cx);
            return;
        }
        if key.key == "escape" && (self.file_menu.is_some() || self.top_file_menu) {
            self.file_menu = None;
            self.top_file_menu = false;
            cx.notify();
            return;
        }
        if key.key == "f2"
            && self.sidebar_visible
            && self.sidebar == Sidebar::Files
            && !self.settings_open
            && !self.palette_open
            && !self.find_has_focus
            && !self.commit_focused
            && !self.search_focused
        {
            if let Some(path) = self.selected.clone().filter(|path| {
                !path.is_absolute() && self.files.iter().any(|entry| entry.path == *path)
            }) {
                self.begin_file_edit(FileEdit::Rename(path), cx);
            }
            return;
        }
        if matches!(key.key.as_str(), "delete" | "supr")
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.platform
            && self.files_focused
            && self.sidebar_visible
            && self.sidebar == Sidebar::Files
            && !self.settings_open
            && !self.palette_open
            && !self.find_has_focus
            && !self.search_focused
            && !self.commit_focused
        {
            if let Some(path) = self.selected.clone().filter(|path| {
                !path.is_absolute() && self.files.iter().any(|entry| entry.path == *path)
            }) {
                self.confirm_delete = Some(path);
                self.file_menu = None;
                cx.notify();
            }
            return;
        }
        if key.modifiers.secondary() && key.key == "o" {
            self.pick_file(cx);
            return;
        }
        if self.settings_open {
            if key.key == "escape" {
                self.settings_open = false;
                cx.notify();
            }
            return;
        }
        if key.modifiers.control && key.key == "p" {
            if key.modifiers.shift {
                self.open_commands(window, cx);
            } else {
                self.open_palette(window, cx);
            }
            return;
        }
        if self.terminal_visible && self.terminal_focused && !self.palette_open {
            self.on_terminal_key(event, cx);
            return;
        }
        if key.modifiers.secondary()
            && key.key == "f"
            && !self.palette_open
            && (!self.show_diff || self.side_by_side)
            && self
                .active
                .and_then(|i| self.tabs.get(i))
                .is_some_and(|tab| !tab.loading)
        {
            self.open_find(cx);
            return;
        }
        if self.find_open && self.find_has_focus {
            self.on_find_key(event, cx);
            return;
        }
        if key.key == "escape" && self.find_open {
            self.close_find();
            cx.notify();
            return;
        }
        if key.key == "escape" {
            if self.palette_open {
                self.palette_open = false;
            } else if self.sidebar == Sidebar::Search && self.search_focused {
                self.search_focused = false;
            } else if self.original_focused {
                self.original_buffer.clear_selection();
                self.original_focused = false;
            } else if self.editor_active() {
                if let Some(index) = self.active {
                    self.tabs[index].buffer.clear_selection();
                }
            } else if self.sidebar == Sidebar::Git {
                self.commit_focused = false;
            } else {
                if self.sidebar == Sidebar::Search {
                    self.sidebar = Sidebar::Files;
                }
                self.search_id += 1;
            }
            cx.notify();
            return;
        }
        if self.original_focused
            && self.show_diff
            && self.side_by_side
            && !self.commit_focused
            && !self.palette_open
            && !(self.sidebar == Sidebar::Search && self.search_focused)
        {
            self.on_original_key(event, cx);
            return;
        }
        if self.editor_active() {
            self.on_editor_key(event, cx);
            return;
        }
        if !self.palette_open
            && !self.commit_focused
            && !(self.sidebar == Sidebar::Search && self.search_focused)
        {
            return;
        }
        if self.palette_open && (key.key == "up" || key.key == "down") {
            let count = if self.palette_mode == PaletteMode::Files {
                self.quick.len()
            } else {
                self.palette_entries().len()
            };
            if count > 0 {
                self.palette_selected = if key.key == "up" {
                    self.palette_selected.saturating_sub(1)
                } else {
                    (self.palette_selected + 1).min(count - 1)
                };
                self.palette_scroll
                    .scroll_to_item(self.palette_selected, ScrollStrategy::Center);
                cx.notify();
            }
            return;
        }
        if self.sidebar == Sidebar::Search && self.search_focused && !self.palette_open {
            self.cursor_blink_visible = true;
            if key.key == "enter" {
                self.run_search(cx);
            } else if Self::edit_input(&mut self.query, event, cx) {
                self.run_search(cx);
            } else {
                cx.notify();
            }
            return;
        }
        if self.commit_focused && !self.palette_open {
            self.cursor_blink_visible = true;
            if key.key == "enter" {
                self.git_operation(GitOperation::Commit(self.commit_message.text.clone()), cx);
            } else {
                Self::edit_input(&mut self.commit_message, event, cx);
                cx.notify();
            }
            return;
        }
        if key.key == "enter" {
            match &self.palette_mode {
                PaletteMode::Files => {
                    self.choose_palette(self.quick.get(self.palette_selected).cloned(), cx)
                }
                PaletteMode::BranchName(source) => {
                    let name = self.palette_query.text.trim().to_owned();
                    if !name.is_empty() {
                        let operation = match source {
                            Some(source) => GitOperation::CreateBranchFrom(name, source.clone()),
                            None => GitOperation::CreateBranch(name),
                        };
                        self.git_operation(operation, cx);
                    }
                }
                _ => {
                    if let Some(item) = self.palette_entries().get(self.palette_selected).cloned() {
                        self.select_palette_item(item, cx);
                    }
                }
            }
            return;
        }
        if Self::edit_input(&mut self.palette_query, event, cx) {
            self.update_query();
        }
        self.cursor_blink_visible = true;
        cx.notify();
    }

    pub(super) fn on_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        self.cursor_blink_visible = true;
        let Some(index) = self.active else {
            return;
        };

        if modifiers.control && key == "s" {
            self.save_active(cx);
            return;
        }
        if modifiers.control && key == "a" {
            let cursor = self.tabs[index].buffer.cursor();
            self.tabs[index].buffer.select_all();
            if self.tabs[index].buffer.cursor() != cursor {
                self.ensure_editor_cursor_visible(index);
            }
            cx.notify();
            return;
        }
        if modifiers.secondary() && key == "d" {
            if self.tabs[index].buffer.select_next_occurrence() {
                self.ensure_editor_cursor_visible(index);
                cx.notify();
            }
            return;
        }
        if modifiers.control && key == "c" {
            let buffer = &self.tabs[index].buffer;
            let text = buffer
                .selected_text()
                .map(str::to_owned)
                .unwrap_or_else(|| buffer.current_line_for_clipboard());
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        if modifiers.control && key == "x" {
            let buffer = &mut self.tabs[index].buffer;
            let text = if let Some(text) = buffer.selected_text().map(str::to_owned) {
                buffer.delete_backward();
                text
            } else {
                let text = buffer.current_line_for_clipboard();
                buffer.delete_line();
                text
            };
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.rehighlight_tab(index, cx);
            if self.find_open {
                self.refresh_find_matches();
            }
            self.ensure_editor_cursor_visible(index);
            cx.notify();
            return;
        }
        if modifiers.control && key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let changed = self.tabs[index].buffer.insert_text(&text);
                if changed {
                    self.rehighlight_tab(index, cx);
                    if self.find_open {
                        self.refresh_find_matches();
                    }
                    self.ensure_editor_cursor_visible(index);
                }
                cx.notify();
            }
            return;
        }
        if modifiers.control && key == "z" {
            let changed = if modifiers.shift {
                self.tabs[index].buffer.redo()
            } else {
                self.tabs[index].buffer.undo()
            };
            if changed {
                self.rehighlight_tab(index, cx);
                if self.find_open {
                    self.refresh_find_matches();
                }
                self.ensure_editor_cursor_visible(index);
            }
            cx.notify();
            return;
        }
        if modifiers.control && key == "y" {
            if self.tabs[index].buffer.redo() {
                self.rehighlight_tab(index, cx);
                if self.find_open {
                    self.refresh_find_matches();
                }
                self.ensure_editor_cursor_visible(index);
            }
            cx.notify();
            return;
        }

        let secondary = modifiers.secondary();
        if let Some(moved) = navigate_editor(
            &mut self.tabs[index].buffer,
            key,
            secondary,
            modifiers.alt,
            modifiers.shift,
        ) {
            if moved {
                self.ensure_editor_cursor_visible(index);
                cx.notify();
            }
            return;
        }
        let (handled, text_changed) = {
            let buffer = &mut self.tabs[index].buffer;
            if secondary && key == "backspace" {
                (true, buffer.delete_word_backward())
            } else if secondary && key == "delete" {
                (true, buffer.delete_word_forward())
            } else if modifiers.control && modifiers.shift && key == "k" {
                (true, buffer.delete_line())
            } else if modifiers.alt && modifiers.shift && key == "down" {
                (true, buffer.duplicate_line_down())
            } else if modifiers.alt && key == "up" {
                (true, buffer.move_line_up())
            } else if modifiers.alt && key == "down" {
                (true, buffer.move_line_down())
            } else if key == "backspace" {
                (true, buffer.delete_backward())
            } else if key == "delete" {
                (true, buffer.delete_forward())
            } else if key == "enter" {
                (true, buffer.insert_text("\n"))
            } else if key == "tab" && !secondary && !modifiers.alt {
                (
                    true,
                    if modifiers.shift {
                        buffer.unindent()
                    } else {
                        buffer.indent()
                    },
                )
            } else {
                (false, false)
            }
        };
        if text_changed {
            self.rehighlight_tab(index, cx);
            if self.find_open {
                self.refresh_find_matches();
            }
        }
        if handled {
            self.ensure_editor_cursor_visible(index);
        }
        if handled {
            cx.notify();
        }
    }

    pub(super) fn on_original_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && key == "s" {
            self.save_active(cx);
            return;
        }
        if modifiers.control && key == "a" {
            self.original_buffer.select_all();
            cx.notify();
            return;
        }
        if modifiers.control && key == "c" {
            if let Some(text) = self.original_buffer.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
            }
            return;
        }
        let buffer = &mut self.original_buffer;
        let moved = if modifiers.secondary() && key == "left" {
            buffer.move_word_left(modifiers.shift)
        } else if modifiers.secondary() && key == "right" {
            buffer.move_word_right(modifiers.shift)
        } else if modifiers.secondary() && key == "home" {
            buffer.move_document_start(modifiers.shift)
        } else if modifiers.secondary() && key == "end" {
            buffer.move_document_end(modifiers.shift)
        } else if !modifiers.control && !modifiers.alt && !modifiers.platform {
            match key {
                "left" => buffer.move_left(modifiers.shift),
                "right" => buffer.move_right(modifiers.shift),
                "up" => buffer.move_up(modifiers.shift),
                "down" => buffer.move_down(modifiers.shift),
                "home" => buffer.move_home(modifiers.shift),
                "end" => buffer.move_end(modifiers.shift),
                _ => false,
            }
        } else {
            false
        };
        if moved {
            let physical = buffer.cursor_position().line;
            let visual = self
                .diff_highlights
                .before_to_visual
                .get(physical)
                .copied()
                .unwrap_or(physical);
            self.original_scroll
                .scroll_to_item(visual, ScrollStrategy::Center);
            self.editor_scroll
                .scroll_to_item(visual, ScrollStrategy::Center);
            cx.notify();
        }
    }
}
