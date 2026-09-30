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
        } else if self.find_has_focus {
            self.find_query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.update_find();
        } else if self.palette_open {
            self.palette_query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.update_query();
        } else if self.sidebar == Sidebar::Search && self.search_focused {
            self.query
                .replace_selection(&text.replace(['\n', '\r'], " "));
            self.run_search(cx);
        } else if self.commit_focused {
            self.commit_message
                .replace_selection(&text.replace(['\n', '\r'], " "));
        } else if self.editor_active() {
            if let Some(index) = self.active {
                if self.tabs[index].buffer.insert_text(text) {
                    self.rehighlight_tab(index, cx);
                    if self.find_open {
                        self.refresh_find_matches();
                    }
                    self.ensure_editor_cursor_visible(index);
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
