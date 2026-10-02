//! OpenCode sessions and historical snapshot diffs. External I/O stays off the UI thread.
use super::*;

pub(super) struct SessionDiff {
    pub(super) session_id: String,
    pub(super) path: String,
    pub(super) before: Vec<highlight::HighlightedLine>,
    pub(super) after: Vec<highlight::HighlightedLine>,
    pub(super) before_chars: usize,
    pub(super) after_chars: usize,
    pub(super) marks: DiffHighlights,
    pub(super) error: Option<String>,
}

fn scroll_drag_offset(
    pointer: Pixels,
    origin: Pixels,
    height: Pixels,
    thumb: Pixels,
    grab: Pixels,
    max: Pixels,
) -> Pixels {
    let fraction = ((pointer - origin - grab) / (height - thumb).max(px(1.))).clamp(0., 1.);
    -max * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_drag_preserves_grab_point_and_reaches_both_ends() {
        let offset = |pointer| {
            scroll_drag_offset(px(pointer), px(100.), px(200.), px(40.), px(10.), px(600.))
        };
        assert_eq!(offset(110.), px(0.));
        assert_eq!(offset(190.), px(-300.));
        assert_eq!(offset(270.), px(-600.));
        assert_eq!(offset(300.), px(-600.));
    }
}

impl Reviewer {
    pub(super) fn load_opencode(&mut self, cx: &mut Context<Self>) {
        if self.opencode_loading {
            return;
        }
        self.opencode_loading = true;
        self.opencode_progress = 0.;
        self.opencode_error = None;
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move { opencode::sessions(&root) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.opencode_loading = false;
                        match result {
                            Ok(sessions) => {
                                this.opencode_sessions = sessions;
                                this.opencode_error = None;
                            }
                            Err(error) => this.opencode_error = Some(error),
                        }
                        cx.notify();
                    });
                    gpui::Timer::after(Duration::from_millis(40)).await;
                    let _ = weak.update(&mut cx, |_, cx| cx.notify());
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn select_opencode(&mut self, id: String, cx: &mut Context<Self>) {
        if self.opencode_selected.as_ref() == Some(&id) {
            self.opencode_selected = None;
            self.opencode_files_loading = false;
            cx.notify();
            return;
        }
        self.opencode_selected = Some(id.clone());
        self.opencode_files_loading = true;
        self.opencode_error = None;
        let Some(session) = self
            .opencode_sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
        else {
            return;
        };
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move { opencode::files(&session) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.opencode_selected.as_deref() != Some(&id) {
                            return;
                        }
                        this.opencode_files_loading = false;
                        match result {
                            Ok(files) => {
                                if let Some(session) = this
                                    .opencode_sessions
                                    .iter_mut()
                                    .find(|session| session.id == id)
                                {
                                    session.files = files;
                                }
                                this.opencode_error = None;
                            }
                            Err(error) => this.opencode_error = Some(error),
                        }
                        cx.notify();
                    });
                    gpui::Timer::after(Duration::from_millis(40)).await;
                    let _ = weak.update(&mut cx, |_, cx| cx.notify());
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn open_opencode_diff(&mut self, id: String, path: String, cx: &mut Context<Self>) {
        let Some(session) = self
            .opencode_sessions
            .iter()
            .find(|session| session.id == id)
            .cloned()
        else {
            return;
        };
        self.opencode_diff_request = self.opencode_diff_request.wrapping_add(1);
        let request = self.opencode_diff_request;
        self.opencode_before_scroll = UniformListScrollHandle::new();
        self.opencode_after_scroll = UniformListScrollHandle::new();
        self.opencode_diff = Some(SessionDiff {
            session_id: id,
            path: path.clone(),
            before: Vec::new(),
            after: Vec::new(),
            before_chars: 0,
            after_chars: 0,
            marks: DiffHighlights::default(),
            error: None,
        });
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move {
                            let (before, after) = opencode::file_versions(&session, &path)?;
                            let marks = line_diff_highlights(&before, &after);
                            let syntax_path = Path::new(&path);
                            Ok::<_, String>((
                                max_line_chars(&before),
                                max_line_chars(&after),
                                highlight::line(&before, syntax_path),
                                highlight::line(&after, syntax_path),
                                marks,
                            ))
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.opencode_diff_request != request {
                            return;
                        }
                        if let Some(diff) = this.opencode_diff.as_mut() {
                            match result {
                                Ok((before_chars, after_chars, before, after, marks)) => {
                                    if let Some(first) = marks.first_visual() {
                                        this.opencode_before_scroll
                                            .scroll_to_item(first, ScrollStrategy::Center);
                                        this.opencode_after_scroll
                                            .scroll_to_item(first, ScrollStrategy::Center);
                                    }
                                    diff.before = before;
                                    diff.after = after;
                                    diff.before_chars = before_chars;
                                    diff.after_chars = after_chars;
                                    diff.marks = marks;
                                }
                                Err(error) => diff.error = Some(error),
                            }
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn refresh_quota(&mut self, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor.spawn(async { opencode::quota() }).await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        match result {
                            Ok(quota) => {
                                this.openai_quota = Some(quota);
                                this.openai_quota_error = None;
                            }
                            Err(error) => {
                                this.openai_quota = None;
                                this.openai_quota_error = Some(error);
                            }
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    pub(super) fn drag_opencode_scrollbar(&mut self, pointer: Pixels, cx: &mut Context<Self>) {
        let Some(grab) = self.opencode_scroll_grab else {
            return;
        };
        let bounds = self.opencode_scroll.bounds();
        let height = bounds.size.height;
        let max = self.opencode_scroll.max_offset().height;
        let (thumb, _) = scrollbar_thumb(height, height, max, px(0.));
        self.opencode_scroll.set_offset(point(
            px(0.),
            scroll_drag_offset(pointer, bounds.origin.y, height, thumb, grab, max),
        ));
        cx.notify();
    }

    pub(super) fn opencode_diff_side_line(
        &self,
        index: usize,
        original: bool,
        width: Pixels,
    ) -> gpui::Div {
        let diff = self.opencode_diff.as_ref().expect("diff visible");
        let row = &diff.marks.layout[index];
        let (number, text, range, changed) = if original {
            (
                row.before,
                &diff.before,
                row.before_range.clone(),
                row.before.is_some_and(|n| diff.marks.removed.contains(&n)),
            )
        } else {
            (
                row.after,
                &diff.after,
                row.after_range.clone(),
                row.after.is_some_and(|n| diff.marks.added.contains(&n)),
            )
        };
        let gere = self.settings.is_gere();
        let bg = if number.is_none() {
            diff_view::ghost(gere)
        } else {
            diff_view::pane_background(gere, self.settings.background(), changed, original)
        };
        div()
            .w(width)
            .h(px((self.settings.font_size as f32 + 8.).max(22.)))
            .bg(rgb(bg))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .relative()
                    .text_color(rgb(if changed {
                        if original {
                            0xee938e
                        } else {
                            0x9ad7ae
                        }
                    } else {
                        0x5c6370
                    }))
                    .child(number.map(|n| format!("{:>5}", n + 1)).unwrap_or_default())
                    .when(changed, |gutter| {
                        gutter.child(div().absolute().left_0().child(if original {
                            "−"
                        } else {
                            "+"
                        }))
                    }),
            )
            .when_some(number.and_then(|n| text.get(n)), |v, line| {
                v.child(line.render_editor(
                    None,
                    &[],
                    range.map(|range| (range, diff_view::changed_text(gere, original))),
                    &[],
                ))
            })
    }

    pub(super) fn opencode_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let gere = self.settings.is_gere();
        let selected = self.opencode_selected.as_deref();
        let bounds = self.opencode_scroll.bounds();
        let max = self.opencode_scroll.max_offset().height;
        let viewport = bounds.size.height;
        let (thumb, position) =
            scrollbar_thumb(viewport, viewport, max, -self.opencode_scroll.offset().y);
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(38.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("SESIONES · {}", self.opencode_sessions.len())),
                    )
                    .child(Self::icon_button(
                        "refresh-cw",
                        "Actualizar sesiones",
                        cx.listener(|this, _, _, cx| this.load_opencode(cx)),
                    )),
            )
            .when(self.opencode_loading, |v| {
                v.child(
                    div()
                        .relative()
                        .h(px(3.))
                        .mx_2()
                        .bg(rgb(0x3e4451))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left(
                                    (self.sidebar_width - px(46.)).max(px(0.))
                                        * self.opencode_progress,
                                )
                                .w(px(46.))
                                .h_full()
                                .bg(rgb(0x61afef)),
                        ),
                )
            })
            .when_some(self.opencode_error.as_ref(), |v, error| {
                v.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(rgb(0xe06c75))
                        .whitespace_normal()
                        .child(error.clone()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        div()
                            .id("opencode-sessions")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.opencode_scroll)
                            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                            .flex()
                            .flex_col()
                            .pb_2()
                            .when(
                                self.opencode_sessions.is_empty()
                                    && !self.opencode_loading
                                    && self.opencode_error.is_none(),
                                |v| {
                                    v.child(
                                        div()
                                            .px_3()
                                            .py_2()
                                            .text_color(rgb(MUTED))
                                            .child("No hay sesiones en este proyecto"),
                                    )
                                },
                            )
                            .children(self.opencode_sessions.iter().map(|session| {
                                let id = session.id.clone();
                                let expanded = selected == Some(id.as_str());
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .w_full()
                                            .min_h(px(36.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .cursor_pointer()
                                            .bg(rgb(if expanded {
                                                if gere {
                                                    0x22252e
                                                } else {
                                                    0x343a44
                                                }
                                            } else {
                                                self.settings.panel()
                                            }))
                                            .hover(|s| s.bg(rgb(0x3e4451)))
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.select_opencode(id.clone(), cx)
                                                }),
                                            )
                                            .child(
                                                icons::icon(
                                                    if expanded {
                                                        "chevron-down"
                                                    } else {
                                                        "chevron-right"
                                                    },
                                                    MUTED,
                                                )
                                                .size(px(14.)),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .whitespace_normal()
                                                    .py_1()
                                                    .text_sm()
                                                    .child(session.title.clone()),
                                            ),
                                    )
                                    .when(expanded && self.opencode_error.is_none(), |v| {
                                        v.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .pl_3()
                                                .pb_2()
                                                .child(
                                                    div()
                                                        .pl_2()
                                                        .py_1()
                                                        .text_xs()
                                                        .text_color(rgb(MUTED))
                                                        .child(
                                                            "Branch: no registrada por OpenCode",
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .h(px(26.))
                                                        .flex()
                                                        .items_center()
                                                        .text_xs()
                                                        .text_color(rgb(MUTED))
                                                        .child(format!(
                                                            "ARCHIVOS MODIFICADOS ({})",
                                                            session.files.len()
                                                        )),
                                                )
                                                .when(self.opencode_files_loading, |v| {
                                                    v.child(
                                                        div()
                                                            .pl_3()
                                                            .text_xs()
                                                            .text_color(rgb(MUTED))
                                                            .child("Cargando archivos…"),
                                                    )
                                                })
                                                .when(
                                                    session.files.is_empty()
                                                        && !self.opencode_files_loading,
                                                    |v| {
                                                        v.child(
                                                            div()
                                                                .pl_3()
                                                                .text_xs()
                                                                .text_color(rgb(MUTED))
                                                                .child("Sin cambios registrados"),
                                                        )
                                                    },
                                                )
                                                .children(session.files.iter().map(|file| {
                                                    let id = session.id.clone();
                                                    let path = file.clone();
                                                    let active = self
                                                        .opencode_diff
                                                        .as_ref()
                                                        .is_some_and(|diff| {
                                                            diff.session_id == id
                                                                && diff.path == path
                                                        });
                                                    div()
                                                        .h(px(28.))
                                                        .w_full()
                                                        .pl_2()
                                                        .pr_2()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .cursor_pointer()
                                                        .bg(rgb(if active {
                                                            0x3e4451
                                                        } else {
                                                            self.settings.panel()
                                                        }))
                                                        .hover(|s| s.bg(rgb(0x3e4451)))
                                                        .on_mouse_up(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                this.open_opencode_diff(
                                                                    id.clone(),
                                                                    path.clone(),
                                                                    cx,
                                                                )
                                                            }),
                                                        )
                                                        .child(
                                                            icons::file_icon(Path::new(file))
                                                                .size(px(16.)),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .overflow_hidden()
                                                                .whitespace_nowrap()
                                                                .text_xs()
                                                                .child(file.clone()),
                                                        )
                                                })),
                                        )
                                    })
                            })),
                    )
                    .when(max > px(0.), |v| {
                        v.child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .w(px(9.))
                                .h_full()
                                .bg(rgb(0x2c313a))
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                        let bounds = this.opencode_scroll.bounds();
                                        let height = bounds.size.height;
                                        let max = this.opencode_scroll.max_offset().height;
                                        let (thumb, position) = scrollbar_thumb(
                                            height,
                                            height,
                                            max,
                                            -this.opencode_scroll.offset().y,
                                        );
                                        let top = bounds.origin.y + position;
                                        this.opencode_scroll_grab = Some(
                                            if event.position.y >= top
                                                && event.position.y <= top + thumb
                                            {
                                                event.position.y - top
                                            } else {
                                                thumb / 2.
                                            },
                                        );
                                        this.drag_opencode_scrollbar(event.position.y, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top(position)
                                        .right_0()
                                        .w_full()
                                        .h(thumb)
                                        .rounded_sm()
                                        .bg(rgb(0x7f848e)),
                                ),
                        )
                    }),
            )
    }
}
