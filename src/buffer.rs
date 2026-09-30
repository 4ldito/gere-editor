use std::ops::Range;
use std::time::{Duration, Instant};

const UNDO_GROUP_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Whitespace,
    Backspace,
    Delete,
}

pub const TAB_WIDTH: usize = 4;

pub fn visual_column(text: &str, column: usize) -> usize {
    text.chars().take(column).fold(0, |width, ch| {
        width
            + if ch == '\t' {
                TAB_WIDTH - width % TAB_WIDTH
            } else {
                1
            }
    })
}

pub fn source_column(text: &str, visual: usize) -> usize {
    let mut width = 0;
    for (column, ch) in text.chars().enumerate() {
        let next = width
            + if ch == '\t' {
                TAB_WIDTH - width % TAB_WIDTH
            } else {
                1
            };
        if visual < next {
            return column + usize::from(visual - width >= next - visual);
        }
        width = next;
    }
    text.chars().count()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct EditorBuffer {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    saved_text: String,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    preferred_column: Option<usize>,
    edit_group: Option<(EditKind, Instant)>,
}

impl EditorBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            saved_text: text.clone(),
            text,
            cursor: 0,
            anchor: None,
            undo: Vec::new(),
            redo: Vec::new(),
            preferred_column: None,
            edit_group: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_dirty(&self) -> bool {
        self.text != self.saved_text
    }

    pub fn mark_saved(&mut self) {
        self.saved_text = self.text.clone();
    }

    pub fn reload(&mut self, text: String) {
        let position = self.cursor_position();
        *self = Self::new(text);
        self.cursor = self.offset_at(position);
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn cursor_position(&self) -> Position {
        self.position_at(self.cursor)
    }

    pub fn selection_range(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        let (start, end) = if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        (start < end).then_some(start..end)
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection_range().map(|range| &self.text[range])
    }

    pub fn line_count(&self) -> usize {
        self.text.bytes().filter(|byte| *byte == b'\n').count() + 1
    }

    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        self.line_ranges().get(line).cloned()
    }

    pub fn select_all(&mut self) {
        self.edit_group = None;
        self.anchor = Some(0);
        self.cursor = self.text.len();
        self.preferred_column = None;
    }

    pub fn clear_selection(&mut self) -> bool {
        self.edit_group = None;
        let changed = self.anchor.take().is_some();
        self.preferred_column = None;
        changed
    }

    pub fn set_cursor(&mut self, offset: usize, extend: bool) -> bool {
        self.edit_group = None;
        let old_cursor = self.cursor;
        let old_anchor = self.anchor;
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset;
        self.preferred_column = None;
        self.cursor != old_cursor || self.anchor != old_anchor
    }

    pub fn set_selection(&mut self, range: Range<usize>) -> bool {
        self.edit_group = None;
        let start = range.start.min(self.text.len());
        let end = range.end.min(self.text.len());
        let mut start = start;
        let mut end = end;
        while !self.text.is_char_boundary(start) {
            start -= 1;
        }
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        let changed = self.cursor != end || self.anchor != Some(start);
        self.cursor = end;
        self.anchor = Some(start);
        self.preferred_column = None;
        changed
    }

    pub fn offset_at_position(&self, position: Position) -> usize {
        self.offset_at(position)
    }

    pub fn select_word_at(&mut self, offset: usize) -> bool {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        if self.text.is_empty() {
            return self.set_selection(0..0);
        }

        let (character_start, character) = if offset < self.text.len() {
            (offset, self.text[offset..].chars().next().unwrap())
        } else {
            let (start, character) = self.text[..offset].char_indices().next_back().unwrap();
            (start, character)
        };
        let is_word = |character: char| character.is_alphanumeric() || character == '_';
        let mut start = character_start;
        let mut end = character_start + character.len_utf8();
        if is_word(character) {
            while start > 0 {
                let (previous, character) = self.text[..start].char_indices().next_back().unwrap();
                if !is_word(character) {
                    break;
                }
                start = previous;
            }
            while end < self.text.len() {
                let character = self.text[end..].chars().next().unwrap();
                if !is_word(character) {
                    break;
                }
                end += character.len_utf8();
            }
        }
        self.set_selection(start..end)
    }

    pub fn select_line(&mut self, line: usize) -> bool {
        let Some(range) = self.line_range(line) else {
            return false;
        };
        let end = if line + 1 < self.line_count() {
            range.end + 1
        } else {
            range.end
        };
        self.set_selection(range.start..end)
    }

    pub fn current_line_for_clipboard(&self) -> String {
        let line = self.cursor_position().line;
        let range = self.line_range(line).expect("cursor has a line");
        format!("{}\n", &self.text[range])
    }

    pub fn delete_line(&mut self) -> bool {
        let line = self.cursor_position().line;
        let range = self.line_range(line).expect("cursor has a line");
        let deletion = if range.end < self.text.len() {
            range.start..range.end + 1
        } else if range.start > 0 {
            range.start - 1..range.end
        } else {
            range
        };
        self.replace_range(deletion, "")
    }

    pub fn duplicate_line_down(&mut self) -> bool {
        let (first, last) = self.selected_line_span();
        let start = self.line_range(first).unwrap().start;
        let end = self.line_range(last).unwrap().end;
        let cursor = self.cursor;
        let anchor = self.anchor;
        let content = self.text[start..end].to_owned();
        let (at, insertion) = if end < self.text.len() {
            (end + 1, format!("{content}\n"))
        } else {
            (end, format!("\n{content}"))
        };
        let shift = insertion.len();
        self.replace_range(at..at, &insertion);
        self.cursor = cursor + shift;
        self.anchor = anchor.map(|offset| offset + shift);
        true
    }

    pub fn select_next_occurrence(&mut self) -> bool {
        let Some(selection) = self.selection_range() else {
            return self.select_word_at(self.cursor);
        };
        let needle = self.text[selection.clone()].to_owned();
        if needle.is_empty() {
            return false;
        }
        let next = self.text[selection.end..]
            .find(&needle)
            .map(|offset| selection.end + offset)
            .or_else(|| {
                self.text[..selection.start]
                    .find(&needle)
                    .filter(|offset| *offset != selection.start)
            });
        next.is_some_and(|start| self.set_selection(start..start + needle.len()))
    }

    pub fn insert_text(&mut self, text: &str) -> bool {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let kind = if self.selection_range().is_none() && text.chars().count() == 1 {
            if text.chars().next().unwrap() == '\n' {
                None
            } else if text.chars().next().unwrap().is_whitespace() {
                Some(EditKind::Whitespace)
            } else {
                Some(EditKind::Typing)
            }
        } else {
            None
        };
        self.replace_selection_grouped(&text, kind)
    }

    pub fn indent(&mut self) -> bool {
        if let Some(selection) = self.selection_range() {
            if self.position_at(selection.start).line != self.position_at(selection.end - 1).line {
                return self.change_line_indentation(true);
            }
        }
        let position = self.cursor_position();
        let range = self.line_range(position.line).expect("cursor has a line");
        let column = visual_column(&self.text[range.start..self.cursor], position.column);
        self.insert_text(&" ".repeat(TAB_WIDTH - column % TAB_WIDTH))
    }

    pub fn unindent(&mut self) -> bool {
        self.change_line_indentation(false)
    }

    fn change_line_indentation(&mut self, indent: bool) -> bool {
        let (first, last) = self.selected_line_span();
        let ranges = self.line_ranges();
        let mut edits = Vec::new();
        for range in &ranges[first..=last] {
            let line = &self.text[range.clone()];
            let removal = if indent {
                0
            } else if line.starts_with('\t') {
                1
            } else {
                line.bytes()
                    .take(TAB_WIDTH)
                    .take_while(|byte| *byte == b' ')
                    .count()
            };
            if indent || removal > 0 {
                edits.push((range.start, removal));
            }
        }
        if edits.is_empty() {
            return false;
        }
        self.record_edit(None);
        for &(start, removal) in edits.iter().rev() {
            self.text
                .replace_range(start..start + removal, if indent { "    " } else { "" });
        }
        let adjust = |offset: usize| {
            let mut result = offset;
            for &(start, removal) in &edits {
                if start > offset {
                    break;
                }
                if indent {
                    result += TAB_WIDTH;
                } else {
                    result -= removal.min(offset - start);
                }
            }
            result
        };
        self.cursor = adjust(self.cursor);
        self.anchor = self.anchor.map(adjust);
        self.preferred_column = None;
        true
    }

    pub fn delete_backward(&mut self) -> bool {
        if let Some(range) = self.selection_range() {
            return self.replace_range(range, "");
        }
        if self.cursor == 0 {
            return false;
        }
        let start = self.previous_boundary(self.cursor);
        self.replace_range_grouped(start..self.cursor, "", Some(EditKind::Backspace))
    }

    pub fn delete_forward(&mut self) -> bool {
        if let Some(range) = self.selection_range() {
            return self.replace_range(range, "");
        }
        if self.cursor == self.text.len() {
            return false;
        }
        let end = self.next_boundary(self.cursor);
        self.replace_range_grouped(self.cursor..end, "", Some(EditKind::Delete))
    }

    pub fn delete_word_backward(&mut self) -> bool {
        if let Some(range) = self.selection_range() {
            return self.replace_range(range, "");
        }
        if self.cursor == 0 {
            return false;
        }
        let start = self.previous_word_boundary(self.cursor);
        self.replace_range(start..self.cursor, "")
    }

    pub fn delete_word_forward(&mut self) -> bool {
        if let Some(range) = self.selection_range() {
            return self.replace_range(range, "");
        }
        if self.cursor == self.text.len() {
            return false;
        }
        let end = self.next_word_boundary(self.cursor);
        self.replace_range(self.cursor..end, "")
    }

    pub fn move_left(&mut self, extend: bool) -> bool {
        self.move_horizontal(-1, extend)
    }

    pub fn move_right(&mut self, extend: bool) -> bool {
        self.move_horizontal(1, extend)
    }

    pub fn move_up(&mut self, extend: bool) -> bool {
        self.move_vertical(-1, extend)
    }

    pub fn move_down(&mut self, extend: bool) -> bool {
        self.move_vertical(1, extend)
    }

    pub fn move_home(&mut self, extend: bool) -> bool {
        self.move_to_line_edge(false, extend)
    }

    pub fn move_end(&mut self, extend: bool) -> bool {
        self.move_to_line_edge(true, extend)
    }

    pub fn move_word_left(&mut self, extend: bool) -> bool {
        self.move_word(-1, extend)
    }

    pub fn move_word_right(&mut self, extend: bool) -> bool {
        self.move_word(1, extend)
    }

    pub fn move_document_start(&mut self, extend: bool) -> bool {
        self.move_to_offset(0, -1, extend)
    }

    pub fn move_document_end(&mut self, extend: bool) -> bool {
        self.move_to_offset(self.text.len(), 1, extend)
    }

    pub fn undo(&mut self) -> bool {
        self.edit_group = None;
        let Some(snapshot) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(snapshot);
        self.preferred_column = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        self.edit_group = None;
        let Some(snapshot) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(snapshot);
        self.preferred_column = None;
        true
    }

    pub fn move_line_up(&mut self) -> bool {
        self.move_line(-1)
    }

    pub fn move_line_down(&mut self) -> bool {
        self.move_line(1)
    }

    fn move_horizontal(&mut self, direction: isize, extend: bool) -> bool {
        self.edit_group = None;
        let old_cursor = self.cursor;
        let old_anchor = self.anchor;
        if !extend {
            if let Some(range) = self.selection_range() {
                self.cursor = if direction < 0 {
                    range.start
                } else {
                    range.end
                };
                self.anchor = None;
                self.preferred_column = None;
                return self.cursor != old_cursor || self.anchor != old_anchor;
            }
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }

        self.cursor = if direction < 0 {
            self.previous_boundary(self.cursor)
        } else {
            self.next_boundary(self.cursor)
        };
        if !extend {
            self.anchor = None;
        }
        self.preferred_column = None;
        self.cursor != old_cursor || self.anchor != old_anchor
    }

    fn move_word(&mut self, direction: isize, extend: bool) -> bool {
        let target = if direction < 0 {
            self.previous_word_boundary(self.cursor)
        } else {
            self.next_word_boundary(self.cursor)
        };
        self.move_to_offset(target, direction, extend)
    }

    fn move_to_offset(&mut self, target: usize, direction: isize, extend: bool) -> bool {
        self.edit_group = None;
        let old_cursor = self.cursor;
        let old_anchor = self.anchor;
        if !extend {
            if let Some(range) = self.selection_range() {
                self.cursor = if direction < 0 {
                    range.start
                } else {
                    range.end
                };
                self.anchor = None;
                self.preferred_column = None;
                return self.cursor != old_cursor || self.anchor != old_anchor;
            }
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }

        self.cursor = target;
        if !extend {
            self.anchor = None;
        }
        self.preferred_column = None;
        self.cursor != old_cursor || self.anchor != old_anchor
    }

    fn move_vertical(&mut self, direction: isize, extend: bool) -> bool {
        self.edit_group = None;
        let old_cursor = self.cursor;
        let old_anchor = self.anchor;
        if !extend {
            if let Some(range) = self.selection_range() {
                self.cursor = if direction < 0 {
                    range.start
                } else {
                    range.end
                };
                self.anchor = None;
                self.preferred_column = None;
                return self.cursor != old_cursor || self.anchor != old_anchor;
            }
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }

        let position = self.position_at(self.cursor);
        let desired_column = self.preferred_column.unwrap_or(position.column);
        let target_line = if direction < 0 {
            position.line.saturating_sub(1)
        } else {
            (position.line + 1).min(self.line_count() - 1)
        };
        self.cursor = self.offset_at(Position {
            line: target_line,
            column: desired_column,
        });
        if !extend {
            self.anchor = None;
        }
        self.preferred_column = Some(desired_column);
        self.cursor != old_cursor || self.anchor != old_anchor
    }

    fn move_to_line_edge(&mut self, end: bool, extend: bool) -> bool {
        self.edit_group = None;
        let old_cursor = self.cursor;
        let old_anchor = self.anchor;
        if !extend {
            if let Some(range) = self.selection_range() {
                self.cursor = if end { range.end } else { range.start };
                self.anchor = None;
                self.preferred_column = None;
                return self.cursor != old_cursor || self.anchor != old_anchor;
            }
        } else if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }

        let position = self.position_at(self.cursor);
        let range = self.line_range(position.line).expect("cursor has a line");
        self.cursor = if end { range.end } else { range.start };
        if !extend {
            self.anchor = None;
        }
        self.preferred_column = None;
        self.cursor != old_cursor || self.anchor != old_anchor
    }

    fn move_line(&mut self, direction: isize) -> bool {
        let lines = self.line_ranges();
        let trailing_newline = self.text.ends_with('\n');
        let real_line_count = lines.len() - if trailing_newline { 1 } else { 0 };
        let (start, end) = self.selected_line_span();
        if start >= real_line_count
            || end >= real_line_count
            || direction < 0 && start == 0
            || direction > 0 && end + 1 >= real_line_count
        {
            return false;
        }

        let cursor_position = self.position_at(self.cursor);
        let anchor_position = self.anchor.map(|offset| self.position_at(offset));
        let mut contents: Vec<String> = lines
            .iter()
            .take(real_line_count)
            .map(|range| self.text[range.clone()].to_owned())
            .collect();
        if direction < 0 {
            contents[start - 1..=end].rotate_left(1);
        } else {
            contents[start..=end + 1].rotate_right(1);
        }
        let mut text = contents.join("\n");
        if trailing_newline {
            text.push('\n');
        }
        if text == self.text {
            return false;
        }

        self.record_edit(None);
        self.text = text;
        self.cursor = self.offset_at(Position {
            line: moved_line(cursor_position.line, start, end, direction),
            column: cursor_position.column,
        });
        self.anchor = anchor_position.map(|position| {
            self.offset_at(Position {
                line: moved_line(position.line, start, end, direction),
                column: position.column,
            })
        });
        self.preferred_column = None;
        true
    }

    fn selected_line_span(&self) -> (usize, usize) {
        let Some(range) = self.selection_range() else {
            let line = self.position_at(self.cursor).line;
            return (line, line);
        };
        let start = self.position_at(range.start).line;
        let end = self.position_at(range.end - 1).line;
        (start.min(end), start.max(end))
    }

    fn replace_selection_grouped(&mut self, replacement: &str, kind: Option<EditKind>) -> bool {
        let range = self.selection_range().unwrap_or(self.cursor..self.cursor);
        self.replace_range_grouped(range, replacement, kind)
    }

    fn replace_range(&mut self, range: Range<usize>, replacement: &str) -> bool {
        self.replace_range_grouped(range, replacement, None)
    }

    fn record_edit(&mut self, kind: Option<EditKind>) {
        let now = Instant::now();
        if !matches!(self.edit_group, Some((previous, time)) if Some(previous) == kind && now.duration_since(time) < UNDO_GROUP_DELAY)
            || kind.is_none()
        {
            self.undo.push(self.snapshot());
        }
        self.edit_group = kind.map(|kind| (kind, now));
        self.redo.clear();
    }

    fn replace_range_grouped(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        kind: Option<EditKind>,
    ) -> bool {
        if range.start == range.end && replacement.is_empty() {
            return false;
        }
        self.record_edit(kind);
        self.text.replace_range(range.clone(), replacement);
        self.cursor = range.start + replacement.len();
        self.anchor = None;
        self.preferred_column = None;
        true
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.cursor = snapshot.cursor;
        self.anchor = snapshot.anchor;
    }

    pub(crate) fn line_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::with_capacity(self.line_count());
        let mut start = 0;
        for (index, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                ranges.push(start..index);
                start = index + 1;
            }
        }
        ranges.push(start..self.text.len());
        ranges
    }

    fn position_at(&self, offset: usize) -> Position {
        debug_assert!(self.text.is_char_boundary(offset));
        let before = &self.text[..offset];
        let start = before.rfind('\n').map_or(0, |at| at + 1);
        Position {
            line: before.bytes().filter(|byte| *byte == b'\n').count(),
            column: before[start..].chars().count(),
        }
    }

    fn offset_at(&self, position: Position) -> usize {
        let range = self
            .line_range(position.line.min(self.line_count() - 1))
            .expect("position has a line");
        let offset = self.text[range.clone()]
            .char_indices()
            .nth(position.column)
            .map(|(offset, _)| offset)
            .unwrap_or(range.len());
        range.start + offset
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map(|character| offset + character.len_utf8())
            .unwrap_or(self.text.len())
    }

    fn previous_word_boundary(&self, mut offset: usize) -> usize {
        while offset > 0 {
            let (index, character) = self.text[..offset]
                .char_indices()
                .next_back()
                .expect("offset has a previous character");
            if !character.is_whitespace() {
                break;
            }
            offset = index;
        }
        while offset > 0 {
            let (index, character) = self.text[..offset]
                .char_indices()
                .next_back()
                .expect("offset has a previous character");
            if character.is_whitespace() {
                break;
            }
            offset = index;
        }
        offset
    }

    fn next_word_boundary(&self, mut offset: usize) -> usize {
        while offset < self.text.len() {
            let character = self.text[offset..]
                .chars()
                .next()
                .expect("offset has a next character");
            if character.is_whitespace() {
                break;
            }
            offset += character.len_utf8();
        }
        while offset < self.text.len() {
            let character = self.text[offset..]
                .chars()
                .next()
                .expect("offset has a next character");
            if !character.is_whitespace() {
                break;
            }
            offset += character.len_utf8();
        }
        offset
    }
}

