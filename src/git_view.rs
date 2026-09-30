use std::collections::{HashMap, HashSet};

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

    diff_middle(
        &before,
        &after,
        prefix..old_end,
        prefix..new_end,
        &mut matches,
        &mut marks,
    );
    matches.extend((0..before.len() - old_end).map(|i| (old_end + i, new_end + i)));
    let (layout, before_to_visual, after_to_visual) = aligned_lines(&before, &after, &matches);
    marks.layout = layout;
    marks.before_to_visual = before_to_visual;
    marks.after_to_visual = after_to_visual;
    marks.first_before = Some(prefix.min(before.len() - 1));
    marks.first_after = Some(prefix.min(after.len() - 1));
    marks
}

fn diff_middle(
    before: &[&str],
    after: &[&str],
    old: std::ops::Range<usize>,
    new: std::ops::Range<usize>,
    matches: &mut Vec<(usize, usize)>,
    marks: &mut DiffHighlights,
) {
    let old_len = old.len();
    let new_len = new.len();
    // Find stable anchors before allocating an LCS matrix. Two small edits far apart
    // must not turn all the unchanged lines between them into one large edit.
    if old_len.saturating_mul(new_len) > 400_000 {
        let anchors = unique_anchors(&before[old.clone()], &after[new.clone()]);
        if !anchors.is_empty() {
            let (mut old_start, mut new_start) = (old.start, new.start);
            for (old_anchor, new_anchor) in anchors {
                let old_anchor = old.start + old_anchor;
                let new_anchor = new.start + new_anchor;
                diff_middle(
                    before,
                    after,
                    old_start..old_anchor,
                    new_start..new_anchor,
                    matches,
                    marks,
                );
                matches.push((old_anchor, new_anchor));
                old_start = old_anchor + 1;
                new_start = new_anchor + 1;
            }
            diff_middle(
                before,
                after,
                old_start..old.end,
                new_start..new.end,
                matches,
                marks,
            );
            return;
        }
        marks.removed.extend(old);
        marks.added.extend(new.clone());
        if new_len == 0 {
            marks
                .deletion_anchors
                .insert(new.start.min(after.len() - 1));
        }
        return;
    }

    let width = new_len + 1;
    let mut lcs = vec![0u32; (old_len + 1) * width];
    for i in (0..old_len).rev() {
        for j in (0..new_len).rev() {
            lcs[i * width + j] = if before[old.start + i] == after[new.start + j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut deletion_at: Option<usize> = None;
    while i < old_len || j < new_len {
        if i < old_len && j < new_len && before[old.start + i] == after[new.start + j] {
            matches.push((old.start + i, new.start + j));
            if let Some(anchor) = deletion_at.take() {
                marks.deletion_anchors.insert(anchor.min(after.len() - 1));
            }
            i += 1;
            j += 1;
        } else if i < old_len
            && (j == new_len || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
        {
            marks.removed.insert(old.start + i);
            deletion_at.get_or_insert(new.start + j);
            i += 1;
        } else {
            marks.added.insert(new.start + j);
            deletion_at = None;
            j += 1;
        }
    }
    if let Some(anchor) = deletion_at {
        marks.deletion_anchors.insert(anchor.min(after.len() - 1));
    }
}

fn unique_anchors(before: &[&str], after: &[&str]) -> Vec<(usize, usize)> {
    let mut positions = HashMap::new();
    for (index, line) in after.iter().enumerate() {
        positions
            .entry(*line)
            .and_modify(|position| *position = None)
            .or_insert(Some(index));
    }
    let mut before_positions = HashMap::new();
    for (index, line) in before.iter().enumerate() {
        before_positions
            .entry(*line)
            .and_modify(|position| *position = None)
            .or_insert(Some(index));
    }
    let candidates: Vec<_> = before
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            if before_positions.get(line) != Some(&Some(index)) {
                return None;
            }
            let position = positions.get(line).copied().flatten()?;
            Some((index, position))
        })
        .collect();
    let mut tails = Vec::<usize>::new();
    let mut indices = Vec::<usize>::new();
    let mut previous = vec![None; candidates.len()];
    for (index, &(_, position)) in candidates.iter().enumerate() {
        let slot = tails.partition_point(|&tail| tail < position);
        if slot > 0 {
            previous[index] = Some(indices[slot - 1]);
        }
        if slot == tails.len() {
            tails.push(position);
            indices.push(index);
        } else {
            tails[slot] = position;
            indices[slot] = index;
        }
    }
    let mut anchors = Vec::new();
    let mut cursor = indices.last().copied();
    while let Some(index) = cursor {
        anchors.push(candidates[index]);
        cursor = previous[index];
    }
    anchors.reverse();
    anchors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distant_insertions_leave_intervening_lines_unmarked() {
        let original = (0..1_100).map(|n| format!("line {n}")).collect::<Vec<_>>();
        let mut current = original.clone();
        current.insert(250, "new function".into());
        current.insert(1_001, "new test".into());
        let original = original.join("\n") + "\n";
        let current = current.join("\n") + "\n";
        let marks = line_diff_highlights(&original, &current);
        assert!(marks.removed.is_empty());
        assert_eq!(marks.added, HashSet::from([250, 1_001]));
        assert_eq!(marks.layout[500].before, Some(499));
        assert_eq!(marks.layout[500].after, Some(500));
        assert!(marks.layout[500].after_range.is_none());
    }
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
