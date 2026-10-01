//! Shared controls and dialogs used by the reviewer renderer.
use super::*;

pub(super) struct IconTooltip(pub(super) &'static str);

impl Render for IconTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(0x3e4451))
            .text_color(rgb(FG))
            .child(self.0)
    }
}

pub(super) struct DiagnosticTooltip {
    pub(super) diagnostics: Vec<highlight::Diagnostic>,
}

impl Render for DiagnosticTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(500.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(0x3e4451))
            .text_color(rgb(FG))
            .flex()
            .flex_col()
            .gap_1()
            .children(self.diagnostics.iter().map(|diagnostic| {
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .text_color(rgb(diagnostic.severity.color()))
                            .child(diagnostic.severity.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(diagnostic.message.clone()),
                    )
            }))
    }
}

impl Reviewer {
    pub(super) fn change_row(
        &self,
        index: usize,
        staged: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let change = &self.changes[index];
        let path = change.path.clone();
        let is_selected = self.selected.as_ref() == Some(&path);
        let gere = self.settings.is_gere();
        let (added, removed) = self.change_counts.get(index).copied().unwrap_or_default();
        div()
            .h(px((self.settings.git_font_size as f32 + 14.).max(26.)))
            .w_full()
            .pl(px(18.))
            .pr_1()
            .cursor_pointer()
            .bg(rgb(if is_selected {
                if gere {
                    0x22252e
                } else {
                    0x3e4451
                }
            } else {
                self.settings.panel()
            }))
            .text_color(rgb(if gere { 0xd7dce2 } else { FG }))
            .text_size(px(self.settings.git_font_size as f32))
            .hover(move |style| style.bg(rgb(if gere { 0x22252e } else { 0x3e4451 })))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.commit_focused = false;
                    let show_diff = !this.changes.iter().any(|change| {
                        change.path == path
                            && (change.index == 'U'
                                || change.worktree == 'U'
                                || (change.index == 'A' && change.worktree == 'A')
                                || (change.index == 'D' && change.worktree == 'D'))
                    });
                    if show_diff {
                        this.pending_diff_path = Some(path.clone());
                    }
                    this.open(path.clone(), cx);
                    this.sidebar = Sidebar::Git;
                    this.side_by_side = true;
                    if show_diff {
                        this.load_diff_async(cx);
                    } else {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener({
                    let path = change.path.clone();
                    move |this, _, _, cx| {
                        this.open(path.clone(), cx);
                        this.sidebar = Sidebar::Git;
                    }
                }),
            )
            .flex()
            .items_center()
            .gap_1()
            .child(icons::file_icon(&change.path).size(px(16.)))
            .child(
                div().flex_1().min_w_0().h_full().overflow_hidden().child(
                    change
                        .path
                        .file_name()
                        .unwrap_or(change.path.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
            .child(
                div()
                    .text_color(rgb(if gere { 0x7bcb9d } else { 0x9ad7ae }))
                    .child(format!("+{added}")),
            )
            .child(
                div()
                    .text_color(rgb(if gere { 0xe07a82 } else { 0xee938e }))
                    .child(format!("-{removed}")),
            )
            .child(div().ml_1().child(Self::icon_button(
                if staged { "minus" } else { "plus" },
                if staged { "Unstage" } else { "Stage" },
                cx.listener({
                    let path = change.path.clone();
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stage_row(&path, staged, cx);
                    }
                }),
            )))
    }

    pub(super) fn button(
        label: impl Into<String>,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(PANEL))
            .text_color(rgb(FG))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x3e4451)))
            .on_mouse_up(MouseButton::Left, click)
            .child(label.into())
    }

    pub(super) fn icon_button(
        name: &'static str,
        label: &'static str,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        Self::icon_button_sized(name, label, px(22.), false, None, click)
    }

    pub(super) fn icon_button_sized(
        name: &'static str,
        label: &'static str,
        size: Pixels,
        active: bool,
        badge: Option<usize>,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(label)
            .relative()
            .size(size)
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .when(active, |button| button.bg(rgb(0x3e4451)))
            .hover(|s| s.bg(rgb(0x3e4451)))
            .on_hover(|hovered, window, _| {
                if *hovered {
                    window.refresh();
                }
            })
            .tooltip(move |_, cx| cx.new(|_| IconTooltip(label)).into())
            .on_mouse_up(MouseButton::Left, click)
            .child(icons::icon(name, if size == px(38.) { FG } else { MUTED }))
            .when_some(badge.filter(|count| *count > 0), |button, count| {
                button.child(
                    div()
                        .absolute()
                        .right(px(0.))
                        .bottom(px(0.))
                        .min_w(px(18.))
                        .h(px(18.))
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(0x61afef))
                        .text_color(rgb(0x181a1f))
                        .text_size(px(10.))
                        .child(count.to_string()),
                )
            })
    }

    pub(super) fn menu_item(
        label: &'static str,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .w_full()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(FG))
            .hover(|s| s.bg(rgb(0x3e4451)))
            .on_mouse_up(MouseButton::Left, click)
            .child(label)
    }

    pub(super) fn alert_dialog(
        title: &'static str,
        detail: String,
        confirm: impl IntoElement,
        cancel: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .absolute()
            .top(px(0.))
            .left(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x101116aa))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .w(px(430.))
                    .max_w_full()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(0xee938e))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(rgb(0xd7dae0)).child(title))
                    .child(detail)
                    .child(div().flex().gap_2().child(confirm).child(cancel)),
            )
    }

    pub(super) fn option(
        icon: &'static str,
        label: &'static str,
        enabled: bool,
        click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(label)
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(if enabled { 0x3e4451 } else { BG }))
            .cursor_pointer()
            .on_hover(|hovered, window, _| {
                if *hovered {
                    window.refresh();
                }
            })
            .tooltip(move |_, cx| cx.new(|_| IconTooltip(label)).into())
            .on_mouse_up(MouseButton::Left, click)
            .child(icons::icon(icon, if enabled { FG } else { MUTED }))
    }
}
