mod buffer;
mod csv;
mod definition;
mod explorer;
mod folding;
mod git_view;
mod highlight;
mod icons;
mod ime;
mod input;
mod lint;
mod markdown;
mod project;
mod reviewer_files;
mod reviewer_find;
mod reviewer_git;
mod reviewer_input;
mod reviewer_keys;
mod reviewer_palette;
mod reviewer_search;
mod reviewer_terminal;
mod reviewer_ui;
mod search_view;
mod session;
mod settings;
mod terminal;

use explorer::{can_move_into, explorer_rows, file_tree, selected_folder, visible_entries};
#[cfg(test)]
use git_view::changed_text_ranges;
use git_view::{
    aligned_diff, aligned_lines, conflict_blocks, line_diff_highlights, resolve_conflict,
    ConflictBlock, ConflictChoice, DiffHighlights, DiffRow,
};
use gpui::{
    anchored, deferred, div, img, point, prelude::*, px, rgb, rgba, size, uniform_list, App,
    Bounds, ClipboardItem, Context, ExternalPaths, FocusHandle, KeyDownEvent,
    ListHorizontalSizingBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, PathPromptOptions, Pixels, ScrollStrategy, StatefulInteractiveElement,
    UniformListScrollHandle, Window, WindowBounds, WindowDecorations, WindowOptions,
};
use input::{input_view, sidebar_input_columns, SingleLineInput};
use reviewer_files::{ExplorerDrag, ExplorerRow, FileEdit};
#[cfg(test)]
use reviewer_find::matching_ranges;
use reviewer_git::{DiscardState, GitOperation, GitRow};
#[cfg(test)]
use reviewer_palette::palette_items;
#[cfg(test)]
use reviewer_palette::PaletteItem;
use reviewer_palette::PaletteMode;
use reviewer_ui::DiagnosticTooltip;
use search_view::{match_preview, palette_target, search_rows, SearchRow};
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
const CODE_TEXT_LEFT: f32 = 406.;
const CODE_CELL_LEFT: f32 = 72.;
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

fn active_after_close(active: Option<usize>, closed: usize, remaining: usize) -> Option<usize> {
    match active {
        Some(index) if index == closed => (remaining > 0).then(|| closed.min(remaining - 1)),
        Some(index) if index > closed => Some(index - 1),
        other => other,
    }
}

