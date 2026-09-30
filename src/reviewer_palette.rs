//! Command and file palette state.
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum PaletteMode {
    Files,
    Commands,
    Branches,
    BranchName(Option<String>),
    BranchSource,
}

#[derive(Clone)]
pub(super) enum PaletteItem {
    CreateBranch,
    CreateBranchFrom,
    Branch(String),
    CommandBranches,
    CommandSettings,
}

pub(super) fn palette_items(
    mode: &PaletteMode,
    query: &str,
    branches: &[String],
) -> Vec<PaletteItem> {
    let needle = query.trim().to_lowercase();
    match mode {
        PaletteMode::Commands => [
            ("Branches", PaletteItem::CommandBranches),
            ("Settings", PaletteItem::CommandSettings),
        ]
        .into_iter()
        .filter(|(name, _)| name.to_lowercase().contains(&needle))
        .map(|(_, item)| item)
        .collect(),
        PaletteMode::Branches => {
            let mut actions = Vec::new();
            if "create new branch".contains(&needle) {
                actions.push(PaletteItem::CreateBranch);
            }
            if "create new branch from".contains(&needle) {
                actions.push(PaletteItem::CreateBranchFrom);
            }
            let mut matches: Vec<_> = branches
                .iter()
                .filter(|branch| branch.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            if !needle.is_empty() {
                matches.sort_by_key(|branch| !branch.to_lowercase().starts_with(&needle));
            }
            let mut items: Vec<_> = matches.into_iter().map(PaletteItem::Branch).collect();
            if needle.is_empty() {
                actions.extend(items);
                return actions;
            }
            items.extend(actions);
            items
        }
        PaletteMode::BranchSource => {
            let mut matches: Vec<_> = branches
                .iter()
                .filter(|branch| branch.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            if !needle.is_empty() {
                matches.sort_by_key(|branch| !branch.to_lowercase().starts_with(&needle));
            }
            matches.into_iter().map(PaletteItem::Branch).collect()
        }
        _ => Vec::new(),
    }
}

impl Reviewer {
    pub(super) fn update_query(&mut self) {
        if self.palette_mode == PaletteMode::Files && self.palette_query.text.starts_with('>') {
            self.palette_mode = PaletteMode::Commands;
            self.palette_query
                .set_text(self.palette_query.text[1..].trim_start().to_owned());
        } else if self.palette_mode == PaletteMode::Commands
            && self.palette_query.text.starts_with('>')
        {
            self.palette_query
                .set_text(self.palette_query.text[1..].trim_start().to_owned());
        }
        if self.palette_mode != PaletteMode::Files {
            self.palette_selected = 0;
            self.palette_scroll.scroll_to_item(0, ScrollStrategy::Top);
            return;
        }
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

    pub(super) fn choose_palette(&mut self, chosen: Option<PathBuf>, cx: &mut Context<Self>) {
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

    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_mode = PaletteMode::Files;
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

    pub(super) fn open_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_mode = PaletteMode::Commands;
        self.palette_query.set_text(String::new());
        self.palette_selected = 0;
        self.files_focused = false;
        self.close_find();
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn palette_entries(&self) -> Vec<PaletteItem> {
        palette_items(&self.palette_mode, &self.palette_query.text, &self.branches)
    }

    pub(super) fn select_palette_item(&mut self, item: PaletteItem, cx: &mut Context<Self>) {
        match item {
            PaletteItem::CommandBranches => {
                self.palette_mode = PaletteMode::Branches;
                self.palette_query.set_text(String::new());
                self.load_branches(cx);
            }
            PaletteItem::CommandSettings => {
                self.palette_open = false;
                self.settings_open = true;
            }
            PaletteItem::CreateBranch => {
                self.palette_mode = PaletteMode::BranchName(None);
                self.palette_query.set_text(String::new());
            }
            PaletteItem::CreateBranchFrom => {
                self.palette_mode = PaletteMode::BranchSource;
                self.palette_query.set_text(String::new());
            }
            PaletteItem::Branch(name) if self.palette_mode == PaletteMode::BranchSource => {
                self.palette_mode = PaletteMode::BranchName(Some(name));
                self.palette_query.set_text(String::new());
            }
            PaletteItem::Branch(name) => self.git_operation(GitOperation::SwitchBranch(name), cx),
        }
        self.palette_selected = 0;
        self.palette_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }
}
