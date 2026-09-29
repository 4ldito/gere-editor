use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct AlignedLine {
    pub(super) before: Option<usize>,
    pub(super) after: Option<usize>,
    pub(super) before_range: Option<std::ops::Range<usize>>,
    pub(super) after_range: Option<std::ops::Range<usize>>,
}

pub(super) fn changed_text_ranges(
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

pub(super) fn aligned_lines(
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
pub(super) struct DiffHighlights {
    pub(super) removed: HashSet<usize>,
    pub(super) added: HashSet<usize>,
    pub(super) deletion_anchors: HashSet<usize>,
    pub(super) first_before: Option<usize>,
    pub(super) first_after: Option<usize>,
    pub(super) layout: Vec<AlignedLine>,
    pub(super) before_to_visual: Vec<usize>,
    pub(super) after_to_visual: Vec<usize>,
}

impl DiffHighlights {
    pub(super) fn first_visual(&self) -> Option<usize> {
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

pub(super) fn line_diff_highlights(original: &str, current: &str) -> DiffHighlights {
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

pub(super) struct DiffRow {
    pub(super) raw: String,
    pub(super) before: String,
    pub(super) after: String,
}

pub(super) fn aligned_diff(text: &str) -> Vec<DiffRow> {
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
pub(super) enum ConflictChoice {
    Current,
    Incoming,
    Both,
}

#[derive(Clone, Copy)]
pub(super) struct ConflictBlock {
    pub(super) start: usize,
    pub(super) divider: usize,
    pub(super) end: usize,
}

pub(super) fn conflict_blocks(text: &str) -> Vec<ConflictBlock> {
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

pub(super) fn resolve_conflict(text: &str, block: ConflictBlock, choice: ConflictChoice) -> String {
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
