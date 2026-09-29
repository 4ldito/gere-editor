mod buffer;
mod csv;
mod definition;
mod highlight;
mod icons;
mod lint;
mod project;
mod session;
mod settings;

use gpui::{
    anchored, deferred, div, img, point, prelude::*, px, rgb, rgba, size, uniform_list, App,
    Bounds, ClipboardItem, Context, ExternalPaths, FocusHandle, HighlightStyle, KeyDownEvent,
    ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, PathPromptOptions, Pixels, ScrollStrategy, StatefulInteractiveElement, StyledText,
    UniformListScrollHandle, Window, WindowBounds, WindowDecorations, WindowOptions,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

const BG: u32 = 0x21252b;
const PANEL: u32 = 0x282c34;
const FG: u32 = 0xabb2bf;
const MUTED: u32 = 0x7f848e;
#[cfg(test)]
const CODE_TEXT_LEFT: f32 = 390.;
const CODE_CELL_LEFT: f32 = 56.;
#[cfg(test)]
const EDITOR_AREA_LEFT: f32 = 334.;
const RAIL_WIDTH: f32 = 46.;
const SIDEBAR_MIN: f32 = 160.;
const SIDEBAR_MAX: f32 = 600.;
const EDITOR_MIN: f32 = 180.;

fn sidebar_width(window_width: Pixels, requested: Pixels) -> Pixels {
    requested.clamp(
        px(SIDEBAR_MIN),
        (window_width - px(RAIL_WIDTH + EDITOR_MIN))
            .max(px(SIDEBAR_MIN))
            .min(px(SIDEBAR_MAX)),
    )
}

fn split_left_width(window_width: Pixels, fraction: f32, editor_left: Pixels) -> Pixels {
    let available = (window_width - editor_left).max(px(0.));
    if available <= px(280.) {
        available / 2.
    } else {
        (available * fraction).clamp(px(140.), available - px(140.))
    }
}

fn project_relative_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let path = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => relative.push(component),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sidebar {
    Files,
    Search,
    Git,
}

enum GitOperation {
    Commit(String),
    Push,
    Sync,
    SwitchBranch(String),
    ApplyStash(String),
    StageAll,
    UnstageAll,
    DiscardAll,
}

#[derive(Clone)]
enum FileEdit {
    Create(PathBuf),
    CreateFolder(PathBuf),
    Rename(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplorerRow {
    Entry(usize, usize),
    NewFile(usize),
}

#[derive(Clone)]
struct ExplorerDrag(PathBuf);

impl Render for ExplorerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0x3e4451))
            .text_color(rgb(FG))
            .shadow_md()
            .child(icons::file_icon(&self.0))
            .child(
                self.0
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
    }
}

fn can_move_into(path: &Path, folder: &Path) -> bool {
    path.parent() != Some(folder) && !folder.starts_with(path)
}

fn selected_folder(selected: Option<&PathBuf>, files: &[project::FileEntry]) -> PathBuf {
    selected
        .filter(|path| {
            files
                .iter()
                .any(|entry| entry.path == **path && entry.is_dir)
        })
        .cloned()
        .unwrap_or_default()
}

fn explorer_rows(
    visible: &[(usize, usize)],
    files: &[project::FileEntry],
    edit: Option<&FileEdit>,
) -> Vec<ExplorerRow> {
    let mut rows: Vec<_> = visible
        .iter()
        .map(|&(index, depth)| ExplorerRow::Entry(index, depth))
        .collect();
    if let Some(FileEdit::Create(parent) | FileEdit::CreateFolder(parent)) = edit {
        let (at, depth) = if parent.as_os_str().is_empty() {
            (0, 0)
        } else if let Some(at) = visible
            .iter()
            .position(|(index, _)| files[*index].path == *parent)
        {
            (at + 1, visible[at].1 + 1)
        } else {
            return rows;
        };
        rows.insert(at, ExplorerRow::NewFile(depth));
    }
    rows
}

fn file_tree(entries: &[project::FileEntry]) -> BTreeMap<PathBuf, Vec<usize>> {
    let mut children: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        children
            .entry(entry.path.parent().unwrap_or(Path::new("")).to_path_buf())
            .or_default()
            .push(index);
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| {
            let a = &entries[*a];
            let b = &entries[*b];
            b.is_dir.cmp(&a.is_dir).then_with(|| {
                a.path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(
                        &b.path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase(),
                    )
            })
        });
    }
    children
}

fn visible_entries(
    parent: &Path,
    depth: usize,
    entries: &[project::FileEntry],
    children: &BTreeMap<PathBuf, Vec<usize>>,
    expanded: &HashSet<PathBuf>,
    result: &mut Vec<(usize, usize)>,
) {
    if let Some(siblings) = children.get(parent) {
        for &index in siblings {
            let entry = &entries[index];
            result.push((index, depth));
            if entry.is_dir && expanded.contains(&entry.path) {
                visible_entries(&entry.path, depth + 1, entries, children, expanded, result);
            }
        }
    }
}

fn active_after_close(active: Option<usize>, closed: usize, remaining: usize) -> Option<usize> {
    match active {
        Some(index) if index == closed => (remaining > 0).then(|| closed.min(remaining - 1)),
        Some(index) if index > closed => Some(index - 1),
        other => other,
    }
}

fn matching_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
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

#[derive(Default)]
struct SingleLineInput {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

impl SingleLineInput {
    fn set_text(&mut self, text: String) {
        self.cursor = text.len();
        self.anchor = None;
        self.text = text;
    }

    fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    fn selected_text(&self) -> Option<&str> {
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

    fn replace_selection(&mut self, insert: &str) -> bool {
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
    fn handle(&mut self, event: &KeyDownEvent, paste: Option<&str>) -> bool {
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
        } else if !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.platform
            && !key.modifiers.function
        {
            if let Some(text) = &key.key_char {
                if !text.chars().any(char::is_control) {
                    return self.replace_selection(text);
                }
            }
        }
        false
    }

    fn display(&self, width: usize) -> (StyledText, usize) {
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

fn sidebar_input_columns(sidebar: Pixels, reserved: f32, cell_width: Pixels) -> usize {
    ((f32::from(sidebar) - reserved) / f32::from(cell_width))
        .floor()
        .max(3.) as usize
}

fn input_view(
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
                    .top(px(if compact { 4. } else { 6. }))
                    .w(px(1.5))
                    .h(px(if compact { 14. } else { 17. }))
                    .bg(rgb(0x61afef)),
            )
        })
}

fn max_line_chars(text: &str) -> usize {
    text.split('\n')
        .map(|line| buffer::visual_column(line, line.chars().count()))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
fn code_column_at_x(
    window_x: Pixels,
    scroll_x: Pixels,
    cell_width: Pixels,
    line_chars: usize,
) -> usize {
    code_column_at_x_from(
        window_x,
        scroll_x,
        cell_width,
        line_chars,
        px(CODE_TEXT_LEFT),
    )
}

fn code_column_at_x_from(
    window_x: Pixels,
    scroll_x: Pixels,
    cell_width: Pixels,
    line_chars: usize,
    text_left: Pixels,
) -> usize {
    let column = (window_x + scroll_x - text_left) / cell_width;
    // Keep a click on the glyph in that glyph; only its rightmost part advances.
    ((column + 0.3).floor().max(0.) as usize).min(line_chars)
}

fn utf16_byte_column(line: &str, column: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in line.char_indices() {
        if units >= column {
            return byte;
        }
        units += ch.len_utf16();
    }
    line.len()
}

fn scroll_editor_line(handle: &UniformListScrollHandle, line: usize, center: bool) {
    if center {
        handle.scroll_to_item_strict(line, ScrollStrategy::Center);
    } else {
        handle.scroll_to_item(line, ScrollStrategy::Center);
    }
}

fn scrollbar_thumb(
    viewport: Pixels,
    track: Pixels,
    max_scroll: Pixels,
    scroll: Pixels,
) -> (Pixels, Pixels) {
    let thumb = (viewport * (viewport / (viewport + max_scroll)))
        .max(px(28.))
        .min(track);
    let travel = track - thumb;
    let position = if max_scroll > px(0.) {
        travel * (scroll / max_scroll).clamp(0., 1.)
    } else {
        px(0.)
    };
    (thumb, position)
}

fn scrollbar_target(
    pointer: Pixels,
    start: Pixels,
    track: Pixels,
    thumb: Pixels,
    max: Pixels,
) -> Pixels {
    let travel = track - thumb;
    if travel <= px(0.) {
        return px(0.);
    }
    ((pointer - start - thumb / 2.) / travel * max).clamp(px(0.), max)
}

#[cfg(test)]
mod explorer_tests {
    use super::*;

    #[test]
    fn new_file_toolbar_uses_selected_folder_only() {
        let files = [
            project::FileEntry {
                path: "src".into(),
                is_dir: true,
            },
            project::FileEntry {
                path: "src/main.rs".into(),
                is_dir: false,
            },
        ];
        let folder = PathBuf::from("src");
        let file = PathBuf::from("src/main.rs");
        assert_eq!(selected_folder(Some(&folder), &files), folder);
        assert_eq!(selected_folder(Some(&file), &files), PathBuf::new());
        assert_eq!(selected_folder(None, &files), PathBuf::new());
    }

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
    fn folders_come_first_and_only_expanded_children_are_visible() {
        let entries = [
            project::FileEntry {
                path: "readme.md".into(),
                is_dir: false,
            },
            project::FileEntry {
                path: "src/main.rs".into(),
                is_dir: false,
            },
            project::FileEntry {
                path: "src".into(),
                is_dir: true,
            },
            project::FileEntry {
                path: "empty".into(),
                is_dir: true,
            },
        ];
        let tree = file_tree(&entries);
        let expanded = HashSet::new();
        let mut visible = Vec::new();
        visible_entries(Path::new(""), 0, &entries, &tree, &expanded, &mut visible);
        let collapsed: Vec<_> = visible
            .iter()
            .map(|(index, depth)| (entries[*index].path.as_path(), *depth))
            .collect();
        assert_eq!(
            collapsed,
            [
                (Path::new("empty"), 0),
                (Path::new("src"), 0),
                (Path::new("readme.md"), 0)
            ]
        );

        let expanded = HashSet::from([PathBuf::from("src")]);
        visible.clear();
        visible_entries(Path::new(""), 0, &entries, &tree, &expanded, &mut visible);
        let open: Vec<_> = visible
            .iter()
            .map(|(index, depth)| (entries[*index].path.as_path(), *depth))
            .collect();
        assert_eq!(
            open,
            [
                (Path::new("empty"), 0),
                (Path::new("src"), 0),
                (Path::new("src/main.rs"), 1),
                (Path::new("readme.md"), 0)
            ]
        );
    }

    #[test]
    fn new_file_row_follows_its_parent_and_rename_keeps_the_original_row() {
        let files = [
            project::FileEntry {
                path: "src".into(),
                is_dir: true,
            },
            project::FileEntry {
                path: "src/main.rs".into(),
                is_dir: false,
            },
            project::FileEntry {
                path: "readme.md".into(),
                is_dir: false,
            },
        ];
        let visible = [(0, 0), (1, 1), (2, 0)];
        assert_eq!(
            explorer_rows(&visible, &files, Some(&FileEdit::Create("src".into()))),
            [
                ExplorerRow::Entry(0, 0),
                ExplorerRow::NewFile(1),
                ExplorerRow::Entry(1, 1),
                ExplorerRow::Entry(2, 0)
            ]
        );
        assert_eq!(
            explorer_rows(&visible, &files, Some(&FileEdit::Create(PathBuf::new()))),
            [
                ExplorerRow::NewFile(0),
                ExplorerRow::Entry(0, 0),
                ExplorerRow::Entry(1, 1),
                ExplorerRow::Entry(2, 0)
            ]
        );
        assert_eq!(
            explorer_rows(
                &visible,
                &files,
                Some(&FileEdit::Rename("src/main.rs".into()))
            ),
            [
                ExplorerRow::Entry(0, 0),
                ExplorerRow::Entry(1, 1),
                ExplorerRow::Entry(2, 0)
            ]
        );
    }

    #[test]
    fn closing_tabs_preserves_the_visible_tab_and_chooses_a_neighbor() {
        assert_eq!(active_after_close(Some(2), 0, 2), Some(1));
        assert_eq!(active_after_close(Some(0), 1, 2), Some(0));
        assert_eq!(active_after_close(Some(1), 1, 2), Some(1));
        assert_eq!(active_after_close(Some(2), 2, 2), Some(1));
        assert_eq!(active_after_close(Some(0), 0, 0), None);
    }

    #[test]
    fn find_matches_are_case_insensitive_and_utf8_safe() {
        assert_eq!(matching_ranges("Gato gato", "GATO"), [0..4, 5..9]);
        assert_eq!(matching_ranges("á🙂Á🙂", "á🙂"), [0..6, 6..12]);
        assert!(matching_ranges("texto", "").is_empty());
    }

    #[test]
    fn editor_click_column_accounts_for_horizontal_scroll() {
        let cell = px(8.4);
        let click = px(CODE_TEXT_LEFT + 2. * 8.4 + 1.);
        assert_eq!(code_column_at_x(click, px(0.), cell, 20), 2);
        assert_eq!(code_column_at_x(click, cell, cell, 20), 3);
        let right_origin = px(CODE_TEXT_LEFT + 320.);
        assert_eq!(
            code_column_at_x_from(click + px(320.), px(0.), cell, 20, right_origin),
            2
        );
        assert_eq!(
            code_column_at_x_from(click + px(320.), cell, cell, 20, right_origin),
            3
        );
        assert_eq!(
            code_column_at_x(px(CODE_TEXT_LEFT + 2. * 8.4 + 5.), px(0.), cell, 20),
            2
        );
        assert_eq!(
            code_column_at_x(px(CODE_TEXT_LEFT + 2. * 8.4 + 6.), px(0.), cell, 20),
            3
        );
    }

    #[test]
    fn definition_scroll_centers_only_the_destination_tab() {
        let source = UniformListScrollHandle::new();
        let destination = UniformListScrollHandle::new();
        scroll_editor_line(&source, 110, false);
        scroll_editor_line(&destination, 12, true);
        let source_scroll = source.0.borrow();
        let destination_scroll = destination.0.borrow();
        assert_eq!(
            source_scroll.deferred_scroll_to_item.unwrap().item_index,
            110
        );
        let target = destination_scroll.deferred_scroll_to_item.unwrap();
        assert_eq!(target.item_index, 12);
        assert!(target.scroll_strict);
    }

    #[test]
    fn scrollbar_thumb_has_a_minimum_size_and_tracks_scroll_position() {
        let (thumb, start) = scrollbar_thumb(px(100.), px(100.), px(300.), px(0.));
        assert_eq!(thumb, px(28.));
        assert_eq!(start, px(0.));
        let (_, end) = scrollbar_thumb(px(100.), px(100.), px(300.), px(300.));
        assert_eq!(end, px(72.));
        assert_eq!(
            scrollbar_target(px(50.), px(0.), px(100.), thumb, px(300.)),
            px(150.)
        );
        assert_eq!(
            scrollbar_target(px(0.), px(0.), px(100.), thumb, px(300.)),
            px(0.)
        );
    }

    #[test]
    fn quick_open_parses_line_without_breaking_colon_in_filename() {
        assert_eq!(
            palette_target("src/main.rs:500"),
            ("src/main.rs", Some(500))
        );
        assert_eq!(palette_target(":500"), ("", Some(500)));
        assert_eq!(palette_target("file:part.js"), ("file:part.js", None));
        assert_eq!(palette_target("file:0"), ("file:0", None));
    }

    #[test]
    fn palette_paths_are_normalized_only_inside_the_project() {
        let root = Path::new("/workspace/project");
        assert_eq!(
            project_relative_path(root, Path::new("./src/main.rs")),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(
            project_relative_path(root, Path::new("/workspace/project/src/main.rs")),
            Some(PathBuf::from("src/main.rs"))
        );
        assert_eq!(project_relative_path(root, Path::new("/tmp/main.rs")), None);
        assert_eq!(project_relative_path(root, Path::new("../main.rs")), None);
    }

    #[test]
    fn global_results_group_matches_under_each_file() {
        let matches = [
            project::Match {
                path: "b.rs".into(),
                line: 2,
                text: "hi".into(),
                start: 0,
                end: 2,
            },
            project::Match {
                path: "a.rs".into(),
                line: 1,
                text: "hi".into(),
                start: 0,
                end: 2,
            },
            project::Match {
                path: "b.rs".into(),
                line: 4,
                text: "hi".into(),
                start: 0,
                end: 2,
            },
        ];
        let rows = search_rows(&matches, &HashSet::new());
        assert!(matches!(rows.as_slice(), [
            SearchRow::File(path_a, 1), SearchRow::Match(1),
            SearchRow::File(path_b, 2), SearchRow::Match(0), SearchRow::Match(2)
        ] if path_a == Path::new("a.rs") && path_b == Path::new("b.rs")));
        let collapsed = search_rows(&matches, &HashSet::from([PathBuf::from("b.rs")]));
        assert!(matches!(
            collapsed.as_slice(),
            [
                SearchRow::File(_, 1),
                SearchRow::Match(1),
                SearchRow::File(_, 2)
            ]
        ));
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
    fn preview_keeps_match_visible_when_line_starts_far_to_the_left() {
        let found = project::Match {
            path: "x.rs".into(),
            line: 2,
            text: format!("{}hola al vendedor", "palabra ".repeat(20)),
            start: "palabra ".repeat(20).len(),
            end: "palabra ".repeat(20).len() + 4,
        };
        let (text, range) = preview_context(&found);
        assert!(text.starts_with('…'));
        assert_eq!(&text[range], "hola");
        assert!(text.chars().count() < found.text.chars().count());
    }
}

struct Tab {
    path: PathBuf,
    scroll: UniformListScrollHandle,
    buffer: buffer::EditorBuffer,
    lines: Vec<highlight::HighlightedLine>,
    diagnostics: Vec<highlight::Diagnostic>,
    lint_source: Option<String>,
    lint_diagnostics: Vec<highlight::Diagnostic>,
    csv: Option<csv::Layout>,
    max_line_chars: usize,
    loading: bool,
    loaded_stamp: Option<project::FileStamp>,
}

fn lint_for_unchanged_lines(
    previous: &str,
    current: &str,
    diagnostics: &[highlight::Diagnostic],
) -> Vec<highlight::Diagnostic> {
    let old_lines = previous.split('\n').collect::<Vec<_>>();
    let new_lines = current.split('\n').collect::<Vec<_>>();
    if old_lines.len() != new_lines.len() {
        return Vec::new();
    }
    diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let old = *old_lines.get(diagnostic.line)?;
            let new = *new_lines.get(diagnostic.line)?;
            let mut preserved = diagnostic.clone();
            if old != new {
                let prefix = old
                    .bytes()
                    .zip(new.bytes())
                    .take_while(|(left, right)| left == right)
                    .count();
                if diagnostic.range.end <= prefix {
                    // The edit is after this underline.
                } else {
                    let suffix = old
                        .bytes()
                        .rev()
                        .zip(new.bytes().rev())
                        .take_while(|(left, right)| left == right)
                        .count()
                        .min(old.len() - prefix)
                        .min(new.len() - prefix);
                    if diagnostic.range.start < old.len() - suffix {
                        return None;
                    }
                    let shift = new.len() as isize - old.len() as isize;
                    preserved.range = diagnostic.range.start.checked_add_signed(shift)?
                        ..diagnostic.range.end.checked_add_signed(shift)?;
                }
            }
            Some(preserved)
        })
        .collect()
}

#[cfg(test)]
mod lint_display_tests {
    use super::*;

    #[test]
    fn editing_one_line_keeps_only_lint_from_unchanged_lines() {
        let diagnostics = (0..3)
            .map(|line| highlight::Diagnostic {
                line,
                range: 0..1,
                message: "lint".into(),
                severity: highlight::DiagnosticSeverity::Warning,
            })
            .collect::<Vec<_>>();
        let kept = lint_for_unchanged_lines("one\ntwo\nthree", "one\nchanged\nthree", &diagnostics);
        assert_eq!(
            kept.iter()
                .map(|diagnostic| diagnostic.line)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        let shifted =
            lint_for_unchanged_lines("one\ntwo\nthree", "new\none\ntwo\nthree", &diagnostics);
        assert!(shifted.is_empty());
        let after_word = lint_for_unchanged_lines("unused foo", "unused foo!", &diagnostics[..1]);
        assert_eq!(after_word[0].range, 0..1);
        let before_word = lint_for_unchanged_lines("x unused", "long x unused", &diagnostics[..1]);
        assert_eq!(before_word[0].range, 5..6);
    }
}

#[derive(Clone, Copy)]
struct EditorScrollMetrics {
    viewport_width: Pixels,
    viewport_height: Pixels,
    scroll_x: Pixels,
    scroll_y: Pixels,
    max_x: Pixels,
    max_y: Pixels,
}

#[derive(Clone, Copy)]
enum EditorScrollbarDrag {
    Horizontal {
        pointer_start: Pixels,
        scroll_start: Pixels,
    },
    Vertical {
        pointer_start: Pixels,
        scroll_start: Pixels,
    },
}

struct DiffRow {
    raw: String,
    before: String,
    after: String,
}

#[derive(Debug, PartialEq, Eq)]
struct AlignedLine {
    before: Option<usize>,
    after: Option<usize>,
    before_range: Option<std::ops::Range<usize>>,
    after_range: Option<std::ops::Range<usize>>,
}

fn changed_text_ranges(
    before: &str,
    after: &str,
) -> (
    Option<std::ops::Range<usize>>,
    Option<std::ops::Range<usize>>,
) {
    let prefix = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let suffix = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(ch, _)| ch.len_utf8())
        .sum::<usize>();
    let old = prefix..before.len() - suffix;
    let new = prefix..after.len() - suffix;
    (
        (old.start < old.end).then_some(old),
        (new.start < new.end).then_some(new),
    )
}

fn aligned_lines(
    before: &[&str],
    after: &[&str],
    matches: &[(usize, usize)],
) -> (Vec<AlignedLine>, Vec<usize>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut before_to_visual = vec![0; before.len()];
    let mut after_to_visual = vec![0; after.len()];
    let (mut old, mut new) = (0, 0);
    let mut append = |old_end: usize, new_end: usize, context: bool| {
        let count = (old_end - old).max(new_end - new);
        for index in 0..count {
            let left = (old + index < old_end).then_some(old + index);
            let right = (new + index < new_end).then_some(new + index);
            let (before_range, after_range) = if context {
                (None, None)
            } else {
                changed_text_ranges(
                    left.map_or("", |i| before[i]),
                    right.map_or("", |i| after[i]),
                )
            };
            if let Some(i) = left {
                before_to_visual[i] = rows.len();
            }
            if let Some(i) = right {
                after_to_visual[i] = rows.len();
            }
            rows.push(AlignedLine {
                before: left,
                after: right,
                before_range,
                after_range,
            });
        }
        old = old_end;
        new = new_end;
    };
    for &(old_match, new_match) in matches {
        append(old_match, new_match, false);
        append(old_match + 1, new_match + 1, true);
    }
    append(before.len(), after.len(), false);
    (rows, before_to_visual, after_to_visual)
}

#[derive(Default)]
struct DiffHighlights {
    removed: HashSet<usize>,
    added: HashSet<usize>,
    deletion_anchors: HashSet<usize>,
    first_before: Option<usize>,
    first_after: Option<usize>,
    layout: Vec<AlignedLine>,
    before_to_visual: Vec<usize>,
    after_to_visual: Vec<usize>,
}

impl DiffHighlights {
    fn first_visual(&self) -> Option<usize> {
        self.first_before
            .and_then(|line| self.before_to_visual.get(line))
            .into_iter()
            .chain(
                self.first_after
                    .and_then(|line| self.after_to_visual.get(line)),
            )
            .copied()
            .min()
    }
}

fn line_diff_highlights(original: &str, current: &str) -> DiffHighlights {
    let before: Vec<_> = original.split('\n').collect();
    let after: Vec<_> = current.split('\n').collect();
    let mut marks = DiffHighlights::default();
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut old_end = before.len();
    let mut new_end = after.len();
    while old_end > prefix && new_end > prefix && before[old_end - 1] == after[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    let old_len = old_end - prefix;
    let new_len = new_end - prefix;
    let mut matches: Vec<_> = (0..prefix).map(|i| (i, i)).collect();
    if old_len == 0 && new_len == 0 {
        matches.extend((0..before.len() - old_end).map(|i| (old_end + i, new_end + i)));
        let (layout, before_to_visual, after_to_visual) = aligned_lines(&before, &after, &matches);
        marks.layout = layout;
        marks.before_to_visual = before_to_visual;
        marks.after_to_visual = after_to_visual;
        return marks;
    }

    // Bound work on very large edits; normal edits use an exact line-level LCS.
    if old_len.saturating_mul(new_len) > 400_000 {
        marks.removed.extend(prefix..old_end);
        marks.added.extend(prefix..new_end);
        if new_len == 0 {
            marks.deletion_anchors.insert(prefix.min(after.len() - 1));
        }
    } else {
        let width = new_len + 1;
        let mut lcs = vec![0u32; (old_len + 1) * width];
        for i in (0..old_len).rev() {
            for j in (0..new_len).rev() {
                lcs[i * width + j] = if before[prefix + i] == after[prefix + j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        let mut deletion_at: Option<usize> = None;
        while i < old_len || j < new_len {
            if i < old_len && j < new_len && before[prefix + i] == after[prefix + j] {
                matches.push((prefix + i, prefix + j));
                if let Some(anchor) = deletion_at.take() {
                    marks.deletion_anchors.insert(anchor.min(after.len() - 1));
                }
                i += 1;
                j += 1;
            } else if i < old_len
                && (j == new_len || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
            {
                marks.removed.insert(prefix + i);
                deletion_at.get_or_insert(prefix + j);
                i += 1;
            } else {
                marks.added.insert(prefix + j);
                deletion_at = None;
                j += 1;
            }
        }
        if let Some(anchor) = deletion_at {
            marks.deletion_anchors.insert(anchor.min(after.len() - 1));
        }
    }
    matches.extend((0..before.len() - old_end).map(|i| (old_end + i, new_end + i)));
    let (layout, before_to_visual, after_to_visual) = aligned_lines(&before, &after, &matches);
    marks.layout = layout;
    marks.before_to_visual = before_to_visual;
    marks.after_to_visual = after_to_visual;
    marks.first_before = Some(prefix.min(before.len() - 1));
    marks.first_after = Some(prefix.min(after.len() - 1));
    marks
}

fn aligned_diff(text: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let flush = |rows: &mut Vec<DiffRow>, removed: &mut Vec<String>, added: &mut Vec<String>| {
        let count = removed.len().max(added.len());
        for i in 0..count {
            let before = removed.get(i).cloned().unwrap_or_default();
            let after = added.get(i).cloned().unwrap_or_default();
            rows.push(DiffRow {
                raw: format!("{before}  {after}"),
                before,
                after,
            });
        }
        removed.clear();
        added.clear();
    };
    for raw in text.lines() {
        if raw.starts_with("--- ") || raw.starts_with("+++ ") {
            continue;
        }
        if let Some(line) = raw.strip_prefix('-') {
            removed.push(format!("- {line}"));
        } else if let Some(line) = raw.strip_prefix('+') {
            added.push(format!("+ {line}"));
        } else {
            flush(&mut rows, &mut removed, &mut added);
            let display = raw.strip_prefix(' ').unwrap_or(raw).to_owned();
            rows.push(DiffRow {
                raw: raw.to_owned(),
                before: display.clone(),
                after: display,
            });
        }
    }
    flush(&mut rows, &mut removed, &mut added);
    rows
}

#[derive(Clone, Copy)]
enum ConflictChoice {
    Current,
    Incoming,
    Both,
}

#[derive(Clone, Copy)]
struct ConflictBlock {
    start: usize,
    divider: usize,
    end: usize,
}

fn conflict_blocks(text: &str) -> Vec<ConflictBlock> {
    let mut result = Vec::new();
    let mut start = None;
    let mut divider = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("<<<<<<< ") {
            start = Some(offset);
            divider = None;
        } else if line.trim_end_matches('\n') == "=======" && start.is_some() {
            divider = Some(offset);
        } else if line.starts_with(">>>>>>> ") {
            if let (Some(start), Some(divider)) = (start, divider) {
                result.push(ConflictBlock {
                    start,
                    divider,
                    end: offset + line.len(),
                });
            }
            start = None;
            divider = None;
        }
        offset += line.len();
    }
    result
}

fn resolve_conflict(text: &str, block: ConflictBlock, choice: ConflictChoice) -> String {
    let current_start = text[block.start..]
        .find('\n')
        .map_or(block.start, |n| block.start + n + 1);
    let incoming_start = text[block.divider..]
        .find('\n')
        .map_or(block.divider, |n| block.divider + n + 1);
    let current = &text[current_start..block.divider];
    let incoming = &text[incoming_start..text[..block.end].rfind(">>>>>>> ").unwrap_or(block.end)];
    match choice {
        ConflictChoice::Current => current.to_owned(),
        ConflictChoice::Incoming => incoming.to_owned(),
        ConflictChoice::Both => format!("{current}{incoming}"),
    }
}

#[cfg(test)]
mod git_view_tests {
    use super::*;

    #[test]
    fn aligned_rows_pad_insertions_and_deletions_without_losing_file_lines() {
        let marks = line_diff_highlights("a\nold\nkeep\nend\n", "a\nnew\nextra\nkeep\n");
        let pairs: Vec<_> = marks
            .layout
            .iter()
            .map(|row| (row.before, row.after))
            .collect();
        assert_eq!(
            pairs,
            [
                (Some(0), Some(0)),
                (Some(1), Some(1)),
                (None, Some(2)),
                (Some(2), Some(3)),
                (Some(3), None),
                (Some(4), Some(4))
            ]
        );
        assert_eq!(marks.before_to_visual[2], 3);
        assert_eq!(marks.after_to_visual[3], 3);
        assert_eq!(marks.layout[2].after_range, Some(0..5));
        assert!(marks.layout[2].before_range.is_none());
        assert_eq!(marks.layout[4].before_range, Some(0..3));
        assert!(marks.layout[4].after_range.is_none());
    }

    #[test]
    fn intraline_ranges_keep_utf8_boundaries_and_preserve_unchanged_text() {
        let (old, new) = changed_text_ranges("prefijo á🚀 final", "prefijo á🌟 final");
        assert_eq!(
            old.as_ref().map(|r| &"prefijo á🚀 final"[r.clone()]),
            Some("🚀")
        );
        assert_eq!(
            new.as_ref().map(|r| &"prefijo á🌟 final"[r.clone()]),
            Some("🌟")
        );
        assert_eq!(changed_text_ranges("igual", "igual"), (None, None));
        let marks = line_diff_highlights("a\nb\n", "a\nb\n");
        assert_eq!(marks.layout.len(), 3);
        assert!(marks
            .layout
            .iter()
            .all(|row| row.before_range.is_none() && row.after_range.is_none()));

        let (old, new) = changed_text_ranges("let label = \"really old\";", "let label = \"new\";");
        assert_eq!(
            old.map(|r| "let label = \"really old\";"[r].to_owned()),
            Some("really old".into())
        );
        assert_eq!(
            new.map(|r| "let label = \"new\";"[r].to_owned()),
            Some("new".into())
        );
    }

    #[test]
    fn first_change_targets_ghost_row_for_deletion_at_start() {
        let marks = line_diff_highlights("removed\nstill here\n", "still here\n");
        assert_eq!(marks.layout[0].before, Some(0));
        assert_eq!(marks.layout[0].after, None);
        assert_eq!(marks.after_to_visual[0], 1);
        assert_eq!(marks.first_visual(), Some(0));
    }

    #[test]
    fn line_highlights_track_multiple_edits_and_first_change() {
        let marks = line_diff_highlights(
            "same\nold\nkeep\nremove\ntail\n",
            "same\nnew\nkeep\ntail\nextra\n",
        );
        assert_eq!(marks.first_before, Some(1));
        assert_eq!(marks.first_after, Some(1));
        assert_eq!(marks.removed, HashSet::from([1, 3]));
        assert_eq!(marks.added, HashSet::from([1, 4]));
        assert!(marks.deletion_anchors.contains(&3));
        let insert = line_diff_highlights("a\nb\n", "a\nnew\nb\n");
        assert_eq!(insert.added, HashSet::from([1]));
        assert!(insert.removed.is_empty());
        let delete = line_diff_highlights("a\nb\n", "a\n");
        assert_eq!(delete.removed, HashSet::from([1]));
        assert_eq!(delete.deletion_anchors, HashSet::from([1]));
        assert!(line_diff_highlights("same\n", "same\n")
            .first_after
            .is_none());
    }

    #[test]
    fn divider_clamps_both_panels_and_follows_window_width() {
        assert_eq!(
            split_left_width(px(1134.), 0.5, px(EDITOR_AREA_LEFT)),
            px(400.)
        );
        assert_eq!(
            split_left_width(px(1134.), 0.0, px(EDITOR_AREA_LEFT)),
            px(140.)
        );
        assert_eq!(
            split_left_width(px(1134.), 1.0, px(EDITOR_AREA_LEFT)),
            px(660.)
        );
        assert_eq!(
            split_left_width(px(500.), 0.5, px(EDITOR_AREA_LEFT)),
            px(83.)
        );
        assert_eq!(sidebar_width(px(1200.), px(280.)), px(280.));
        assert_eq!(sidebar_width(px(500.), px(600.)), px(274.));
        assert_eq!(sidebar_width(px(1200.), px(0.)), px(160.));
        assert_eq!(split_left_width(px(1134.), 0.5, px(46.)), px(544.));
    }

    #[test]
    fn aligns_replacements_and_keeps_context_on_both_sides() {
        let rows = aligned_diff("@@ -1,3 +1,3 @@\n context\n-old\n+new\n+extra\n unchanged\n");
        assert_eq!(
            (&rows[1].before[..], &rows[1].after[..]),
            ("context", "context")
        );
        assert_eq!(
            (&rows[2].before[..], &rows[2].after[..]),
            ("- old", "+ new")
        );
        assert_eq!((&rows[3].before[..], &rows[3].after[..]), ("", "+ extra"));
    }

    #[test]
    fn resolves_multiple_conflicts_without_removing_surrounding_text() {
        let source = "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nafter\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> branch\n";
        let blocks = conflict_blocks(source);
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            resolve_conflict(source, blocks[0], ConflictChoice::Current),
            "ours\n"
        );
        assert_eq!(
            resolve_conflict(source, blocks[0], ConflictChoice::Incoming),
            "theirs\n"
        );
        assert_eq!(
            resolve_conflict(source, blocks[0], ConflictChoice::Both),
            "ours\ntheirs\n"
        );
        let mut buffer = buffer::EditorBuffer::new(source);
        buffer.set_selection(blocks[0].start..blocks[0].end);
        buffer.insert_text(&resolve_conflict(source, blocks[0], ConflictChoice::Both));
        assert!(buffer.text().starts_with("before\nours\ntheirs\nafter\n"));
        assert_eq!(conflict_blocks(buffer.text()).len(), 1);
    }
}

#[derive(Clone, Copy)]
enum GitRow {
    StagedHeader,
    UnstagedHeader,
    Change(usize, bool),
}

#[derive(Clone)]
enum SearchRow {
    File(PathBuf, usize),
    Match(usize),
}

fn search_rows(matches: &[project::Match], collapsed: &HashSet<PathBuf>) -> Vec<SearchRow> {
    let mut groups: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (index, found) in matches.iter().enumerate() {
        groups.entry(found.path.clone()).or_default().push(index);
    }
    let mut rows = Vec::new();
    for (path, indices) in groups {
        let is_collapsed = collapsed.contains(&path);
        rows.push(SearchRow::File(path, indices.len()));
        if !is_collapsed {
            rows.extend(indices.into_iter().map(SearchRow::Match));
        }
    }
    rows
}

fn preview_context(found: &project::Match) -> (String, std::ops::Range<usize>) {
    let text = found.text.trim_end();
    let start = found.start.min(text.len());
    let end = found.end.min(text.len());
    let boundaries: Vec<_> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
        .collect();
    let match_char = boundaries.partition_point(|at| *at < start);
    let first = match_char.saturating_sub(10);
    let last = (match_char + 45).min(boundaries.len() - 1);
    let prefix = if first > 0 { "…" } else { "" };
    let suffix = if last < boundaries.len() - 1 {
        "…"
    } else {
        ""
    };
    let preview = format!(
        "{prefix}{}{suffix}",
        &text[boundaries[first]..boundaries[last]]
    );
    let offset = prefix.len();
    (
        preview,
        offset + start - boundaries[first]..offset + end.min(boundaries[last]) - boundaries[first],
    )
}

fn match_preview(found: &project::Match) -> StyledText {
    let (text, range) = preview_context(found);
    let mut preview = StyledText::new(text);
    if range.start < range.end {
        let mut style = HighlightStyle::default();
        style.color = Some(rgb(0xe5c07b).into());
        style.background_color = Some(rgb(0x3e4451).into());
        preview = preview.with_highlights(vec![(range, style)]);
    }
    preview
}

fn palette_target(query: &str) -> (&str, Option<usize>) {
    if let Some((name, line)) = query.rsplit_once(':') {
        if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) {
            if let Some(line) = line.parse::<usize>().ok().filter(|line| *line > 0) {
                return (name, Some(line));
            }
        }
    }
    (query, None)
}

#[derive(PartialEq, Eq)]
struct DiscardState {
    change: project::Change,
    len: u64,
    modified: SystemTime,
}

struct IconTooltip(&'static str);

impl Render for IconTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(0x3e4451))
            .text_color(rgb(FG))
            .child(self.0)
    }
}

struct DiagnosticTooltip {
    diagnostics: Vec<highlight::Diagnostic>,
}

impl Render for DiagnosticTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(500.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(0x3e4451))
            .text_color(rgb(FG))
            .flex()
            .flex_col()
            .gap_1()
            .children(self.diagnostics.iter().map(|diagnostic| {
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(diagnostic.severity.color()))
                            .child(diagnostic.severity.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(diagnostic.message.clone()),
                    )
            }))
    }
}