fn tab_title(tab: &Tab) -> String {
    if tab.untitled {
        format!(
            "Sin título {}",
            tab.path.to_string_lossy().rsplit('-').next().unwrap_or("")
        )
    } else {
        tab.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

fn reorder_tab<T>(items: &mut Vec<T>, active: &mut Option<usize>, from: usize, to: usize) {
    if from >= items.len() || to >= items.len() || from == to {
        return;
    }
    let moved = items.remove(from);
    items.insert(to, moved);
    *active = active.map(|index| {
        if index == from {
            to
        } else if from < index && index <= to {
            index - 1
        } else if to <= index && index < from {
            index + 1
        } else {
            index
        }
    });
}

#[cfg(test)]
mod tab_reorder_tests {
    use super::reorder_tab;

    #[test]
    fn moving_tabs_keeps_the_same_item_active() {
        let mut tabs = vec!["a", "b", "c", "d"];
        let mut active = Some(1);
        reorder_tab(&mut tabs, &mut active, 0, 2);
        assert_eq!(tabs, ["b", "c", "a", "d"]);
        assert_eq!(active, Some(0));
        reorder_tab(&mut tabs, &mut active, 0, 3);
        assert_eq!(tabs, ["c", "a", "d", "b"]);
        assert_eq!(active, Some(3));
        reorder_tab(&mut tabs, &mut active, 3, 1);
        assert_eq!(tabs, ["c", "b", "a", "d"]);
        assert_eq!(active, Some(1));
        reorder_tab(&mut tabs, &mut active, 99, 0);
        assert_eq!(tabs, ["c", "b", "a", "d"]);
    }
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

fn vertical_scrollbar_origin(
    window_height: Pixels,
    terminal_height: Pixels,
    track: Pixels,
    cross: bool,
    split: bool,
) -> Pixels {
    window_height
        - px(26.)
        - terminal_height
        - if split { px(4.) } else { px(0.) }
        - if cross { px(14.) } else { px(0.) }
        - track
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
    fn vertical_scrollbar_click_uses_the_rendered_track_origin() {
        let track = px(200.);
        let thumb = px(28.);
        let origin = vertical_scrollbar_origin(px(600.), px(250.), track, false, false);
        assert_eq!(origin, px(124.));
        assert_eq!(
            scrollbar_target(origin + px(100.), origin, track, thumb, px(300.)),
            px(150.)
        );
        assert_eq!(
            vertical_scrollbar_origin(px(600.), px(0.), track - px(22.), true, true),
            px(378.)
        );
    }

    #[test]
    fn branch_palette_filters_actions_and_prioritizes_branch_prefixes() {
        let branches = vec![
            "topic/main".into(),
            "main".into(),
            "feature".into(),
            "branch-fix".into(),
        ];
        let items = palette_items(&PaletteMode::Branches, "MAI", &branches);
        assert!(
            matches!(&items[..], [PaletteItem::Branch(first), PaletteItem::Branch(second)] if first == "main" && second == "topic/main")
        );
        let items = palette_items(&PaletteMode::Branches, "", &branches);
        assert!(matches!(items.first(), Some(PaletteItem::CreateBranch)));
        assert!(matches!(items.get(1), Some(PaletteItem::CreateBranchFrom)));
        let items = palette_items(&PaletteMode::Branches, "branch", &branches);
        assert!(matches!(
            &items[..],
            [PaletteItem::Branch(name), PaletteItem::CreateBranch, PaletteItem::CreateBranchFrom]
                if name == "branch-fix"
        ));
        let commands = palette_items(&PaletteMode::Commands, "sett", &branches);
        assert!(matches!(&commands[..], [PaletteItem::CommandSettings]));
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
}

struct Tab {
    path: PathBuf,
    untitled: bool,
    blame_source: Option<String>,
    blame_lines: Vec<Option<project::BlameLine>>,
    pending_highlight: Option<gpui::Task<()>>,
    scroll: UniformListScrollHandle,
    buffer: buffer::EditorBuffer,
    lines: Vec<highlight::HighlightedLine>,
    folding: folding::Folding,
    preview: Option<Vec<markdown::Row>>,
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

enum NoticeAction {
    PullRequest(String),
    GitLog,
}

fn wrap_segments(text: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut column = 0;
    for (at, ch) in text.char_indices() {
        let size = if ch == '\t' {
            buffer::TAB_WIDTH - column % buffer::TAB_WIDTH
        } else {
            1
        };
        if column + size > width && at > start {
            segments.push(start..at);
            start = at;
            column = 0;
        }
        column += if ch == '\t' {
            buffer::TAB_WIDTH - column % buffer::TAB_WIDTH
        } else {
            1
        };
    }
    segments.push(start..text.len());
    segments
}

#[cfg(test)]
mod wrap_tests {
    use super::wrap_segments;

    #[test]
    fn wraps_on_utf8_boundaries_and_keeps_empty_lines() {
        let text = "ábcdef";
        let parts = wrap_segments(text, 3);
        assert_eq!(
            parts
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["ábc", "def"]
        );
        assert_eq!(wrap_segments("", 10), [0..0]);
        assert_eq!(wrap_segments("abc", 3), [0..3]);
    }
}

struct Notice {
    text: String,
    action: NoticeAction,
}

struct Reviewer {
    settings: settings::Settings,
    settings_open: bool,
    blame_reveal_key: Option<(PathBuf, usize)>,
    blame_visible: bool,
    pending_blame_reveal: Option<gpui::Task<()>>,
    notice: Option<Notice>,
    git_log: Vec<String>,
    git_log_open: bool,
    wrap_lines: bool,
    wrap_columns: usize,
    wrap_row_count: usize,
    language_menu_open: bool,
    language_overrides: HashMap<PathBuf, String>,
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
    branch_menu_loading: bool,
    sync_status: Option<project::SyncStatus>,
    git_rows: Vec<GitRow>,
    staged_expanded: bool,
    unstaged_expanded: bool,
    stashes: Vec<project::Stash>,
    git_busy: bool,
    git_progress_offset: f32,
    commit_message: SingleLineInput,
    commit_focused: bool,
    tabs: Vec<Tab>,
    closed_tabs: Vec<Tab>,
    next_untitled: usize,
    comment_chord: bool,
    pending_saves: HashSet<PathBuf>,
    pending_session: HashMap<PathBuf, session::TabState>,
    active: Option<usize>,
    selected: Option<PathBuf>,
    query: SingleLineInput,
    search_focused: bool,
    search_options: project::SearchOptions,
    search_id: u64,
    palette_open: bool,
    palette_mode: PaletteMode,
    palette_query: SingleLineInput,
    palette_error: Option<String>,
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
    startup_loading: bool,
    session_loading: bool,
    refresh_id: u64,
    refresh_pending: u8,
    message: String,
    focus: FocusHandle,
    terminals: Vec<terminal::Terminal>,
    terminal_active: Option<usize>,
    terminal_visible: bool,
    terminal_shell_menu: bool,
    terminal_shell: Option<String>,
    terminal_focused: bool,
    terminal_height: Pixels,
    terminal_dragging: bool,
    terminal_scroll_dragging: bool,
    terminal_scroll_grab: f32,
    terminal_size: (u16, u16),
    terminal_revision: u64,
    next_terminal_id: usize,
    terminal_selection: Option<((u16, u16), (u16, u16))>,
    terminal_selecting: bool,
    terminal_find_open: bool,
    terminal_find_query: SingleLineInput,
    terminal_find_matches: Vec<reviewer_terminal::TerminalMatch>,
    terminal_find_active: Option<usize>,
    terminal_find_scrollback: usize,
}

impl Reviewer {
    fn show_git_error(&mut self, operation: &str, error: String) {
        self.git_log.push(format!("{operation}: {error}"));
        if self.git_log.len() > 100 {
            self.git_log.remove(0);
        }
        self.notice = Some(Notice {
            text: format!(
                "Git · {operation}: {}",
                error.lines().next().unwrap_or("Error desconocido")
            ),
            action: NoticeAction::GitLog,
        });
        self.message = error;
    }

    fn set_language(&mut self, mode: Option<&str>, cx: &mut Context<Self>) {
        self.language_menu_open = false;
        let Some(index) = self.active else { return };
        let path = self.tabs[index].path.clone();
        match mode {
            Some(mode) => {
                self.language_overrides.insert(path, mode.to_owned());
            }
            None => {
                self.language_overrides.remove(&path);
            }
        }
        if !self.tabs[index].loading {
            self.tabs[index].lint_source = None;
            self.tabs[index].lint_diagnostics.clear();
            self.rehighlight_tab(index, cx);
            if self.original_path.as_ref() == Some(&self.tabs[index].path) {
                let syntax_path = highlight::syntax_path(
                    &self.tabs[index].path,
                    self.language_overrides
                        .get(&self.tabs[index].path)
                        .map(String::as_str),
                );
                self.original_lines = highlight::line(&self.original_text, &syntax_path);
            }
        }
        let root = self.root.clone();
        let snapshot = self.session_snapshot();
        let revision = session::revision();
        cx.background_executor()
            .spawn(async move {
                let _ = session::save(&root, &snapshot, revision);
            })
            .detach();
        cx.notify();
    }

    fn session_snapshot(&self) -> session::Session {
        let row_height = (self.settings.font_size as f32 + 8.).max(22.);
        session::Session {
            language_overrides: self.language_overrides.clone(),
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
                        untitled: tab.untitled,
                        cursor: tab.buffer.cursor(),
                        scroll_line: line,
                        dirty_text: (tab.untitled || tab.buffer.is_dirty())
                            .then(|| tab.buffer.text().to_owned()),
                    }
                })
                .collect(),
            active: self.active,
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
        self.language_menu_open = false;
        self.terminal_focused = false;
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
        cx.spawn(|weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    gpui::Timer::after(Duration::from_secs(2)).await;
                    if weak
                        .update(&mut cx, |this, cx| {
                            if this.refresh_pending == 0 {
                                this.refresh(cx);
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
        cx.spawn(|weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    gpui::Timer::after(Duration::from_secs(1)).await;
                    let Ok((root, tabs)) = weak.update(&mut cx, |this, _| {
                        (
                            this.root.clone(),
                            this.tabs
                                .iter()
                                .filter(|tab| !tab.loading && !tab.untitled)
                                .map(|tab| (tab.path.clone(), tab.loaded_stamp.clone()))
                                .collect::<Vec<_>>(),
                        )
                    }) else {
                        break;
                    };
                    if tabs.is_empty() {
                        continue;
                    }
                    let updates = cx
                        .background_executor()
                        .spawn(async move {
                            tabs.into_iter()
                                .filter_map(|(path, loaded)| {
                                    let stamp = project::file_stamp(&root, &path).ok()?;
                                    (Some(&stamp) != loaded.as_ref()).then(|| {
                                        let result = project::read_with_stamp(&root, &path);
                                        (path, loaded, result)
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .await;
                    if weak
                        .update(&mut cx, |this, cx| this.apply_external_updates(updates, cx))
                        .is_err()
                    {
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
                        (
                            this.root.clone(),
                            (!this.session_loading).then(|| this.session_snapshot()),
                        )
                    }) else {
                        break;
                    };
                    let Some(snapshot) = snapshot else { continue };
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
                                || (this.terminal_visible && this.terminal_focused)
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
        cx.spawn(|weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    let Ok(delay) = weak.update(&mut cx, |this, _| {
                        if this.terminals.is_empty() {
                            250
                        } else if this.terminal_visible {
                            40
                        } else {
                            120
                        }
                    }) else {
                        break;
                    };
                    gpui::Timer::after(Duration::from_millis(delay)).await;
                    if weak
                        .update(&mut cx, |this, cx| {
                            if this.terminals.is_empty() {
                                return;
                            }
                            let mut changed = false;
                            let mut refresh_terminal_find = false;
                            for (index, terminal) in this.terminals.iter_mut().enumerate() {
                                if this.terminal_visible && terminal.size != this.terminal_size {
                                    terminal.resize(this.terminal_size);
                                }
                                if terminal.check_exit() {
                                    changed = true;
                                }
                                if this.terminal_visible && this.terminal_active == Some(index) {
                                    let revision = terminal
                                        .revision
                                        .load(std::sync::atomic::Ordering::Acquire);
                                    if revision != this.terminal_revision {
                                        this.terminal_revision = revision;
                                        this.terminal_selection = None;
                                        if this.terminal_find_open {
                                            refresh_terminal_find = true;
                                        }
                                        changed = true;
                                    }
                                }
                            }
                            if refresh_terminal_find {
                                this.refresh_terminal_find(false);
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
            blame_reveal_key: None,
            blame_visible: false,
            pending_blame_reveal: None,
            notice: None,
            git_log: Vec::new(),
            git_log_open: false,
            wrap_lines: false,
            wrap_columns: 80,
            wrap_row_count: 0,
            language_menu_open: false,
            language_overrides: HashMap::new(),
            root,
            sidebar: Sidebar::Files,
            sidebar_visible: true,
            sidebar_width: px(280.),
            dragging_sidebar: false,
            files: Vec::new(),
            ignored: HashSet::new(),
            root_expanded: true,
            expanded: HashSet::new(),
            tree: BTreeMap::new(),
            visible: Vec::new(),
            files_scroll: UniformListScrollHandle::new(),
            changes: Vec::new(),
            change_counts: Vec::new(),
            branch: None,
            branches: Vec::new(),
            branch_menu_loading: false,
            sync_status: None,
            git_rows: Vec::new(),
            staged_expanded: true,
            unstaged_expanded: true,
            stashes: Vec::new(),
            git_busy: false,
            git_progress_offset: 0.,
            commit_message: SingleLineInput::default(),
            commit_focused: false,
            tabs: Vec::new(),
            closed_tabs: Vec::new(),
            next_untitled: 1,
            comment_chord: false,
            pending_saves: HashSet::new(),
            pending_session: HashMap::new(),
            active: None,
            selected: None,
            query: SingleLineInput::default(),
            search_focused: false,
            search_options: project::SearchOptions::default(),
            search_id: 0,
            palette_open: false,
            palette_mode: PaletteMode::Files,
            palette_query: SingleLineInput::default(),
            palette_error: None,
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
            startup_loading: true,
            session_loading: true,
            refresh_id: 0,
            refresh_pending: 0,
            message: String::new(),
            focus: cx.focus_handle(),
            terminals: Vec::new(),
            terminal_active: None,
            terminal_visible: false,
            terminal_shell_menu: false,
            terminal_shell: None,
            terminal_focused: false,
            terminal_height: px(380.),
            terminal_dragging: false,
            terminal_scroll_dragging: false,
            terminal_scroll_grab: 0.,
            terminal_size: (24, 80),
            terminal_revision: 0,
            next_terminal_id: 1,
            terminal_selection: None,
            terminal_selecting: false,
            terminal_find_open: false,
            terminal_find_query: SingleLineInput::default(),
            terminal_find_matches: Vec::new(),
            terminal_find_active: None,
            terminal_find_scrollback: 0,
        };
        reviewer.update_tree();
        reviewer.update_git_rows();
        let root = reviewer.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let previous = executor.spawn(async move { session::load(&root) }).await;
                    let _ = weak.update(&mut cx, |this, cx| this.restore_session(previous, cx));
                }
            },
        )
        .detach();
        reviewer.refresh(cx);
        reviewer
    }

    fn restore_session(&mut self, previous: session::Session, cx: &mut Context<Self>) {
        let restored_languages: HashMap<_, _> = previous
            .language_overrides
            .into_iter()
            .filter(|(path, mode)| {
                project_relative_path(&self.root, path).is_some()
                    && highlight::MODES.iter().any(|(key, _)| mode == key)
            })
            .collect();
        for (path, mode) in restored_languages {
            if !self.language_overrides.contains_key(&path) {
                self.language_overrides.insert(path.clone(), mode);
                if let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.path == path && !tab.loading)
                {
                    self.rehighlight_tab(index, cx);
                }
            }
        }
        let current_active = self
            .active
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.path.clone());
        let active_path = previous
            .active
            .and_then(|index| previous.tabs.get(index))
            .map(|tab| tab.path.clone());
        for tab in previous.tabs {
            if tab.untitled {
                if !self
                    .tabs
                    .iter()
                    .any(|open| open.path == tab.path && open.untitled)
                {
                    self.open_untitled(
                        tab.path.clone(),
                        tab.dirty_text.as_deref().unwrap_or(""),
                        cx,
                    );
                    if let Some(open) = self.tabs.last_mut() {
                        open.buffer.set_cursor(tab.cursor, false);
                        open.scroll
                            .scroll_to_item_strict(tab.scroll_line, ScrollStrategy::Top);
                    }
                }
                continue;
            }
            if project_relative_path(&self.root, &tab.path).is_some()
                && (self.root.join(&tab.path).is_file() || tab.dirty_text.is_some())
                && !self
                    .tabs
                    .iter()
                    .any(|open| !open.untitled && open.path == tab.path)
                && !self.pending_session.contains_key(&tab.path)
            {
                self.pending_session.insert(tab.path.clone(), tab.clone());
                self.open(tab.path, cx);
            }
        }
        if let Some(index) = current_active
            .or(active_path)
            .and_then(|path| self.tabs.iter().position(|tab| tab.path == path))
        {
            self.activate_tab(Some(index));
            self.selected = (!self.tabs[index].untitled).then(|| self.tabs[index].path.clone());
            self.follow_blame_cursor(cx);
        }
        self.session_loading = false;
        cx.notify();
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
            if self.staged_expanded {
                self.git_rows.extend(staged);
            }
        }
        self.git_rows.push(GitRow::UnstagedHeader);
        if self.unstaged_expanded {
            self.git_rows.extend(unstaged);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        self.refresh_id += 1;
        self.refresh_pending = 3;
        let refresh_id = self.refresh_id;
        let root = self.root.clone();
        let show_git = self.sidebar == Sidebar::Git;
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let preview = executor
                        .spawn({
                            let root = root.clone();
                            async move { project::root_files(&root) }
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.refresh_id == refresh_id && this.startup_loading {
                            this.files = preview;
                            this.update_tree();
                            cx.notify();
                        }
                    });
                    let (files, ignore_files) = executor
                        .spawn({
                            let root = root.clone();
                            async move {
                                let files = project::files(&root);
                                let ignore_files = files.clone();
                                (files, ignore_files)
                            }
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.refresh_id == refresh_id {
                            this.apply_files(files, cx);
                            this.refresh_pending -= 1;
                        }
                    });
                    let ignored = executor
                        .spawn(async move { project::ignored_paths(&root, &ignore_files) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.refresh_id == refresh_id {
                            if this.ignored != ignored {
                                this.ignored = ignored;
                                cx.notify();
                            }
                            this.refresh_pending -= 1;
                        }
                    });
                }
            },
        )
        .detach();
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let (changes, counts, branch, sync_status, stashes) = executor
                        .spawn(async move {
                            let changes = project::status(&root);
                            let counts = if show_git {
                                changes
                                    .as_ref()
                                    .map(|items| {
                                        items
                                            .iter()
                                            .map(|c| project::change_counts(&root, c))
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            let branch = project::branch(&root);
                            let sync_status = project::sync_status(&root);
                            let stashes = show_git.then(|| project::stashes(&root));
                            (changes, counts, branch, sync_status, stashes)
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.refresh_id == refresh_id {
                            this.apply_git_refresh(
                                changes,
                                counts,
                                branch,
                                sync_status,
                                stashes,
                                cx,
                            );
                            this.refresh_pending -= 1;
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn apply_files(&mut self, files: Vec<project::FileEntry>, cx: &mut Context<Self>) {
        let initial = std::mem::take(&mut self.startup_loading);
        if initial {
            if let Some(candidate) = files.iter().find(|entry| {
                !entry.is_dir
                    && definition::supports_js(&entry.path)
                    && !entry
                        .path
                        .components()
                        .any(|component| component.as_os_str() == "node_modules")
            }) {
                let root = self.root.clone();
                let path = candidate.path.clone();
                cx.background_executor()
                    .spawn(async move {
                        let _ = definition::warm_project(&root, Some(&path));
                    })
                    .detach();
            }
        }
        let files_changed = files != self.files;
        let changed = files_changed || initial;
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
        if changed {
            cx.notify();
        }
    }

    fn apply_git_refresh(
        &mut self,
        changes: Result<Vec<project::Change>, String>,
        counts: Vec<(usize, usize)>,
        branch: Option<String>,
        sync_status: Option<project::SyncStatus>,
        stashes: Option<Result<Vec<project::Stash>, String>>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = branch != self.branch || sync_status != self.sync_status;
        self.branch = branch;
        self.sync_status = sync_status;
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

    fn apply_external_updates(
        &mut self,
        updates: Vec<(
            PathBuf,
            Option<project::FileStamp>,
            Result<(String, project::FileStamp), String>,
        )>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        let mut lint = Vec::new();
        for (path, previous, result) in updates {
            let Some(index) = self.tabs.iter().position(|tab| tab.path == path) else {
                continue;
            };
            let tab = &mut self.tabs[index];
            if tab.loading || tab.loaded_stamp != previous {
                continue;
            }
            let Ok((text, stamp)) = result else {
                // A temporary removal or incomplete write must not erase the open buffer.
                continue;
            };
            if tab.buffer.is_dirty() {
                let message = format!(
                    "{} cambió fuera del editor; hay cambios locales sin guardar",
                    path.display()
                );
                if self.message != message {
                    self.message = message;
                    changed = true;
                }
                continue;
            }
            if text != tab.buffer.text() {
                tab.buffer.reload(text);
                let syntax_path = highlight::syntax_path(
                    &path,
                    self.language_overrides.get(&path).map(String::as_str),
                );
                let (lines, ends) = highlight::lines_and_folds(tab.buffer.text(), &syntax_path);
                tab.lines = lines;
                tab.folding.update(ends, tab.buffer.cursor_position().line);
                tab.diagnostics = highlight::diagnostics(tab.buffer.text(), &syntax_path);
                tab.lint_source = None;
                tab.lint_diagnostics.clear();
                tab.csv = (path.extension().and_then(|ext| ext.to_str()) == Some("csv"))
                    .then(|| csv::layout(tab.buffer.text()))
                    .flatten();
                tab.max_line_chars = tab
                    .csv
                    .as_ref()
                    .map_or_else(|| max_line_chars(tab.buffer.text()), |csv| csv.max_chars);
                lint.push(index);
                changed = true;
            }
            tab.loaded_stamp = Some(stamp);
        }
        for index in lint {
            self.schedule_lint(index, cx);
        }
        if changed {
            self.refresh_find_matches();
            self.load_change_decorations();
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
        self.blame_reveal_key = None;
        self.blame_visible = false;
        self.pending_blame_reveal = None;
        self.editor_scroll = index
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.scroll.clone())
            .unwrap_or_else(UniformListScrollHandle::new);
    }

    fn request_blame(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.path == path && !tab.loading)
        else {
            return;
        };
        if tab.blame_source.as_deref() == Some(tab.buffer.text()) {
            return;
        }
        let source = tab.buffer.text().to_owned();
        tab.blame_source = Some(source.clone());
        tab.blame_lines.clear();
        let path = path.to_path_buf();
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    gpui::Timer::after(Duration::from_millis(250)).await;
                    let Ok(true) = weak.update(&mut cx, |this, _| {
                        this.tabs.iter().any(|tab| {
                            tab.path == path
                                && tab.blame_source.as_deref() == Some(source.as_str())
                                && tab.buffer.text() == source
                        })
                    }) else {
                        return;
                    };
                    let source_for_git = source.clone();
                    let path_for_git = path.clone();
                    let lines = executor
                        .spawn(async move { project::blame(&root, &path_for_git, &source_for_git) })
                        .await
                        .unwrap_or_default();
                    let _ = weak.update(&mut cx, |this, cx| {
                        if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.path == path) {
                            if tab.blame_source.as_deref() == Some(source.as_str())
                                && tab.buffer.text() == source
                            {
                                tab.blame_lines = lines;
                                cx.notify();
                            }
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn follow_blame_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active.and_then(|index| self.tabs.get(index)) else {
            return;
        };
        if tab.loading || tab.untitled {
            return;
        }
        let key = (tab.path.clone(), tab.buffer.cursor_position().line);
        if self.blame_reveal_key.as_ref() == Some(&key) {
            self.request_blame(&key.0, cx);
            return;
        }
        self.blame_reveal_key = Some(key.clone());
        self.blame_visible = false;
        self.pending_blame_reveal = None;
        self.request_blame(&key.0, cx);
        let delay = self.settings.blame_delay_ms;
        if delay == 0 {
            self.blame_visible = true;
            cx.notify();
            return;
        }
        self.pending_blame_reveal = Some(cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    gpui::Timer::after(Duration::from_millis(delay.into())).await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.blame_reveal_key.as_ref() == Some(&key)
                            && this
                                .active
                                .and_then(|index| this.tabs.get(index))
                                .is_some_and(|tab| {
                                    tab.path == key.0 && tab.buffer.cursor_position().line == key.1
                                })
                        {
                            this.blame_visible = true;
                            cx.notify();
                        }
                    });
                }
            },
        ));
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.language_menu_open = false;
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
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| !tab.untitled && tab.path == path)
        {
            self.activate_tab(Some(index));
            self.follow_blame_cursor(cx);
        } else {
            self.tabs.push(Tab {
                pending_highlight: None,
                path: path.clone(),
                untitled: false,
                blame_source: None,
                blame_lines: Vec::new(),
                scroll: UniformListScrollHandle::new(),
                buffer: buffer::EditorBuffer::new(""),
                lines: Vec::new(),
                folding: folding::Folding::default(),
                preview: None,
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
            let syntax_path = highlight::syntax_path(
                &path,
                self.language_overrides.get(&path).map(String::as_str),
            );
            let executor = cx.background_executor().clone();
            cx.spawn(
                move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                    let mut cx = cx.clone();
                    async move {
                        let (path, parsed_path, result) = executor
                            .spawn(async move {
                                let parsed_path = syntax_path.clone();
                                let result =
                                    project::read_with_stamp(&root, &path).map(|(text, stamp)| {
                                        let (lines, ends) =
                                            highlight::lines_and_folds(&text, &syntax_path);
                                        let max_line_chars = max_line_chars(&text);
                                        (text, lines, ends, stamp, max_line_chars)
                                    });
                                (path, parsed_path, result)
                            })
                            .await;
                        let _ = weak.update(&mut cx, |this, cx| {
                            let syntax_path = highlight::syntax_path(
                                &path,
                                this.language_overrides.get(&path).map(String::as_str),
                            );
                            let restored = this.pending_session.remove(&path);
                            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.path == path) {
                                tab.loading = false;
                                match result {
                                    Ok((text, lines, ends, stamp, max_line_chars)) => {
                                        tab.buffer = buffer::EditorBuffer::new(text);
                                        tab.lines = lines;
                                        tab.folding.update(ends, 0);
                                        tab.diagnostics =
                                            highlight::diagnostics(tab.buffer.text(), &syntax_path);
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
                                                let (lines, ends) = highlight::lines_and_folds(
                                                    &dirty,
                                                    &syntax_path,
                                                );
                                                tab.lines = lines;
                                                tab.folding.update(
                                                    ends,
                                                    tab.buffer.cursor_position().line,
                                                );
                                                tab.diagnostics =
                                                    highlight::diagnostics(&dirty, &syntax_path);
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
                                        if parsed_path != syntax_path {
                                            if let Some(index) =
                                                this.tabs.iter().position(|tab| tab.path == path)
                                            {
                                                this.rehighlight_tab(index, cx);
                                            }
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
                                                let (lines, ends) = highlight::lines_and_folds(
                                                    &dirty,
                                                    &syntax_path,
                                                );
                                                tab.lines = lines;
                                                tab.folding.update(
                                                    ends,
                                                    tab.buffer.cursor_position().line,
                                                );
                                                tab.diagnostics =
                                                    highlight::diagnostics(&dirty, &syntax_path);
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
                                if this
                                    .active
                                    .is_some_and(|index| this.tabs[index].path == path)
                                {
                                    this.follow_blame_cursor(cx);
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
            self.follow_blame_cursor(cx);
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
            self.follow_blame_cursor(cx);
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
        if self.pending_saves.contains(&self.tabs[index].path) {
            self.message = "Esperá a que termine de guardarse la pestaña".into();
            cx.notify();
            return;
        }
        let tab = self.tabs.remove(index);
        let closed = tab.path.clone();
        self.closed_tabs.push(tab);
        self.activate_tab(active_after_close(self.active, index, self.tabs.len()));
        self.follow_blame_cursor(cx);
        self.close_find();
        if self.selected.as_ref() == Some(&closed) {
            self.selected = self
                .active
                .and_then(|i| (!self.tabs[i].untitled).then(|| self.tabs[i].path.clone()));
            self.show_diff = false;
            self.confirm_discard = None;
            self.load_change_decorations();
        }
        cx.notify();
    }

    fn open_untitled(&mut self, path: PathBuf, text: &str, cx: &mut Context<Self>) {
        let (lines, ends) = highlight::lines_and_folds(text, &path);
        let mut folding = folding::Folding::default();
        folding.update(ends, 0);
        self.tabs.push(Tab {
            path,
            untitled: true,
            blame_source: None,
            blame_lines: Vec::new(),
            pending_highlight: None,
            scroll: UniformListScrollHandle::new(),
            buffer: buffer::EditorBuffer::new(text),
            lines,
            folding,
            preview: None,
            diagnostics: Vec::new(),
            lint_source: None,
            lint_diagnostics: Vec::new(),
            csv: None,
            max_line_chars: max_line_chars(text),
            loading: false,
            loaded_stamp: None,
        });
        self.activate_tab(Some(self.tabs.len() - 1));
        self.selected = None;
        self.show_diff = false;
        self.original_focused = false;
        self.terminal_focused = false;
        self.files_focused = false;
        self.search_focused = false;
        self.commit_focused = false;
        self.palette_open = false;
        self.close_find();
        cx.notify();
    }

    fn new_untitled(&mut self, cx: &mut Context<Self>) {
        loop {
            let number = self.next_untitled;
            self.next_untitled += 1;
            let path = PathBuf::from(format!(".gere-untitled-{number}"));
            if !self
                .tabs
                .iter()
                .chain(&self.closed_tabs)
                .any(|tab| tab.path == path)
            {
                self.open_untitled(path, "", cx);
                break;
            }
        }
    }

    fn reopen_tab(&mut self, cx: &mut Context<Self>) {
        while let Some(tab) = self.closed_tabs.pop() {
            if self
                .tabs
                .iter()
                .any(|open| open.path == tab.path && open.untitled == tab.untitled)
            {
                continue;
            }
            if tab.untitled || tab.buffer.is_dirty() {
                self.tabs.push(tab);
                self.activate_tab(Some(self.tabs.len() - 1));
                self.selected = (!self.tabs.last().unwrap().untitled)
                    .then(|| self.tabs.last().unwrap().path.clone());
                self.show_diff = false;
                self.close_find();
                self.follow_blame_cursor(cx);
                cx.notify();
            } else {
                self.open(tab.path, cx);
            }
            break;
        }
    }

    fn ensure_editor_cursor_visible(&mut self, index: usize, cx: &mut Context<Self>) {
        self.scroll_editor_cursor(index, false);
        self.follow_blame_cursor(cx);
    }

    fn scroll_editor_cursor(&mut self, index: usize, center: bool) {
        if !self.show_diff {
            if let Some(tab) = self.tabs.get_mut(index) {
                tab.folding.reveal(tab.buffer.cursor_position().line);
            }
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let line_count = tab.lines.len().min(10_000);
        if line_count > 0 {
            let physical = tab.buffer.cursor_position().line.min(line_count - 1);
            let visual = if self.wrap_lines && !self.show_diff && tab.csv.is_none() {
                let cursor = tab.buffer.offset_at_position(tab.buffer.cursor_position());
                tab.folding
                    .visible
                    .iter()
                    .take(10_000)
                    .take_while(|&&line| line < physical)
                    .map(|&line| wrap_segments(tab.lines[line].text(), self.wrap_columns).len())
                    .sum::<usize>()
                    + wrap_segments(tab.lines[physical].text(), self.wrap_columns)
                        .iter()
                        .position(|segment| {
                            cursor.saturating_sub(
                                tab.buffer
                                    .line_range(physical)
                                    .map_or(0, |range| range.start),
                            ) < segment.end
                        })
                        .unwrap_or_else(|| {
                            wrap_segments(tab.lines[physical].text(), self.wrap_columns)
                                .len()
                                .saturating_sub(1)
                        })
            } else if self.show_diff && self.side_by_side {
                self.diff_highlights
                    .after_to_visual
                    .get(physical)
                    .copied()
                    .unwrap_or(physical)
            } else {
                tab.folding.visual(physical)
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

    fn wrap_width(&self, window: &Window, cell_width: Pixels) -> usize {
        ((window.bounds().size.width - self.editor_left(window) - px(CODE_CELL_LEFT + 30.))
            / cell_width)
            .floor()
            .max(1.) as usize
    }

    fn wrapped_rows(
        &self,
        window: &Window,
        cell_width: Pixels,
    ) -> Vec<(usize, std::ops::Range<usize>)> {
        let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) else {
            return Vec::new();
        };
        let width = self.wrap_width(window, cell_width);
        tab.folding
            .visible
            .iter()
            .take(10_000)
            .flat_map(|&line| {
                wrap_segments(tab.lines.get(line).map_or("", |row| row.text()), width)
                    .into_iter()
                    .map(move |segment| (line, segment))
            })
            .collect()
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
        let wrapping = self.wrap_lines && !self.show_diff && !original && tab?.csv.is_none();
        let wrapped_count = wrapping.then_some(self.wrap_row_count);
        let line_count = if let Some(count) = wrapped_count {
            count
        } else if self.show_diff && self.side_by_side {
            self.diff_highlights.layout.len()
        } else if original {
            self.original_lines.len()
        } else {
            tab?.folding.visible.len()
        }
        .min(if wrapping { usize::MAX } else { 10_000 });
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
        let content_width = if wrapping {
            viewport_width
        } else {
            (px(CODE_CELL_LEFT) + cell_width * max_chars).max(viewport_width)
        };
        let content_height = if wrapping {
            px((self.settings.font_size as f32 + 8.).max(22.)) * line_count
        } else {
            measured.map_or_else(
                || px((self.settings.font_size as f32 + 8.).max(22.)) * line_count,
                |item| item.contents.height,
            )
        };
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
            vertical_scrollbar_origin(
                window.bounds().size.height,
                if self.terminal_visible {
                    self.terminal_height
                        .min((window.bounds().size.height - px(145.)).max(px(110.)))
                } else {
                    px(0.)
                },
                track,
                cross,
                self.show_diff && self.side_by_side,
            )
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
                    px(8.)
                })
                .bottom(if metrics.max_x > px(0.) {
                    px(if self.show_diff && self.side_by_side {
                        18.
                    } else {
                        14.
                    })
                } else {
                    px(if self.show_diff && self.side_by_side {
                        4.
                    } else {
                        0.
                    })
                })
                .right(px(0.))
                .w(px(12.))
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

    fn editor_active(&self) -> bool {
        !self.palette_open
            && !self.settings_open
            && !self.terminal_focused
            && !self.commit_focused
            && !(self.sidebar == Sidebar::Search && self.search_focused)
            && (!self.show_diff || self.side_by_side)
            && self
                .active
                .and_then(|index| self.tabs.get(index))
                .is_some_and(|tab| !tab.loading && tab.preview.is_none())
    }

    fn schedule_lint(&self, index: usize, cx: &mut Context<Self>) {
        let tab = &self.tabs[index];
        if self.language_overrides.contains_key(&tab.path) {
            return;
        }
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
                        if this.language_overrides.contains_key(&path) {
                            return;
                        }
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
        self.tabs[index].pending_highlight = None;
        let path = self.tabs[index].path.clone();
        let syntax_path = highlight::syntax_path(
            &path,
            self.language_overrides.get(&path).map(String::as_str),
        );
        let text = self.tabs[index].buffer.text().to_owned();
        self.tabs[index].max_line_chars = max_line_chars(&text);
        let (lines, ends) = highlight::lines_and_folds(&text, &syntax_path);
        self.tabs[index].lines = lines;
        let cursor_line = self.tabs[index].buffer.cursor_position().line;
        self.tabs[index].folding.update(ends, cursor_line);
        if self.tabs[index].preview.is_some() {
            self.tabs[index].preview = Some(markdown::parse(&text));
        }
        let tab = &mut self.tabs[index];
        tab.diagnostics = highlight::diagnostics(&text, &syntax_path);
        if self.language_overrides.get(&path).is_none() {
            if let Some(previous) = &tab.lint_source {
                tab.diagnostics.extend(lint_for_unchanged_lines(
                    previous,
                    &text,
                    &tab.lint_diagnostics,
                ));
            }
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

    // Keep the edited line responsive during key repeat; coalesce full parsing and lint
    // until typing pauses. Other edits still take the normal full-update path.
    fn update_typed_line(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
        let line = tab.buffer.cursor_position().line;
        if line >= tab.lines.len() {
            self.rehighlight_tab(index, cx);
            return;
        }
        let source = tab.buffer.text();
        let cursor = tab.buffer.cursor();
        let start = source[..cursor].rfind('\n').map_or(0, |at| at + 1);
        let end = source[cursor..]
            .find('\n')
            .map_or(source.len(), |at| cursor + at);
        let text = &source[start..end];
        tab.max_line_chars = tab.max_line_chars.max(text.chars().count());
        tab.lines[line] = highlight::HighlightedLine::plain(text);
        tab.diagnostics.retain(|diagnostic| diagnostic.line != line);

        let path = tab.path.clone();
        let syntax_path = highlight::syntax_path(
            &path,
            self.language_overrides.get(&path).map(String::as_str),
        );
        let executor = cx.background_executor().clone();
        tab.pending_highlight = Some(cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    gpui::Timer::after(Duration::from_millis(50)).await;
                    let Ok(Some(text)) = weak.update(&mut cx, |this, _| {
                        this.tabs
                            .iter()
                            .find(|tab| tab.path == path)
                            .map(|tab| tab.buffer.text().to_owned())
                    }) else {
                        return;
                    };
                    let source = text.clone();
                    let syntax_path = syntax_path.clone();
                    let parsed_path = syntax_path.clone();
                    let (lines, ends, diagnostics, max_chars) = executor
                        .spawn(async move {
                            let (lines, ends) = highlight::lines_and_folds(&source, &parsed_path);
                            let diagnostics = highlight::diagnostics(&source, &parsed_path);
                            (lines, ends, diagnostics, max_line_chars(&source))
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if let Some(index) = this
                            .tabs
                            .iter()
                            .position(|tab| tab.path == path && tab.buffer.text() == text)
                        {
                            let tab = &mut this.tabs[index];
                            if highlight::syntax_path(
                                &path,
                                this.language_overrides.get(&path).map(String::as_str),
                            ) != syntax_path
                            {
                                this.rehighlight_tab(index, cx);
                                cx.notify();
                                return;
                            }
                            tab.pending_highlight = None;
                            tab.max_line_chars = max_chars;
                            tab.lines = lines;
                            tab.folding.update(ends, tab.buffer.cursor_position().line);
                            tab.diagnostics = diagnostics;
                            if this.language_overrides.get(&path).is_none() {
                                if let Some(previous) = &tab.lint_source {
                                    tab.diagnostics.extend(lint_for_unchanged_lines(
                                        previous,
                                        &text,
                                        &tab.lint_diagnostics,
                                    ));
                                }
                            }
                            this.schedule_lint(index, cx);
                            cx.notify();
                        }
                    });
                }
            },
        ));
    }

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.untitled {
            let identity = tab.path.clone();
            if !self.pending_saves.insert(identity.clone()) {
                return;
            }
            let receiver = cx.prompt_for_new_path(&self.root, Some("sin-titulo"));
            let root = self.root.clone();
            cx.spawn(
                move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                    let mut cx = cx.clone();
                    async move {
                        let result = receiver.await;
                        let _ = weak.update(&mut cx, |this, cx| match result {
                            Ok(Ok(Some(path))) => {
                                let Some(index) = this
                                    .tabs
                                    .iter()
                                    .position(|tab| tab.untitled && tab.path == identity)
                                else {
                                    this.pending_saves.remove(&identity);
                                    return;
                                };
                                let path = if path.is_absolute() {
                                    path
                                } else {
                                    root.join(path)
                                };
                                if this
                                    .tabs
                                    .iter()
                                    .any(|tab| !tab.untitled && (root.join(&tab.path) == path))
                                {
                                    this.pending_saves.remove(&identity);
                                    this.message =
                                        "El archivo ya está abierto; elegí otro nombre".into();
                                    cx.notify();
                                    return;
                                }
                                let text = this.tabs[index].buffer.text().to_owned();
                                let identity = identity.clone();
                                let root = root.clone();
                                let executor = cx.background_executor().clone();
                                cx.spawn(
                                    move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                                        let mut cx = cx.clone();
                                        async move {
                                            let result = executor
                                                .spawn(async move {
                                                    let saved = project::write_new(&path, &text);
                                                    let stamp =
                                                        saved.as_ref().ok().and_then(|_| {
                                                            project::file_stamp(&root, &path).ok()
                                                        });
                                                    (path, text, saved, stamp)
                                                })
                                                .await;
                                            let _ = weak.update(&mut cx, |this, cx| {
                                                let (path, text, saved, stamp) = result;
                                                this.pending_saves.remove(&identity);
                                                match saved {
                                                    Ok(()) => {
                                                        if let Some(tab) =
                                                            this.tabs.iter_mut().find(|tab| {
                                                                tab.untitled && tab.path == identity
                                                            })
                                                        {
                                                            tab.path = path
                                                                .strip_prefix(&this.root)
                                                                .unwrap_or(&path)
                                                                .to_path_buf();
                                                            tab.untitled = false;
                                                            tab.loaded_stamp = stamp;
                                                            if tab.buffer.text() == text {
                                                                tab.buffer.mark_saved();
                                                            }
                                                            let index = this
                                                                .tabs
                                                                .iter()
                                                                .position(|tab| {
                                                                    tab.path == path
                                                                        || this.root.join(&tab.path)
                                                                            == path
                                                                })
                                                                .unwrap();
                                                            if let Some(mode) = this
                                                                .language_overrides
                                                                .remove(&identity)
                                                            {
                                                                this.language_overrides.insert(
                                                                    this.tabs[index].path.clone(),
                                                                    mode,
                                                                );
                                                            }
                                                            this.rehighlight_tab(index, cx);
                                                            this.message = format!(
                                                                "Guardado {}",
                                                                path.display()
                                                            );
                                                            this.refresh(cx);
                                                        }
                                                    }
                                                    Err(error) => {
                                                        this.message = error;
                                                        cx.notify();
                                                    }
                                                }
                                            });
                                        }
                                    },
                                )
                                .detach();
                            }
                            Ok(Ok(None)) => {
                                this.pending_saves.remove(&identity);
                            }
                            other => {
                                this.pending_saves.remove(&identity);
                                this.message = format!("No se pudo abrir el selector: {other:?}");
                                cx.notify();
                            }
                        });
                    }
                },
            )
            .detach();
            return;
        }
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

    fn update_settings(
        &mut self,
        change: impl FnOnce(&mut settings::Settings),
        cx: &mut Context<Self>,
    ) {
        let previous_delay = self.settings.blame_delay_ms;
        change(&mut self.settings);
        if self.settings.blame_delay_ms != previous_delay {
            self.blame_reveal_key = None;
            self.pending_blame_reveal = None;
            self.blame_visible = false;
            self.follow_blame_cursor(cx);
        }
        if let Err(error) = self.settings.save() {
            self.message = format!("No se pudieron guardar las preferencias: {error}");
        }
        cx.notify();
    }
}

impl Render for Reviewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = self.settings.background();
        let panel = self.settings.panel();
        let gere = self.settings.is_gere();
        let ink = if gere { 0xd7dce2 } else { FG };
        let muted = if gere { 0x9aa5b1 } else { MUTED };
        let border = if gere { 0x273549 } else { 0x3a3f4b };
        let accent = if gere { 0x1b6de1 } else { 0x61afef };
        let font_name = self.settings.font_name();
        let font_size = self.settings.font_size as f32;
        let git_view = self.sidebar == Sidebar::Git;
        let search_mode = self.sidebar == Sidebar::Search;
        let font_id = cx.text_system().resolve_font(&gpui::font(font_name));
        let editor_cell_width = cx
            .text_system()
            .ch_advance(font_id, px(font_size))
            .unwrap_or(px(8.4));
        if self.wrap_lines {
            self.wrap_columns = self.wrap_width(window, editor_cell_width);
        }
        let wrapped_rows = (self.wrap_lines && !self.show_diff)
            .then(|| self.active.and_then(|i| self.tabs.get(i)))
            .flatten()
            .filter(|tab| !tab.loading && tab.csv.is_none())
            .map(|_| std::sync::Arc::new(self.wrapped_rows(window, editor_cell_width)));
        self.wrap_row_count = wrapped_rows.as_ref().map_or(0, |rows| rows.len());
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
            .bg(rgb(if gere { 0x11151b } else { 0x181a1f }))
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
            .child(Self::icon_button_sized(
                "terminal",
                "Terminales · Ctrl+J",
                px(38.),
                self.terminal_visible,
                Some(self.terminals.len()),
                cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx)),
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
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.terminal_focused = false))
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
                    .text_color(rgb(muted))
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
                            this.new_untitled(cx);
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
            .child(div().h(px(1.)).bg(rgb(border)))
            .when(self.startup_loading && self.files.is_empty() && !search_mode && !git_view, |v| {
                v.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Cargando proyecto…"),
                )
            })
            .when(search_mode, |v| {
                v.child(
                    div()
                        .h(px(30.))
                        .mx_3()
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(if self.search_focused { accent } else { border }))
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
                                                 .child(div().flex_shrink_0().child(if is_dir {
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
                                                 }))
                                                 .child(if this.file_edit.as_ref().is_some_and(|edit| matches!(edit, FileEdit::Rename(target) if target == &entry.path)) {
                                                     input_view(&this.file_name, "Nombre…", true,
                                                         this.cursor_blink_visible && this.focus.is_focused(window),
                                                          ((220. - depth as f32 * 16.) / f32::from(find_cell_width)).max(2.) as usize,
                                                          find_cell_width, background, true)
                                                          .flex_1().min_w_0().font_family(font_name).into_any_element()
                                                  } else {
                                                       div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(name).into_any_element()
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
                    input_view(&self.commit_message, "commit", self.commit_focused,
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
                                .h(px(26.))
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
                                .gap_1()
                                .when(has_staged || !can_sync && !can_push, |button| {
                                    button.child(icons::icon("check", 0xffffff).size(px(14.)))
                                })
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
                     div().flex_1().min_h_0().w_full().px_2().child(
                        uniform_list(
                            "git-changes",
                            self.git_rows.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|index| match this.git_rows[index] {
                                         GitRow::StagedHeader => div().h(px(26.)).w_full().flex().items_center().gap_1().pl_1().text_color(rgb(MUTED))
                                             .text_size(px(this.settings.git_font_size as f32))
                                             .cursor_pointer().hover(|s| s.bg(rgb(0x3e4451)))
                                             .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                 this.staged_expanded = !this.staged_expanded;
                                                 this.update_git_rows();
                                                 cx.notify();
                                             }))
                                             .child(icons::icon(if this.staged_expanded { "chevron-down" } else { "chevron-right" }, MUTED).size(px(14.)))
                                             .child(format!("Staged Changes ({})", this.changes.iter().filter(|change| change.index != ' ' && change.index != '?').count()))
                                             .child(div().flex_1())
                                             .child(Self::icon_button("minus", "Unstage All", cx.listener(|this, _, _, cx| {
                                                 cx.stop_propagation();
                                                 this.git_operation(GitOperation::UnstageAll, cx);
                                             }))),
                                         GitRow::UnstagedHeader => div()
                                             .h(px(26.))
                                             .w_full()
                                             .flex().items_center().gap_1().pl_1()
                                              .text_color(rgb(MUTED))
                                              .text_size(px(this.settings.git_font_size as f32))
                                              .bg(rgb(0x343a44)).rounded_sm()
                                              .cursor_pointer().hover(|s| s.bg(rgb(0x3e4451)))
                                             .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                 this.unstaged_expanded = !this.unstaged_expanded;
                                                 this.update_git_rows();
                                                 cx.notify();
                                             }))
                                             .child(icons::icon(if this.unstaged_expanded { "chevron-down" } else { "chevron-right" }, MUTED).size(px(14.)))
                                             .child(format!("Changes ({})", this.changes.iter().filter(|change| change.worktree != ' ').count()))
                                             .child(div().flex_1())
                                              .child(Self::icon_button("trash", "Discard All", cx.listener(|this, _, _, cx| {
                                                  cx.stop_propagation();
                                                  this.confirm_discard_all = true;
                                                 cx.notify();
                                             })))
                                              .child(Self::icon_button("plus", "Stage All", cx.listener(|this, _, _, cx| {
                                                  cx.stop_propagation();
                                                  this.git_operation(GitOperation::StageAll, cx);
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
                self.active
                    .and_then(|i| self.tabs.get(i))
                    .is_some_and(|tab| {
                        tab.path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                    }),
                |v| {
                    v.child(Self::button(
                        if self
                            .active
                            .and_then(|i| self.tabs.get(i))
                            .is_some_and(|tab| tab.preview.is_some())
                        {
                            "Editor"
                        } else {
                            "Vista previa"
                        },
                        cx.listener(|this, _, _, cx| {
                            if let Some(tab) = this.active.and_then(|i| this.tabs.get_mut(i)) {
                                tab.preview = if tab.preview.is_some() {
                                    None
                                } else {
                                    Some(markdown::parse(tab.buffer.text()))
                                };
                                if tab.preview.is_some() {
                                    this.show_diff = false;
                                }
                                cx.notify();
                            }
                        }),
                    ))
                },
            )
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
        let tabs = div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.terminal_focused = false),
            )
            .flex()
            .h(px(34.))
            .bg(rgb(if gere { 0x11151b } else { 0x181a1f }))
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                let path = tab.path.clone();
                let untitled = tab.untitled;
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_2()
                    .border_r_1()
                    .border_color(rgb(if gere { 0x273549 } else { 0x181a1f }))
                    .can_drop(|drag, _, _| {
                        drag.downcast_ref::<reviewer_files::FileTabDrag>().is_some()
                    })
                    .drag_over::<reviewer_files::FileTabDrag>(|style, _, _, _| {
                        style.bg(rgb(0x505766))
                    })
                    .on_drop(
                        cx.listener(move |this, drag: &reviewer_files::FileTabDrag, _, cx| {
                            if let Some(from) = this.tabs.iter().position(|tab| tab.path == drag.0)
                            {
                                reorder_tab(&mut this.tabs, &mut this.active, from, i);
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, _, _, cx| this.close_tab(i, cx)),
                    )
                    .bg(rgb(if self.active == Some(i) {
                        background
                    } else {
                        if gere {
                            0x1b222b
                        } else {
                            0x21252b
                        }
                    }))
                    .text_color(rgb(if self.active == Some(i) {
                        if gere {
                            ink
                        } else {
                            0xd7dae0
                        }
                    } else {
                        muted
                    }))
                    .child(
                        div()
                            .id(("file-tab-label", i))
                            .flex()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .on_drag(
                                reviewer_files::FileTabDrag(path.clone()),
                                |drag, _, _, cx| cx.new(|_| drag.clone()),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if !cx.has_active_drag() {
                                        if untitled {
                                            this.activate_tab(Some(i));
                                            this.selected = None;
                                            this.show_diff = false;
                                            cx.notify();
                                        } else {
                                            this.open(path.clone(), cx);
                                        }
                                    }
                                }),
                            )
                            .child(icons::file_icon(&tab.path))
                            .when(tab.buffer.is_dirty(), |v| {
                                v.child(div().size(px(7.)).rounded_full().bg(rgb(0xe5c07b)))
                            })
                            .child(tab_title(tab)),
                    )
                    .child(
                        div()
                            .p_1()
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(if gere { 0x22252e } else { 0x3e4451 })))
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
            .text_color(rgb(ink))
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
                                            if this.settings.is_gere() {
                                                0x7bcb9d
                                            } else {
                                                0x9ad7ae
                                            }
                                        } else if s.starts_with('-') {
                                            if this.settings.is_gere() {
                                                0xe07a82
                                            } else {
                                                0xee938e
                                            }
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
        } else if let Some(rows) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .and_then(|tab| tab.preview.as_ref())
        {
            content = content.h_full().child(
                div()
                    .id("markdown-preview")
                    .h_full()
                    .w_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows.iter().map(markdown::view)),
            );
        } else if let Some(tab) = self.active.and_then(|i| self.tabs.get(i)) {
            let wrapped = wrapped_rows;
            let row_count = wrapped.as_ref().map_or_else(
                || {
                    if self.show_diff && self.side_by_side {
                        self.diff_highlights.layout.len().min(10000)
                    } else {
                        tab.folding.visible.len().min(10000)
                    }
                },
                |rows| rows.len(),
            );
            content = content.h_full().child(
                uniform_list(
                    ("code-lines", self.active.unwrap_or(0)),
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
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
                        let blame_current = tab.blame_source.as_deref() == Some(source);
                        let blame_line = this
                            .blame_reveal_key
                            .as_ref()
                            .filter(|(path, _)| this.blame_visible && path == &tab.path)
                            .map(|(_, line)| *line);
                        let line_ranges = tab.buffer.ranges_for_render();
                        let wrapped = wrapped.as_ref();
                        range
                            .map(|visual| {
                                let diff_row = if this.show_diff && this.side_by_side {
                                    this.diff_highlights.layout.get(visual)
                                } else {
                                    None
                                };
                                let n = if let Some(rows) = wrapped {
                                    rows.get(visual).map(|(n, _)| *n)
                                } else if this.show_diff && this.side_by_side {
                                    diff_row.and_then(|row| row.after)
                                } else {
                                    tab.folding.visible.get(visual).copied()
                                };
                                let row_width = if wrapped.is_some() { viewport_width } else { (px(CODE_CELL_LEFT)
                                    + cell_width * tab.max_line_chars)
                                    .max(viewport_width) };
                                let Some(n) = n else {
                                    return div()
                                        .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                        .w(row_width)
                                        .bg(rgb(if this.settings.is_gere() { 0x1b222b } else { 0x30363c }))
                                        .border_b_1()
                                        .border_color(rgb(if this.settings.is_gere() { 0x273549 } else { 0x3b424b }))
                                        .into_any_element();
                                };
                                let logical_range = line_ranges.get(n).cloned().unwrap_or(0..0);
                                let segment = wrapped.and_then(|rows| rows.get(visual).map(|(_, span)| span.clone()))
                                    .unwrap_or(0..logical_range.len());
                                let line_range = logical_range.start + segment.start..logical_range.start + segment.end;
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
                                    .filter_map(|diagnostic| {
                                        let start = diagnostic.range.start.max(segment.start);
                                        let end = diagnostic.range.end.min(segment.end);
                                        (start < end).then(|| highlight::Diagnostic {
                                            range: start - segment.start..end - segment.start,
                                            ..diagnostic.clone()
                                        })
                                    })
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
                                let cursor_byte = tab.buffer.offset_at_position(cursor_position).saturating_sub(logical_range.start);
                                let cursor_column = (cursor_position.line == n
                                    && cursor_byte >= segment.start
                                    && (cursor_byte < segment.end || cursor_byte == segment.end && segment.end == logical_range.len())
                                    && this.cursor_blink_visible
                                    && !this.find_has_focus
                                    && !this.original_focused
                                    && this.editor_active()
                                    && this.focus.is_focused(window))
                                .then_some(csv_row.map_or_else(
                                    || buffer::visual_column(&line_text, line_text[..cursor_byte.saturating_sub(segment.start).min(line_text.len())].chars().count()),
                                    |row| {
                                        row.columns
                                            [cursor_position.column.min(row.columns.len() - 1)]
                                    },
                                ));
                                let mouse_down_text = line_text.clone();
                                let mouse_move_text = line_text;
                                let segment_column = source[logical_range.start..line_range.start].chars().count();
                                let mouse_down_start = segment_column;
                                let mouse_move_start = segment_column;
                                let mut line = div()
                                    .id(("editor-line", visual))
                                    .h(px((this.settings.font_size as f32 + 8.).max(22.)))
                                    .w(row_width)
                                    .bg(rgb(
                                        if this.show_diff
                                            && this.side_by_side
                                            && this.diff_highlights.added.contains(&n)
                                        {
                                             if this.settings.is_gere() { 0x1a302b } else { 0x26392f }
                                        } else if this.show_diff
                                            && this.side_by_side
                                            && this.diff_highlights.deletion_anchors.contains(&n)
                                        {
                                             if this.settings.is_gere() { 0x302126 } else { 0x3b292c }
                                        } else {
                                            this.settings.background()
                                        },
                                    ))
                                    .flex()
                                    .gap_2()
                                    .whitespace_nowrap()
                                    .relative()
                                    .cursor_text()
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
                                                            mouse_down_start + buffer::source_column(
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
                                                        let source_column = mouse_down_start + buffer::source_column(
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
                                                this.ensure_editor_cursor_visible(index, cx);
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
                                                            mouse_move_start + buffer::source_column(
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
                                            this.ensure_editor_cursor_visible(index, cx);
                                            cx.notify();
                                        },
                                    ))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, _| this.mouse_selecting = false),
                                    )
                                    .child(
                                             div()
                                                 .w(px(64.))
                                            .relative()
                                            .text_color(rgb(
                                                if this.show_diff
                                                    && this.side_by_side
                                                    && this.diff_highlights.added.contains(&n)
                                                {
                                                     if this.settings.is_gere() { 0x7bcb9d } else { 0x9ad7ae }
                                                } else if this.show_diff
                                                    && this.side_by_side
                                                    && this
                                                        .diff_highlights
                                                        .deletion_anchors
                                                        .contains(&n)
                                                {
                                                     if this.settings.is_gere() { 0xe07a82 } else { 0xee938e }
                                                } else {
                                                    0x5c6370
                                                },
                                            ))
                                              .child(if segment.start == 0 { format!("{:>5}", n + 1) } else { String::new() })
                                             .when(!this.show_diff && this.diff_highlights.added.contains(&n), |gutter| gutter.child(
                                                 div().absolute().left(px(0.)).text_color(rgb(if this.settings.is_gere() { 0x7bcb9d } else { 0x9ad7ae })).child("+")))
                                             .when(!this.show_diff && this.diff_highlights.deletion_anchors.contains(&n), |gutter| gutter.child(
                                                 div().absolute().left(px(0.)).text_color(rgb(if this.settings.is_gere() { 0xe07a82 } else { 0xee938e })).child("−")))
                                             .when(this.show_diff && this.side_by_side && this.diff_highlights.added.contains(&n), |gutter| gutter.child(
                                                 div().absolute().left(px(0.)).text_color(rgb(0x7bcb9d)).child("+")))
                                             .when(this.show_diff && this.side_by_side && this.diff_highlights.deletion_anchors.contains(&n), |gutter| gutter.child(
                                                 div().absolute().left(px(0.)).text_color(rgb(0xe07a82)).child("−")))
                                            .when(!this.show_diff && tab.folding.ends.get(n).is_some_and(Option::is_some), |gutter| {
                                                let collapsed = tab.folding.collapsed.contains(&n);
                                                gutter.child(
                                                    div()
                                                        .absolute()
                                                        .right(px(1.))
                                                        .top(px(2.))
                                                        .w(px(15.))
                                                        .h(px(18.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .cursor_pointer()
                                                        .text_color(rgb(MUTED))
                                                        .hover(|style| style.text_color(rgb(FG)))
                                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            if let Some(tab) = this.active.and_then(|i| this.tabs.get_mut(i)) {
                                                                if tab.folding.ends.get(n).is_some_and(Option::is_some) {
                                                                    if !tab.folding.collapsed.contains(&n) {
                                                                        if let Some(end) = tab.folding.ends[n] {
                                                                            if (n + 1..end).contains(&tab.buffer.cursor_position().line) {
                                                                                let offset = tab.buffer.offset_at_position(buffer::Position { line: n, column: 0 });
                                                                                tab.buffer.set_cursor(offset, false);
                                                                            }
                                                                        }
                                                                    }
                                                                    tab.folding.toggle(n);
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                        .child(if collapsed { "▸" } else { "▾" }),
                                                )
                                            }),
                                    )
                                    .child(csv_row.map_or_else(
                                        || {
                                             tab.lines[n].slice(segment.clone()).render_editor(
                                                local_selection.clone(),
                                                &local_matches,
                                                diff_row.and_then(|row| {
                                                     row.after_range
                                                        .clone()
                                                         .and_then(|range| {
                                                             let start = range.start.max(segment.start);
                                                             let end = range.end.min(segment.end);
                                                              (start < end).then_some((start - segment.start..end - segment.start, if this.settings.is_gere() { 0x1e4437 } else { 0x345f42 }))
                                                         })
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
                                if blame_current
                                    && blame_line == Some(n)
                                    && cursor_position.line == n
                                    && cursor_byte >= segment.start
                                    && (cursor_byte < segment.end
                                        || cursor_byte == segment.end
                                            && segment.end == logical_range.len())
                                    && !this.show_diff
                                {
                                    if let Some(blame) = tab.blame_lines.get(n).and_then(Option::as_ref) {
                                        let label = if blame.commit.is_empty() {
                                            blame.author.clone()
                                        } else {
                                            format!("{} · {} · {}", blame.author, &blame.commit[..8], blame.summary)
                                        };
                                        line = line.child(
                                            div()
                                                .absolute()
                                                .left(px(CODE_CELL_LEFT) + cell_width * (line_chars + 2))
                                                .top(px(3.))
                                                .text_color(rgb(MUTED))
                                                .child(label),
                                        );
                                    }
                                }
                                if !this.show_diff {
                                    let marker = if this.diff_highlights.added.contains(&n) {
                                        Some(if this.settings.is_gere() { 0x7bcb9d } else { 0x9ad7ae })
                                    } else if this.diff_highlights.deletion_anchors.contains(&n) {
                                        Some(if this.settings.is_gere() { 0xe07a82 } else { 0xee938e })
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
                    .child(div().text_color(rgb(MUTED)).child(if self.startup_loading {
                        "Cargando proyecto…"
                    } else {
                        "Tu espacio para crear"
                    }))
                    .when(!self.startup_loading, |view| {
                        view.child(
                            div()
                                .text_color(rgb(0x5c6370))
                                .text_xs()
                                .child("Elegí un archivo del explorador o presioná Ctrl+P"),
                        )
                    }),
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
                                            .bg(rgb(if this.settings.is_gere() {
                                                0x1b222b
                                            } else {
                                                0x30363c
                                            }))
                                            .border_b_1()
                                            .border_color(rgb(if this.settings.is_gere() {
                                                0x273549
                                            } else {
                                                0x3b424b
                                            }))
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
                                            if this.settings.is_gere() {
                                                0x302126
                                            } else {
                                                0x3b292c
                                            }
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
                                                .w(px(64.))
                                                .relative()
                                                .text_color(rgb(
                                                    if this.diff_highlights.removed.contains(&n) {
                                                        if this.settings.is_gere() {
                                                            0xe07a82
                                                        } else {
                                                            0xee938e
                                                        }
                                                    } else {
                                                        0x5c6370
                                                    },
                                                ))
                                                .child(format!("{:>5}", n + 1))
                                                .when(
                                                    this.diff_highlights.removed.contains(&n),
                                                    |gutter| {
                                                        gutter.child(
                                                            div()
                                                                .absolute()
                                                                .left(px(0.))
                                                                .text_color(rgb(0xe07a82))
                                                                .child("−"),
                                                        )
                                                    },
                                                ),
                                        )
                                        .child(this.original_lines[n].render_editor(
                                            local_selection,
                                            &[],
                                            row.and_then(|row| {
                                                row.before_range.clone().map(|range| {
                                                    (
                                                        range,
                                                        if this.settings.is_gere() {
                                                            0x4a2930
                                                        } else {
                                                            0x703839
                                                        },
                                                    )
                                                })
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
                    if this.terminal_dragging && event.dragging() {
                        this.terminal_height = (window.bounds().size.height - event.position.y - px(26.))
                            .clamp(px(110.), (window.bounds().size.height - px(145.)).max(px(110.)));
                        cx.notify();
                    }
                    if this.terminal_selecting && event.dragging() {
                        this.extend_terminal_selection(event.position, window, cx);
                    }
                    if this.terminal_scroll_dragging && event.dragging() {
                        this.drag_terminal_scrollbar(event.position.y, window, cx);
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
                    let dragging_terminal = std::mem::take(&mut this.terminal_dragging);
                    let dragging_terminal_scrollbar = std::mem::take(&mut this.terminal_scroll_dragging);
                    let selecting_terminal = std::mem::take(&mut this.terminal_selecting);
                    this.mouse_selecting = false;
                    this.original_mouse_selecting = false;
                    if dragging_scrollbar
                        || dragging_original
                        || dragging_split
                        || dragging_svg_split
                        || dragging_sidebar
                        || dragging_terminal
                        || dragging_terminal_scrollbar
                        || selecting_terminal
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
                    .bg(rgb(if gere { 0x11151b } else { 0x181a1f }))
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
                                 .child(div().w(px(220.)).p_1().rounded_md().bg(rgb(PANEL))
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
                                     })))
                                      .child(Self::menu_item("New File    Ctrl+T", cx.listener(|this, _, _, cx| {
                                          this.top_file_menu = false;
                                          this.new_untitled(cx);
                                      })))
                                      .child(Self::menu_item("New Folder", cx.listener(|this, _, _, cx| {
                                          this.sidebar = Sidebar::Files;
                                          this.sidebar_visible = true;
                                          this.begin_file_edit(FileEdit::CreateFolder(selected_folder(this.selected.as_ref(), &this.files)), cx);
                                     })))
                                     .child(Self::menu_item("Save    Ctrl+S", cx.listener(|this, _, _, cx| {
                                         this.top_file_menu = false;
                                         this.save_active(cx);
                                     })))
                                     .child(Self::menu_item("Close Tab", cx.listener(|this, _, _, cx| {
                                         this.top_file_menu = false;
                                         if let Some(index) = this.active { this.close_tab(index, cx); }
                                      }))))))))
                    .child(div().flex_1().min_w_0().h_full().flex().items_center().justify_center()
                        .child(div().id("quick-open-bar").h(px(24.)).w(px(340.)).max_w_full().px_2()
                            .flex().items_center().gap_2().rounded_sm().border_1()
                            .border_color(rgb(border)).bg(rgb(if gere { 0x1b222b } else { PANEL }))
                            .cursor_pointer().hover(move |s| s.bg(rgb(if gere { 0x22252e } else { 0x3e4451 })))
                            .tooltip(|_, cx| cx.new(|_| reviewer_ui::IconTooltip("Abrir archivo · Ctrl+P")).into())
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
                            .child(icons::icon("search", muted))
                            .child(div().flex_1().min_w_0().overflow_hidden().text_xs().text_color(rgb(muted)).child("Buscar archivos o > comandos…"))
                            .child(div().text_xs().text_color(rgb(muted)).child("Ctrl+P"))))
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
                             .bg(rgb(if self.dragging_sidebar { accent } else { border }))
                             .hover(move |s| s.bg(rgb(accent)))
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
                            .child(tabs)
                            .when_some(self.active.and_then(|i| self.tabs.get(i)), |view, tab| {
                                let path = (!tab.untitled).then(|| project_relative_path(&self.root, &tab.path)).flatten()
                                    .or_else(|| tab.path.file_name().map(PathBuf::from))
                                    .unwrap_or_default();
                                view.child(
                                    div()
                                        .h(px(24.))
                                        .w_full()
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .overflow_hidden()
                                        .bg(rgb(background))
                                        .text_color(rgb(MUTED))
                                        .text_xs()
                                        .child(if tab.untitled { tab_title(tab) } else { path.to_string_lossy().into_owned() }),
                                )
                            })
                            .when(
                                 change.is_some_and(|c| c.index != '?')
                                     || self.show_diff
                                     || !conflicts.is_empty()
                                     || self.active.and_then(|i| self.tabs.get(i)).is_some_and(|tab| tab.path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md"))),
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
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| {
                                        this.files_focused = false;
                                        this.terminal_focused = false;
                                    }))
                                    .child(content.h_full())
                                    .children(normal_vertical)
                                    .children(normal_horizontal),
                            )
                            .when(self.terminal_visible, |view| view.child(self.terminal_view(window, cx))),
                    ),
            )
            .child(
                 div()
                     .h(px(26.))
                     .w_full()
                     .px_2()
                     .bg(rgb(if gere { 0x11151b } else { 0x61afef }))
                     .when(gere, |bar| bar.border_t_1().border_color(rgb(border)))
                     .flex()
                     .items_center()
                     .gap_2()
                     .text_xs()
                     .text_color(rgb(if gere { ink } else { 0x21252b }))
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
                                  cx.listener(|this, _, window, cx| this.toggle_branch_menu(window, cx)),
                             )
                              .px_1().rounded_sm()
                              .hover(move |s| s.bg(rgb(if gere { 0x22252e } else { 0x2573dc })))
                              .child(icons::icon("git", if gere { accent } else { 0x21252b }))
                              .child(self.branch.clone().unwrap_or_else(|| "HEAD".into()))
                              .child(icons::icon("chevron-down", if gere { muted } else { 0x21252b }))
                      )
                     .child(div().h(px(14.)).w(px(1.)).bg(rgb(border)))
                     .child(div().flex().items_center().gap_1().cursor_pointer()
                         .hover(move |s| s.bg(rgb(if gere { 0x22252e } else { 0x2573dc })))
                         .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| this.toggle_sidebar(Sidebar::Git, window, cx)))
                         .child(icons::icon("git", if gere { muted } else { 0x21252b }))
                         .child(format!("{} cambios", self.changes.len())))
                     .child(div().flex_1())
                     .child(div().min_w_0().overflow_hidden().text_color(rgb(if gere { muted } else { 0x21252b })).child(self.message.clone()))
                      .child(div().flex_1())
                       .when_some(self.active.and_then(|i| self.tabs.get(i)), |bar, tab| {
                           let current_diagnostics: Vec<_> = tab.diagnostics.iter()
                               .filter(|diagnostic| diagnostic.line == tab.buffer.cursor_position().line)
                               .cloned().collect();
                           bar.child(div().id("status-diagnostics").flex().items_center().gap_1()
                               .when(!current_diagnostics.is_empty(), |status| status.tooltip(move |_, cx| {
                                   cx.new(|_| DiagnosticTooltip { diagnostics: current_diagnostics.clone() }).into()
                               }))
                               .child(icons::icon("alert-circle", if gere { 0xe07a82 } else { 0x21252b }))
                              .child(format!("{}", tab.diagnostics.len())))
                          .child(div().h(px(14.)).w(px(1.)).bg(rgb(border)))
                          .child(div().child(format!("Ln {}, Col {}",
                              tab.buffer.cursor_position().line + 1,
                              tab.buffer.cursor_position().column + 1)))
                          .child(div().h(px(14.)).w(px(1.)).bg(rgb(border)))
                          .child("UTF-8")
                          .child(div().h(px(14.)).w(px(1.)).bg(rgb(border)))
                          .child(div()
                              .id("language-selector")
                             .h_full()
                             .flex()
                             .items_center()
                             .cursor_pointer()
                             .tooltip(|_, cx| cx.new(|_| reviewer_ui::IconTooltip("Cambiar lenguaje")).into())
                             .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                 this.language_menu_open = !this.language_menu_open;
                                 cx.notify();
                             }))
                              .hover(move |s| s.bg(rgb(if gere { 0x22252e } else { 0x2573dc })))
                              .child(icons::icon("file-code", if gere { muted } else { 0x21252b }))
                              .child(highlight::label(&tab.path, self.language_overrides.get(&tab.path).map(String::as_str))))
                      }),
            )
            .when(self.language_menu_open && self.active.is_some(), |view| {
                let active_path = &self.tabs[self.active.unwrap()].path;
                let selected = self.language_overrides.get(active_path).map(String::as_str);
                view.child(div()
                    .absolute()
                    .bottom(px(26.))
                    .right(px(8.))
                    .w(px(170.))
                    .p_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x3e4451))
                    .bg(rgb(panel))
                    .shadow_lg()
                    .child(Self::menu_item(if selected.is_none() { "✓ Automático" } else { "Automático" },
                        cx.listener(|this, _, _, cx| this.set_language(None, cx))))
                    .children(highlight::MODES.iter().map(|(mode, name)| {
                        let mode = *mode;
                        let title = if selected == Some(mode) { format!("✓ {name}") } else { (*name).to_owned() };
                        div().w_full().px_2().py_1().rounded_sm().cursor_pointer()
                            .text_color(rgb(FG))
                            .hover(|s| s.bg(rgb(0x3e4451)))
                            .on_mouse_up(MouseButton::Left, cx.listener(move |this, _, _, cx| this.set_language(Some(mode), cx)))
                            .child(title)
                    })))
            })
            .when(self.git_log_open, |view| view.child(
                div().absolute().bottom(px(110.)).right(px(12.))
                    .w(px(520.)).max_h(px(330.)).p_3().rounded_md()
                    .border_1().border_color(rgb(0x4b5261)).bg(rgb(panel)).shadow_lg()
                    .flex().flex_col().gap_2()
                    .child(div().flex().justify_between()
                        .child("Git log")
                        .child(div().cursor_pointer().hover(|s| s.text_color(rgb(FG)))
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.git_log_open = false;
                                cx.notify();
                            })).child("✕")))
                    .child(div().id("git-log-entries").max_h(px(270.)).overflow_y_scroll()
                        .children(self.git_log.iter().rev().map(|entry| div()
                            .py_1().whitespace_normal().font_family("monospace")
                            .child(entry.clone()))))
            ))
            .when_some(self.notice.as_ref(), |view, notice| {
                let action_label = match &notice.action {
                    NoticeAction::PullRequest(_) => "Create pull request",
                    NoticeAction::GitLog => "Show Git log",
                };
                view.child(div().absolute().bottom(px(38.)).right(px(12.))
                    .w(px(350.)).p_3().rounded_md().border_1()
                    .border_color(rgb(0x4b5261)).bg(rgb(panel)).shadow_lg()
                    .flex().flex_col().gap_2()
                    .child(div().flex().justify_between().gap_2()
                        .child(div().flex_1().min_w_0().whitespace_normal().child(notice.text.clone()))
                        .child(div().cursor_pointer().hover(|s| s.text_color(rgb(0x61afef)))
                            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            })).child("✕")))
                    .child(div().flex().child(div().px_2().py_1().rounded_sm()
                        .bg(rgb(0x3e4451)).cursor_pointer()
                        .hover(|s| s.bg(rgb(0x4b5261)))
                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            let Some(notice) = this.notice.take() else { return };
                            match notice.action {
                                NoticeAction::GitLog => this.git_log_open = true,
                                NoticeAction::PullRequest(url) => {
                                    if let Err(error) = std::process::Command::new("xdg-open").arg(url).spawn() {
                                        this.show_git_error("abrir navegador", error.to_string());
                                    }
                                }
                            }
                            cx.notify();
                        })).child(action_label))))
            })
            .child(ime::ImeElement(cx.entity()))
            .when(self.find_open, |view| {
                view.child(self.find_view(cx, panel, background, font_name, find_cell_width, find_caret_visible))
            })
            .when(self.palette_open, |v| {
                v.child(self.palette_view(window, cx, panel, background, font_name, find_cell_width))
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
                                             match self.settings.theme {
                                                 settings::Theme::Darker => "One Dark Pro Darker  ▾",
                                                 settings::Theme::Classic => "One Dark Pro  ▾",
                                                 settings::Theme::Gere => "Gere Theme  ▾",
                                             },
                                            cx.listener(|this, _, _, cx| {
                                                this.update_settings(
                                                    |s| {
                                                         s.theme = match s.theme {
                                                             settings::Theme::Darker => settings::Theme::Classic,
                                                             settings::Theme::Classic => settings::Theme::Gere,
                                                             settings::Theme::Gere => settings::Theme::Darker,
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
                                 )
                                 .child(
                                     div()
                                         .flex()
                                         .items_center()
                                         .justify_between()
                                         .child("Tamaño de fuente de Git")
                                         .child(
                                             div()
                                                 .flex()
                                                 .items_center()
                                                 .gap_2()
                                                 .child(Self::button(
                                                     "−",
                                                     cx.listener(|this, _, _, cx| {
                                                         this.update_settings(
                                                             |s| s.git_font_size = s.git_font_size.saturating_sub(1).max(10),
                                                             cx,
                                                         )
                                                     }),
                                                 ))
                                                  .child(format!("{} px", self.settings.git_font_size))
                                                  .child(Self::button(
                                                      "+",
                                                      cx.listener(|this, _, _, cx| {
                                                          this.update_settings(
                                                              |s| s.git_font_size = (s.git_font_size + 1).min(16),
                                                              cx,
                                                          )
                                                      }),
                                                  )),
                                          ),
                                  )
                                  .child(div().h(px(1.)).bg(rgb(0x3e4451)))
                                  .child(div().text_xs().text_color(rgb(MUTED)).child("EDITOR GIT"))
                                  .child(
                                      div()
                                          .flex()
                                          .items_center()
                                          .justify_between()
                                          .child("Demora de autoría")
                                          .child(
                                              div()
                                                  .flex()
                                                  .items_center()
                                                  .gap_2()
                                                  .child(Self::button(
                                                      "−",
                                                      cx.listener(|this, _, _, cx| {
                                                          this.update_settings(
                                                              |s| s.blame_delay_ms = s.blame_delay_ms.saturating_sub(100),
                                                              cx,
                                                          )
                                                      }),
                                                  ))
                                                  .child(format!("{} ms", self.settings.blame_delay_ms))
                                                  .child(Self::button(
                                                      "+",
                                                      cx.listener(|this, _, _, cx| {
                                                          this.update_settings(
                                                              |s| s.blame_delay_ms = (s.blame_delay_ms + 100).min(2000),
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
                .update(cx, |view, window, _| {
                    window.focus(&view.focus);
                    window.zoom_window();
                })
                .expect("enfocar ventana");
            cx.activate(true);
        });
}
