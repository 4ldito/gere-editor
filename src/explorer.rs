use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{project, ExplorerRow, FileEdit};

pub(super) fn can_move_into(path: &Path, folder: &Path) -> bool {
    path.parent() != Some(folder) && !folder.starts_with(path)
}

pub(super) fn selected_folder(selected: Option<&PathBuf>, files: &[project::FileEntry]) -> PathBuf {
    selected
        .filter(|path| {
            files
                .iter()
                .any(|entry| entry.path == **path && entry.is_dir)
        })
        .cloned()
        .unwrap_or_default()
}

pub(super) fn explorer_rows(
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

pub(super) fn file_tree(entries: &[project::FileEntry]) -> BTreeMap<PathBuf, Vec<usize>> {
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

pub(super) fn visible_entries(
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
