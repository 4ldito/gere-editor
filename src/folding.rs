//! Per-tab projection of physical source lines onto visible editor rows.
use std::collections::HashSet;

#[derive(Default)]
pub struct Folding {
    pub ends: Vec<Option<usize>>,
    pub collapsed: HashSet<usize>,
    pub visible: Vec<usize>,
}

impl Folding {
    pub fn update(&mut self, ends: Vec<Option<usize>>, cursor: usize) {
        if self.ends.len() != ends.len() {
            self.collapsed.clear();
        }
        self.ends = ends;
        self.collapsed
            .retain(|start| self.ends.get(*start).is_some_and(Option::is_some));
        self.rebuild();
        self.reveal(cursor);
    }

    pub fn toggle(&mut self, line: usize) {
        if self.ends.get(line).is_none_or(Option::is_none) {
            return;
        }
        if !self.collapsed.insert(line) {
            self.collapsed.remove(&line);
        }
        self.rebuild();
    }

    pub fn reveal(&mut self, line: usize) {
        if self.visible.binary_search(&line).is_ok() {
            return;
        }
        let before = self.collapsed.len();
        self.collapsed
            .retain(|start| !(*start < line && self.ends[*start].is_some_and(|end| line < end)));
        if self.collapsed.len() != before {
            self.rebuild();
        }
    }

    pub fn visual(&self, line: usize) -> usize {
        match self.visible.binary_search(&line) {
            Ok(row) | Err(row) => row.min(self.visible.len().saturating_sub(1)),
        }
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let mut line = 0;
        while line < self.ends.len() {
            self.visible.push(line);
            line = if self.collapsed.contains(&line) {
                self.ends[line].map_or(line + 1, |end| end.max(line + 1))
            } else {
                line + 1
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_folds_keep_source_lines_and_reveal_navigation() {
        let mut folding = Folding::default();
        folding.update(vec![Some(5), None, Some(4), None, None, None, None], 0);
        folding.toggle(2);
        assert_eq!(folding.visible, [0, 1, 2, 4, 5, 6]);
        folding.toggle(0);
        assert_eq!(folding.visible, [0, 5, 6]);
        folding.reveal(5);
        assert_eq!(folding.visible, [0, 5, 6]);
        folding.reveal(3);
        assert_eq!(folding.visible, [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(folding.visual(3), 3);
    }

    #[test]
    fn closing_line_stays_visible_without_expanding_its_block() {
        let mut folding = Folding::default();
        folding.update(vec![Some(3), None, None, None, None], 0);
        folding.toggle(0);
        assert_eq!(folding.visible, [0, 3, 4]);
        folding.reveal(3);
        assert!(folding.collapsed.contains(&0));
        folding.toggle(0);
        assert_eq!(folding.visible, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn inserting_lines_drops_stale_fold_positions() {
        let mut folding = Folding::default();
        folding.update(vec![Some(2), None, None], 0);
        folding.toggle(0);
        folding.update(vec![None, Some(3), None, None], 0);
        assert!(folding.collapsed.is_empty());
        assert_eq!(folding.visible, [0, 1, 2, 3]);
    }
}
