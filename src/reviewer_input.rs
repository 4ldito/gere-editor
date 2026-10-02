//! Native text input dispatch and session persistence on exit.
use super::*;
use gpui::{EntityInputHandler, UTF16Selection};

impl Drop for Reviewer {
    fn drop(&mut self) {
        if !self.session_loading {
            let _ = session::save(&self.root, &self.session_snapshot(), session::revision());
        }
    }
}

impl EntityInputHandler for Reviewer {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        Some(String::new())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }
    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            return;
        }
        if self.file_edit.is_some() {
            self.file_name
                .replace_selection(&text.replace(['\n', '\r'], " "));
        } else if self.trello_credentials_open {
            let input = if self.trello_credential_focus_token {
                &mut self.trello_credential_token
            } else {
                &mut self.trello_credential_key
            };
            input.replace_selection(&text.replace(['\n', '\r'], " "));
        } else if self.trello_edit.is_some() {
            self.trello_input
                .replace_selection(&text.replace(['\n', '\r'], " "));
        } else if self.find_has_focus {
            self.find_query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.update_find();
        } else if self.terminal_find_open && self.terminal_focused {
            self.terminal_find_query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.refresh_terminal_find(true);
        } else if self.palette_open {
            self.palette_query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.update_query();
        } else if self.terminal_visible && self.terminal_focused && !self.settings_open {
            self.send_terminal_text(text, cx);
        } else if self.sidebar == Sidebar::Search && self.search_focused {
            self.query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.run_search(cx);
        } else if self.commit_focused {
            if !text.contains(['\n', '\r']) {
                self.commit_message.replace_selection(text);
            }
        } else if self.editor_active() {
            if let Some(index) = self.active {
                let fast_typing = text.chars().count() == 1
                    && !text.contains(['\n', '\r'])
                    && self.tabs[index].buffer.selection_range().is_none()
                    && !self.show_diff
                    && self.tabs[index].csv.is_none();
                if self.tabs[index].buffer.insert_text(text) {
                    if fast_typing {
                        self.update_typed_line(index, cx);
                    } else {
                        self.rehighlight_tab(index, cx);
                    }
                    if self.find_open {
                        self.refresh_find_matches();
                    }
                    self.ensure_editor_cursor_visible(index, cx);
                }
            }
        }
        self.cursor_blink_visible = true;
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        _: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
