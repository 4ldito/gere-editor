//! Git actions and branch loading. Filesystem/Git operations stay outside render callbacks.
use super::*;

pub(super) enum GitOperation {
    Commit(String),
    Push,
    Sync,
    SwitchBranch(String),
    CreateBranch(String),
    CreateBranchFrom(String, String),
    ApplyStash(String),
    StageAll,
    UnstageAll,
    DiscardAll,
}

#[derive(Clone, Copy)]
pub(super) enum GitRow {
    StagedHeader,
    UnstagedHeader,
    Change(usize, bool),
}

#[derive(PartialEq, Eq)]
pub(super) struct DiscardState {
    pub(super) change: project::Change,
    pub(super) len: u64,
    pub(super) modified: SystemTime,
}

impl Reviewer {
    pub(super) fn stage_row(&mut self, path: &Path, staged: bool, cx: &mut Context<Self>) {
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

    pub(super) fn load_diff(&mut self) {
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

    pub(super) fn load_change_decorations(&mut self) {
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

    pub(super) fn update_diff_highlights(&mut self) {
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

    pub(super) fn scroll_to_first_change(&self) {
        if let Some(visual) = self.diff_highlights.first_visual() {
            self.original_scroll
                .scroll_to_item_strict(visual, ScrollStrategy::Center);
            self.editor_scroll
                .scroll_to_item_strict(visual, ScrollStrategy::Center);
        }
    }

    pub(super) fn accept_conflict(
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

    pub(super) fn focus_first_conflict(&mut self) {
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

    pub(super) fn action(&mut self, op: &str, cx: &mut Context<Self>) {
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

    pub(super) fn toggle_branch_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_busy {
            return;
        }
        self.palette_open = true;
        self.palette_mode = PaletteMode::Branches;
        self.palette_error = None;
        self.palette_query.set_text(String::new());
        self.palette_selected = 0;
        self.files_focused = false;
        self.close_find();
        window.focus(&self.focus);
        self.load_branches(cx);
    }

    pub(super) fn load_branches(&mut self, cx: &mut Context<Self>) {
        self.branches.clear();
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

    pub(super) fn git_operation(&mut self, operation: GitOperation, cx: &mut Context<Self>) {
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
        self.palette_open = false;
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
                                GitOperation::CreateBranch(branch) => {
                                    ("Crear branch", project::create_branch(&root, &branch))
                                }
                                GitOperation::CreateBranchFrom(branch, source) => (
                                    "Crear branch",
                                    project::create_branch_from(&root, &branch, &source),
                                ),
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
                            Err(error) => {
                                if label == "Cambiar branch" && !this.palette_open {
                                    this.palette_mode = PaletteMode::Branches;
                                    this.palette_open = true;
                                    this.palette_error = Some(error.clone());
                                }
                                error
                            }
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
}
