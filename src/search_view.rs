use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use gpui::{rgb, HighlightStyle, StyledText};

use crate::project;

#[derive(Clone)]
pub(super) enum SearchRow {
    File(PathBuf, usize),
    Match(usize),
}

pub(super) fn search_rows(
    matches: &[project::Match],
    collapsed: &HashSet<PathBuf>,
) -> Vec<SearchRow> {
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

pub(super) fn match_preview(found: &project::Match) -> StyledText {
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

pub(super) fn palette_target(query: &str) -> (&str, Option<usize>) {
    if let Some((name, line)) = query.rsplit_once(':') {
        if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) {
            if let Some(line) = line.parse::<usize>().ok().filter(|line| *line > 0) {
                return (name, Some(line));
            }
        }
    }
    (query, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