fn moved_line(line: usize, start: usize, end: usize, direction: isize) -> usize {
    if direction < 0 {
        if line == start - 1 {
            end
        } else if (start..=end).contains(&line) {
            line - 1
        } else {
            line
        }
    } else if line == end + 1 {
        start
    } else if (start..=end).contains(&line) {
        line + 1
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_stops_and_existing_tabs_keep_visual_and_source_columns_aligned() {
        let line = "a\tb\t";
        assert_eq!(visual_column(line, 2), 4);
        assert_eq!(visual_column(line, 4), 8);
        assert_eq!(source_column(line, 4), 2);
        assert_eq!(source_column(line, 7), 4);
        let mut buffer = EditorBuffer::new("a\tb");
        buffer.set_cursor(2, false);
        assert!(buffer.indent());
        assert_eq!(buffer.text(), "a\t    b");
        assert_eq!(buffer.cursor_position().column, 6);
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "a\tb");
    }

    #[test]
    fn tab_and_shift_tab_indent_selected_lines_and_restore_selection_on_undo() {
        let mut buffer = EditorBuffer::new("one\n  two\nthree");
        buffer.set_selection(1.."one\n  two\n".len());
        assert!(buffer.indent());
        assert_eq!(buffer.text(), "    one\n      two\nthree");
        assert_eq!(buffer.selected_text(), Some("ne\n      two\n"));
        assert!(buffer.unindent());
        assert_eq!(buffer.text(), "one\n  two\nthree");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "    one\n      two\nthree");
    }

    #[test]
    fn shift_tab_removes_spaces_or_a_tab_from_current_line() {
        let mut buffer = EditorBuffer::new("  foo\n\tbar");
        buffer.set_cursor(2, false);
        assert!(buffer.unindent());
        assert_eq!(buffer.text(), "foo\n\tbar");
        assert_eq!(buffer.cursor(), 0);
        buffer.move_down(false);
        assert!(buffer.unindent());
        assert_eq!(buffer.text(), "foo\nbar");
    }

    #[test]
    fn cursor_and_deletion_respect_utf8_boundaries() {
        let mut buffer = EditorBuffer::new("aé🙂b");
        buffer.move_right(false);
        buffer.move_right(false);
        assert_eq!(buffer.cursor_position().column, 2);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "a🙂b");
        buffer.insert_text("界");
        assert_eq!(buffer.text(), "a界🙂b");
        assert!(buffer.text().is_char_boundary(buffer.cursor()));
    }

    #[test]
    fn word_navigation_respects_unicode_and_shift_selection() {
        let text = "uno café 世界";
        let mut buffer = EditorBuffer::new(text);

        assert!(buffer.move_word_right(false));
        assert_eq!(&text[..buffer.cursor()], "uno ");
        assert!(buffer.move_word_right(true));
        assert_eq!(buffer.selected_text(), Some("café "));

        assert!(buffer.move_word_left(false));
        assert_eq!(buffer.cursor(), "uno ".len());
        buffer.move_document_end(false);
        assert!(buffer.move_word_left(false));
        assert_eq!(&text[..buffer.cursor()], "uno café ");
        assert!(buffer.move_word_left(true));
        assert_eq!(buffer.selected_text(), Some("café "));
        assert!(!buffer.undo());
    }

    #[test]
    fn word_deletion_handles_utf8_selection_and_undo() {
        let original = "🙂 café 世界";
        let mut buffer = EditorBuffer::new(original);

        assert!(buffer.delete_word_forward());
        assert_eq!(buffer.text(), "café 世界");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), original);

        buffer.move_document_end(false);
        assert!(buffer.delete_word_backward());
        assert_eq!(buffer.text(), "🙂 café ");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), original);
        assert!(!buffer.undo());

        buffer.move_document_start(false);
        assert!(buffer.move_word_right(true));
        assert_eq!(buffer.selected_text(), Some("🙂 "));
        assert!(buffer.delete_word_forward());
        assert_eq!(buffer.text(), "café 世界");
    }

    #[test]
    fn document_navigation_respects_unicode_and_shift_selection() {
        let text = "á\n世界";
        let mut buffer = EditorBuffer::new(text);

        assert!(buffer.move_document_end(false));
        assert_eq!(buffer.cursor(), text.len());
        assert!(buffer.move_document_start(true));
        assert_eq!(buffer.selected_text(), Some(text));

        assert!(buffer.move_document_end(false));
        assert_eq!(buffer.selection_range(), None);
        assert_eq!(buffer.cursor(), text.len());
        assert!(buffer.move_document_start(false));
        assert_eq!(buffer.cursor(), 0);
        assert!(!buffer.undo());
    }

    #[test]
    fn mouse_cursor_and_word_selection_respect_utf8_offsets() {
        let mut buffer = EditorBuffer::new("uno café mundo\notra");
        let cafe_offset = "uno ca".len();
        let offset = buffer.offset_at_position(Position { line: 0, column: 6 });
        assert_eq!(offset, cafe_offset);
        assert!(buffer.select_word_at(offset));
        assert_eq!(buffer.selected_text(), Some("café"));

        let other_line = buffer.offset_at_position(Position { line: 1, column: 2 });
        assert!(buffer.set_cursor(other_line, false));
        assert_eq!(buffer.cursor_position(), Position { line: 1, column: 2 });
    }

    #[test]
    fn select_next_occurrence_advances_and_wraps() {
        let mut buffer = EditorBuffer::new("uno dos uno");
        buffer.set_cursor(1, false);
        assert!(buffer.select_next_occurrence());
        assert_eq!(buffer.selected_text(), Some("uno"));
        assert!(buffer.select_next_occurrence());
        assert_eq!(buffer.selected_text(), Some("uno"));
        assert_eq!(
            buffer.cursor_position(),
            Position {
                line: 0,
                column: 11
            }
        );
        assert!(buffer.select_next_occurrence());
        assert_eq!(buffer.cursor_position(), Position { line: 0, column: 3 });
    }

    #[test]
    fn triple_click_line_selection_includes_its_newline() {
        let mut buffer = EditorBuffer::new("uno\ndos");
        assert!(buffer.select_line(0));
        assert_eq!(buffer.selected_text(), Some("uno\n"));
        assert!(buffer.select_line(1));
        assert_eq!(buffer.selected_text(), Some("dos"));
    }

    #[test]
    fn selection_replaces_text_and_preserves_newlines() {
        let mut buffer = EditorBuffer::new("uno\ndos");
        buffer.select_all();
        assert_eq!(buffer.selected_text(), Some("uno\ndos"));
        buffer.insert_text("tres\n四");
        assert_eq!(buffer.text(), "tres\n四");
        assert_eq!(buffer.line_count(), 2);

        buffer.select_all();
        buffer.insert_text("uno\r\ndos");
        assert_eq!(buffer.text(), "uno\ndos");
    }

    #[test]
    fn undo_redo_tracks_edits_and_clears_redo_after_new_edit() {
        let mut buffer = EditorBuffer::new("uno");
        buffer.move_end(false);
        buffer.insert_text("!");
        assert!(buffer.is_dirty());
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "uno");
        assert!(!buffer.is_dirty());
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "uno!");
        assert!(buffer.undo());
        buffer.insert_text("?");
        assert!(!buffer.redo());
        assert_eq!(buffer.text(), "uno?");
    }

    #[test]
    fn external_reload_preserves_cursor_position_and_discards_old_undo_history() {
        let mut buffer = EditorBuffer::new("first\n🙂old");
        buffer.set_cursor("first\n🙂".len(), false);
        buffer.reload("new\n🙂replacement".into());
        assert_eq!(buffer.cursor_position(), Position { line: 1, column: 1 });
        assert_eq!(buffer.text(), "new\n🙂replacement");
        assert!(!buffer.is_dirty());
        assert!(!buffer.undo());

        buffer.move_document_end(false);
        buffer.reload("short".into());
        assert_eq!(buffer.cursor(), 5);
    }

    #[test]
    fn typing_groups_words_but_not_navigation_newlines_or_paste() {
        let mut buffer = EditorBuffer::new("");
        for ch in ["h", "o", "l", "a", " ", "🙂"] {
            buffer.insert_text(ch);
        }
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hola ");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hola");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "");
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "hola");
        buffer.insert_text(" mundo");
        assert!(!buffer.redo());
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hola");
        buffer.insert_text("!");
        buffer.move_left(false);
        buffer.insert_text("?");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hola!");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hola");
        buffer.insert_text("\n");
        buffer.insert_text("x");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "hola\n");
    }

    #[test]
    fn line_clipboard_and_deletion_handle_last_line_and_empty_lines() {
        let mut buffer = EditorBuffer::new("á🙂\nlast");
        buffer.set_cursor(2, false);
        assert_eq!(buffer.current_line_for_clipboard(), "á🙂\n");
        assert!(buffer.delete_line());
        assert_eq!(buffer.text(), "last");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "á🙂\nlast");
        buffer.move_document_end(false);
        assert_eq!(buffer.current_line_for_clipboard(), "last\n");
        assert!(buffer.delete_line());
        assert_eq!(buffer.text(), "á🙂");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "á🙂\nlast");

        let mut buffer = EditorBuffer::new("a\n");
        buffer.move_document_end(false);
        assert_eq!(buffer.current_line_for_clipboard(), "\n");
        assert!(buffer.delete_line());
        assert_eq!(buffer.text(), "a");
        let mut buffer = EditorBuffer::new("");
        assert_eq!(buffer.current_line_for_clipboard(), "\n");
        assert!(!buffer.delete_line());
    }

    #[test]
    fn duplicate_line_preserves_cursor_selection_and_undo() {
        let mut buffer = EditorBuffer::new("a\n🙂b\nz");
        buffer.set_cursor("a\n🙂".len(), false);
        assert!(buffer.duplicate_line_down());
        assert_eq!(buffer.text(), "a\n🙂b\n🙂b\nz");
        assert_eq!(buffer.cursor_position(), Position { line: 2, column: 1 });
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "a\n🙂b\nz");
        assert_eq!(buffer.cursor_position(), Position { line: 1, column: 1 });

        buffer.set_selection(0.."a\n🙂b\n".len());
        assert!(buffer.duplicate_line_down());
        assert_eq!(buffer.text(), "a\n🙂b\na\n🙂b\nz");
        assert_eq!(buffer.selected_text(), Some("a\n🙂b\n"));
        buffer.move_document_end(false);
        assert!(buffer.duplicate_line_down());
        assert_eq!(buffer.text(), "a\n🙂b\na\n🙂b\nz\nz");

        let mut buffer = EditorBuffer::new("a\n");
        assert!(buffer.duplicate_line_down());
        assert_eq!(buffer.text(), "a\na\n");
    }

    #[test]
    fn moving_lines_moves_the_cursor_and_selected_lines() {
        let mut buffer = EditorBuffer::new("a\nb\nc");
        buffer.move_down(false);
        buffer.move_line_down();
        assert_eq!(buffer.text(), "a\nc\nb");
        assert_eq!(buffer.cursor_position(), Position { line: 2, column: 0 });

        buffer.move_home(false);
        buffer.move_up(false);
        buffer.move_home(false);
        buffer.move_down(true);
        buffer.move_right(true);
        buffer.move_line_up();
        assert_eq!(buffer.text(), "c\nb\na");
        assert_eq!(buffer.selected_text(), Some("c\nb"));
    }

    #[test]
    fn moving_lines_preserves_the_final_newline_without_moving_synthetic_line() {
        let mut buffer = EditorBuffer::new("a\nb\n");
        assert!(buffer.move_line_down());
        assert_eq!(buffer.text(), "b\na\n");
        assert!(!buffer.move_line_down());
        assert_eq!(buffer.text(), "b\na\n");
        assert!(buffer.move_line_up());
        assert_eq!(buffer.text(), "a\nb\n");

        let mut buffer = EditorBuffer::new("a\nb");
        assert!(buffer.move_line_down());
        assert_eq!(buffer.text(), "b\na");
        assert!(buffer.move_line_up());
        assert_eq!(buffer.text(), "a\nb");
    }

    #[test]
    fn moving_real_empty_lines_keeps_the_final_newline() {
        let mut buffer = EditorBuffer::new("a\n\nb\n");
        buffer.move_down(false);
        assert!(buffer.move_line_down());
        assert_eq!(buffer.text(), "a\nb\n\n");
        assert!(buffer.move_line_up());
        assert_eq!(buffer.text(), "a\n\nb\n");
    }

    #[test]
    fn vertical_navigation_collapses_selection_without_an_extra_move() {
        let mut buffer = EditorBuffer::new("uno\ndos\ntres");
        buffer.select_all();
        buffer.preferred_column = Some(2);
        assert!(buffer.move_up(false));
        assert_eq!(buffer.cursor(), 0);
        assert_eq!(buffer.selection_range(), None);
        assert_eq!(buffer.preferred_column, None);

        buffer.select_all();
        buffer.preferred_column = Some(2);
        assert!(buffer.move_down(false));
        assert_eq!(buffer.cursor(), buffer.text().len());
        assert_eq!(buffer.selection_range(), None);
        assert_eq!(buffer.preferred_column, None);
    }
}