struct Reviewer {
    settings: settings::Settings,
    settings_open: bool,
    root: PathBuf,
    sidebar: Sidebar,
    sidebar_visible: bool,
    sidebar_width: Pixels,
    dragging_sidebar: bool,
    files: Vec<project::FileEntry>,
    ignored: HashSet<PathBuf>,
    root_expanded: bool,
    expanded: HashSet<PathBuf>,
    tree: BTreeMap<PathBuf, Vec<usize>>,
    visible: Vec<(usize, usize)>,
    files_scroll: UniformListScrollHandle,
    changes: Vec<project::Change>,
    change_counts: Vec<(usize, usize)>,
    branch: Option<String>,
    branches: Vec<String>,
    branch_menu_open: bool,
    branch_menu_loading: bool,
    branch_scroll: UniformListScrollHandle,
    sync_status: Option<project::SyncStatus>,
    git_rows: Vec<GitRow>,
    stashes: Vec<project::Stash>,
    git_busy: bool,
    git_progress_offset: f32,
    commit_message: SingleLineInput,
    commit_focused: bool,
    tabs: Vec<Tab>,
    pending_session: HashMap<PathBuf, session::TabState>,
    active: Option<usize>,
    selected: Option<PathBuf>,
    query: SingleLineInput,
    search_focused: bool,
    search_options: project::SearchOptions,
    search_id: u64,
    palette_open: bool,
    palette_query: SingleLineInput,
    palette_selected: usize,
    palette_scroll: UniformListScrollHandle,
    editor_scroll: UniformListScrollHandle,
    original_scroll: UniformListScrollHandle,
    original_buffer: buffer::EditorBuffer,
    original_path: Option<PathBuf>,
    original_focused: bool,
    original_mouse_selecting: bool,
    find_open: bool,
    find_query: SingleLineInput,
    find_matches: Vec<std::ops::Range<usize>>,
    find_active: Option<usize>,
    find_has_focus: bool,
    mouse_selecting: bool,
    cursor_blink_visible: bool,
    editor_scroll_drag: Option<EditorScrollbarDrag>,
    original_scroll_drag: Option<EditorScrollbarDrag>,
    diff_split: f32,
    dragging_diff_split: bool,
    svg_split: f32,
    dragging_svg_split: bool,
    file_index: Vec<PathBuf>,
    quick: Vec<PathBuf>,
    matches: Vec<project::Match>,
    search_rows: Vec<SearchRow>,
    collapsed_search: HashSet<PathBuf>,
    pending_navigation: Option<(PathBuf, usize, usize, usize)>,
    side_by_side: bool,
    show_diff: bool,
    diff_text: String,
    diff_rows: Vec<DiffRow>,
    original_lines: Vec<highlight::HighlightedLine>,
    original_text: String,
    original_max_chars: usize,
    diff_highlights: DiffHighlights,
    confirm_discard: Option<DiscardState>,
    confirm_discard_all: bool,
    file_menu: Option<(PathBuf, bool, gpui::Point<Pixels>)>,
    top_file_menu: bool,
    file_edit: Option<FileEdit>,
    file_name: SingleLineInput,
    confirm_delete: Option<PathBuf>,
    files_focused: bool,
    refresh_id: u64,
    message: String,
    focus: FocusHandle,
}

impl Reviewer {
    fn session_snapshot(&self) -> session::Session {
        let row_height = (self.settings.font_size as f32 + 8.).max(22.);
        session::Session {
            tabs: self
                .tabs
                .iter()
                .map(|tab| {
                    if let Some(pending) = self.pending_session.get(&tab.path) {
                        return pending.clone();
                    }
                    let scroll = tab.scroll.0.borrow();
                    let line = scroll
                        .deferred_scroll_to_item
                        .map(|target| target.item_index)
                        .unwrap_or_else(|| {
                            (-f32::from(scroll.base_handle.offset().y) / row_height).max(0.)
                                as usize
                        });
                    session::TabState {
                        path: tab.path.clone(),
                        cursor: tab.buffer.cursor(),
                        scroll_line: line,
                        dirty_text: tab.buffer.is_dirty().then(|| tab.buffer.text().to_owned()),
                    }
                })
                .collect(),
            active: self.active,
        }
    }

    fn relocate_paths(&mut self, from: &Path, to: &Path) {
        let relocate = |path: &mut PathBuf| {
            if let Ok(suffix) = path.strip_prefix(from) {
                *path = to.join(suffix);
            }
        };
        for tab in &mut self.tabs {
            relocate(&mut tab.path);
        }
        if let Some(selected) = &mut self.selected {
            relocate(selected);
        }
        if let Some(original) = &mut self.original_path {
            relocate(original);
        }
        if let Some((pending, _, _, _)) = &mut self.pending_navigation {
            relocate(pending);
        }
        self.expanded = self
            .expanded
            .iter()
            .cloned()
            .map(|mut path| {
                relocate(&mut path);
                path
            })
            .collect();
        self.file_index.clear();
        self.quick.clear();
    }

    fn move_explorer_entry(&mut self, path: &Path, folder: &Path, cx: &mut Context<Self>) {
        if !can_move_into(path, folder) {
            return;
        }
        if self
            .tabs
            .iter()
            .any(|tab| tab.path.starts_with(path) && tab.loading)
        {
            self.message = "Esperá a que termine de cargar el archivo antes de moverlo".into();
            cx.notify();
            return;
        }
        match project::move_entry(&self.root, path, folder) {
            Ok(target) => {
                self.relocate_paths(path, &target);
                self.selected = Some(target.clone());
                if !folder.as_os_str().is_empty() {
                    self.expanded.insert(folder.to_path_buf());
                }
                self.files_focused = true;
                self.message = format!("Movido a {}", target.display());
                self.refresh(cx);
            }
            Err(error) => {
                self.message = error;
                cx.notify();
            }
        }
    }

    fn editor_left(&self, window: &Window) -> Pixels {
        px(RAIL_WIDTH)
            + if self.sidebar_visible {
                sidebar_width(window.bounds().size.width, self.sidebar_width) + px(8.)
            } else {
                px(0.)
            }
    }

    fn toggle_sidebar(&mut self, sidebar: Sidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.files_focused = false;
        if self.sidebar == sidebar && self.sidebar_visible {
            self.sidebar_visible = false;
        } else {
            self.sidebar = sidebar;
            self.sidebar_visible = true;
            if sidebar == Sidebar::Git {
                self.search_id += 1;
                self.refresh(cx);
            }
        }
        self.file_edit = None;
        self.search_focused = self.sidebar_visible && sidebar == Sidebar::Search;
        self.commit_focused = false;
        self.palette_open = false;
        window.focus(&self.focus);
        cx.notify();
    }

    fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let files = project::files(&root);
        let warm_file = files
            .iter()
            .find(|entry| {
                !entry.is_dir
                    && definition::supports_js(&entry.path)
                    && !entry
                        .path
                        .components()
                        .any(|component| component.as_os_str() == "node_modules")
            })
            .map(|entry| entry.path.clone());
        let warm_root = root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(move |_: gpui::WeakEntity<Self>, _: &mut gpui::AsyncApp| {
            async move {
                // Let GPUI paint the window first; initialization and indexing stay off the UI thread.
                gpui::Timer::after(Duration::from_millis(150)).await;
                executor
                    .spawn(async move {
                        let _ = definition::warm_project(&warm_root, warm_file.as_deref());
                    })
                    .detach();
            }
        })
        .detach();
        let ignored = project::ignored_paths(&root, &files);
        let branch = project::branch(&root);
        let sync_status = project::sync_status(&root);
        let (changes, message) = match project::status(&root) {
            Ok(changes) => (changes, String::new()),
            Err(error) => (Vec::new(), format!("Git: {error}")),
        };
        let change_counts = changes
            .iter()
            .map(|c| project::change_counts(&root, c))
            .collect();
        cx.spawn(|weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    gpui::Timer::after(Duration::from_secs(2)).await;
                    if weak.update(&mut cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
        cx.spawn(|weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let mut last = None;
                loop {
                    gpui::Timer::after(Duration::from_secs(3)).await;
                    let Ok((root, snapshot)) = weak.update(&mut cx, |this, _| {
                        (this.root.clone(), this.session_snapshot())
                    }) else {
                        break;
                    };
                    if last.as_ref() == Some(&snapshot) {
                        continue;
                    }
                    let revision = session::revision();
                    let saved = snapshot.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move { session::save(&root, &saved, revision) })
                        .await;
                    if result.is_ok() {
                        last = Some(snapshot);
                    }
                }
            }
        })
        .detach();
        cx.spawn(|weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    gpui::Timer::after(Duration::from_millis(530)).await;
                    if weak
                        .update(&mut cx, |this, cx| {
                            let mut changed = false;
                            if this.git_busy {
                                this.git_progress_offset = (this.git_progress_offset + 0.2) % 1.;
                                changed = true;
                            }
                            if this.file_edit.is_some()
                                || this.palette_open
                                || this.find_open
                                || this.editor_active()
                                || (this.sidebar == Sidebar::Search && this.search_focused)
                                || (this.sidebar == Sidebar::Git && this.commit_focused)
                            {
                                this.cursor_blink_visible = !this.cursor_blink_visible;
                                changed = true;
                            } else if !this.cursor_blink_visible {
                                this.cursor_blink_visible = true;
                                changed = true;
                            }
                            if changed {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        })
        .detach();
        let mut reviewer = Self {
            settings: settings::Settings::load(),
            settings_open: false,
            root,
            sidebar: Sidebar::Files,
            sidebar_visible: true,
            sidebar_width: px(280.),
            dragging_sidebar: false,
            files,
            ignored,
            root_expanded: true,
            expanded: HashSet::new(),
            tree: BTreeMap::new(),
            visible: Vec::new(),
            files_scroll: UniformListScrollHandle::new(),
            changes,
            change_counts,
            branch,
            branches: Vec::new(),
            branch_menu_open: false,
            branch_menu_loading: false,
            branch_scroll: UniformListScrollHandle::new(),
            sync_status,
            git_rows: Vec::new(),
            stashes: Vec::new(),
            git_busy: false,
            git_progress_offset: 0.,
            commit_message: SingleLineInput::default(),
            commit_focused: false,
            tabs: Vec::new(),
            pending_session: HashMap::new(),
            active: None,
            selected: None,
            query: SingleLineInput::default(),
            search_focused: false,
            search_options: project::SearchOptions::default(),
            search_id: 0,
            palette_open: false,
            palette_query: SingleLineInput::default(),
            palette_selected: 0,
            palette_scroll: UniformListScrollHandle::new(),
            editor_scroll: UniformListScrollHandle::new(),
            original_scroll: UniformListScrollHandle::new(),
            original_buffer: buffer::EditorBuffer::new(""),
            original_path: None,
            original_focused: false,
            original_mouse_selecting: false,
            find_open: false,
            find_query: SingleLineInput::default(),
            find_matches: Vec::new(),
            find_active: None,
            find_has_focus: false,
            mouse_selecting: false,
            cursor_blink_visible: true,
            editor_scroll_drag: None,
            original_scroll_drag: None,
            diff_split: 0.5,
            dragging_diff_split: false,
            svg_split: 0.72,
            dragging_svg_split: false,
            file_index: Vec::new(),
            quick: Vec::new(),
            matches: Vec::new(),
            search_rows: Vec::new(),
            collapsed_search: HashSet::new(),
            pending_navigation: None,
            side_by_side: true,
            show_diff: false,
            diff_text: String::new(),
            diff_rows: Vec::new(),
            original_lines: Vec::new(),
            original_text: String::new(),
            original_max_chars: 0,
            diff_highlights: DiffHighlights::default(),
            confirm_discard: None,
            confirm_discard_all: false,
            file_menu: None,
            top_file_menu: false,
            file_edit: None,
            file_name: SingleLineInput::default(),
            confirm_delete: None,
            files_focused: false,
            refresh_id: 0,
            message,
            focus: cx.focus_handle(),
        };
        reviewer.update_tree();
        reviewer.update_git_rows();
        let previous = session::load(&reviewer.root);
        let active_path = previous
            .active
            .and_then(|index| previous.tabs.get(index))
            .map(|tab| tab.path.clone());
        for tab in previous.tabs {
            if project_relative_path(&reviewer.root, &tab.path).is_some()
                && (reviewer.root.join(&tab.path).is_file() || tab.dirty_text.is_some())
                && !reviewer.pending_session.contains_key(&tab.path)
            {
                reviewer
                    .pending_session
                    .insert(tab.path.clone(), tab.clone());
                reviewer.open(tab.path, cx);
            }
        }
        if let Some(index) =
            active_path.and_then(|path| reviewer.tabs.iter().position(|tab| tab.path == path))
        {
            reviewer.activate_tab(Some(index));
            reviewer.selected = Some(reviewer.tabs[index].path.clone());
        }
        reviewer
    }

    fn update_git_rows(&mut self) {
        self.git_rows.clear();
        let staged: Vec<_> = self
            .changes
            .iter()
            .enumerate()
            .filter(|(_, change)| change.index != ' ' && change.index != '?')
            .map(|(index, _)| GitRow::Change(index, true))
            .collect();
        let unstaged: Vec<_> = self
            .changes
            .iter()
            .enumerate()
            .filter(|(_, change)| change.worktree != ' ')
            .map(|(index, _)| GitRow::Change(index, false))
            .collect();
        if !staged.is_empty() {
            self.git_rows.push(GitRow::StagedHeader);
            self.git_rows.extend(staged);
        }
        if !unstaged.is_empty() {
            self.git_rows.push(GitRow::UnstagedHeader);
            self.git_rows.extend(unstaged);
        }
    }

    fn update_tree(&mut self) {
        self.tree = file_tree(&self.files);
        self.update_visible();
    }

    fn update_visible(&mut self) {
        self.visible.clear();
        visible_entries(
            Path::new(""),
            0,
            &self.files,
            &self.tree,
            &self.expanded,
            &mut self.visible,
        );
    }

    fn reveal_file(&mut self, path: &Path) {
        let Some(path) = project_relative_path(&self.root, path) else {
            return;
        };
        self.sidebar = Sidebar::Files;
        self.sidebar_visible = true;
        self.search_focused = false;
        self.root_expanded = true;
        for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
            if !ancestor.as_os_str().is_empty() {
                self.expanded.insert(ancestor.to_path_buf());
            }
        }
        self.update_visible();
        let rows = explorer_rows(&self.visible, &self.files, self.file_edit.as_ref());
        if let Some(index) = rows.iter().position(|row| match row {
            ExplorerRow::Entry(file, _) => self.files[*file].path == path,
            ExplorerRow::NewFile(_) => false,
        }) {
            self.files_scroll
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    fn toggle_folder(&mut self, path: &Path) {
        let Some(position) = self
            .visible
            .iter()
            .position(|(index, _)| self.files[*index].path == path)
        else {
            return;
        };
        let depth = self.visible[position].1;
        if self.expanded.remove(path) {
            let end = position
                + 1
                + self.visible[position + 1..]
                    .iter()
                    .take_while(|(_, level)| *level > depth)
                    .count();
            self.visible.drain(position + 1..end);
        } else {
            self.expanded.insert(path.to_path_buf());
            let mut children = Vec::new();
            visible_entries(
                path,
                depth + 1,
                &self.files,
                &self.tree,
                &self.expanded,
                &mut children,
            );
            self.visible.splice(position + 1..position + 1, children);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        self.refresh_id += 1;
        let refresh_id = self.refresh_id;
        let root = self.root.clone();
        let show_git = self.sidebar == Sidebar::Git;
        let paths: Vec<_> = self.tabs.iter().map(|tab| tab.path.clone()).collect();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let (files, ignored, changes, counts, branch, sync_status, stashes, tabs) =
                        executor
                            .spawn(async move {
                                let files = project::files(&root);
                                let ignored = project::ignored_paths(&root, &files);
                                let changes = project::status(&root);
                                let counts = changes
                                    .as_ref()
                                    .map(|items| {
                                        items
                                            .iter()
                                            .map(|c| project::change_counts(&root, c))
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default();
                                let branch = project::branch(&root);
                                let sync_status = project::sync_status(&root);
                                let stashes = show_git.then(|| project::stashes(&root));
                                let tabs: Vec<_> = paths
                                    .into_iter()
                                    .map(|path| {
                                        let result = project::read_with_stamp(&root, &path);
                                        (path, result)
                                    })
                                    .collect();
                                (
                                    files,
                                    ignored,
                                    changes,
                                    counts,
                                    branch,
                                    sync_status,
                                    stashes,
                                    tabs,
                                )
                            })
                            .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.refresh_id == refresh_id {
                            this.apply_refresh(
                                files,
                                ignored,
                                changes,
                                counts,
                                branch,
                                sync_status,
                                stashes,
                                tabs,
                                cx,
                            );
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn apply_refresh(
        &mut self,
        files: Vec<project::FileEntry>,
        ignored: HashSet<PathBuf>,
        changes: Result<Vec<project::Change>, String>,
        counts: Vec<(usize, usize)>,
        branch: Option<String>,
        sync_status: Option<project::SyncStatus>,
        stashes: Option<Result<Vec<project::Stash>, String>>,
        tabs: Vec<(PathBuf, Result<(String, project::FileStamp), String>)>,
        cx: &mut Context<Self>,
    ) {
        let files_changed = files != self.files;
        let mut changed = files_changed;
        changed |= ignored != self.ignored;
        self.ignored = ignored;
        changed |= branch != self.branch;
        self.branch = branch;
        changed |= sync_status != self.sync_status;
        self.sync_status = sync_status;
        if files_changed {
            self.files = files;
            let directories: HashSet<_> = self
                .files
                .iter()
                .filter(|e| e.is_dir)
                .map(|e| &e.path)
                .collect();
            self.expanded.retain(|path| directories.contains(path));
            self.update_tree();
        }
        match changes {
            Ok(changes) => {
                if counts != self.change_counts {
                    self.change_counts = counts;
                    changed = true;
                }
                if changes != self.changes {
                    self.changes = changes;
                    self.update_git_rows();
                    changed = true;
                }
            }
            Err(error) => {
                self.changes.clear();
                self.change_counts.clear();
                self.git_rows.clear();
                self.message = format!("Git: {error}");
                changed = true;
            }
        }
        if let Some(stashes) = stashes {
            match stashes {
                Ok(stashes) => {
                    changed |= stashes != self.stashes;
                    self.stashes = stashes;
                }
                Err(error) => {
                    self.stashes.clear();
                    self.message = format!("Git: {error}");
                    changed = true;
                }
            }
        }
        for (path, result) in tabs {
            let Some(tab) = self.tabs.iter_mut().find(|tab| tab.path == path) else {
                continue;
            };
            if tab.loading || tab.buffer.is_dirty() {
                continue;
            }
            match result {
                Ok((text, stamp)) => {
                    tab.loaded_stamp = Some(stamp);
                    if text != tab.buffer.text() {
                        tab.lint_source = None;
                        tab.lint_diagnostics.clear();
                        tab.lines = highlight::line(&text, &tab.path);
                        tab.diagnostics = highlight::diagnostics(&text, &tab.path);
                        tab.csv = (tab.path.extension().and_then(|ext| ext.to_str())
                            == Some("csv"))
                        .then(|| csv::layout(&text))
                        .flatten();
                        tab.max_line_chars = max_line_chars(&text);
                        if let Some(csv) = &tab.csv {
                            tab.max_line_chars = csv.max_chars;
                        }
                        tab.buffer = buffer::EditorBuffer::new(text);
                        changed = true;
                    }
                }
                Err(error) if !tab.buffer.text().is_empty() => {
                    tab.buffer = buffer::EditorBuffer::new("");
                    tab.lines.clear();
                    tab.diagnostics.clear();
                    tab.lint_source = None;
                    tab.lint_diagnostics.clear();
                    tab.csv = None;
                    tab.max_line_chars = 0;
                    tab.loaded_stamp = None;
                    self.message = format!("{}: {error}", tab.path.display());
                    changed = true;
                }
                _ => {}
            }
        }
        if self
            .confirm_discard
            .as_ref()
            .is_some_and(|state| self.discard_state().as_ref() != Ok(state))
        {
            self.confirm_discard = None;
            changed = true;
        }
        if changed {
            self.load_change_decorations();
        }
        if changed {
            cx.notify();
        }
    }

    fn discard_state(&self) -> Result<DiscardState, String> {
        let path = self
            .selected
            .as_ref()
            .ok_or("Ningún archivo seleccionado")?;
        let change = self
            .changes
            .iter()
            .find(|c| &c.path == path && (c.worktree != ' ' && c.worktree != '?' || c.index == '?'))
            .ok_or("Cambio no disponible")?
            .clone();
        let metadata = std::fs::metadata(self.root.join(path)).map_err(|e| e.to_string())?;
        Ok(DiscardState {
            change,
            len: metadata.len(),
            modified: metadata.modified().map_err(|e| e.to_string())?,
        })
    }

    fn activate_tab(&mut self, index: Option<usize>) {
        self.active = index;
        self.editor_scroll = index
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.scroll.clone())
            .unwrap_or_else(UniformListScrollHandle::new);
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.top_file_menu = false;
        self.file_menu = None;
        self.original_focused = false;
        self.original_mouse_selecting = false;
        self.pending_navigation = None;
        self.sidebar = Sidebar::Files;
        self.commit_focused = false;
        self.palette_open = false;
        self.close_find();
        self.mouse_selecting = false;
        self.confirm_discard = None;
        self.selected = Some(path.clone());
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.activate_tab(Some(index));
        } else {
            self.tabs.push(Tab {
                path: path.clone(),
                scroll: UniformListScrollHandle::new(),
                buffer: buffer::EditorBuffer::new(""),
                lines: Vec::new(),
                diagnostics: Vec::new(),
                lint_source: None,
                lint_diagnostics: Vec::new(),
                csv: None,
                max_line_chars: 0,
                loading: true,
                loaded_stamp: None,
            });
            self.activate_tab(Some(self.tabs.len() - 1));
            self.message = format!("Abriendo {}…", path.display());
            let root = self.root.clone();
            let executor = cx.background_executor().clone();
            cx.spawn(
                move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                    let mut cx = cx.clone();
                    async move {
                        let (path, result) = executor
                            .spawn(async move {
                                let result =
                                    project::read_with_stamp(&root, &path).map(|(text, stamp)| {
                                        let lines = highlight::line(&text, &path);
                                        let max_line_chars = max_line_chars(&text);
                                        (text, lines, stamp, max_line_chars)
                                    });
                                (path, result)
                            })
                            .await;
                        let _ = weak.update(&mut cx, |this, cx| {
                            let restored = this.pending_session.remove(&path);
                            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.path == path) {
                                tab.loading = false;
                                match result {
                                    Ok((text, lines, stamp, max_line_chars)) => {
                                        tab.buffer = buffer::EditorBuffer::new(text);
                                        tab.lines = lines;
                                        tab.diagnostics =
                                            highlight::diagnostics(tab.buffer.text(), &path);
                                        tab.lint_source = None;
                                        tab.lint_diagnostics.clear();
                                        tab.csv = (path.extension().and_then(|ext| ext.to_str())
                                            == Some("csv"))
                                        .then(|| csv::layout(tab.buffer.text()))
                                        .flatten();
                                        tab.max_line_chars = max_line_chars;
                                        if let Some(csv) = &tab.csv {
                                            tab.max_line_chars = csv.max_chars;
                                        }
                                        tab.loaded_stamp = Some(stamp);
                                        if let Some(state) = restored.as_ref() {
                                            if let Some(dirty) = state.dirty_text.as_ref() {
                                                tab.buffer.select_all();
                                                tab.buffer.insert_text(&dirty);
                                                tab.lines = highlight::line(&dirty, &path);
                                                tab.diagnostics =
                                                    highlight::diagnostics(&dirty, &path);
                                                tab.csv =
                                                    (path.extension().and_then(|ext| ext.to_str())
                                                        == Some("csv"))
                                                    .then(|| csv::layout(&dirty))
                                                    .flatten();
                                                tab.max_line_chars = tab.csv.as_ref().map_or_else(
                                                    || crate::max_line_chars(&dirty),
                                                    |csv| csv.max_chars,
                                                );
                                            }
                                            tab.buffer.set_cursor(state.cursor, false);
                                            tab.scroll.scroll_to_item_strict(
                                                state.scroll_line,
                                                ScrollStrategy::Top,
                                            );
                                        }
                                        this.message.clear();
                                        if definition::supports_js(&path) {
                                            let root = this.root.clone();
                                            let text = tab.buffer.text().to_owned();
                                            let path = path.clone();
                                            cx.background_executor()
                                                .spawn(async move {
                                                    let _ = definition::warm(&root, &path, &text);
                                                })
                                                .detach();
                                        }
                                        if let Some(index) =
                                            this.tabs.iter().position(|tab| tab.path == path)
                                        {
                                            this.schedule_lint(index, cx);
                                        }
                                    }
                                    Err(error) => {
                                        tab.loaded_stamp = None;
                                        this.message = error;
                                        if let Some(state) = restored {
                                            if let Some(dirty) = state.dirty_text {
                                                tab.buffer = buffer::EditorBuffer::new("");
                                                tab.buffer.insert_text(&dirty);
                                                tab.buffer.set_cursor(state.cursor, false);
                                                tab.lines = highlight::line(&dirty, &path);
                                                tab.diagnostics =
                                                    highlight::diagnostics(&dirty, &path);
                                                tab.max_line_chars = max_line_chars(&dirty);
                                                tab.scroll.scroll_to_item_strict(
                                                    state.scroll_line,
                                                    ScrollStrategy::Top,
                                                );
                                            }
                                        }
                                    }
                                }
                                if this
                                    .pending_navigation
                                    .as_ref()
                                    .is_some_and(|(target, _, _, _)| *target == path)
                                {
                                    if let Some((_, line, start, end)) =
                                        this.pending_navigation.take()
                                    {
                                        if this
                                            .active
                                            .and_then(|i| this.tabs.get(i))
                                            .is_some_and(|active| active.path == path)
                                        {
                                            this.go_to(line, start, end);
                                        }
                                    }
                                }
                                if this.original_path.as_ref() == Some(&path)
                                    && this.selected.as_ref() == Some(&path)
                                {
                                    this.update_diff_highlights();
                                    if this.show_diff && this.side_by_side {
                                        this.scroll_to_first_change();
                                    }
                                }
                                if this.sidebar == Sidebar::Git
                                    && !this.show_diff
                                    && this
                                        .active
                                        .and_then(|i| this.tabs.get(i))
                                        .is_some_and(|tab| tab.path == path)
                                {
                                    this.focus_first_conflict();
                                }
                                cx.notify();
                            }
                        });
                    }
                },
            )
            .detach();
        }
        self.show_diff = false;
        self.load_change_decorations();
        cx.notify();
    }

    fn open_external(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let path = match path.canonicalize() {
            Ok(path) if path.is_file() => path,
            Ok(_) => {
                self.message = "Seleccioná un archivo, no una carpeta".into();
                cx.notify();
                return;
            }
            Err(error) => {
                self.message = format!("{}: {error}", path.display());
                cx.notify();
                return;
            }
        };
        let path = path
            .strip_prefix(&self.root)
            .map_or_else(|_| path.clone(), Path::to_path_buf);
        self.open(path, cx);
    }

    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open File".into()),
        });
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = receiver.await;
                    let _ = weak.update(&mut cx, |this, cx| match result {
                        Ok(Ok(Some(paths))) => {
                            if let Some(path) = paths.into_iter().next() {
                                this.open_external(path, cx);
                            }
                        }
                        Ok(Ok(None)) => {}
                        other => {
                            this.message = format!("No se pudo abrir el selector: {other:?}");
                            cx.notify();
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn begin_file_edit(&mut self, edit: FileEdit, cx: &mut Context<Self>) {
        self.sidebar = Sidebar::Files;
        if let FileEdit::Create(parent) | FileEdit::CreateFolder(parent) = &edit {
            self.root_expanded = true;
            for ancestor in parent.ancestors() {
                if !ancestor.as_os_str().is_empty() {
                    self.expanded.insert(ancestor.to_path_buf());
                }
            }
            self.update_visible();
        }
        let name = match &edit {
            FileEdit::Create(_) | FileEdit::CreateFolder(_) => String::new(),
            FileEdit::Rename(path) => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        };
        self.file_name.set_text(name);
        if let FileEdit::Rename(path) = &edit {
            let stem_end = if self
                .files
                .iter()
                .any(|entry| entry.path == *path && !entry.is_dir)
            {
                self.file_name.text.rfind('.').filter(|&at| at > 0)
            } else {
                None
            };
            self.file_name.anchor = Some(0);
            self.file_name.cursor = stem_end.unwrap_or(self.file_name.text.len());
        }
        if let Some(index) = explorer_rows(&self.visible, &self.files, Some(&edit))
            .iter()
            .position(|row| match (row, &edit) {
                (ExplorerRow::NewFile(_), FileEdit::Create(_) | FileEdit::CreateFolder(_)) => true,
                (ExplorerRow::Entry(file, _), FileEdit::Rename(path)) => {
                    self.files[*file].path == *path
                }
                _ => false,
            })
        {
            self.files_scroll
                .scroll_to_item(index, ScrollStrategy::Center);
        }
        self.file_edit = Some(edit);
        self.file_menu = None;
        self.top_file_menu = false;
        self.search_focused = false;
        self.commit_focused = false;
        self.palette_open = false;
        self.find_has_focus = false;
        self.cursor_blink_visible = true;
        cx.notify();
    }

    fn finish_file_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.file_edit.as_ref() else {
            return;
        };
        if let FileEdit::Rename(path) = &edit {
            if self
                .tabs
                .iter()
                .any(|tab| tab.loading && tab.path.starts_with(path))
            {
                self.message =
                    "Esperá a que termine de cargar el archivo antes de renombrarlo".into();
                cx.notify();
                return;
            }
        }
        let edit = edit.clone();
        let name = self.file_name.text.trim();
        let renamed_from = match &edit {
            FileEdit::Rename(path) => Some(path.clone()),
            _ => None,
        };
        let result = match edit {
            FileEdit::Create(parent) => project::create_file(&self.root, &parent, name),
            FileEdit::CreateFolder(parent) => project::create_folder(&self.root, &parent, name),
            FileEdit::Rename(path) => {
                project::rename_entry(&self.root, &path, name).map(|target| {
                    if target != path {
                        self.relocate_paths(&path, &target);
                    }
                    target
                })
            }
        };
        match result {
            Ok(path) => {
                self.file_edit = None;
                for ancestor in path.parent().into_iter().flat_map(Path::ancestors) {
                    if !ancestor.as_os_str().is_empty() {
                        self.expanded.insert(ancestor.to_path_buf());
                    }
                }
                if let Some(from) = renamed_from {
                    let affected: Vec<_> = self
                        .tabs
                        .iter()
                        .enumerate()
                        .filter(|(_, tab)| {
                            tab.path == path || tab.path.starts_with(&path) && tab.path != from
                        })
                        .map(|(index, _)| index)
                        .collect();
                    for index in affected {
                        self.rehighlight_tab(index, cx);
                    }
                }
                self.selected = Some(path.clone());
                if self.root.join(&path).is_dir() {
                    self.expanded.insert(path.clone());
                }
                self.message = format!("Archivo: {}", path.display());
                self.refresh(cx);
                if self.root.join(&path).is_file() && !self.tabs.iter().any(|tab| tab.path == path)
                {
                    self.open(path, cx);
                }
            }
            Err(error) => self.message = error,
        }
        cx.notify();
    }

    fn delete_selected_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.confirm_delete.take() else {
            return;
        };
        let is_dir = self
            .files
            .iter()
            .any(|entry| entry.path == path && entry.is_dir);
        if self
            .tabs
            .iter()
            .any(|tab| tab.path.starts_with(&path) && (tab.buffer.is_dirty() || tab.loading))
        {
            self.message =
                "Guardá los cambios y esperá a que terminen de cargar los archivos antes de borrar"
                    .into();
        } else {
            let result = if is_dir {
                project::delete_folder(&self.root, &path)
            } else {
                project::delete_file(&self.root, &path)
            };
            match result {
                Ok(()) => {
                    while let Some(index) =
                        self.tabs.iter().position(|tab| tab.path.starts_with(&path))
                    {
                        self.tabs.remove(index);
                        self.activate_tab(active_after_close(self.active, index, self.tabs.len()));
                    }
                    self.selected = self.active.map(|index| self.tabs[index].path.clone());
                    self.expanded.retain(|entry| !entry.starts_with(&path));
                    if self
                        .original_path
                        .as_ref()
                        .is_some_and(|original| original.starts_with(&path))
                    {
                        self.original_path = None;
                    }
                    if self
                        .pending_navigation
                        .as_ref()
                        .is_some_and(|(pending, _, _, _)| pending.starts_with(&path))
                    {
                        self.pending_navigation = None;
                    }
                    self.show_diff = false;
                    self.close_find();
                    self.load_change_decorations();
                    self.message = format!("Eliminado {}", path.display());
                    self.refresh(cx);
                }
                Err(error) => self.message = error,
            }
        }
        cx.notify();
    }

    fn confirm_discard_all(&mut self, cx: &mut Context<Self>) {
        self.confirm_discard_all = false;
        if self.tabs.iter().any(|tab| {
            tab.buffer.is_dirty()
                && self
                    .changes
                    .iter()
                    .any(|change| change.path == tab.path && change.worktree != ' ')
        }) {
            self.message =
                "Guardá los archivos abiertos antes de descartar todos los cambios".into();
            cx.notify();
        } else {
            self.git_operation(GitOperation::DiscardAll, cx);
        }
    }

    fn open_at(
        &mut self,
        path: PathBuf,
        line: usize,
        start: usize,
        end: usize,
        from_search: bool,
        cx: &mut Context<Self>,
    ) {
        let path = if from_search {
            path
        } else {
            project_relative_path(&self.root, &path).unwrap_or(path)
        };
        if !from_search
            && self
                .active
                .and_then(|i| self.tabs.get(i))
                .is_some_and(|tab| tab.path == path && !tab.loading)
        {
            self.go_to(line, start, end);
            cx.notify();
            return;
        }
        self.open(path.clone(), cx);
        if from_search {
            self.sidebar = Sidebar::Search;
            self.search_focused = false;
        } else {
            self.reveal_file(&path);
        }
        if self
            .active
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|tab| tab.loading)
        {
            self.pending_navigation = Some((path, line, start, end));
        } else {
            self.go_to(line, start, end);
        }
        cx.notify();
    }

    fn follow_definition(
        &mut self,
        index: usize,
        line: usize,
        column: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let path = tab.path.clone();
        if !definition::supports(&path) || tab.loading {
            return;
        }
        let text = tab.buffer.text().to_owned();
        let clicked_path = path.clone();
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move {
                            let target = definition::lookup(&root, &path, &text, line, column)?;
                            let disk_text = target.as_ref().and_then(|target| {
                                std::fs::read_to_string(root.join(&target.path)).ok()
                            });
                            Ok::<_, String>((target, disk_text))
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| match result {
                        Ok((Some(target), disk_text)) => {
                            if this
                                .active
                                .and_then(|i| this.tabs.get(i))
                                .is_none_or(|tab| tab.path != clicked_path)
                            {
                                return;
                            }
                            let source = this
                                .tabs
                                .iter()
                                .find(|tab| tab.path == target.path)
                                .map(|tab| tab.buffer.text().to_owned())
                                .or(disk_text);
                            if let Some(source) = source {
                                let line_text = source.split('\n').nth(target.line).unwrap_or("");
                                let start = utf16_byte_column(line_text, target.start);
                                let end = utf16_byte_column(line_text, target.end);
                                this.open_at(target.path, target.line + 1, start, end, false, cx);
                            }
                        }
                        Ok((None, _)) => {}
                        Err(error) => {
                            this.message = if error == "El servidor de lenguaje se cerró" {
                                "Ir a definición: el servidor de lenguaje se cerró; revisá rust-analyzer o typescript-language-server".into()
                            } else {
                                format!("Ir a definición: {error}")
                            };
                            cx.notify();
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn go_to(&mut self, line: usize, start: usize, end: usize) {
        let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) else {
            return;
        };
        let line = line
            .saturating_sub(1)
            .min(tab.buffer.line_count().saturating_sub(1));
        let Some(range) = tab.buffer.line_range(line) else {
            return;
        };
        let text = tab.buffer.text();
        let start = (range.start + start.min(range.len())).min(text.len());
        let end = (range.start + end.min(range.len())).min(text.len());
        if end > start {
            tab.buffer.set_selection(start..end);
        } else {
            tab.buffer.set_cursor(start, false);
        }
        if let Some(index) = self.active {
            self.scroll_editor_cursor(index, true);
        }
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs[index].buffer.is_dirty() {
            self.message = "Guardá la pestaña con Ctrl+S antes de cerrarla".into();
            cx.notify();
            return;
        }
        let closed = self.tabs.remove(index).path;
        self.activate_tab(active_after_close(self.active, index, self.tabs.len()));
        self.close_find();
        if self.selected.as_ref() == Some(&closed) {
            self.selected = self.active.map(|i| self.tabs[i].path.clone());
            self.show_diff = false;
            self.confirm_discard = None;
            self.load_change_decorations();
        }
        cx.notify();
    }

    fn ensure_editor_cursor_visible(&self, index: usize) {
        self.scroll_editor_cursor(index, false);
    }

    fn scroll_editor_cursor(&self, index: usize, center: bool) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let line_count = tab.lines.len().min(10_000);
        if line_count > 0 {
            let physical = tab.buffer.cursor_position().line.min(line_count - 1);
            let visual = if self.show_diff && self.side_by_side {
                self.diff_highlights
                    .after_to_visual
                    .get(physical)
                    .copied()
                    .unwrap_or(physical)
            } else {
                physical
            };
            scroll_editor_line(&self.editor_scroll, visual, center);
            if self.show_diff && self.side_by_side {
                scroll_editor_line(&self.original_scroll, visual, center);
            }
        }
    }

    fn editor_scroll_metrics(
        &self,
        window: &Window,
        cell_width: Pixels,
    ) -> Option<EditorScrollMetrics> {
        self.scroll_metrics(false, window, cell_width)
    }

    fn scroll_metrics(
        &self,
        original: bool,
        window: &Window,
        cell_width: Pixels,
    ) -> Option<EditorScrollMetrics> {
        let state = if original {
            self.original_scroll.0.borrow()
        } else {
            self.editor_scroll.0.borrow()
        };
        let tab = self.active.and_then(|index| self.tabs.get(index));
        if !original && tab?.loading {
            return None;
        }
        let line_count = if self.show_diff && self.side_by_side {
            self.diff_highlights.layout.len()
        } else if original {
            self.original_lines.len()
        } else {
            tab?.lines.len()
        }
        .min(10_000);
        let max_chars = if original {
            self.original_max_chars
        } else {
            tab?.max_line_chars
        };
        let measured = state.last_item_size;
        let bounds = window.bounds();
        let viewport_width = measured.map_or_else(
            || {
                if original {
                    split_left_width(bounds.size.width, self.diff_split, self.editor_left(window))
                } else if self.show_diff && self.side_by_side {
                    (bounds.size.width
                        - self.editor_left(window)
                        - px(6.)
                        - split_left_width(
                            bounds.size.width,
                            self.diff_split,
                            self.editor_left(window),
                        ))
                    .max(px(0.))
                } else {
                    (bounds.size.width - self.editor_left(window) - px(16.)).max(px(0.))
                }
            },
            |item| item.item.width,
        );
        let viewport_height = measured.map_or_else(
            || (bounds.size.height - px(122.)).max(px(0.)),
            |item| item.item.height,
        );
        if viewport_width <= px(0.) || viewport_height <= px(0.) {
            return None;
        }
        let content_width = (px(CODE_CELL_LEFT) + cell_width * max_chars).max(viewport_width);
        let content_height = px((self.settings.font_size as f32 + 8.).max(22.)) * line_count;
        let offset = state.base_handle.offset();
        let max_x = (content_width - viewport_width).max(px(0.));
        let max_y = (content_height - viewport_height).max(px(0.));
        Some(EditorScrollMetrics {
            viewport_width,
            viewport_height,
            scroll_x: (-offset.x).max(px(0.)).min(max_x),
            scroll_y: (-offset.y).max(px(0.)).min(max_y),
            max_x,
            max_y,
        })
    }

    fn begin_editor_scroll_drag(
        &mut self,
        original: bool,
        vertical: bool,
        pointer: Pixels,
        window: &Window,
        cell_width: Pixels,
    ) {
        let Some(metrics) = self.scroll_metrics(original, window, cell_width) else {
            return;
        };
        let drag = if vertical && metrics.max_y > px(0.) {
            Some(EditorScrollbarDrag::Vertical {
                pointer_start: pointer,
                scroll_start: metrics.scroll_y,
            })
        } else if !vertical && metrics.max_x > px(0.) {
            Some(EditorScrollbarDrag::Horizontal {
                pointer_start: pointer,
                scroll_start: metrics.scroll_x,
            })
        } else {
            None
        };
        if original {
            self.original_scroll_drag = drag;
        } else {
            self.editor_scroll_drag = drag;
        }
    }

    fn click_editor_scrollbar(
        &mut self,
        original: bool,
        vertical: bool,
        pointer: Pixels,
        window: &Window,
        cell_width: Pixels,
        cx: &mut Context<Self>,
    ) {
        let Some(metrics) = self.scroll_metrics(original, window, cell_width) else {
            return;
        };
        let (viewport, max, cross) = if vertical {
            (
                metrics.viewport_height,
                metrics.max_y,
                metrics.max_x > px(0.),
            )
        } else {
            (
                metrics.viewport_width,
                metrics.max_x,
                metrics.max_y > px(0.),
            )
        };
        let track = viewport
            - if cross { px(14.) } else { px(0.) }
            - if vertical && self.show_diff && self.side_by_side {
                px(8.)
            } else {
                px(0.)
            };
        let (thumb, _) = scrollbar_thumb(viewport, track, max, px(0.));
        let origin = if vertical {
            window.bounds().size.height
                - px(26.) // status bar
                - if self.show_diff && self.side_by_side { px(4.) } else { px(12.) }
                - if cross { px(14.) } else { px(0.) }
                - track
        } else {
            if original {
                self.editor_left(window) + px(12.)
            } else if self.show_diff && self.side_by_side {
                self.editor_left(window)
                    + px(6.)
                    + split_left_width(
                        window.bounds().size.width,
                        self.diff_split,
                        self.editor_left(window),
                    )
                    + px(12.)
            } else {
                self.editor_left(window) + px(12.)
            }
        };
        let scroll = scrollbar_target(pointer, origin, track, thumb, max);
        let handle = if original {
            &self.original_scroll
        } else {
            &self.editor_scroll
        };
        let offset = handle.0.borrow().base_handle.offset();
        handle.0.borrow().base_handle.set_offset(if vertical {
            point(offset.x, -scroll)
        } else {
            point(-scroll, offset.y)
        });
        self.mouse_selecting = false;
        self.original_mouse_selecting = false;
        if vertical {
            self.sync_diff_scroll_from(original, cx);
        }
        self.begin_editor_scroll_drag(original, vertical, pointer, window, cell_width);
        cx.notify();
    }

    fn drag_editor_scrollbar(
        &mut self,
        original: bool,
        event: &MouseMoveEvent,
        window: &Window,
        cell_width: Pixels,
        cx: &mut Context<Self>,
    ) {
        let drag = if original {
            self.original_scroll_drag
        } else {
            self.editor_scroll_drag
        };
        let (vertical, pointer_start, scroll_start) = match drag {
            Some(EditorScrollbarDrag::Vertical {
                pointer_start,
                scroll_start,
            }) => (true, pointer_start, scroll_start),
            Some(EditorScrollbarDrag::Horizontal {
                pointer_start,
                scroll_start,
            }) => (false, pointer_start, scroll_start),
            None => return,
        };
        let Some(metrics) = self.scroll_metrics(original, window, cell_width) else {
            return;
        };
        let (viewport, max_scroll, pointer, has_cross_scroll) = if vertical {
            (
                metrics.viewport_height,
                metrics.max_y,
                event.position.y,
                metrics.max_x > px(0.),
            )
        } else {
            (
                metrics.viewport_width,
                metrics.max_x,
                event.position.x,
                metrics.max_y > px(0.),
            )
        };
        if max_scroll <= px(0.) {
            return;
        }
        let track = viewport
            - if has_cross_scroll { px(14.) } else { px(0.) }
            - if vertical && self.show_diff && self.side_by_side {
                px(8.)
            } else {
                px(0.)
            };
        let thumb_size = scrollbar_thumb(viewport, track, max_scroll, px(0.)).0;
        let travel = track - thumb_size;
        if travel <= px(0.) {
            return;
        }
        let delta = (pointer - pointer_start) / travel * max_scroll;
        let scroll = (scroll_start + delta).max(px(0.)).min(max_scroll);
        let handle = if original {
            &self.original_scroll
        } else {
            &self.editor_scroll
        };
        let offset = handle.0.borrow().base_handle.offset();
        handle.0.borrow().base_handle.set_offset(if vertical {
            point(offset.x, -scroll)
        } else {
            point(-scroll, offset.y)
        });
        if vertical {
            self.sync_diff_scroll_from(original, cx);
        }
        cx.notify();
    }

    fn sync_diff_scroll_from(&self, original: bool, cx: &mut Context<Self>) {
        if !self.show_diff || !self.side_by_side {
            return;
        }
        let (source, destination) = if original {
            (&self.original_scroll, &self.editor_scroll)
        } else {
            (&self.editor_scroll, &self.original_scroll)
        };
        let y = source.0.borrow().base_handle.offset().y;
        let state = destination.0.borrow();
        let handle = &state.base_handle;
        let offset = handle.offset();
        if offset.y != y {
            handle.set_offset(point(offset.x, y));
            cx.notify();
        }
    }

    fn editor_scrollbars(
        &self,
        original: bool,
        window: &Window,
        cell_width: Pixels,
        cx: &mut Context<Self>,
    ) -> (Option<gpui::Div>, Option<gpui::Div>) {
        let loaded = original
            || self
                .active
                .and_then(|index| self.tabs.get(index))
                .is_some_and(|tab| !tab.loading);
        let Some(metrics) = self
            .scroll_metrics(original, window, cell_width)
            .filter(|_| {
                (!self.show_diff || self.side_by_side)
                    && loaded
                    && (!original || self.show_diff && self.side_by_side)
            })
        else {
            return (None, None);
        };

        let vertical = (metrics.max_y > px(0.)).then(|| {
            let track_height = metrics.viewport_height
                - if self.show_diff && self.side_by_side {
                    px(8.)
                } else {
                    px(0.)
                }
                - if metrics.max_x > px(0.) {
                    px(14.)
                } else {
                    px(0.)
                };
            let (thumb_size, thumb_position) = scrollbar_thumb(
                metrics.viewport_height,
                track_height,
                metrics.max_y,
                metrics.scroll_y,
            );
            let dragging = matches!(
                if original {
                    self.original_scroll_drag
                } else {
                    self.editor_scroll_drag
                },
                Some(EditorScrollbarDrag::Vertical { .. })
            );
            let line_count = if self.show_diff && self.side_by_side {
                self.diff_highlights.layout.len()
            } else if original {
                self.original_lines.len()
            } else {
                self.active
                    .and_then(|index| self.tabs.get(index))
                    .map_or(0, |tab| tab.lines.len())
            }
            .max(1);
            let mut markers = BTreeMap::new();
            let mut mark = |line: usize, color: u32, visual: bool| {
                let line = if visual {
                    Some(line)
                } else if self.show_diff && self.side_by_side {
                    if original {
                        self.diff_highlights.before_to_visual.get(line).copied()
                    } else {
                        self.diff_highlights.after_to_visual.get(line).copied()
                    }
                } else {
                    Some(line)
                };
                let Some(line) = line else { return };
                let y = (f32::from((track_height - px(3.)).max(px(0.))) * line as f32
                    / line_count as f32)
                    .round() as usize;
                markers.insert(y, color);
            };
            if original {
                for &line in &self.diff_highlights.removed {
                    mark(line, 0xee938e, false);
                }
            } else {
                if self.show_diff && self.side_by_side {
                    for (visual, row) in self.diff_highlights.layout.iter().enumerate() {
                        if row.before.is_some() && row.after.is_none() {
                            mark(visual, 0xee938e, true);
                        }
                    }
                } else {
                    for &line in &self.diff_highlights.deletion_anchors {
                        mark(line, 0xee938e, false);
                    }
                }
                for &line in &self.diff_highlights.added {
                    mark(line, 0x9ad7ae, false);
                }
                if let Some(tab) = self.active.and_then(|index| self.tabs.get(index)) {
                    for diagnostic in tab.diagnostics.iter().filter(|diagnostic| {
                        diagnostic.severity == highlight::DiagnosticSeverity::Warning
                    }) {
                        mark(diagnostic.line, diagnostic.severity.color(), false);
                    }
                    for diagnostic in tab.diagnostics.iter().filter(|diagnostic| {
                        diagnostic.severity == highlight::DiagnosticSeverity::Error
                    }) {
                        mark(diagnostic.line, diagnostic.severity.color(), false);
                    }
                }
            }
            let has_markers = !markers.is_empty();
            div()
                .absolute()
                .top(if self.show_diff && self.side_by_side {
                    px(32.)
                } else {
                    px(12.)
                })
                .bottom(if metrics.max_x > px(0.) {
                    px(if self.show_diff && self.side_by_side {
                        18.
                    } else {
                        26.
                    })
                } else {
                    px(if self.show_diff && self.side_by_side {
                        4.
                    } else {
                        12.
                    })
                })
                .right(px(12.))
                .w(px(10.))
                .bg(rgb(0x2c313a))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.click_editor_scrollbar(
                            original,
                            true,
                            event.position.y,
                            window,
                            cell_width,
                            cx,
                        );
                    }),
                )
                .children(markers.into_iter().map(|(y, color)| {
                    div()
                        .absolute()
                        .top(px(y as f32))
                        .right(px(0.))
                        .w(px(4.))
                        .h(px(3.))
                        .bg(rgb(color))
                }))
                .child(
                    div()
                        .absolute()
                        .left(px(1.))
                        .right(px(if has_markers { 4. } else { 1. }))
                        .top(thumb_position)
                        .h(thumb_size)
                        .bg(rgb(if dragging { 0xabb2bf } else { 0x5c6370 }))
                        .hover(|style| style.bg(rgb(0xabb2bf)))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.mouse_selecting = false;
                                this.original_mouse_selecting = false;
                                this.begin_editor_scroll_drag(
                                    original,
                                    true,
                                    event.position.y,
                                    window,
                                    cell_width,
                                );
                            }),
                        ),
                )
        });

        let horizontal = (metrics.max_x > px(0.)).then(|| {
            let track_width = metrics.viewport_width
                - if metrics.max_y > px(0.) {
                    px(14.)
                } else {
                    px(0.)
                };
            let (thumb_size, thumb_position) = scrollbar_thumb(
                metrics.viewport_width,
                track_width,
                metrics.max_x,
                metrics.scroll_x,
            );
            let dragging = matches!(
                if original {
                    self.original_scroll_drag
                } else {
                    self.editor_scroll_drag
                },
                Some(EditorScrollbarDrag::Horizontal { .. })
            );
            div()
                .absolute()
                .left(px(12.))
                .right(if metrics.max_y > px(0.) {
                    px(26.)
                } else {
                    px(12.)
                })
                .bottom(px(12.))
                .h(px(10.))
                .bg(rgb(0x2c313a))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.click_editor_scrollbar(
                            original,
                            false,
                            event.position.x,
                            window,
                            cell_width,
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(thumb_position)
                        .top(px(1.))
                        .bottom(px(1.))
                        .w(thumb_size)
                        .bg(rgb(if dragging { 0xabb2bf } else { 0x5c6370 }))
                        .hover(|style| style.bg(rgb(0xabb2bf)))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.mouse_selecting = false;
                                this.original_mouse_selecting = false;
                                this.begin_editor_scroll_drag(
                                    original,
                                    false,
                                    event.position.x,
                                    window,
                                    cell_width,
                                );
                            }),
                        ),
                )
        });

        (vertical, horizontal)
    }

    fn open_find(&mut self, cx: &mut Context<Self>) {
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

    fn close_find(&mut self) {
        self.find_open = false;
        self.find_has_focus = false;
        self.find_matches.clear();
        self.find_active = None;
        self.cursor_blink_visible = true;
    }

    fn update_find(&mut self) {
        self.refresh_find_matches();
        if let (Some(index), Some(match_index)) = (self.active, self.find_active) {
            self.tabs[index]
                .buffer
                .set_selection(self.find_matches[match_index].clone());
        } else if let Some(index) = self.active {
            self.tabs[index].buffer.clear_selection();
        }
    }

    fn refresh_find_matches(&mut self) {
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

    fn move_find(&mut self, backwards: bool) {
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

    fn edit_input(
        input: &mut SingleLineInput,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = &event.keystroke;
        if key.modifiers.secondary() && (key.key == "c" || key.key == "x") {
            if let Some(selected) = input.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
                if key.key == "x" {
                    return input.replace_selection("");
                }
            }
            return false;
        }
        let paste = if key.modifiers.secondary() && key.key == "v" {
            cx.read_from_clipboard().and_then(|item| item.text())
        } else {
            None
        };
        input.handle(event, paste.as_deref())
    }

    fn on_find_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
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

    fn update_query(&mut self) {
        let (needle, line) = palette_target(&self.palette_query.text);
        if needle.is_empty() && line.is_some() {
            self.quick = self
                .active
                .and_then(|i| self.tabs.get(i))
                .map(|tab| vec![tab.path.clone()])
                .unwrap_or_default();
            self.palette_selected = 0;
            return;
        }
        let needle = needle.to_lowercase();
        self.quick = self
            .file_index
            .iter()
            .filter(|path| path.to_string_lossy().to_lowercase().contains(&needle))
            .take(100)
            .cloned()
            .collect();
        self.palette_selected = 0;
        if !self.quick.is_empty() {
            self.palette_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
    }

    fn choose_palette(&mut self, chosen: Option<PathBuf>, cx: &mut Context<Self>) {
        let (name, line) = palette_target(&self.palette_query.text);
        let path = if name.is_empty() && line.is_some() {
            self.active
                .and_then(|i| self.tabs.get(i))
                .map(|tab| tab.path.clone())
        } else {
            chosen
        };
        if let Some(path) = path {
            self.open_at(path, line.unwrap_or(1), 0, 0, false, cx);
        }
        self.palette_open = false;
        cx.notify();
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.files_focused = false;
        self.close_find();
        self.palette_query.set_text(String::new());
        self.palette_selected = 0;
        self.quick.clear();
        window.focus(&self.focus);
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move { project::file_index(&root) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        match result {
                            Ok(paths) => {
                                this.file_index = paths;
                                if this.palette_open {
                                    this.update_query();
                                }
                            }
                            Err(error) => this.message = error,
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.search_id += 1;
        self.matches.clear();
        self.search_rows.clear();
        if self.query.text.is_empty() {
            self.message.clear();
            cx.notify();
            return;
        }
        let id = self.search_id;
        let root = self.root.clone();
        let query = self.query.text.clone();
        let options = self.search_options;
        self.message = "Buscando…".into();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    gpui::Timer::after(Duration::from_millis(180)).await;
                    if !weak
                        .update(&mut cx, |this, _| this.search_id == id)
                        .unwrap_or(false)
                    {
                        return;
                    }
                    let result = executor
                        .spawn(async move { project::search(&root, &query, options) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.search_id == id {
                            match result {
                                Ok(matches) => {
                                    this.matches = matches;
                                    this.search_rows =
                                        search_rows(&this.matches, &this.collapsed_search);
                                    this.message =
                                        format!("{} coincidencias (máx. 300)", this.matches.len());
                                }
                                Err(error) => {
                                    this.matches.clear();
                                    this.search_rows.clear();
                                    this.message = error;
                                }
                            }
                            cx.notify();
                        }
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    fn editor_active(&self) -> bool {
        !self.palette_open
            && !self.settings_open
            && !self.commit_focused
            && !(self.sidebar == Sidebar::Search && self.search_focused)
            && (!self.show_diff || self.side_by_side)
            && self
                .active
                .and_then(|index| self.tabs.get(index))
                .is_some_and(|tab| !tab.loading)
    }

    fn schedule_lint(&self, index: usize, cx: &mut Context<Self>) {
        let tab = &self.tabs[index];
        if !matches!(
            tab.path.extension().and_then(|ext| ext.to_str()),
            Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
        ) {
            return;
        }
        let root = self.root.clone();
        let path = tab.path.clone();
        let text = tab.buffer.text().to_owned();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    gpui::Timer::after(Duration::from_millis(350)).await;
                    if !weak
                        .update(&mut cx, |this, _| {
                            this.tabs
                                .iter()
                                .any(|tab| tab.path == path && tab.buffer.text() == text)
                        })
                        .unwrap_or(false)
                    {
                        return;
                    }
                    let result = executor
                        .spawn({
                            let root = root.clone();
                            let path = path.clone();
                            let text = text.clone();
                            async move { lint::eslint(&root, &path, &text) }
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if let Some(tab) = this
                            .tabs
                            .iter_mut()
                            .find(|tab| tab.path == path && tab.buffer.text() == text)
                        {
                            tab.lint_source = Some(text.clone());
                            tab.lint_diagnostics = result;
                            tab.diagnostics = highlight::diagnostics(&text, &path);
                            tab.diagnostics.extend(tab.lint_diagnostics.iter().cloned());
                            cx.notify();
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn rehighlight_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let path = self.tabs[index].path.clone();
        let text = self.tabs[index].buffer.text().to_owned();
        self.tabs[index].max_line_chars = max_line_chars(&text);
        self.tabs[index].lines = highlight::line(&text, &path);
        let tab = &mut self.tabs[index];
        tab.diagnostics = highlight::diagnostics(&text, &path);
        if let Some(previous) = &tab.lint_source {
            tab.diagnostics.extend(lint_for_unchanged_lines(
                previous,
                &text,
                &tab.lint_diagnostics,
            ));
        }
        self.tabs[index].csv = (path.extension().and_then(|ext| ext.to_str()) == Some("csv"))
            .then(|| csv::layout(&text))
            .flatten();
        if let Some(csv) = &self.tabs[index].csv {
            self.tabs[index].max_line_chars = csv.max_chars;
        }
        self.schedule_lint(index, cx);
        if self.original_path.as_ref() == Some(&path) && self.active == Some(index) {
            self.diff_highlights = line_diff_highlights(&self.original_text, &text);
        }
    }

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let path = tab.path.clone();
        let text = tab.buffer.text().to_owned();
        let loaded_stamp = tab.loaded_stamp.clone();
        let current_stamp = match project::file_stamp(&self.root, &path) {
            Ok(stamp) => stamp,
            Err(error) => {
                self.message = format!("No se pudo verificar {}: {error}", path.display());
                cx.notify();
                return;
            }
        };
        if loaded_stamp.as_ref() != Some(&current_stamp) {
            self.message = format!(
                "{} cambió fuera del editor; recargalo antes de guardar",
                path.display()
            );
            cx.notify();
            return;
        }
        match project::write(&self.root, &path, &text) {
            Ok(()) => {
                if let Some(tab) = self.tabs.get_mut(index) {
                    tab.buffer.mark_saved();
                    tab.loaded_stamp = project::file_stamp(&self.root, &path).ok();
                }
                self.message = format!("Guardado {}", path.display());
                self.refresh(cx);
            }
            Err(error) => self.message = error,
        }
        cx.notify();
    }

    fn on_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.clone();
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
        let (handled, text_changed) = {
            let buffer = &mut self.tabs[index].buffer;
            if secondary && key == "left" {
                (true, buffer.move_word_left(modifiers.shift))
            } else if secondary && key == "right" {
                (true, buffer.move_word_right(modifiers.shift))
            } else if secondary && key == "backspace" {
                (true, buffer.delete_word_backward())
            } else if secondary && key == "delete" {
                (true, buffer.delete_word_forward())
            } else if secondary && key == "home" {
                (true, buffer.move_document_start(modifiers.shift))
            } else if secondary && key == "end" {
                (true, buffer.move_document_end(modifiers.shift))
            } else if modifiers.control && modifiers.shift && key == "k" {
                (true, buffer.delete_line())
            } else if modifiers.alt && modifiers.shift && key == "down" {
                (true, buffer.duplicate_line_down())
            } else if modifiers.alt && key == "up" {
                (true, buffer.move_line_up())
            } else if modifiers.alt && key == "down" {
                (true, buffer.move_line_down())
            } else if key == "left" {
                (true, buffer.move_left(modifiers.shift))
            } else if key == "right" {
                (true, buffer.move_right(modifiers.shift))
            } else if key == "up" {
                (true, buffer.move_up(modifiers.shift))
            } else if key == "down" {
                (true, buffer.move_down(modifiers.shift))
            } else if key == "home" {
                (true, buffer.move_home(modifiers.shift))
            } else if key == "end" {
                (true, buffer.move_end(modifiers.shift))
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
            } else if !modifiers.control
                && !modifiers.alt
                && !modifiers.platform
                && !modifiers.function
            {
                if let Some(text) = key_char {
                    if !text.chars().any(char::is_control) {
                        (true, buffer.insert_text(&text))
                    } else {
                        (false, false)
                    }
                } else {
                    (false, false)
                }
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

    fn on_original_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
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

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
            self.open_palette(window, cx);
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
            if !self.quick.is_empty() {
                self.palette_selected = if key.key == "up" {
                    self.palette_selected.saturating_sub(1)
                } else {
                    (self.palette_selected + 1).min(self.quick.len() - 1)
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
            self.choose_palette(self.quick.get(self.palette_selected).cloned(), cx);
            return;
        }
        if Self::edit_input(&mut self.palette_query, event, cx) {
            self.update_query();
        }
        self.cursor_blink_visible = true;
        cx.notify();
    }
    fn action(&mut self, op: &str, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        let Some(path) = self.selected.clone() else {
            return;
        };
        if op == "discard"
            && !self.confirm_discard.as_ref().is_some_and(|state| {
                self.discard_state().as_ref() == Ok(state)
                    && project::status(&self.root)
                        .is_ok_and(|changes| changes.contains(&state.change))
            })
        {
            self.confirm_discard = None;
            self.message = "El archivo cambió; confirmá el descarte nuevamente".into();
            cx.notify();
            return;
        }
        let result = match op {
            "stage" => project::stage(&self.root, &path),
            "unstage" => self
                .changes
                .iter()
                .find(|c| c.path == path)
                .ok_or_else(|| "Cambio no disponible".to_string())
                .and_then(|change| project::unstage(&self.root, change)),
            "discard" => self
                .changes
                .iter()
                .find(|c| c.path == path)
                .ok_or_else(|| "Cambio no disponible".to_string())
                .and_then(|change| project::discard_change(&self.root, change)),
            _ => return,
        };
        self.confirm_discard = None;
        self.message = match result {
            Ok(()) => format!("{op}: {}", path.display()),
            Err(e) => e,
        };
        self.refresh(cx);
        cx.notify();
    }

    fn toggle_branch_menu(&mut self, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        if self.branch_menu_open {
            self.branch_menu_open = false;
            cx.notify();
            return;
        }
        self.branch_menu_open = true;
        if self.branch_menu_loading {
            cx.notify();
            return;
        }
        self.branch_menu_loading = true;
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move { project::local_branches(&root) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.branch_menu_loading = false;
                        match result {
                            Ok(branches) => this.branches = branches,
                            Err(error) => this.message = format!("Git: {error}"),
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    fn git_operation(&mut self, operation: GitOperation, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        if let GitOperation::Commit(message) = &operation {
            if message.trim().is_empty() {
                self.message = "Escribí un mensaje de commit".into();
                cx.notify();
                return;
            }
        }
        self.branch_menu_open = false;
        self.branch_menu_loading = false;
        self.git_busy = true;
        self.git_progress_offset = 0.;
        self.message = "Procesando operación Git…".into();
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let (label, result) = executor
                        .spawn(async move {
                            match operation {
                                GitOperation::Commit(message) => {
                                    ("Commit", project::commit(&root, &message))
                                }
                                GitOperation::Push => ("Push", project::push(&root)),
                                GitOperation::Sync => ("Sync", project::sync(&root)),
                                GitOperation::SwitchBranch(branch) => {
                                    ("Cambiar branch", project::switch_branch(&root, &branch))
                                }
                                GitOperation::ApplyStash(reference) => {
                                    ("Aplicar stash", project::apply_stash(&root, &reference))
                                }
                                GitOperation::StageAll => ("Stage All", project::stage_all(&root)),
                                GitOperation::UnstageAll => {
                                    ("Unstage All", project::unstage_all(&root))
                                }
                                GitOperation::DiscardAll => {
                                    ("Discard All", project::discard_all(&root))
                                }
                            }
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.git_busy = false;
                        this.git_progress_offset = 0.;
                        if label == "Commit" && result.is_ok() {
                            this.commit_message.set_text(String::new());
                        }
                        this.message = match result {
                            Ok(()) => format!("{label} completado"),
                            Err(error) => error,
                        };
                        this.refresh(cx);
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    fn load_diff(&mut self) {
        self.diff_text = if self.show_diff {
            self.selected.as_ref().map_or_else(String::new, |path| {
                project::diff(&self.root, path).unwrap_or_else(|error| error)
            })
        } else {
            String::new()
        };
        self.diff_rows = aligned_diff(&self.diff_text);
        let original = self
            .selected
            .as_ref()
            .and_then(|path| {
                self.changes
                    .iter()
                    .find(|change| &change.path == path)
                    .map(|change| {
                        project::head_file(&self.root, change)
                            .unwrap_or_else(|error| format!("No se pudo leer HEAD: {error}"))
                    })
            })
            .unwrap_or_default();
        self.original_max_chars = max_line_chars(&original);
        self.original_lines = self
            .selected
            .as_ref()
            .map_or_else(Vec::new, |path| highlight::line(&original, path));
        self.original_text = original;
        if self.original_path != self.selected || self.original_buffer.text() != self.original_text
        {
            self.original_buffer = buffer::EditorBuffer::new(self.original_text.clone());
        }
        self.original_path = self.selected.clone();
        self.update_diff_highlights();
    }

    fn load_change_decorations(&mut self) {
        if self
            .selected
            .as_ref()
            .is_some_and(|path| self.changes.iter().any(|change| &change.path == path))
        {
            self.load_diff();
        } else {
            self.show_diff = false;
            self.diff_text.clear();
            self.diff_rows.clear();
            self.original_path = None;
            self.original_text.clear();
            self.original_lines.clear();
            self.diff_highlights = DiffHighlights::default();
            self.original_focused = false;
        }
    }

    fn update_diff_highlights(&mut self) {
        self.diff_highlights = self
            .active
            .and_then(|index| self.tabs.get(index))
            .filter(|tab| !tab.loading && self.selected.as_ref() == Some(&tab.path))
            .map(|tab| line_diff_highlights(&self.original_text, tab.buffer.text()))
            .unwrap_or_default();
        if self.diff_highlights.layout.is_empty() && self.show_diff && self.side_by_side {
            let before: Vec<_> = self.original_text.split('\n').collect();
            let (layout, before_to_visual, after_to_visual) = aligned_lines(&before, &[], &[]);
            self.diff_highlights.layout = layout;
            self.diff_highlights.before_to_visual = before_to_visual;
            self.diff_highlights.after_to_visual = after_to_visual;
        }
    }

    fn scroll_to_first_change(&self) {
        if let Some(visual) = self.diff_highlights.first_visual() {
            self.original_scroll
                .scroll_to_item_strict(visual, ScrollStrategy::Center);
            self.editor_scroll
                .scroll_to_item_strict(visual, ScrollStrategy::Center);
        }
    }

    fn accept_conflict(
        &mut self,
        block: ConflictBlock,
        choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.active else { return };
        let text = self.tabs[index].buffer.text().to_owned();
        if !conflict_blocks(&text)
            .iter()
            .any(|found| found.start == block.start && found.end == block.end)
        {
            return;
        }
        let replacement = resolve_conflict(&text, block, choice);
        self.tabs[index]
            .buffer
            .set_selection(block.start..block.end);
        self.tabs[index].buffer.insert_text(&replacement);
        self.rehighlight_tab(index, cx);
        self.focus_first_conflict();
        self.message = "Conflicto resuelto; guardá el archivo y hacé Stage".into();
        cx.notify();
    }

    fn focus_first_conflict(&mut self) {
        let Some(index) = self.active else { return };
        let text = self.tabs[index].buffer.text();
        let Some(block) = conflict_blocks(text).first().copied() else {
            return;
        };
        let line = text[..block.start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1;
        self.go_to(line, 0, 0);
    }

    fn update_settings(
        &mut self,
        change: impl FnOnce(&mut settings::Settings),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.settings);
        if let Err(error) = self.settings.save() {
            self.message = format!("No se pudieron guardar las preferencias: {error}");
        }
        cx.notify();
    }

    fn button(
        label: impl Into<String>,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .text_color(rgb(FG))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x3e4451)))
            .on_mouse_up(MouseButton::Left, click)
            .child(label.into())
    }

    fn icon_button(
        name: &'static str,
        label: &'static str,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        Self::icon_button_sized(name, label, px(22.), false, None, click)
    }

    fn icon_button_sized(
        name: &'static str,
        label: &'static str,
        size: Pixels,
        active: bool,
        badge: Option<usize>,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(label)
            .relative()
            .size(size)
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .when(active, |button| button.bg(rgb(0x3e4451)))
            .hover(|s| s.bg(rgb(0x3e4451)))
            .on_hover(|hovered, window, _| {
                if *hovered {
                    window.refresh();
                }
            })
            .tooltip(move |_, cx| cx.new(|_| IconTooltip(label)).into())
            .on_mouse_up(MouseButton::Left, click)
            .child(icons::icon(name, if size == px(38.) { FG } else { MUTED }))
            .when_some(badge.filter(|count| *count > 0), |button, count| {
                button.child(
                    div()
                        .absolute()
                        .right(px(0.))
                        .bottom(px(0.))
                        .min_w(px(18.))
                        .h(px(18.))
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(0x61afef))
                        .text_color(rgb(0x181a1f))
                        .text_size(px(10.))
                        .child(count.to_string()),
                )
            })
    }

    fn menu_item(
        label: &'static str,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .w_full()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(FG))
            .hover(|s| s.bg(rgb(0x3e4451)))
            .on_mouse_up(MouseButton::Left, click)
            .child(label)
    }

    fn alert_dialog(
        title: &'static str,
        detail: String,
        confirm: impl IntoElement,
        cancel: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x101116aa))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w(px(430.))
                    .max_w_full()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(0xee938e))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(rgb(0xd7dae0)).child(title))
                    .child(detail)
                    .child(div().flex().gap_2().child(confirm).child(cancel)),
            )
    }

    fn option(
        icon: &'static str,
        label: &'static str,
        enabled: bool,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(label)
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(if enabled { 0x3e4451 } else { BG }))
            .cursor_pointer()
            .on_hover(|hovered, window, _| {
                if *hovered {
                    window.refresh();
                }
            })
            .tooltip(move |_, cx| cx.new(|_| IconTooltip(label)).into())
            .on_mouse_up(MouseButton::Left, click)
            .child(icons::icon(icon, if enabled { FG } else { MUTED }))
    }

    fn stage_row(&mut self, path: &Path, staged: bool, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        let result = if staged {
            self.changes
                .iter()
                .find(|change| change.path == path)
                .ok_or_else(|| "Cambio no disponible".to_string())
                .and_then(|change| project::unstage(&self.root, change))
        } else {
            project::stage(&self.root, path)
        };
        self.message = match result {
            Ok(()) => format!(
                "{}: {}",
                if staged { "Unstage" } else { "Stage" },
                path.display()
            ),
            Err(error) => error,
        };
        self.refresh(cx);
        cx.notify();
    }

    fn change_row(&self, index: usize, staged: bool, cx: &mut Context<Self>) -> gpui::Div {
        let change = &self.changes[index];
        let path = change.path.clone();
        let is_selected = self.selected.as_ref() == Some(&path);
        let (added, removed) = self.change_counts.get(index).copied().unwrap_or_default();
        let group = format!("git-row-{index}-{staged}");
        div()
            .h(px(26.))
            .w_full()
            .p_1()
            .group(group.clone())
            .cursor_pointer()
            .bg(rgb(if is_selected { 0x3e4451 } else { PANEL }))
            .text_color(rgb(FG))
            .hover(|style| style.bg(rgb(0x3e4451)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.commit_focused = false;
                    this.open(path.clone(), cx);
                    this.sidebar = Sidebar::Git;
                    this.show_diff = !this.changes.iter().any(|change| {
                        change.path == path
                            && (change.index == 'U'
                                || change.worktree == 'U'
                                || (change.index == 'A' && change.worktree == 'A')
                                || (change.index == 'D' && change.worktree == 'D'))
                    });
                    this.side_by_side = true;
                    if this.show_diff {
                        this.load_diff();
                        if this
                            .active
                            .and_then(|i| this.tabs.get(i))
                            .is_some_and(|tab| !tab.loading)
                        {
                            this.scroll_to_first_change();
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener({
                    let path = change.path.clone();
                    move |this, _, _, cx| {
                        this.open(path.clone(), cx);
                        this.sidebar = Sidebar::Git;
                    }
                }),
            )
            .flex()
            .items_center()
            .gap_1()
            .child(format!("{}{}", change.index, change.worktree))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(change.path.display().to_string()),
            )
            .child(
                div()
                    .w(px(20.))
                    .h(px(20.))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.)
                    .group_hover(group, |style| style.opacity(1.))
                    .hover(|style| style.bg(rgb(0x505766)))
                    .text_color(rgb(if staged { 0xee938e } else { 0x9ad7ae }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener({
                            let path = change.path.clone();
                            move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.stage_row(&path, staged, cx);
                            }
                        }),
                    )
                    .child(if staged { "−" } else { "+" }),
            )
            .child(div().text_color(rgb(0x9ad7ae)).child(format!("+{added}")))
            .child(div().text_color(rgb(0xee938e)).child(format!("-{removed}")))
    }
}

impl Render for Reviewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = self.settings.background();
        let panel = self.settings.panel();
        let font_name = self.settings.font_name();
        let font_size = self.settings.font_size as f32;
        let git_view = self.sidebar == Sidebar::Git;
        let search_mode = self.sidebar == Sidebar::Search;
        let font_id = cx.text_system().resolve_font(&gpui::font(font_name));
        let editor_cell_width = cx
            .text_system()
            .ch_advance(font_id, px(font_size))
            .unwrap_or(px(8.4));
        let (vertical_scrollbar, horizontal_scrollbar) =
            self.editor_scrollbars(false, window, editor_cell_width, cx);
        let (original_vertical, original_horizontal) =
            self.editor_scrollbars(true, window, editor_cell_width, cx);
        let (split_vertical, normal_vertical) = if self.show_diff && self.side_by_side {
            (vertical_scrollbar, None)
        } else {
            (None, vertical_scrollbar)
        };
        let (split_horizontal, normal_horizontal) = if self.show_diff && self.side_by_side {
            (horizontal_scrollbar, None)
        } else {
            (None, horizontal_scrollbar)
        };
        let find_caret_visible = self.find_open
            && self.find_has_focus
            && self.cursor_blink_visible
            && self.focus.is_focused(window);
        let search_caret_visible = search_mode
            && self.search_focused
            && self.cursor_blink_visible
            && self.focus.is_focused(window);
        let commit_caret_visible = git_view
            && self.commit_focused
            && self.cursor_blink_visible
            && self.focus.is_focused(window);
        let find_cell_width = cx
            .text_system()
            .ch_advance(font_id, px(14.))
            .unwrap_or(px(8.4));
        let selected = self.selected.clone();
        let change = selected
            .as_ref()
            .and_then(|p| self.changes.iter().find(|c| &c.path == p));
        let can_stage = change.is_some_and(|c| c.worktree != ' ' || c.index == '?');
        let can_unstage = change.is_some_and(|c| c.index != ' ' && c.index != '?');
        let can_discard =
            change.is_some_and(|c| c.index == '?' || c.worktree != ' ' && c.worktree != '?');
        let has_staged = self
            .changes
            .iter()
            .any(|c| c.index != ' ' && c.index != '?');
        let can_push = self.sync_status.is_some_and(|status| status.ahead > 0);
        let can_sync = self.sync_status.is_some_and(|status| status.behind > 0);
        let primary_enabled = if has_staged {
            !self.commit_message.text.trim().is_empty()
        } else {
            can_sync || can_push
        } && !self.git_busy;
        let git_progress_width =
            (sidebar_width(window.bounds().size.width, self.sidebar_width) - px(24.)).max(px(1.));
        let git_progress_segment = px(48.);
        let git_progress_left =
            (git_progress_width - git_progress_segment).max(px(0.)) * self.git_progress_offset;
        let branch_menu_open = self.branch_menu_open;
        let branch_menu_loading = self.branch_menu_loading;
        let branches = self.branches.clone();
        let branch_count = branches.len();
        let branch_scroll = self.branch_scroll.clone();
        let conflicts = self
            .active
            .and_then(|i| self.tabs.get(i))
            .filter(|tab| !tab.loading && (!self.show_diff || self.side_by_side))
            .map(|tab| conflict_blocks(tab.buffer.text()))
            .unwrap_or_default();
        let rail = div()
            .w(px(46.))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt_2()
            .bg(rgb(0x181a1f))
            .child(Self::icon_button_sized(
                "files",
                "Explorador",
                px(38.),
                self.sidebar_visible && self.sidebar == Sidebar::Files,
                None,
                cx.listener(|this, _, window, cx| this.toggle_sidebar(Sidebar::Files, window, cx)),
            ))
            .child(Self::icon_button_sized(
                "search",
                "Búsqueda",
                px(38.),
                self.sidebar_visible && search_mode,
                None,
                cx.listener(|this, _, window, cx| this.toggle_sidebar(Sidebar::Search, window, cx)),
            ))
            .child(Self::icon_button_sized(
                "git",
                "Control de código fuente",
                px(38.),
                self.sidebar_visible && git_view,
                Some(self.changes.len()),
                cx.listener(|this, _, window, cx| this.toggle_sidebar(Sidebar::Git, window, cx)),
            ))
            .child(div().flex_1())
            .child(Self::icon_button_sized(
                "settings",
                "Preferencias",
                px(38.),
                self.settings_open,
                None,
                cx.listener(|this, _, window, cx| {
                    this.settings_open = !this.settings_open;
                    this.file_edit = None;
                    this.files_focused = false;
                    this.palette_open = false;
                    this.close_find();
                    window.focus(&this.focus);
                    cx.notify();
                }),
            ));
        let sidebar = div()
            .w(sidebar_width(window.bounds().size.width, self.sidebar_width))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(panel))
            .child(
                div()
                    .h(px(36.))
                    .pl_3()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(if search_mode {
                        "BÚSQUEDA"
                    } else if git_view {
                        "CONTROL DE CÓDIGO FUENTE"
                    } else {
                        "EXPLORADOR"
                    }),
            )
            .child(
                div()
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(FG))
                    .when(!search_mode && !git_view, |v| {
                        v.child(div().flex().items_center().flex_1().min_w_0().cursor_pointer()
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                if cx.has_active_drag() { return; }
                                this.root_expanded = !this.root_expanded;
                                cx.notify();
                            }))
                            .child(icons::icon(if self.root_expanded { "chevron-down" } else { "chevron-right" }, MUTED))
                            .child(icons::icon(if self.root_expanded { "folder-open" } else { "folder" }, 0xe5c07b))
                            .child(div().ml_1().overflow_hidden().child(
                                self.root.file_name().unwrap_or_default().to_string_lossy().to_uppercase()
                            ))
                            .can_drop(|drag, _, _| drag.downcast_ref::<ExplorerDrag>()
                                .is_some_and(|drag| can_move_into(&drag.0, Path::new(""))))
                            .drag_over::<ExplorerDrag>(|style, drag, _, _| {
                                if can_move_into(&drag.0, Path::new("")) { style.bg(rgb(0x3e4451)) } else { style }
                            })
                            .on_drop(cx.listener(|this, drag: &ExplorerDrag, window, cx| {
                                window.focus(&this.focus);
                                this.move_explorer_entry(&drag.0, Path::new(""), cx);
                            })))
                        .child(Self::icon_button("file-plus-corner", "Nuevo archivo", cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus);
                            this.begin_file_edit(FileEdit::Create(selected_folder(this.selected.as_ref(), &this.files)), cx);
                        })))
                        .child(Self::icon_button("folder-plus", "Nueva carpeta", cx.listener(|this, _, window, cx| {
                            window.focus(&this.focus);
                            this.begin_file_edit(FileEdit::CreateFolder(PathBuf::new()), cx);
                        })))
                        .child(Self::icon_button("refresh-cw", "Actualizar explorador", cx.listener(|this, _, _, cx| this.refresh(cx))))
                        .child(Self::icon_button("collapse-all", "Contraer carpetas", cx.listener(|this, _, _, cx| {
                            this.expanded.clear();
                            this.update_visible();
                            cx.notify();
                        })))
                    })
                    .when(search_mode || git_view, |v| v.child(
                        self.root.file_name().unwrap_or_default().to_string_lossy().to_uppercase()
                    )),
            )
            .child(div().h(px(1.)).bg(rgb(0x3a3f4b)))
            .when(search_mode, |v| {
                v.child(
                    div()
                        .h(px(30.))
                        .mx_3()
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(if self.search_focused { 0x61afef } else { 0x3e4451 }))
                        .bg(rgb(background))
                        .child(
                            input_view(&self.query, "Buscar en archivos…", self.search_focused,
                                search_caret_visible,
                                sidebar_input_columns(sidebar_width(window.bounds().size.width, self.sidebar_width), 126., find_cell_width),
                                find_cell_width, background, false)
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .border_0()
                                .font_family(font_name)
                                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| {
                                    this.search_focused = true;
                                    this.find_has_focus = false;
                                    this.cursor_blink_visible = true;
                                    window.focus(&this.focus);
                                    cx.notify();
                                })),
                        )
                        .child(div().flex().items_center().mr_1()
                        .child(Self::option(
                            "case-sensitive",
                            "Distinguir mayúsculas",
                            self.search_options.case_sensitive,
                            cx.listener(|this, _, _, cx| {
                                this.search_options.case_sensitive =
                                    !this.search_options.case_sensitive;
                                if !this.query.text.is_empty() {
                                    this.run_search(cx);
                                } else {
                                    cx.notify();
                                }
                            }),
                        ))
                        .child(Self::option(
                            "whole-word",
                            "Palabra completa",
                            self.search_options.whole_word,
                            cx.listener(|this, _, _, cx| {
                                this.search_options.whole_word = !this.search_options.whole_word;
                                if !this.query.text.is_empty() {
                                    this.run_search(cx);
                                } else {
                                    cx.notify();
                                }
                            }),
                        ))
                        .child(Self::option(
                            "regex",
                            "Regex",
                            self.search_options.regex,
                            cx.listener(|this, _, _, cx| {
                                this.search_options.regex = !this.search_options.regex;
                                if !this.query.text.is_empty() {
                                    this.run_search(cx);
                                } else {
                                    cx.notify();
                                }
                            }),
                        ))
                        .child(Self::option(
                            "eye",
                            "Ignorados",
                            self.search_options.include_ignored,
                            cx.listener(|this, _, _, cx| {
                                this.search_options.include_ignored =
                                    !this.search_options.include_ignored;
                                if !this.query.text.is_empty() {
                                    this.run_search(cx);
                                } else {
                                    cx.notify();
                                }
                            }),
                        ))),
                )
            })
            .when(!git_view, |v| {
                v.child(
                    div().flex_1().min_h_0()
                        .on_mouse_up(MouseButton::Right, cx.listener(|this, event: &MouseUpEvent, window, cx| {
                            if this.sidebar != Sidebar::Files { return; }
                            this.selected = None;
                            this.files_focused = true;
                            this.file_menu = Some((PathBuf::new(), true, event.position));
                            window.focus(&this.focus);
                            cx.stop_propagation();
                            cx.notify();
                        }))
                        .child(
                        uniform_list(
                            "files",
                            if search_mode {
                                self.search_rows.len()
                            } else {
                                if self.root_expanded { explorer_rows(&self.visible, &self.files, self.file_edit.as_ref()).len() } else { 0 }
                            },
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, window, cx| {
                                    let explorer = explorer_rows(&this.visible, &this.files, this.file_edit.as_ref());
                                    range
                                        .map(|i| {
                                            if this.sidebar == Sidebar::Search {
                                                return match &this.search_rows[i] {
                                                    SearchRow::File(path, count) => {
                                                        let collapsed = this.collapsed_search.contains(path);
                                                        let target = path.clone();
                                                        div().h(px(27.)).w_full().flex().items_center().gap_1().px_2()
                                                            .text_color(rgb(0xd7dae0)).bg(rgb(panel))
                                                            .cursor_pointer().hover(|s| s.bg(rgb(0x3e4451)))
                                                            .on_mouse_up(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                                                if !this.collapsed_search.insert(target.clone()) {
                                                                    this.collapsed_search.remove(&target);
                                                                }
                                                                this.search_rows = search_rows(&this.matches, &this.collapsed_search);
                                                                cx.notify();
                                                            }))
                                                            .child(icons::icon(if collapsed { "chevron-right" } else { "chevron-down" }, MUTED))
                                                            .child(icons::file_icon(path))
                                                            .child(path.display().to_string())
                                                            .child(div().text_color(rgb(MUTED)).child(format!(" ({count})")))
                                                    },
                                                    SearchRow::Match(index) => {
                                                        let found = &this.matches[*index];
                                                        let (path, line, start, end) = (
                                                            found.path.clone(),
                                                            found.line,
                                                            found.start,
                                                            found.end,
                                                        );
                                                        div()
                                                            .h(px(27.))
                                                            .w_full()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1()
                                                            .pl_4()
                                                            .overflow_hidden()
                                                            .text_color(rgb(MUTED))
                                                            .cursor_pointer()
                                                            .hover(|s| s.bg(rgb(0x3e4451)))
                                                            .on_mouse_up(
                                                                MouseButton::Left,
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.open_at(
                                                                            path.clone(),
                                                                            line,
                                                                            start,
                                                                            end,
                                                                            true,
                                                                            cx,
                                                                        );
                                                                    },
                                                                ),
                                                            )
                                                            .child(format!("{line}"))
                                                            .child(
                                                                div()
                                                                    .text_color(rgb(FG))
                                                                    .child(match_preview(found)),
                                                            )
                                                    }
                                                }.into_any_element();
                                            }
                                              let row = explorer[i];
                                             if let ExplorerRow::NewFile(depth) = row {
                                                 return div().h(px(23.)).w_full().flex().items_center()
                                                      .pl(px(22. + depth as f32 * 16.))
                                                     .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                                                         window.focus(&this.focus);
                                                         this.cursor_blink_visible = true;
                                                         cx.notify();
                                                     }))
                                                      .child(icons::icon(if matches!(this.file_edit, Some(FileEdit::CreateFolder(_))) { "folder" } else { "file" }, MUTED))
                                                      .child(input_view(&this.file_name, "Nombre…", true,
                                                          this.cursor_blink_visible && this.focus.is_focused(window),
                                                          ((245. - depth as f32 * 16.) / f32::from(find_cell_width)).max(2.) as usize,
                                                          find_cell_width, background, true).flex_1().min_w_0()
                                                          .font_family(font_name)).into_any_element();
                                             }
                                             let ExplorerRow::Entry(index, depth) = row else { unreachable!() };
                                            let entry = &this.files[index];
                                            let path = entry.path.clone();
                                            let row_selected =
                                                this.selected.as_ref() == Some(&path);
                                             let file_icon = icons::file_icon(&path);
                                             let file_icon_name = icons::file_icon_name(&path);
                                             let ignored = this.ignored.contains(&path);
                                             let renaming = this.file_edit.as_ref().is_some_and(|edit| matches!(edit, FileEdit::Rename(target) if target == &path));
                                            let is_dir = entry.is_dir;
                                            let expanded = this.expanded.contains(&path);
                                            let name = path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned();
                                             div()
                                                 .id(("explorer-entry", i))
                                                 .h(px(23.))
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                 .pl(px(22. + depth as f32 * 16.))
                                                 .text_color(rgb(if ignored { 0x626975 } else { FG }))
                                                 .cursor_pointer()
                                                 .when(!renaming, |row| row.on_drag(ExplorerDrag(path.clone()), |drag, _, _, cx| cx.new(|_| drag.clone())))
                                                 .hover(|s| s.bg(rgb(0x3e4451)))
                                                .bg(rgb(if row_selected {
                                                    0x3e4451
                                                } else {
                                                    panel
                                                }))
                                                .on_mouse_down(MouseButton::Left, cx.listener({
                                                     let path = entry.path.clone();
                                                     move |this, _, window, cx| {
                                                         window.focus(&this.focus);
                                                         this.files_focused = true;
                                                         if this.file_edit.as_ref().is_some_and(|edit| matches!(edit, FileEdit::Rename(target) if target == &path)) {
                                                             this.cursor_blink_visible = true;
                                                            cx.notify();
                                                        }
                                                    }
                                                }))
                                                 .on_mouse_up(
                                                     MouseButton::Left,
                                                       cx.listener(move |this, _, _, cx| {
                                                           if cx.has_active_drag() { return; }
                                                           if this.file_edit.as_ref().is_some_and(|edit| matches!(edit, FileEdit::Rename(target) if target == &path)) {
                                                              return;
                                                          }
                                                           this.file_edit = None;
                                                           this.file_menu = None;
                                                          this.selected = Some(path.clone());
                                                          if is_dir {
                                                             this.toggle_folder(&path);
                                                             cx.notify();
                                                          } else {
                                                             this.open(path.clone(), cx);
                                                          }
                                                          this.files_focused = true;
                                                      }),
                                                 )
                                                 .on_mouse_up(MouseButton::Right, cx.listener({
                                                     let path = entry.path.clone();
                                                     move |this, event: &MouseUpEvent, window, cx| {
                                                          this.selected = Some(path.clone());
                                                          this.files_focused = true;
                                                         this.file_menu = Some((path.clone(), is_dir, event.position));
                                                         window.focus(&this.focus);
                                                         cx.stop_propagation();
                                                         cx.notify();
                                                     }
                                                  }))
                                                 .when(is_dir, |row| {
                                                     let folder = entry.path.clone();
                                                     row.can_drop({
                                                         let folder = folder.clone();
                                                         move |value, _, _| value.downcast_ref::<ExplorerDrag>()
                                                             .is_some_and(|drag| can_move_into(&drag.0, &folder))
                                                     })
                                                     .drag_over::<ExplorerDrag>({
                                                         let folder = folder.clone();
                                                         move |style, drag, _, _| {
                                                             if can_move_into(&drag.0, &folder) { style.bg(rgb(0x505766)) } else { style }
                                                         }
                                                     })
                                                     .on_drop(cx.listener(move |this, drag: &ExplorerDrag, window, cx| {
                                                         window.focus(&this.focus);
                                                         this.move_explorer_entry(&drag.0, &folder, cx);
                                                     }))
                                                 })
                                                .child(div().size(px(16.)).flex_shrink_0().when(is_dir, |slot| slot.child(icons::icon(
                                                    if expanded { "chevron-down" } else { "chevron-right" },
                                                    if ignored { 0x626975 } else { MUTED },
                                                ))))
                                                .child(if is_dir {
                                                    icons::icon(
                                                        if expanded {
                                                            "folder-open"
                                                        } else {
                                                            "folder"
                                                        },
                                                         if ignored { 0x626975 } else { 0xe5c07b },
                                                    )
                                                } else {
                                                     if ignored { icons::icon(file_icon_name, 0x626975) } else { file_icon }
                                                })
                                                 .child(if this.file_edit.as_ref().is_some_and(|edit| matches!(edit, FileEdit::Rename(target) if target == &entry.path)) {
                                                     input_view(&this.file_name, "Nombre…", true,
                                                         this.cursor_blink_visible && this.focus.is_focused(window),
                                                          ((220. - depth as f32 * 16.) / f32::from(find_cell_width)).max(2.) as usize,
                                                          find_cell_width, background, true)
                                                          .flex_1().min_w_0().font_family(font_name).into_any_element()
                                                  } else {
                                                      div().child(name).into_any_element()
                                                 }).into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                },
                            ),
                        )
                        .track_scroll(self.files_scroll.clone())
                        .h_full(),
                    ),
                )
            })
            .when(git_view, |v| {
                v.child(
                    input_view(&self.commit_message, "Mensaje de commit…", self.commit_focused,
                        commit_caret_visible,
                        sidebar_input_columns(sidebar_width(window.bounds().size.width, self.sidebar_width), 42., find_cell_width),
                        find_cell_width, background, false)
                        .h(px(30.))
                        .mx_3()
                        .mt_2()
                        .font_family(font_name)
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.commit_focused = true;
                                this.original_focused = false;
                                this.find_has_focus = false;
                                this.cursor_blink_visible = true;
                                window.focus(&this.focus);
                                cx.notify();
                            }),
                        )
                        ,
                )
                .child(
                    div()
                        .flex()
                        .w_full()
                        .px_3()
                        .mt_2()
                        .mb_3()
                        .gap_1()
                        .child(
                            div()
                                .h(px(30.))
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .bg(rgb(if primary_enabled { 0x2374b8 } else { 0x3e4451 }))
                                .text_color(rgb(0xffffff))
                                .when(primary_enabled, |v| v.cursor_pointer().hover(|s| s.bg(rgb(0x3184ca))))
                                .on_mouse_up(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                    if !primary_enabled {
                                        return;
                                    }
                                    if has_staged {
                                        this.git_operation(GitOperation::Commit(this.commit_message.text.clone()), cx);
                                    } else if can_sync {
                                        this.git_operation(GitOperation::Sync, cx);
                                    } else {
                                        this.git_operation(GitOperation::Push, cx);
                                    }
                                }))
                                .child(if has_staged || !can_sync && !can_push {
                                    "Commit"
                                } else if can_sync {
                                    "Sincronizar cambios"
                                } else {
                                    "Push"
                                }),
                        ),
                )
                .when(self.git_busy, |view| {
                    view.child(
                        div()
                            .relative()
                            .mx_3()
                            .h(px(3.))
                            .bg(rgb(0x3e4451))
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .left(git_progress_left)
                                    .top(px(0.))
                                    .w(git_progress_segment)
                                    .h_full()
                                    .bg(rgb(0x61afef)),
                            ),
                    )
                })
                .when(has_staged && (can_sync || can_push), |v| {
                    v.child(div().px_3().child(Self::button(
                        if can_sync { "Sync" } else { "Push" },
                        cx.listener(move |this, _, _, cx| {
                            this.git_operation(if can_sync { GitOperation::Sync } else { GitOperation::Push }, cx);
                        }),
                    )))
                })
                .child(
                    div().flex_1().min_h_0().w_full().child(
                        uniform_list(
                            "git-changes",
                            self.git_rows.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| match this.git_rows[index] {
                                        GitRow::StagedHeader => div().h(px(26.)).w_full().flex().items_center().text_color(rgb(MUTED))
                                            .child(format!("STAGED ({})", this.git_rows.iter().filter(|row| matches!(row, GitRow::Change(_, true))).count()))
                                            .child(div().flex_1())
                                            .child(Self::icon_button("minus", "Unstage All", cx.listener(|this, _, _, cx| this.git_operation(GitOperation::UnstageAll, cx)))),
                                        GitRow::UnstagedHeader => div()
                                            .h(px(26.))
                                            .w_full()
                                            .flex().items_center()
                                            .text_color(rgb(MUTED))
                                            .child(format!("SIN STAGE ({})", this.git_rows.iter().filter(|row| matches!(row, GitRow::Change(_, false))).count()))
                                            .child(div().flex_1())
                                            .child(Self::icon_button("plus", "Stage All", cx.listener(|this, _, _, cx| this.git_operation(GitOperation::StageAll, cx))))
                                            .child(Self::icon_button("trash", "Discard All", cx.listener(|this, _, _, cx| {
                                                this.confirm_discard_all = true;
                                                cx.notify();
                                            }))),
                                        GitRow::Change(index, staged) => {
                                              this.change_row(index, staged, cx)
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h_full().w_full(),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .when(can_stage, |v| {
                            v.child(Self::button(
                                "Stage",
                                cx.listener(|this, _, _, cx| this.action("stage", cx)),
                            ))
                        })
                        .when(can_unstage, |v| {
                            v.child(Self::button(
                                "Unstage",
                                cx.listener(|this, _, _, cx| this.action("unstage", cx)),
                            ))
                        })
                        .when(can_discard, |v| {
                            v.child(Self::button(
                                "Descartar",
                                cx.listener(|this, _, _, cx| {
                                    if this.confirm_discard.is_some() {
                                        this.action("discard", cx);
                                    } else {
                                        match this.discard_state() {
                                            Ok(state) => this.confirm_discard = Some(state),
                                            Err(error) => this.message = error,
                                        }
                                        cx.notify();
                                    }
                                }),
                            ))
                        }),
                )
                .when(!self.stashes.is_empty(), |v| {
                    v.child(div().text_color(rgb(MUTED)).child("STASHES"))
                        .child(
                            div().h(px(130.)).child(
                                uniform_list(
                                    "stashes",
                                    self.stashes.len(),
                                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                        range
                                            .map(|index| {
                                                let stash = &this.stashes[index];
                                                let reference = stash.reference.clone();
                                                div()
                                                    .h(px(32.))
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .overflow_hidden()
                                                            .text_color(rgb(MUTED))
                                                            .child(format!(
                                                                "{} {}",
                                                                stash.reference, stash.message
                                                            )),
                                                    )
                                                    .child(Self::button(
                                                        "Aplicar",
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.git_operation(
                                                                GitOperation::ApplyStash(
                                                                    reference.clone(),
                                                                ),
                                                                cx,
                                                            );
                                                        }),
                                                    ))
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .h_full(),
                            ),
                        )
                })
            });
        let toolbar = div()
            .flex()
            .h(px(32.))
            .items_center()
            .gap_2()
            .px_2()
            .bg(rgb(background))
            .when(
                self.show_diff || change.is_some_and(|c| c.index != '?'),
                |v| {
                    v.child(Self::button(
                        if self.show_diff { "Archivo" } else { "Diff" },
                        cx.listener(|this, _, _, cx| {
                            this.show_diff = !this.show_diff;
                            if this.show_diff {
                                this.load_diff();
                                this.scroll_to_first_change();
                            }
                            cx.notify();
                        }),
                    ))
                },
            )
            .when(self.show_diff, |v| {
                v.child(Self::button(
                    if self.side_by_side {
                        "Unificado"
                    } else {
                        "En paralelo"
                    },
                    cx.listener(|this, _, _, cx| {
                        this.side_by_side = !this.side_by_side;
                        if this.side_by_side {
                            this.scroll_to_first_change();
                        }
                        cx.notify();
                    }),
                ))
            })
            .when_some(conflicts.first().copied(), |v, block| {
                v.child(format!("Conflictos: {}", conflicts.len()))
                    .child(Self::button(
                        "Aceptar actual",
                        cx.listener(move |this, _, _, cx| {
                            this.accept_conflict(block, ConflictChoice::Current, cx)
                        }),
                    ))
                    .child(Self::button(
                        "Aceptar entrante",
                        cx.listener(move |this, _, _, cx| {
                            this.accept_conflict(block, ConflictChoice::Incoming, cx)
                        }),
                    ))
                    .child(Self::button(
                        "Aceptar ambos",
                        cx.listener(move |this, _, _, cx| {
                            this.accept_conflict(block, ConflictChoice::Both, cx)
                        }),
                    ))
            });
        let tabs =
            div()
                .flex()
                .h(px(34.))
                .bg(rgb(0x181a1f))
                .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                    let path = tab.path.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(34.))
                        .px_2()
                        .border_r_1()
                        .border_color(rgb(0x181a1f))
                        .on_mouse_up(
                            MouseButton::Middle,
                            cx.listener(move |this, _, _, cx| this.close_tab(i, cx)),
                        )
                        .bg(rgb(if self.active == Some(i) {
                            background
                        } else {
                            0x21252b
                        }))
                        .text_color(rgb(if self.active == Some(i) {
                            0xd7dae0
                        } else {
                            MUTED
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| this.open(path.clone(), cx)),
                                )
                                .child(icons::file_icon(&tab.path))
                                .when(tab.buffer.is_dirty(), |v| {
                                    v.child(div().size(px(7.)).rounded_full().bg(rgb(0xe5c07b)))
                                })
                                .child(
                                    tab.path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string(),
                                ),
                        )
                        .child(
                            div()
                                .p_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0x3e4451)))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| this.close_tab(i, cx)),
                                )
                                .child(icons::icon("close", MUTED)),
                        )
                }));
        let mut content = div()
            .flex()
            .flex_col()
            .text_color(rgb(FG))
            .text_sm()
            .font_family(font_name)
            .text_size(px(font_size))
            .min_w_0();
        if self.show_diff && !self.side_by_side {
            if self.diff_rows.is_empty() {
                content =
                    content.child("Sin diferencias contra HEAD (archivo nuevo o sin cambios)");
            } else {
                content = content.h_full().child(
                    uniform_list(
                        "diff-lines",
                        self.diff_rows.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                            range
                                .map(|n| {
                                    let row = &this.diff_rows[n];
                                    let color = |s: &str| {
                                        if s.starts_with('+') {
                                            0x9ad7ae
                                        } else if s.starts_with('-') {
                                            0xee938e
                                        } else {
                                            MUTED
                                        }
                                    };
                                    let mut line = div()
                                        .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                        .whitespace_nowrap();
                                    line = line
                                        .text_color(rgb(color(&row.raw)))
                                        .child(row.raw.clone());
                                    line
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .h_full(),
                );
            }
        } else if let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) {
            content = content.h_full().child(
                uniform_list(
                    ("code-lines", self.active.unwrap_or(0)),
                    if self.show_diff && self.side_by_side {
                        self.diff_highlights.layout.len().min(10000)
                    } else {
                        tab.lines.len().min(10000)
                    },
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        let font_id = cx
                            .text_system()
                            .resolve_font(&gpui::font(this.settings.font_name()));
                        let cell_width = cx
                            .text_system()
                            .ch_advance(font_id, px(this.settings.font_size as f32))
                            .unwrap_or(px(8.4));
                        let viewport_width = this
                            .editor_scroll_metrics(window, cell_width)
                            .map(|metrics| metrics.viewport_width)
                            .unwrap_or_else(|| {
                                if this.show_diff && this.side_by_side {
                                    (window.bounds().size.width
                                        - this.editor_left(window)
                                        - px(6.)
                                        - split_left_width(
                                            window.bounds().size.width,
                                            this.diff_split,
                                            this.editor_left(window),
                                        ))
                                    .max(px(0.))
                                } else {
                                    (window.bounds().size.width
                                        - this.editor_left(window)
                                        - px(16.))
                                    .max(px(0.))
                                }
                            });
                        let Some(tab) = this.active.and_then(|i| this.tabs.get(i)) else {
                            return Vec::new();
                        };
                        let selection = tab.buffer.selection_range();
                        let cursor_position = tab.buffer.cursor_position();
                        let source = tab.buffer.text();
                        let line_ranges = tab.buffer.line_ranges();
                        range
                            .map(|visual| {
                                let diff_row = if this.show_diff && this.side_by_side {
                                    this.diff_highlights.layout.get(visual)
                                } else {
                                    None
                                };
                                let n = if this.show_diff && this.side_by_side {
                                    diff_row.and_then(|row| row.after)
                                } else {
                                    Some(visual)
                                };
                                let row_width = (px(CODE_CELL_LEFT)
                                    + cell_width * tab.max_line_chars)
                                    .max(viewport_width);
                                let Some(n) = n else {
                                    return div()
                                        .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                        .w(row_width)
                                        .bg(rgb(0x30363c))
                                        .border_b_1()
                                        .border_color(rgb(0x3b424b))
                                        .into_any_element();
                                };
                                let line_range = line_ranges.get(n).cloned().unwrap_or(0..0);
                                let line_text = source[line_range.clone()].to_owned();
                                let csv_row = tab.csv.as_ref().and_then(|csv| csv.lines.get(n));
                                let line_chars = csv_row.map_or_else(
                                    || buffer::visual_column(&line_text, line_text.chars().count()),
                                    |row| row.display.chars().count(),
                                );
                                let line_diagnostics = tab
                                    .diagnostics
                                    .iter()
                                    .filter(|diagnostic| diagnostic.line == n)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                let local_selection = selection.as_ref().and_then(|selection| {
                                    let start = selection.start.max(line_range.start);
                                    let end = selection.end.min(line_range.end);
                                    if start < end {
                                        Some(start - line_range.start..end - line_range.start)
                                    } else if line_range.is_empty()
                                        && selection.start <= line_range.start
                                        && line_range.start < selection.end
                                    {
                                        Some(0..0)
                                    } else {
                                        None
                                    }
                                });
                                let local_matches = this
                                    .find_matches
                                    .iter()
                                    .filter_map(|found| {
                                        let start = found.start.max(line_range.start);
                                        let end = found.end.min(line_range.end);
                                        (start < end).then_some(
                                            start - line_range.start..end - line_range.start,
                                        )
                                    })
                                    .collect::<Vec<_>>();
                                let cursor_column = (cursor_position.line == n
                                    && this.cursor_blink_visible
                                    && !this.find_has_focus
                                    && !this.original_focused
                                    && this.editor_active()
                                    && this.focus.is_focused(window))
                                .then_some(csv_row.map_or_else(
                                    || buffer::visual_column(&line_text, cursor_position.column),
                                    |row| {
                                        row.columns
                                            [cursor_position.column.min(row.columns.len() - 1)]
                                    },
                                ));
                                let mouse_down_text = line_text.clone();
                                let mouse_move_text = line_text;
                                let mut line = div()
                                    .id(("editor-line", n))
                                    .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                    .w(row_width)
                                    .bg(rgb(
                                        if this.show_diff
                                            && this.side_by_side
                                            && this.diff_highlights.added.contains(&n)
                                        {
                                            0x26392f
                                        } else if this.show_diff
                                            && this.side_by_side
                                            && this.diff_highlights.deletion_anchors.contains(&n)
                                        {
                                            0x3b292c
                                        } else {
                                            this.settings.background()
                                        },
                                    ))
                                    .flex()
                                    .gap_2()
                                    .whitespace_nowrap()
                                    .relative()
                                    .cursor_text()
                                    .on_hover(|hovered, window, _| {
                                        if *hovered {
                                            window.refresh();
                                        }
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                let Some(index) = this.active else {
                                                    return;
                                                };
                                                let scroll_x = this
                                                    .editor_scroll_metrics(window, cell_width)
                                                    .map_or(px(0.), |metrics| metrics.scroll_x);
                                                let text_left = if this.show_diff
                                                    && this.side_by_side
                                                {
                                                    this.editor_left(window)
                                                        + px(CODE_CELL_LEFT)
                                                        + split_left_width(
                                                            window.bounds().size.width,
                                                            this.diff_split,
                                                            this.editor_left(window),
                                                        )
                                                        + px(6.)
                                                } else {
                                                    this.editor_left(window) + px(CODE_CELL_LEFT)
                                                };
                                                let displayed = code_column_at_x_from(
                                                    event.position.x,
                                                    scroll_x,
                                                    cell_width,
                                                    line_chars,
                                                    text_left,
                                                );
                                                let column = this.tabs[index]
                                                    .csv
                                                    .as_ref()
                                                    .and_then(|csv| csv.lines.get(n))
                                                    .map_or_else(
                                                        || {
                                                            buffer::source_column(
                                                                &mouse_down_text,
                                                                displayed,
                                                            )
                                                        },
                                                        |row| row.source_column(displayed),
                                                    );
                                                let offset =
                                                    this.tabs[index].buffer.offset_at_position(
                                                        buffer::Position { line: n, column },
                                                    );
                                                if event.modifiers.control
                                                    && event.click_count == 1
                                                    && definition::supports(&this.tabs[index].path)
                                                {
                                                    let glyph = ((event.position.x + scroll_x
                                                        - text_left)
                                                        / cell_width)
                                                        .floor()
                                                        .max(0.)
                                                        as usize;
                                                    if glyph < line_chars {
                                                        let source_column = buffer::source_column(
                                                            &mouse_down_text,
                                                            glyph,
                                                        );
                                                        this.mouse_selecting = false;
                                                        this.follow_definition(
                                                            index,
                                                            n,
                                                            source_column,
                                                            cx,
                                                        );
                                                        return;
                                                    }
                                                }
                                                this.find_has_focus = false;
                                                this.original_focused = false;
                                                this.search_focused = false;
                                                this.cursor_blink_visible = true;
                                                this.mouse_selecting = event.click_count == 1;
                                                if event.click_count >= 3 {
                                                    this.tabs[index].buffer.select_line(n);
                                                    this.mouse_selecting = false;
                                                } else if event.click_count == 2 {
                                                    this.tabs[index].buffer.select_word_at(offset);
                                                    this.mouse_selecting = false;
                                                } else {
                                                    this.tabs[index]
                                                        .buffer
                                                        .set_cursor(offset, event.modifiers.shift);
                                                }
                                                window.focus(&this.focus);
                                                this.ensure_editor_cursor_visible(index);
                                                cx.notify();
                                            },
                                        ),
                                    )
                                    .on_mouse_move(cx.listener(
                                        move |this, event: &MouseMoveEvent, window, cx| {
                                            if !event.dragging() || !this.mouse_selecting {
                                                return;
                                            }
                                            let Some(index) = this.active else {
                                                return;
                                            };
                                            let scroll_x = this
                                                .editor_scroll_metrics(window, cell_width)
                                                .map_or(px(0.), |metrics| metrics.scroll_x);
                                            let text_left = if this.show_diff && this.side_by_side {
                                                this.editor_left(window)
                                                    + px(CODE_CELL_LEFT)
                                                    + split_left_width(
                                                        window.bounds().size.width,
                                                        this.diff_split,
                                                        this.editor_left(window),
                                                    )
                                                    + px(6.)
                                            } else {
                                                this.editor_left(window) + px(CODE_CELL_LEFT)
                                            };
                                            let displayed = code_column_at_x_from(
                                                event.position.x,
                                                scroll_x,
                                                cell_width,
                                                line_chars,
                                                text_left,
                                            );
                                            let column = this.tabs[index]
                                                .csv
                                                .as_ref()
                                                .and_then(|csv| csv.lines.get(n))
                                                .map_or_else(
                                                    || {
                                                        buffer::source_column(
                                                            &mouse_move_text,
                                                            displayed,
                                                        )
                                                    },
                                                    |row| row.source_column(displayed),
                                                );
                                            let offset =
                                                this.tabs[index].buffer.offset_at_position(
                                                    buffer::Position { line: n, column },
                                                );
                                            this.tabs[index].buffer.set_cursor(offset, true);
                                            this.cursor_blink_visible = true;
                                            this.ensure_editor_cursor_visible(index);
                                            cx.notify();
                                        },
                                    ))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, _| this.mouse_selecting = false),
                                    )
                                    .child(
                                        div()
                                            .w(px(48.))
                                            .text_color(rgb(
                                                if this.show_diff
                                                    && this.side_by_side
                                                    && this.diff_highlights.added.contains(&n)
                                                {
                                                    0x9ad7ae
                                                } else if this.show_diff
                                                    && this.side_by_side
                                                    && this
                                                        .diff_highlights
                                                        .deletion_anchors
                                                        .contains(&n)
                                                {
                                                    0xee938e
                                                } else {
                                                    0x5c6370
                                                },
                                            ))
                                            .child(format!("{:>4}", n + 1)),
                                    )
                                    .child(csv_row.map_or_else(
                                        || {
                                            tab.lines[n].render_editor(
                                                local_selection.clone(),
                                                &local_matches,
                                                diff_row.and_then(|row| {
                                                    row.after_range
                                                        .clone()
                                                        .map(|range| (range, 0x345f42))
                                                }),
                                                &line_diagnostics,
                                            )
                                        },
                                        |row| {
                                            let map = |range: std::ops::Range<usize>| {
                                                row.display_byte(
                                                    range.start,
                                                    &source[line_range.clone()],
                                                )
                                                    ..row.display_byte(
                                                        range.end,
                                                        &source[line_range.clone()],
                                                    )
                                            };
                                            tab.lines[n].aligned(row).render_editor(
                                                local_selection.clone().map(&map),
                                                &local_matches
                                                    .iter()
                                                    .cloned()
                                                    .map(map)
                                                    .collect::<Vec<_>>(),
                                                None,
                                                &[],
                                            )
                                        },
                                    ));
                                if !line_diagnostics.is_empty() {
                                    let diagnostics = line_diagnostics.clone();
                                    line = line.tooltip(move |_, cx| {
                                        let diagnostics = diagnostics.clone();
                                        cx.new(|_| DiagnosticTooltip { diagnostics }).into()
                                    });
                                }
                                if !this.show_diff {
                                    let marker = if this.diff_highlights.added.contains(&n) {
                                        Some(0x9ad7ae)
                                    } else if this.diff_highlights.deletion_anchors.contains(&n) {
                                        Some(0xee938e)
                                    } else {
                                        None
                                    };
                                    if let Some(color) = marker {
                                        line = line.child(
                                            div()
                                                .absolute()
                                                .left(px(0.))
                                                .top(px(2.))
                                                .w(px(3.))
                                                .h(px(
                                                    (this.settings.font_size as f32 + 4.).max(18.)
                                                ))
                                                .bg(rgb(color)),
                                        );
                                    }
                                }
                                if let Some(column) = cursor_column {
                                    line = line.child(
                                        div()
                                            .absolute()
                                            .left(px(CODE_CELL_LEFT) + cell_width * column)
                                            .top(px(3.))
                                            .w(px(1.5))
                                            .h(px(this.settings.font_size as f32 + 2.))
                                            .bg(rgb(0x61afef)),
                                    );
                                }
                                line.into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                .track_scroll(self.editor_scroll.clone())
                .h_full(),
            );
        } else {
            content = content.h_full().items_center().justify_center().child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .child(
                        div().text_size(px(28.)).text_color(rgb(0x61afef)).child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icons::icon("code-xml", 0x61afef))
                                .child("Gere"),
                        ),
                    )
                    .child(div().text_color(rgb(MUTED)).child("Tu espacio para crear"))
                    .child(
                        div()
                            .text_color(rgb(0x5c6370))
                            .text_xs()
                            .child("Elegí un archivo del explorador o presioná Ctrl+P"),
                    ),
            );
        }
        let is_svg = !self.show_diff
            && self
                .active
                .and_then(|index| self.tabs.get(index))
                .is_some_and(|tab| {
                    tab.path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
                });
        if is_svg {
            if let Some(svg_path) = self
                .active
                .and_then(|index| self.tabs.get(index))
                .map(|tab| self.root.join(&tab.path))
            {
                let left_width = split_left_width(
                    window.bounds().size.width,
                    self.svg_split,
                    self.editor_left(window),
                );
                content = div()
                    .flex()
                    .w_full()
                    .h_full()
                    .child(
                        div()
                            .w(left_width)
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(content),
                    )
                    .child(
                        div()
                            .w(px(6.))
                            .h_full()
                            .bg(rgb(if self.dragging_svg_split {
                                0x61afef
                            } else {
                                0x3e4451
                            }))
                            .cursor(gpui::CursorStyle::ResizeColumn)
                            .hover(|style| style.bg(rgb(0x61afef)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.dragging_svg_split = true;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_l_1()
                            .border_color(rgb(0x3e4451))
                            .bg(rgb(0x181a1f))
                            .p_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(img(svg_path).size_full().object_fit(ObjectFit::Contain)),
                    );
            }
        }
        if self.show_diff && self.side_by_side {
            let left_width = split_left_width(
                window.bounds().size.width,
                self.diff_split,
                self.editor_left(window),
            );
            let original = div()
                .relative()
                .on_scroll_wheel(cx.listener(|this, _, _, cx| this.sync_diff_scroll_from(true, cx)))
                .flex()
                .flex_col()
                .w(left_width)
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .child(
                    div()
                        .h(px(28.))
                        .px_2()
                        .text_color(rgb(MUTED))
                        .child("HEAD · solo lectura"),
                )
                .child(
                    uniform_list(
                        "original-lines",
                        self.diff_highlights.layout.len().min(10_000),
                        cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                            let font_id = cx
                                .text_system()
                                .resolve_font(&gpui::font(this.settings.font_name()));
                            let cell_width = cx
                                .text_system()
                                .ch_advance(font_id, px(this.settings.font_size as f32))
                                .unwrap_or(px(8.4));
                            let viewport = this.scroll_metrics(true, window, cell_width).map_or(
                                split_left_width(
                                    window.bounds().size.width,
                                    this.diff_split,
                                    this.editor_left(window),
                                ),
                                |metrics| metrics.viewport_width,
                            );
                            let row_width = (px(CODE_CELL_LEFT)
                                + cell_width * this.original_max_chars)
                                .max(viewport);
                            let selection = this.original_buffer.selection_range();
                            let source = this.original_buffer.text();
                            range
                                .map(|visual| {
                                    let row = this.diff_highlights.layout.get(visual);
                                    let Some(n) = row.and_then(|row| row.before) else {
                                        return div()
                                            .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                            .w(row_width)
                                            .bg(rgb(0x30363c))
                                            .border_b_1()
                                            .border_color(rgb(0x3b424b))
                                            .into_any_element();
                                    };
                                    let line_range =
                                        this.original_buffer.line_range(n).unwrap_or(0..0);
                                    let line_text = source[line_range.clone()].to_owned();
                                    let line_chars = buffer::visual_column(
                                        &line_text,
                                        line_text.chars().count(),
                                    );
                                    let mouse_down_text = line_text.clone();
                                    let mouse_move_text = line_text;
                                    let local_selection =
                                        selection.as_ref().and_then(|selection| {
                                            let start = selection.start.max(line_range.start);
                                            let end = selection.end.min(line_range.end);
                                            if start < end {
                                                Some(
                                                    start - line_range.start
                                                        ..end - line_range.start,
                                                )
                                            } else if line_range.is_empty()
                                                && selection.start <= line_range.start
                                                && line_range.start < selection.end
                                            {
                                                Some(0..0)
                                            } else {
                                                None
                                            }
                                        });
                                    div()
                                        .id(("original-line", n))
                                        .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                        .w(row_width)
                                        .bg(rgb(if this.diff_highlights.removed.contains(&n) {
                                            0x3b292c
                                        } else {
                                            this.settings.background()
                                        }))
                                        .flex()
                                        .gap_2()
                                        .whitespace_nowrap()
                                        .cursor_text()
                                        .on_hover(|hovered, window, _| {
                                            if *hovered {
                                                window.refresh();
                                            }
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, window, cx| {
                                                    let scroll_x = this
                                                        .scroll_metrics(true, window, cell_width)
                                                        .map_or(px(0.), |m| m.scroll_x);
                                                    let displayed = code_column_at_x_from(
                                                        event.position.x,
                                                        scroll_x,
                                                        cell_width,
                                                        line_chars,
                                                        this.editor_left(window)
                                                            + px(CODE_CELL_LEFT),
                                                    );
                                                    let column = buffer::source_column(
                                                        &mouse_down_text,
                                                        displayed,
                                                    );
                                                    let offset =
                                                        this.original_buffer.offset_at_position(
                                                            buffer::Position { line: n, column },
                                                        );
                                                    this.original_focused = true;
                                                    this.original_mouse_selecting =
                                                        event.click_count == 1;
                                                    this.mouse_selecting = false;
                                                    this.find_has_focus = false;
                                                    if event.click_count >= 3 {
                                                        this.original_buffer.select_line(n);
                                                        this.original_mouse_selecting = false;
                                                    } else if event.click_count == 2 {
                                                        this.original_buffer.select_word_at(offset);
                                                        this.original_mouse_selecting = false;
                                                    } else {
                                                        this.original_buffer.set_cursor(
                                                            offset,
                                                            event.modifiers.shift,
                                                        );
                                                    }
                                                    window.focus(&this.focus);
                                                    cx.notify();
                                                },
                                            ),
                                        )
                                        .on_mouse_move(cx.listener(
                                            move |this, event: &MouseMoveEvent, window, cx| {
                                                if !event.dragging()
                                                    || !this.original_mouse_selecting
                                                {
                                                    return;
                                                }
                                                let scroll_x = this
                                                    .scroll_metrics(true, window, cell_width)
                                                    .map_or(px(0.), |m| m.scroll_x);
                                                let displayed = code_column_at_x_from(
                                                    event.position.x,
                                                    scroll_x,
                                                    cell_width,
                                                    line_chars,
                                                    this.editor_left(window) + px(CODE_CELL_LEFT),
                                                );
                                                let column = buffer::source_column(
                                                    &mouse_move_text,
                                                    displayed,
                                                );
                                                let offset =
                                                    this.original_buffer.offset_at_position(
                                                        buffer::Position { line: n, column },
                                                    );
                                                this.original_buffer.set_cursor(offset, true);
                                                cx.notify();
                                            },
                                        ))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, _| {
                                                this.original_mouse_selecting = false
                                            }),
                                        )
                                        .child(
                                            div()
                                                .w(px(48.))
                                                .text_color(rgb(
                                                    if this.diff_highlights.removed.contains(&n) {
                                                        0xee938e
                                                    } else {
                                                        0x5c6370
                                                    },
                                                ))
                                                .child(format!("{:>4}", n + 1)),
                                        )
                                        .child(this.original_lines[n].render_editor(
                                            local_selection,
                                            &[],
                                            row.and_then(|row| {
                                                row.before_range
                                                    .clone()
                                                    .map(|range| (range, 0x703839))
                                            }),
                                            &[],
                                        ))
                                        .into_any_element()
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .track_scroll(self.original_scroll.clone())
                    .h_full(),
                )
                .children(original_vertical)
                .children(original_horizontal);
            content = div()
                .flex()
                .w_full()
                .h_full()
                .child(original)
                .child(
                    div()
                        .w(px(6.))
                        .h_full()
                        .bg(rgb(if self.dragging_diff_split {
                            0x61afef
                        } else {
                            0x3e4451
                        }))
                        .cursor(gpui::CursorStyle::ResizeColumn)
                        .hover(|style| style.bg(rgb(0x61afef)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.dragging_diff_split = true;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .relative()
                        .on_scroll_wheel(
                            cx.listener(|this, _, _, cx| this.sync_diff_scroll_from(false, cx)),
                        )
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .h(px(28.))
                                .px_2()
                                .text_color(rgb(0x9ad7ae))
                                .child("Archivo actual · editable"),
                        )
                        .child(div().flex_1().min_h_0().child(content.h_full()))
                        .children(split_vertical)
                        .children(split_horizontal),
                );
        }
        let custom_titlebar = matches!(
            window.window_decorations(),
            gpui::Decorations::Client { .. }
        );
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                for path in paths.paths() {
                    this.open_external(path.clone(), cx);
                }
            }))
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                    if this.dragging_sidebar && event.dragging() {
                        this.sidebar_width = sidebar_width(
                            window.bounds().size.width,
                            event.position.x - px(RAIL_WIDTH),
                        );
                        cx.notify();
                    }
                    if this.dragging_diff_split && event.dragging() {
                        let available =
                            (window.bounds().size.width - this.editor_left(window)).max(px(1.));
                        this.diff_split =
                            ((event.position.x - this.editor_left(window)) / available).clamp(0.0, 1.0);
                        cx.notify();
                    }
                    if this.dragging_svg_split && event.dragging() {
                        let available =
                            (window.bounds().size.width - this.editor_left(window)).max(px(1.));
                        this.svg_split =
                            ((event.position.x - this.editor_left(window)) / available).clamp(0.0, 1.0);
                        cx.notify();
                    }
                    if event.dragging() && this.editor_scroll_drag.is_some() {
                        this.drag_editor_scrollbar(false, event, window, editor_cell_width, cx);
                    }
                    if event.dragging() && this.original_scroll_drag.is_some() {
                        this.drag_editor_scrollbar(true, event, window, editor_cell_width, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let dragging_scrollbar = this.editor_scroll_drag.take().is_some();
                    let dragging_original = this.original_scroll_drag.take().is_some();
                    let dragging_split = std::mem::take(&mut this.dragging_diff_split);
                    let dragging_svg_split = std::mem::take(&mut this.dragging_svg_split);
                    let dragging_sidebar = std::mem::take(&mut this.dragging_sidebar);
                    this.mouse_selecting = false;
                    this.original_mouse_selecting = false;
                    if dragging_scrollbar
                        || dragging_original
                        || dragging_split
                        || dragging_svg_split
                        || dragging_sidebar
                    {
                        cx.notify();
                    }
                }),
            )
            .bg(rgb(background))
            .text_color(rgb(FG))
            .text_sm()
            .child(
                div()
                    .h(px(32.))
                    .w_full()
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_2()
                    .bg(rgb(0x181a1f))
                    .child(div().flex().items_center().gap_2().h_full()
                        .when(custom_titlebar, |area| area.on_mouse_down(MouseButton::Left, |event, window, _| {
                            if event.click_count == 2 { window.zoom_window(); }
                            else if event.click_count == 1 { window.start_window_move(); }
                        }))
                        .child(icons::icon("code-xml", 0x61afef))
                        .child(div().text_color(rgb(0xd7dae0)).child("Gere")))
                    .child(div().relative().h_full().flex().items_center()
                        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if this.top_file_menu && event.position.y < px(32.) {
                                this.top_file_menu = false;
                                cx.notify();
                            }
                        }))
                        .child(div().px_2().py_1().rounded_md().cursor_pointer()
                            .hover(|style| style.bg(rgb(PANEL)))
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.file_menu = None;
                            this.top_file_menu = !this.top_file_menu;
                            cx.notify();
                        })).child("File"))
                        .when(self.top_file_menu, |button| button.child(deferred(
                            anchored()
                                .position_mode(gpui::AnchoredPositionMode::Local)
                                .position(point(px(0.), px(32.)))
                                .snap_to_window_with_margin(px(6.))
                                .child(div().w(px(180.)).p_1().rounded_md().bg(rgb(PANEL))
                                    .border_1().border_color(rgb(0x4b5261)).shadow_lg().occlude()
                                    .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                        if event.position.y >= px(32.) {
                                            this.top_file_menu = false;
                                            cx.notify();
                                        }
                                    }))
                                    .child(Self::menu_item("Open File    Ctrl+O", cx.listener(|this, _, _, cx| {
                                        this.top_file_menu = false;
                                        this.pick_file(cx);
                                    }))))))))
                    .child(div().flex_1().h_full().when(custom_titlebar, |area| area
                        .on_mouse_down(MouseButton::Left, |event, window, _| {
                            if event.click_count == 2 { window.zoom_window(); }
                            else if event.click_count == 1 { window.start_window_move(); }
                        })))
                    .child(div().text_color(rgb(MUTED)).text_xs().child(format!(
                        "— {}",
                        self.root.file_name().unwrap_or_default().to_string_lossy()
                    )))
                    .when(custom_titlebar, |bar| bar
                        .child(Self::icon_button("minus", "Minimize", |_, window, _| window.minimize_window()))
                        .child(Self::icon_button("maximize", "Maximize / restore", |_, window, _| window.zoom_window()))
                        .child(Self::icon_button("close", "Close", |_, window, _| window.remove_window()))),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(rail)
                    .when(self.sidebar_visible, |v| v.child(sidebar)
                        .child(div()
                            .w(px(8.))
                            .h_full()
                            .flex_shrink_0()
                            .cursor_col_resize()
                            .bg(rgb(if self.dragging_sidebar { 0x61afef } else { 0x3a3f4b }))
                            .hover(|s| s.bg(rgb(0x61afef)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.dragging_sidebar = true;
                                cx.notify();
                            }))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| {
                                this.files_focused = false;
                            }))
                            .child(tabs)
                            .when(
                                change.is_some_and(|c| c.index != '?')
                                    || self.show_diff
                                    || !conflicts.is_empty(),
                                |v| v.child(toolbar),
                            )
                            .child(
                                div()
                                    .id("code")
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_hidden()
                                    .pt_2()
                                    .pl_2()
                                    .child(content.h_full())
                                    .children(normal_vertical)
                                    .children(normal_horizontal),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(26.))
                    .w_full()
                    .px_2()
                    .bg(rgb(0x61afef))
                    .flex()
                    .items_center()
                     .gap_2()
                     .text_color(rgb(0x21252b))
                     .child(
                         div()
                             .relative()
                             .h_full()
                             .flex()
                             .items_center()
                             .gap_1()
                             .cursor_pointer()
                             .on_mouse_up(
                                 MouseButton::Left,
                                 cx.listener(|this, _, _, cx| this.toggle_branch_menu(cx)),
                             )
                             .child(icons::icon("git", 0x21252b))
                             .child(self.branch.clone().unwrap_or_else(|| "HEAD".into()))
                             .child(icons::icon("chevron-down", 0x21252b))
                             .when(branch_menu_open, |view| {
                                 view.child(
                                     div()
                                         .absolute()
                                         .left(px(0.))
                                         .bottom(px(26.))
                                         .w(px(240.))
                                         .max_h(px(280.))
                                         .p_1()
                                         .rounded_md()
                                         .bg(rgb(PANEL))
                                         .border_1()
                                         .border_color(rgb(0x4b5261))
                                         .shadow_lg()
                                         .flex()
                                         .flex_col()
                                         .text_color(rgb(FG))
                                         .occlude()
                                         .on_mouse_down_out(cx.listener(
                                             |this, _, _, cx| {
                                                 this.branch_menu_open = false;
                                                 cx.notify();
                                             },
                                         ))
                                         .when(branch_menu_loading, |menu| {
                                             menu.child(div().px_2().py_1().child("Cargando branches…"))
                                         })
                                         .when(
                                             !branch_menu_loading && branches.is_empty(),
                                             |menu| {
                                                 menu.child(
                                                     div()
                                                         .px_2()
                                                         .py_1()
                                                         .text_color(rgb(MUTED))
                                                         .child("No hay branches locales"),
                                                 )
                                             },
                                         )
                                         .when(
                                             !branch_menu_loading && !branches.is_empty(),
                                             |menu| {
                                                 menu.child(
                                                     uniform_list(
                                                         "local-branches",
                                                         branch_count,
                                                         cx.processor(
                                                             |this, range: std::ops::Range<usize>, _, cx| {
                                                                 range
                                                                     .map(|index| {
                                                                         let branch =
                                                                             this.branches[index].clone();
                                                                         let target = branch.clone();
                                                                         div()
                                                                             .h(px(28.))
                                                                             .w_full()
                                                                             .px_2()
                                                                             .flex()
                                                                             .items_center()
                                                                             .rounded_sm()
                                                                             .cursor_pointer()
                                                                             .hover(|style| {
                                                                                 style.bg(rgb(0x3e4451))
                                                                             })
                                                                             .on_mouse_up(
                                                                                 MouseButton::Left,
                                                                                 cx.listener(
                                                                                     move |this, _, _, cx| {
                                                                                         cx.stop_propagation();
                                                                                         this.branch_menu_open = false;
                                                                                         this.git_operation(
                                                                                             GitOperation::SwitchBranch(
                                                                                                 target.clone(),
                                                                                             ),
                                                                                             cx,
                                                                                         );
                                                                                     },
                                                                                 ),
                                                                             )
                                                                             .child(branch)
                                                                     })
                                                                     .collect::<Vec<_>>()
                                                             },
                                                         ),
                                                     )
                                                     .track_scroll(branch_scroll.clone())
                                                     .h(px(240.)),
                                                 )
                                             },
                                         ),
                                 )
                             }),
                     )
                     .child(div().flex_1())
                    .child(self.message.clone())
                    .child(
                        self.active
                            .and_then(|i| self.tabs.get(i))
                            .and_then(|tab| {
                                let line = tab.buffer.cursor_position().line;
                                tab.diagnostics.iter().find(|d| d.line == line).map(|d| {
                                    format!(
                                        "⚠ {}: {} ({} diagnósticos)",
                                        d.severity.label(),
                                        d.message,
                                        tab.diagnostics.len()
                                    )
                                })
                            })
                            .unwrap_or_default(),
                    )
                    .child(div().flex_1())
                    .child(self.active.and_then(|i| self.tabs.get(i)).map_or_else(
                        String::new,
                        |tab| {
                            format!(
                                "{}  ·  UTF-8  ·  Ln {}, Col {}",
                                tab.path
                                    .extension()
                                    .and_then(|ext| ext.to_str())
                                    .unwrap_or("Texto"),
                                tab.buffer.cursor_position().line + 1,
                                tab.buffer.cursor_position().column + 1
                            )
                        },
                    )),
            )
            .when(self.find_open, |view| {
                let match_status = self.find_active.map_or_else(
                    || format!("0 / {}", self.find_matches.len()),
                    |index| format!("{} / {}", index + 1, self.find_matches.len()),
                );
                let find_input = input_view(&self.find_query, "Buscar en archivo…", self.find_has_focus,
                    find_caret_visible, 22, find_cell_width, background, true)
                    .w(px(230.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, _| {
                            this.find_has_focus = true;
                            this.cursor_blink_visible = true;
                            window.focus(&this.focus);
                        }),
                    );
                view.child(
                    div()
                        .absolute()
                        .top(px(110.))
                        .right(px(22.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(0x3e4451))
                        .bg(rgb(panel))
                        .text_color(rgb(FG))
                        .font_family(font_name)
                        .child(find_input)
                        .child(div().px_1().text_color(rgb(MUTED)).child(match_status))
                        .child(
                            div()
                                .p_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0x3e4451)))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.move_find(true);
                                        cx.notify();
                                    }),
                                )
                                .child(icons::icon("arrow-up", FG)),
                        )
                        .child(
                            div()
                                .p_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0x3e4451)))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.move_find(false);
                                        cx.notify();
                                    }),
                                )
                                .child(icons::icon("arrow-down", FG)),
                        )
                        .child(
                            div()
                                .px_2()
                                .cursor_pointer()
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.close_find();
                                        cx.notify();
                                    }),
                                )
                                .child(icons::icon("close", MUTED)),
                        ),
                )
            })
            .when(self.palette_open, |v| {
                v.child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .size_full()
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(75.))
                        .bg(rgba(0x101116aa))
                        .occlude()
                        .child(
                            div()
                                .w(px(620.))
                                .max_w_full()
                                .p_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(0x3e4451))
                                .bg(rgb(panel))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child("ABRIR ARCHIVO  ·  CTRL+P"),
                                )
                                .child(
                                    input_view(&self.palette_query, "Buscar archivos por nombre…", true,
                                        self.focus.is_focused(window) && self.cursor_blink_visible,
                                        65, find_cell_width, background, false)
                                        .font_family(font_name)
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(|this, _, window, _| {
                                                window.focus(&this.focus)
                                            }),
                                        ),
                                )
                                .child(
                                    uniform_list(
                                        "quick-open",
                                        self.quick.len(),
                                        cx.processor(
                                            move |this, range: std::ops::Range<usize>, _, cx| {
                                                range
                                                    .map(|i| {
                                                        let path = this.quick[i].clone();
                                                        div()
                                                            .h(px(28.))
                                                            .px_2()
                                                            .overflow_hidden()
                                                            .cursor_pointer()
                                                            .bg(rgb(
                                                                if i == this.palette_selected {
                                                                    0x3e4451
                                                                } else {
                                                                    panel
                                                                },
                                                            ))
                                                            .text_color(rgb(FG))
                                                            .hover(|s| s.bg(rgb(0x3e4451)))
                                                            .on_mouse_up(
                                                                MouseButton::Left,
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.choose_palette(
                                                                            Some(path.clone()),
                                                                            cx,
                                                                        );
                                                                    },
                                                                ),
                                                            )
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .child(icons::file_icon(&this.quick[i]))
                                                            .child(
                                                                this.quick[i].display().to_string(),
                                                            )
                                                    })
                                                    .collect::<Vec<_>>()
                                            },
                                        ),
                                    )
                                    .track_scroll(self.palette_scroll.clone())
                                    .h(px(240.)),
                                ),
                        ),
                )
            })
            .when(self.settings_open, |v| {
                v.child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(px(0.))
                        .size_full()
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(76.))
                        .bg(rgba(0x101116aa))
                        .occlude()
                        .child(
                            div()
                                .w(px(520.))
                                .max_w_full()
                                .p_4()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(0x3e4451))
                                .bg(rgb(panel))
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(icons::icon("settings", 0x61afef))
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_color(rgb(0xd7dae0))
                                                .child("Configuración"),
                                        )
                                        .child(
                                            div()
                                                .cursor_pointer()
                                                .p_1()
                                                .hover(|s| s.bg(rgb(0x3e4451)))
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.settings_open = false;
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(icons::icon("close", MUTED)),
                                        ),
                                )
                                .child(div().h(px(1.)).bg(rgb(0x3e4451)))
                                .child(div().text_xs().text_color(rgb(MUTED)).child("APARIENCIA"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child("Tema")
                                        .child(Self::button(
                                            if self.settings.theme == settings::Theme::Darker {
                                                "One Dark Pro Darker  ▾"
                                            } else {
                                                "One Dark Pro  ▾"
                                            },
                                            cx.listener(|this, _, _, cx| {
                                                this.update_settings(
                                                    |s| {
                                                        s.theme =
                                                            if s.theme == settings::Theme::Darker {
                                                                settings::Theme::Classic
                                                            } else {
                                                                settings::Theme::Darker
                                                            }
                                                    },
                                                    cx,
                                                )
                                            }),
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child("Fuente del editor")
                                        .child(Self::button(
                                            format!("{}  ▾", self.settings.font_name()),
                                            cx.listener(|this, _, _, cx| {
                                                this.update_settings(|s| s.next_font(), cx)
                                            }),
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child("Tamaño de fuente")
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Self::button(
                                                    "−",
                                                    cx.listener(|this, _, _, cx| {
                                                        this.update_settings(
                                                            |s| {
                                                                s.font_size = s
                                                                    .font_size
                                                                    .saturating_sub(1)
                                                                    .max(10)
                                                            },
                                                            cx,
                                                        )
                                                    }),
                                                ))
                                                .child(format!("{} px", self.settings.font_size))
                                                .child(Self::button(
                                                    "+",
                                                    cx.listener(|this, _, _, cx| {
                                                        this.update_settings(
                                                            |s| {
                                                                s.font_size =
                                                                    (s.font_size + 1).min(24)
                                                            },
                                                            cx,
                                                        )
                                                    }),
                                                )),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.file_menu.as_ref(), |view, (path, is_dir, position)| {
                let path = path.clone();
                let parent = if *is_dir {
                    path.clone()
                } else {
                    path.parent().unwrap_or(Path::new("")).to_path_buf()
                };
                let reveal = self.root.join(&path);
                view.child(
                    anchored().position(*position).snap_to_window_with_margin(px(6.))
                        .child(div()
                        .w(px(205.))
                        .p_1()
                        .rounded_md()
                        .bg(rgb(PANEL)).border_1().border_color(rgb(0x4b5261)).shadow_lg()
                        .flex()
                        .flex_col()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.file_menu = None;
                            cx.notify();
                        }))
                        .child(Self::menu_item(
                            "New File",
                            cx.listener({ let parent = parent.clone(); move |this, _, _, cx| {
                                this.begin_file_edit(FileEdit::Create(parent.clone()), cx)
                            }}),
                        ))
                        .child(Self::menu_item(
                            "New Folder",
                            cx.listener(move |this, _, _, cx| {
                                this.begin_file_edit(FileEdit::CreateFolder(parent.clone()), cx)
                            }),
                        ))
                        .when(!path.as_os_str().is_empty(), |menu| menu.child(Self::menu_item(
                            "Rename (F2)",
                            cx.listener({
                                let path = path.clone();
                                move |this, _, _, cx| {
                                    this.begin_file_edit(FileEdit::Rename(path.clone()), cx)
                                }
                            }),
                        )))
                        .when(!path.as_os_str().is_empty(), |menu| menu.child(Self::menu_item(
                            if *is_dir { "Delete Folder" } else { "Delete File" },
                            cx.listener({
                                let path = path.clone();
                                move |this, _, _, cx| {
                                    this.confirm_delete = Some(path.clone());
                                    this.file_menu = None;
                                    cx.notify();
                                }
                            }),
                        )))
                        .child(Self::menu_item(
                            "Reveal in File Explorer",
                            cx.listener(move |this, _, _, cx| {
                                cx.reveal_path(&reveal);
                                this.file_menu = None;
                                cx.notify();
                            }),
                        ))
                        .when(path.as_os_str().is_empty(), |menu| menu.child(Self::menu_item(
                            "Contraer carpetas",
                            cx.listener(|this, _, _, cx| {
                                this.expanded.clear();
                                this.update_visible();
                                this.file_menu = None;
                                cx.notify();
                            }),
                        )))),
                )
            })
            .when(self.confirm_discard_all, |view| {
                view.child(Self::alert_dialog("Descartar todos los cambios",
                    "Se perderán los cambios sin stage y los archivos nuevos sin seguimiento. Los cambios staged permanecerán intactos.".into(),
                    Self::button("Confirmar descarte", cx.listener(|this, _, _, cx| this.confirm_discard_all(cx))),
                    Self::button("Cancelar", cx.listener(|this, _, _, cx| {
                        this.confirm_discard_all = false;
                        cx.notify();
                    }))))
            })
            .when_some(self.confirm_delete.as_ref(), |view, path| {
                let is_dir = self.files.iter().any(|entry| entry.path == *path && entry.is_dir);
                view.child(Self::alert_dialog(if is_dir { "Eliminar carpeta" } else { "Eliminar archivo" },
                    format!("¿Eliminar {}? Esta acción no se puede deshacer.", path.display()),
                    Self::button(if is_dir { "Eliminar carpeta" } else { "Eliminar archivo" }, cx.listener(|this, _, _, cx| this.delete_selected_file(cx))),
                    Self::button("Cancelar", cx.listener(|this, _, _, cx| {
                        this.confirm_delete = None;
                        cx.notify();
                    }))))
            })
            .when_some(self.confirm_discard.as_ref(), |view, state| {
                view.child(Self::alert_dialog("Descartar cambios",
                    format!("¿Descartar los cambios sin stage de {}? Los cambios staged permanecerán intactos.", state.change.path.display()),
                    Self::button("Confirmar descarte", cx.listener(|this, _, _, cx| this.action("discard", cx))),
                    Self::button("Cancelar", cx.listener(|this, _, _, cx| {
                        this.confirm_discard = None;
                        cx.notify();
                    }))))
            })
    }
}

impl Drop for Reviewer {
    fn drop(&mut self) {
        let _ = session::save(&self.root, &self.session_snapshot(), session::revision());
    }
}

fn main() {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("directorio actual"));
    let root = root.canonicalize().expect("directorio del proyecto");
    if !root.is_dir() {
        eprintln!("Se espera un directorio");
        return;
    }
    gpui::Application::new()
        .with_assets(icons::Icons)
        .run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(1200.), px(780.)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_decorations: Some(WindowDecorations::Client),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| Reviewer::new(root, cx)),
                )
                .expect("abrir ventana");
            window
                .update(cx, |view, window, _| window.focus(&view.focus))
                .expect("enfocar ventana");
            cx.activate(true);
        });
}
