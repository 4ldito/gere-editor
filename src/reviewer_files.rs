//! Explorer selection, expansion and path relocation.
use super::*;

#[derive(Clone)]
pub(super) enum FileEdit {
    Create(PathBuf),
    CreateFolder(PathBuf),
    Rename(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExplorerRow {
    Entry(usize, usize),
    NewFile(usize),
}

#[derive(Clone)]
pub(super) struct ExplorerDrag(pub(super) PathBuf);

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

impl Reviewer {
    pub(super) fn relocate_paths(&mut self, from: &Path, to: &Path) {
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

    pub(super) fn move_explorer_entry(
        &mut self,
        path: &Path,
        folder: &Path,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn update_tree(&mut self) {
        self.tree = file_tree(&self.files);
        self.update_visible();
    }

    pub(super) fn update_visible(&mut self) {
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

    pub(super) fn reveal_file(&mut self, path: &Path) {
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

    pub(super) fn toggle_folder(&mut self, path: &Path) {
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
}
