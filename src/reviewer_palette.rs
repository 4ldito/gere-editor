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

    pub(super) fn palette_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
        panel: u32,
        background: u32,
        font_name: &'static str,
        cell_width: Pixels,
    ) -> gpui::Div {
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(75.))
            .bg(rgba(0x101116aa))
            .occlude()
            .child(
                div()
                    .w(px(620.))
                    .max_w_full()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(0x3e4451))
                    .bg(rgb(panel))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().px_2().py_1().text_xs().text_color(rgb(MUTED)).child(
                        match &self.palette_mode {
                            PaletteMode::Files => "ABRIR ARCHIVO  ·  CTRL+P",
                            PaletteMode::Commands => "COMANDOS  ·  CTRL+SHIFT+P",
                            PaletteMode::Branches => "BRANCHES",
                            PaletteMode::BranchSource => "CREAR BRANCH DESDE…",
                            PaletteMode::BranchName(_) => "NOMBRE DE LA NUEVA BRANCH",
                        },
                    ))
                    .child(
                        input_view(
                            &self.palette_query,
                            match &self.palette_mode {
                                PaletteMode::Files => "Buscar archivos o > comandos…",
                                PaletteMode::Commands => "Buscar comandos…",
                                PaletteMode::Branches => "Buscar branches…",
                                PaletteMode::BranchSource => "Elegir branch de origen…",
                                PaletteMode::BranchName(_) => "Nombre de branch…",
                            },
                            true,
                            self.focus.is_focused(window) && self.cursor_blink_visible,
                            65,
                            cell_width,
                            background,
                            false,
                        )
                        .font_family(font_name)
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, window, _| window.focus(&this.focus)),
                        ),
                    )
                    .when(
                        self.branch_menu_loading
                            && matches!(
                                self.palette_mode,
                                PaletteMode::Branches | PaletteMode::BranchSource
                            ),
                        |view| {
                            view.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_color(rgb(MUTED))
                                    .child("Cargando branches…"),
                            )
                        },
                    )
                    .when(
                        !matches!(self.palette_mode, PaletteMode::BranchName(_)),
                        |view| {
                            view.child(
                                uniform_list(
                                    "quick-open",
                                    if self.palette_mode == PaletteMode::Files {
                                        self.quick.len()
                                    } else {
                                        self.palette_entries().len()
                                    },
                                    cx.processor(
                                        move |this, range: std::ops::Range<usize>, _, cx| {
                                            let items = this.palette_entries();
                                            range
                                                .map(|i| {
                                                    if this.palette_mode != PaletteMode::Files {
                                                        let item = items[i].clone();
                                                        let label = match &item {
                                                            PaletteItem::CreateBranch => {
                                                                "Create New Branch".to_owned()
                                                            }
                                                            PaletteItem::CreateBranchFrom => {
                                                                "Create New Branch From…".to_owned()
                                                            }
                                                            PaletteItem::Branch(name) => {
                                                                name.clone()
                                                            }
                                                            PaletteItem::CommandBranches => {
                                                                "Branches".to_owned()
                                                            }
                                                            PaletteItem::CommandSettings => {
                                                                "Settings".to_owned()
                                                            }
                                                        };
                                                        let separator = this.palette_mode
                                                            == PaletteMode::Branches
                                                            && i > 0
                                                            && matches!(
                                                                items[i - 1],
                                                                PaletteItem::Branch(_)
                                                            ) != matches!(
                                                                item,
                                                                PaletteItem::Branch(_)
                                                            );
                                                        return div()
                                                            .h(px(28.))
                                                            .px_2()
                                                            .w_full()
                                                            .flex()
                                                            .items_center()
                                                            .cursor_pointer()
                                                            .bg(rgb(
                                                                if i == this.palette_selected {
                                                                    0x3e4451
                                                                } else {
                                                                    panel
                                                                },
                                                            ))
                                                            .text_color(rgb(FG))
                                                            .when(separator, |row| {
                                                                row.border_t_1()
                                                                    .border_color(rgb(0x4b5261))
                                                            })
                                                            .hover(|style| style.bg(rgb(0x3e4451)))
                                                            .on_mouse_up(
                                                                MouseButton::Left,
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.select_palette_item(
                                                                            item.clone(),
                                                                            cx,
                                                                        );
                                                                    },
                                                                ),
                                                            )
                                                            .child(label);
                                                    }
                                                    let path = this.quick[i].clone();
                                                    div()
                                                        .h(px(28.))
                                                        .px_2()
                                                        .overflow_hidden()
                                                        .cursor_pointer()
                                                        .bg(rgb(if i == this.palette_selected {
                                                            0x3e4451
                                                        } else {
                                                            panel
                                                        }))
                                                        .text_color(rgb(FG))
                                                        .hover(|s| s.bg(rgb(0x3e4451)))
                                                        .on_mouse_up(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                this.choose_palette(
                                                                    Some(path.clone()),
                                                                    cx,
                                                                );
                                                            }),
                                                        )
                                                        .flex()
                                                        .items_center()
                                                        .gap_2()
                                                        .child(icons::file_icon(&this.quick[i]))
                                                        .child(this.quick[i].display().to_string())
                                                })
                                                .collect::<Vec<_>>()
                                        },
                                    ),
                                )
                                .track_scroll(self.palette_scroll.clone())
                                .h(px(240.)),
                            )
                        },
                    ),
            )
    }
}
