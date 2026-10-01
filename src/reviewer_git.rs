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
    /// Prepare a requested diff without blocking the click that opened it.
    pub(super) fn load_diff_async(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        let Some(text) = self
            .tabs
            .iter()
            .find(|tab| tab.path == path && !tab.loading)
            .map(|tab| tab.buffer.text().to_owned())
        else {
            // The file-open completion will request the diff once the buffer is ready.
            return;
        };
        self.decoration_request = self.decoration_request.wrapping_add(1);
        let request = self.decoration_request;
        let root = self.root.clone();
        let diff_path = path.clone();
        let change = self
            .changes
            .iter()
            .find(|change| change.path == path)
            .cloned();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let (text, diff_text, diff_rows, original, mut lines, max_chars, marks) =
                        executor
                            .spawn(async move {
                                let diff_text =
                                    project::diff(&root, &diff_path).unwrap_or_else(|error| error);
                                let diff_rows = aligned_diff(&diff_text);
                                let original = change.as_ref().map_or_else(String::new, |change| {
                                    project::head_file(&root, change).unwrap_or_else(|error| {
                                        format!("No se pudo leer HEAD: {error}")
                                    })
                                });
                                let max_chars = max_line_chars(&original);
                                let lines = original
                                    .split('\n')
                                    .map(highlight::HighlightedLine::plain)
                                    .collect::<Vec<_>>();
                                let marks = line_diff_highlights(&original, &text);
                                (
                                    text, diff_text, diff_rows, original, lines, max_chars, marks,
                                )
                            })
                            .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.decoration_request != request
                            || this.selected.as_ref() != Some(&path)
                            || !(this.show_diff || this.pending_diff_path.as_ref() == Some(&path))
                        {
                            return;
                        }
                        let Some(index) = this
                            .tabs
                            .iter()
                            .position(|tab| tab.path == path && !tab.loading)
                        else {
                            return;
                        };
                        let tab = &this.tabs[index];
                        if tab.buffer.text() != text {
                            this.load_diff_async(cx);
                            return;
                        }
                        // Matching lines already have the correct syntax colors in the editor.
                        // Reuse them so the HEAD pane never needs a second full repaint.
                        for row in &marks.layout {
                            if let (Some(before), Some(after)) = (row.before, row.after) {
                                if let (Some(line), Some(colored)) =
                                    (lines.get_mut(before), tab.lines.get(after))
                                {
                                    if line.text() == colored.text() {
                                        *line = colored.clone();
                                    }
                                }
                            }
                        }
                        this.diff_text = diff_text;
                        this.diff_rows = diff_rows;
                        this.original_max_chars = max_chars;
                        this.original_lines = lines;
                        this.original_text = original;
                        this.original_buffer =
                            buffer::EditorBuffer::new(this.original_text.clone());
                        this.original_path = Some(path.clone());
                        this.diff_highlights = marks;
                        if this.pending_diff_path.as_ref() == Some(&path) {
                            this.activate_tab(Some(index));
                        }
                        this.pending_diff_path = None;
                        this.show_diff = true;
                        this.follow_blame_cursor(cx);
                        if this.side_by_side {
                            this.scroll_to_first_change();
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    /// Keep Git subprocesses, syntax parsing and line matching off the UI thread on open.
    pub(super) fn load_change_decorations_for_open(&mut self, cx: &mut Context<Self>) {
        self.decoration_request = self.decoration_request.wrapping_add(1);
        let request = self.decoration_request;
        let Some(path) = self.selected.clone() else {
            return;
        };
        let Some(change) = self
            .changes
            .iter()
            .find(|change| change.path == path)
            .cloned()
        else {
            self.load_change_decorations();
            return;
        };
        let Some(text) = self
            .active
            .and_then(|index| self.tabs.get(index))
            .filter(|tab| tab.path == path && !tab.loading)
            .map(|tab| tab.buffer.text().to_owned())
        else {
            return;
        };
        self.original_path = None;
        self.diff_highlights = DiffHighlights::default();
        let root = self.root.clone();
        let syntax_path = highlight::syntax_path(
            &path,
            self.language_overrides.get(&path).map(String::as_str),
        );
        let expected_change = change.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let (text, original, lines, max_chars, marks) = executor
                        .spawn(async move {
                            let original = project::head_file(&root, &change)
                                .unwrap_or_else(|error| format!("No se pudo leer HEAD: {error}"));
                            let lines = highlight::line(&original, &syntax_path);
                            let max_chars = max_line_chars(&original);
                            let marks = line_diff_highlights(&original, &text);
                            (text, original, lines, max_chars, marks)
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.decoration_request != request
                            || this.selected.as_ref() != Some(&path)
                            || this.show_diff
                            || !this.changes.contains(&expected_change)
                        {
                            return;
                        }
                        let Some(tab) = this
                            .active
                            .and_then(|index| this.tabs.get(index))
                            .filter(|tab| tab.path == path && !tab.loading)
                        else {
                            return;
                        };
                        // The buffer can change while Git is running; never install stale marks.
                        if tab.buffer.text() != text {
                            this.load_change_decorations_for_open(cx);
                            return;
                        }
                        this.original_max_chars = max_chars;
                        this.original_lines = lines;
                        this.original_text = original;
                        this.original_buffer =
                            buffer::EditorBuffer::new(this.original_text.clone());
                        this.original_path = Some(path);
                        this.diff_highlights = marks;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

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
            Err(error) => {
                self.show_git_error(if staged { "Unstage" } else { "Stage" }, error.clone());
                error
            }
        };
        self.refresh(cx);
        cx.notify();
    }

    pub(super) fn load_diff(&mut self) {
        self.decoration_request = self.decoration_request.wrapping_add(1);
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
        self.original_lines = self.selected.as_ref().map_or_else(Vec::new, |path| {
            let syntax_path =
                highlight::syntax_path(path, self.language_overrides.get(path).map(String::as_str));
            highlight::line(&original, &syntax_path)
        });
        self.original_text = original;
        if self.original_path != self.selected || self.original_buffer.text() != self.original_text
        {
            self.original_buffer = buffer::EditorBuffer::new(self.original_text.clone());
        }
        self.original_path = self.selected.clone();
        self.update_diff_highlights();
    }

    pub(super) fn load_change_decorations(&mut self) {
        // A pending diff owns these decorations; a status refresh must not cancel it.
        if self.pending_diff_path.as_ref() == self.selected.as_ref()
            && self.pending_diff_path.is_some()
        {
            return;
        }
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
            .filter(|tab| {
                !tab.loading
                    && self.selected.as_ref() == Some(&tab.path)
                    && self.original_path.as_ref() == Some(&tab.path)
            })
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
            Err(e) => {
                self.show_git_error(op, e.clone());
                e
            }
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
                            Err(error) => this.show_git_error("cargar branches", error),
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
                    let (label, result, pr_url) = executor
                        .spawn(async move {
                            let (label, result) = match operation {
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
                            };
                            let pr_url = if result.is_ok() && matches!(label, "Push" | "Sync") {
                                project::pull_request_url(&root)
                            } else {
                                None
                            };
                            (label, result, pr_url)
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.git_busy = false;
                        this.git_progress_offset = 0.;
                        if label == "Commit" && result.is_ok() {
                            this.commit_message.set_text(String::new());
                        }
                        this.message = match result {
                            Ok(()) => {
                                if let Some(url) = pr_url {
                                    this.notice = Some(Notice {
                                        text: "Branch publicado. ¿Querés crear un pull request?"
                                            .into(),
                                        action: NoticeAction::PullRequest(url),
                                    });
                                }
                                format!("{label} completado")
                            }
                            Err(error) => {
                                if label == "Cambiar branch" && !this.palette_open {
                                    this.palette_mode = PaletteMode::Branches;
                                    this.palette_open = true;
                                    this.palette_error = Some(error.clone());
                                }
                                this.show_git_error(label, error.clone());
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
