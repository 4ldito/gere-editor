//! Terminal panel UI and focus/keyboard routing.
use super::*;
use gpui::{HighlightStyle, StyledText};

#[derive(Clone)]
pub(super) struct TerminalTabDrag(pub(super) String);

impl Render for TerminalTabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(0x3e4451))
            .text_color(rgb(FG))
            .shadow_md()
            .child(self.0.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalMatch {
    pub(super) line: usize,
    pub(super) columns: std::ops::Range<u16>,
}

fn line_matches(screen: &vt100::Screen, row: u16, query: &str, line: usize) -> Vec<TerminalMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut text = String::new();
    let mut columns = Vec::new();
    for col in 0..screen.size().1 {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let contents = if cell.has_contents() {
            cell.contents()
        } else {
            " "
        };
        for ch in contents.chars() {
            columns.push((text.len(), col));
            text.push(ch);
        }
    }
    columns.push((text.len(), screen.size().1));
    let lowered = text.to_lowercase();
    lowered
        .match_indices(query)
        .filter_map(|(start, _)| {
            let first = columns.iter().find(|(byte, _)| *byte == start)?.1;
            let end = columns
                .iter()
                .find(|(byte, _)| *byte == start + query.len())?
                .1;
            Some(TerminalMatch {
                line,
                columns: first..end,
            })
        })
        .collect()
}

fn scan_terminal_matches(screen: &mut vt100::Screen, query: &str) -> (usize, Vec<TerminalMatch>) {
    let current = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let oldest = screen.scrollback();
    let mut matches = Vec::new();
    if query.is_empty() {
        screen.set_scrollback(current);
        return (oldest, matches);
    }
    let needs_cell_scan = query.chars().any(char::is_whitespace);
    let mut scan_row = |screen: &vt100::Screen, row, line| {
        if needs_cell_scan
            || screen
                .rows(0, screen.size().1)
                .nth(usize::from(row))
                .is_some_and(|text| text.to_lowercase().contains(query))
        {
            matches.extend(line_matches(screen, row, query, line));
        }
    };
    for offset in (0..=oldest).rev() {
        screen.set_scrollback(offset);
        scan_row(screen, 0, oldest - offset);
    }
    for row in 1..screen.size().0 {
        scan_row(screen, row, oldest + usize::from(row));
    }
    screen.set_scrollback(current);
    (oldest, matches)
}

fn terminal_scrollbar_geometry(
    viewport: f32,
    rows: usize,
    history: usize,
    offset: usize,
) -> (f32, f32) {
    let viewport = viewport.max(0.);
    let thumb = (viewport * rows as f32 / (rows + history).max(1) as f32)
        .max(22.)
        .min(viewport);
    let top = if history == 0 {
        0.
    } else {
        (viewport - thumb) * (history - offset.min(history)) as f32 / history as f32
    };
    (thumb, top)
}

fn terminal_scrollbar_offset(
    position: f32,
    grab: f32,
    viewport: f32,
    rows: usize,
    history: usize,
) -> usize {
    let (thumb, _) = terminal_scrollbar_geometry(viewport, rows, history, 0);
    let travel = viewport - thumb;
    if travel <= 0. {
        return 0;
    }
    let fraction = ((position - grab) / travel).clamp(0., 1.);
    history - (fraction * history as f32).round() as usize
}

fn selected_terminal_text(screen: &vt100::Screen, start: (u16, u16), end: (u16, u16)) -> String {
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    let mut lines = Vec::new();
    for row in start.0..=end.0 {
        let first = if row == start.0 { start.1 } else { 0 };
        let last = if row == end.0 { end.1 } else { screen.size().1 };
        let mut line = String::new();
        for col in first..last {
            if let Some(cell) = screen.cell(row, col) {
                if !cell.is_wide_continuation() {
                    if cell.has_contents() {
                        line.push_str(cell.contents());
                    } else {
                        line.push(' ');
                    }
                }
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    lines.join("\n")
}

impl Reviewer {
    fn focus_terminal(&mut self, window: &mut Window) {
        self.terminal_focused = true;
        self.terminal_find_open = false;
        self.find_has_focus = false;
        self.search_focused = false;
        self.commit_focused = false;
        self.original_focused = false;
        window.focus(&self.focus);
    }

    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_visible {
            self.terminal_visible = false;
            self.terminal_focused = false;
            self.terminal_find_open = false;
            self.terminal_shell_menu = false;
        } else {
            if self.terminals.is_empty() {
                self.new_terminal(cx);
            }
            self.terminal_visible = !self.terminals.is_empty();
            if self.terminal_visible {
                self.focus_terminal(window);
            }
        }
        cx.notify();
    }

    pub(super) fn new_terminal(&mut self, cx: &mut Context<Self>) {
        let id = self.next_terminal_id;
        let result = if let Some(shell) = self.terminal_shell.as_deref() {
            terminal::Terminal::with_shell(
                &self.root,
                format!("Terminal {id}"),
                self.terminal_size,
                Some(shell),
            )
        } else {
            terminal::Terminal::new(&self.root, format!("Terminal {id}"), self.terminal_size)
        };
        match result {
            Ok(terminal) => {
                self.next_terminal_id += 1;
                self.terminals.push(terminal);
                self.terminal_active = Some(self.terminals.len() - 1);
                self.terminal_visible = true;
                self.terminal_focused = true;
                self.terminal_revision = 0;
                self.terminal_selection = None;
            }
            Err(error) => self.message = format!("No se pudo abrir la terminal: {error}"),
        }
        cx.notify();
    }

    fn close_terminal(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        self.terminals.remove(index);
        self.terminal_active = match self.terminal_active {
            _ if self.terminals.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.terminals.len() - 1)),
            other => other,
        };
        if self.terminals.is_empty() {
            self.terminal_visible = false;
            self.terminal_focused = false;
        }
        self.terminal_revision = 0;
        self.terminal_selection = None;
        cx.notify();
    }

    pub(super) fn send_terminal_text(&self, text: &str, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i)) {
            let bracketed = if let Ok(mut parser) = terminal.screen.lock() {
                let screen = parser.screen_mut();
                let bracketed = screen.bracketed_paste();
                if screen.scrollback() > 0 {
                    screen.set_scrollback(0);
                    cx.notify();
                }
                bracketed
            } else {
                false
            };
            if bracketed && text.contains('\n') {
                terminal.send(format!("\x1b[200~{text}\x1b[201~").into_bytes());
            } else {
                terminal.send(text.as_bytes().to_vec());
            }
        }
    }

    pub(super) fn on_terminal_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if key.modifiers.control && key.key == "c" && !key.modifiers.shift {
            if let Some((start, end)) = self.terminal_selection.take() {
                if start != end {
                    if let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i))
                    {
                        if let Ok(parser) = terminal.screen.lock() {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                selected_terminal_text(parser.screen(), start, end),
                            ));
                        }
                    }
                    cx.notify();
                    return;
                }
            }
        }
        self.terminal_selection = None;
        let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i)) else {
            return;
        };
        if let Ok(mut parser) = terminal.screen.lock() {
            if parser.screen().scrollback() > 0 {
                parser.screen_mut().set_scrollback(0);
                cx.notify();
            }
        }
        if key.modifiers.control && key.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                if terminal
                    .screen
                    .lock()
                    .is_ok_and(|parser| parser.screen().bracketed_paste())
                {
                    terminal.send(format!("\x1b[200~{text}\x1b[201~").into_bytes());
                } else {
                    terminal.send(text.into_bytes());
                }
            }
            return;
        }
        let application_cursor = terminal
            .screen
            .lock()
            .is_ok_and(|parser| parser.screen().application_cursor());
        if let Some(bytes) = terminal::key_bytes(key, application_cursor) {
            terminal.send(bytes);
        }
    }

    fn scroll_terminal(&mut self, delta: f32, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i)) {
            if let Ok(mut parser) = terminal.screen.lock() {
                let screen = parser.screen_mut();
                let previous = screen.scrollback();
                let offset = if delta > 0. {
                    previous.saturating_add(delta.ceil() as usize)
                } else {
                    previous.saturating_sub((-delta).ceil() as usize)
                };
                screen.set_scrollback(offset);
                if previous != screen.scrollback() {
                    self.terminal_selection = None;
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn drag_terminal_scrollbar(
        &mut self,
        y: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let height = self
            .terminal_height
            .min((window.bounds().size.height - px(145.)).max(px(110.)));
        let viewport = f32::from(height) - 36.;
        let top = window.bounds().size.height - px(26.) - height + px(36.);
        if let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i)) {
            if let Ok(mut parser) = terminal.screen.lock() {
                let screen = parser.screen_mut();
                let previous = screen.scrollback();
                screen.set_scrollback(usize::MAX);
                let history = screen.scrollback();
                screen.set_scrollback(terminal_scrollbar_offset(
                    f32::from(y - top),
                    self.terminal_scroll_grab,
                    viewport,
                    usize::from(screen.size().0),
                    history,
                ));
                if screen.scrollback() != previous {
                    self.terminal_selection = None;
                    cx.notify();
                }
            }
        }
    }

    fn terminal_point(&self, position: gpui::Point<Pixels>, window: &Window) -> (u16, u16) {
        let font_size = self.settings.font_size as f32;
        let row_height = (font_size + 5.).max(18.);
        let height = self
            .terminal_height
            .min((window.bounds().size.height - px(145.)).max(px(110.)));
        let top = window.bounds().size.height - px(26.) - height + px(40.);
        let font_id = window
            .text_system()
            .resolve_font(&gpui::font(self.settings.font_name()));
        let cell_width = window
            .text_system()
            .ch_advance(font_id, px(font_size))
            .unwrap_or(px(8.4));
        let left = window.bounds().size.width
            - (window.bounds().size.width
                - px(RAIL_WIDTH)
                - if self.sidebar_visible {
                    sidebar_width(window.bounds().size.width, self.sidebar_width) + px(8.)
                } else {
                    px(0.)
                })
            + px(8.);
        let row = ((position.y - top) / px(row_height)).floor().max(0.) as u16;
        let col = ((position.x - left) / cell_width).round().max(0.) as u16;
        (
            row.min(self.terminal_size.0.saturating_sub(1)),
            col.min(self.terminal_size.1),
        )
    }

    pub(super) fn extend_terminal_selection(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.terminal_point(position, window);
        if let Some((_, current)) = self.terminal_selection.as_mut() {
            if *current != end {
                *current = end;
                cx.notify();
            }
        }
    }

    pub(super) fn refresh_terminal_find(&mut self, navigate: bool) {
        let query = self.terminal_find_query.text.to_lowercase();
        let previous = self
            .terminal_find_active
            .and_then(|index| self.terminal_find_matches.get(index))
            .cloned();
        self.terminal_find_matches.clear();
        self.terminal_find_active = None;
        if let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i)) {
            if let Ok(mut parser) = terminal.screen.lock() {
                let screen = parser.screen_mut();
                let current = screen.scrollback();
                let (oldest, matches) = scan_terminal_matches(screen, &query);
                self.terminal_find_scrollback = oldest;
                self.terminal_find_matches = matches;
                if !self.terminal_find_matches.is_empty() {
                    let visible_start = oldest - current;
                    let selected = if navigate {
                        self.terminal_find_matches
                            .iter()
                            .position(|found| found.line >= visible_start)
                            .unwrap_or(0)
                    } else {
                        previous
                            .and_then(|found| {
                                self.terminal_find_matches
                                    .iter()
                                    .position(|candidate| *candidate == found)
                            })
                            .or_else(|| {
                                self.terminal_find_matches
                                    .iter()
                                    .position(|found| found.line >= visible_start)
                            })
                            .unwrap_or(0)
                    };
                    self.terminal_find_active = Some(selected);
                    if navigate {
                        screen.set_scrollback(
                            oldest.saturating_sub(self.terminal_find_matches[selected].line),
                        );
                        self.terminal_selection = None;
                    }
                }
            }
        }
    }

    pub(super) fn move_terminal_find(&mut self, backwards: bool) {
        if self.terminal_find_matches.is_empty() {
            return;
        }
        let count = self.terminal_find_matches.len();
        let current = self.terminal_find_active.unwrap_or(0);
        let next = if backwards {
            (current + count - 1) % count
        } else {
            (current + 1) % count
        };
        if let Some(terminal) = self.terminal_active.and_then(|i| self.terminals.get(i)) {
            if let Ok(mut parser) = terminal.screen.lock() {
                self.terminal_find_active = Some(next);
                parser.screen_mut().set_scrollback(
                    self.terminal_find_scrollback
                        .saturating_sub(self.terminal_find_matches[next].line),
                );
                self.terminal_selection = None;
            }
        }
    }

    pub(super) fn terminal_view(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let gere = self.settings.is_gere();
        let font_size = self.settings.font_size as f32;
        let row_height = (font_size + 5.).max(18.);
        let font = self.settings.font_name();
        let font_id = cx.text_system().resolve_font(&gpui::font(font));
        let cell_width = cx
            .text_system()
            .ch_advance(font_id, px(font_size))
            .unwrap_or(px(8.4));
        let find_cell_width = cx
            .text_system()
            .ch_advance(font_id, px(14.))
            .unwrap_or(px(8.4));
        let height = self
            .terminal_height
            .min((window.bounds().size.height - px(145.)).max(px(110.)));
        let width = window.bounds().size.width
            - px(RAIL_WIDTH)
            - if self.sidebar_visible {
                sidebar_width(window.bounds().size.width, self.sidebar_width) + px(8.)
            } else {
                px(0.)
            };
        self.terminal_size = (
            ((f32::from(height) - 40.) / row_height)
                .floor()
                .max(2.)
                .min(u16::MAX as f32) as u16,
            ((f32::from(width) - 30.) / f32::from(cell_width))
                .floor()
                .max(2.)
                .min(u16::MAX as f32) as u16,
        );

        let tabs = self
            .terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| {
                let title = if terminal.exited {
                    format!("{} (salió)", terminal.title)
                } else {
                    terminal.title.clone()
                };
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .can_drop(|drag, _, _| drag.downcast_ref::<TerminalTabDrag>().is_some())
                    .drag_over::<TerminalTabDrag>(|style, _, _, _| style.bg(rgb(0x505766)))
                    .on_drop(cx.listener(move |this, drag: &TerminalTabDrag, _, cx| {
                        if let Some(from) = this
                            .terminals
                            .iter()
                            .position(|terminal| terminal.title == drag.0)
                        {
                            reorder_tab(
                                &mut this.terminals,
                                &mut this.terminal_active,
                                from,
                                index,
                            );
                            cx.notify();
                        }
                    }))
                    .bg(rgb(if self.terminal_active == Some(index) {
                        if self.settings.is_gere() {
                            0x22252e
                        } else {
                            0x3e4451
                        }
                    } else {
                        self.settings.panel()
                    }))
                    .child(
                        div()
                            .id(("terminal-tab-label", index))
                            .px_2()
                            .cursor_pointer()
                            .on_drag(TerminalTabDrag(terminal.title.clone()), |drag, _, _, cx| {
                                cx.new(|_| drag.clone())
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if cx.has_active_drag() {
                                        return;
                                    }
                                    this.terminal_active = Some(index);
                                    this.focus_terminal(window);
                                    this.terminal_revision = 0;
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Middle,
                                cx.listener(move |this, _, _, cx| this.close_terminal(index, cx)),
                            )
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(icons::icon(
                                "terminal",
                                if self.settings.is_gere() {
                                    0x9aa5b1
                                } else {
                                    MUTED
                                },
                            ))
                            .child(title),
                    )
                    .child(
                        div()
                            .px_1()
                            .cursor_pointer()
                            .text_color(rgb(MUTED))
                            .hover(|style| style.bg(rgb(0x4b5261)))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| this.close_terminal(index, cx)),
                            )
                            .child(icons::icon("close", MUTED)),
                    )
            })
            .collect::<Vec<_>>();

        let (rows, scrollback, scroll_offset) = self
            .terminal_active
            .and_then(|index| self.terminals.get(index))
            .and_then(|terminal| {
                terminal.screen.lock().ok().map(|mut parser| {
                    let screen = parser.screen_mut();
                    let scroll_offset = screen.scrollback();
                    screen.set_scrollback(usize::MAX);
                    let scrollback = screen.scrollback();
                    screen.set_scrollback(scroll_offset);
                    let (count, cols) = screen.size();
                    let (cursor_row, cursor_col) = screen.cursor_position();
                    let selection =
                        self.terminal_selection
                            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
                    let line_start = self
                        .terminal_find_scrollback
                        .saturating_sub(screen.scrollback());
                    let rows = (0..count)
                        .map(|row| {
                            let mut text = String::new();
                            let mut highlights = Vec::new();
                            let line_index = line_start + usize::from(row);
                            let matched = self
                                .terminal_find_open
                                .then_some(&self.terminal_find_matches)
                                .and_then(|matches| {
                                    let first =
                                        matches.partition_point(|found| found.line < line_index);
                                    let last =
                                        matches.partition_point(|found| found.line <= line_index);
                                    (first < last).then_some((first, &matches[first..last]))
                                });
                            let end = (0..cols)
                                .rev()
                                .find(|&col| {
                                    screen.cell(row, col).is_some_and(|cell| {
                                        cell.has_contents()
                                            || cell.bgcolor() != vt100::Color::Default
                                            || matched.is_some_and(|(_, matches)| {
                                                matches
                                                    .iter()
                                                    .any(|found| found.columns.contains(&col))
                                            })
                                            || (!screen.hide_cursor()
                                                && row == cursor_row
                                                && col == cursor_col
                                                && self.terminal_focused
                                                && !self.terminal_find_open
                                                && self.cursor_blink_visible)
                                    })
                                })
                                .map_or(0, |col| col + 1);
                            for col in 0..end {
                                let Some(cell) = screen.cell(row, col) else {
                                    continue;
                                };
                                if cell.is_wide_continuation() {
                                    continue;
                                }
                                let start = text.len();
                                if cell.has_contents() {
                                    text.push_str(cell.contents());
                                } else {
                                    text.push(' ');
                                }
                                let mut style = HighlightStyle::default();
                                let foreground = color(
                                    cell.fgcolor(),
                                    if self.settings.is_gere() {
                                        0xd7dce2
                                    } else {
                                        0xd7dae0
                                    },
                                );
                                let background = color(cell.bgcolor(), self.settings.background());
                                let (foreground, background) = if cell.inverse() {
                                    (background, foreground)
                                } else {
                                    (foreground, background)
                                };
                                if foreground
                                    != if self.settings.is_gere() {
                                        0xd7dce2
                                    } else {
                                        0xd7dae0
                                    }
                                {
                                    style.color = Some(rgb(foreground).into());
                                }
                                if background != self.settings.background() {
                                    style.background_color = Some(rgb(background).into());
                                }
                                if let Some((first, matches)) = matched {
                                    for (index, found) in matches.iter().enumerate() {
                                        if found.columns.contains(&col) {
                                            let active =
                                                self.terminal_find_active == Some(first + index);
                                            style.background_color = Some(
                                                rgb(if active { 0xe5c07b } else { 0x665b32 })
                                                    .into(),
                                            );
                                            if active {
                                                style.color = Some(rgb(0x21252b).into());
                                            }
                                            break;
                                        }
                                    }
                                }
                                if selection.is_some_and(|(start, end)| {
                                    (row, col) >= start && (row, col) < end
                                }) {
                                    style.background_color = Some(rgb(0x3c6386).into());
                                }
                                if !screen.hide_cursor()
                                    && row == cursor_row
                                    && col == cursor_col
                                    && self.terminal_focused
                                    && !self.terminal_find_open
                                    && self.cursor_blink_visible
                                {
                                    style.background_color = Some(
                                        rgb(if self.settings.is_gere() {
                                            0x1b6de1
                                        } else {
                                            0x61afef
                                        })
                                        .into(),
                                    );
                                    style.color = Some(rgb(0x21252b).into());
                                }
                                if style.color.is_some() || style.background_color.is_some() {
                                    highlights.push((start..text.len(), style));
                                }
                            }
                            StyledText::new(text).with_highlights(highlights)
                        })
                        .collect::<Vec<_>>();
                    (rows, scrollback, scroll_offset)
                })
            })
            .unwrap_or_default();

        let viewport = f32::from(height) - 36.;
        let (thumb_height, thumb_top) = terminal_scrollbar_geometry(
            viewport,
            usize::from(self.terminal_size.0),
            scrollback,
            scroll_offset,
        );

        let find_bar = self.terminal_find_open.then(|| {
            let status = self.terminal_find_active.map_or_else(
                || format!("0 / {}", self.terminal_find_matches.len()),
                |index| format!("{} / {}", index + 1, self.terminal_find_matches.len()),
            );
            div()
                .h(px(30.))
                .flex()
                .items_center()
                .gap_1()
                .bg(rgb(PANEL))
                .child(
                    input_view(
                        &self.terminal_find_query,
                        "Buscar en terminal…",
                        true,
                        self.cursor_blink_visible,
                        18,
                        find_cell_width,
                        0x21252b,
                        true,
                    )
                    .w(px(180.))
                    .font_family(font)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, _| {
                            this.terminal_focused = true;
                            window.focus(&this.focus);
                        }),
                    ),
                )
                .child(div().min_w(px(50.)).text_color(rgb(MUTED)).child(status))
                .child(
                    div()
                        .cursor_pointer()
                        .px_1()
                        .hover(|style| style.bg(rgb(0x3e4451)))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.move_terminal_find(true);
                                cx.notify();
                            }),
                        )
                        .child("↑"),
                )
                .child(
                    div()
                        .cursor_pointer()
                        .px_1()
                        .hover(|style| style.bg(rgb(0x3e4451)))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.move_terminal_find(false);
                                cx.notify();
                            }),
                        )
                        .child("↓"),
                )
                .child(
                    div()
                        .cursor_pointer()
                        .px_1()
                        .hover(|style| style.bg(rgb(0x3e4451)))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.terminal_find_open = false;
                                cx.notify();
                            }),
                        )
                        .child("×"),
                )
        });
        div()
            .h(height)
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(self.settings.background()))
            .child(
                div()
                    .h(px(6.))
                    .w_full()
                    .cursor_row_resize()
                    .bg(rgb(if self.terminal_dragging {
                        0x61afef
                    } else {
                        0x3a3f4b
                    }))
                    .hover(|style| style.bg(rgb(0x61afef)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.terminal_dragging = true;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .h(px(30.))
                    .w_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .bg(rgb(self.settings.panel()))
                    .children(tabs)
                    .child(Self::icon_button(
                        "plus",
                        "Nueva terminal",
                        cx.listener(|this, _, window, cx| {
                            this.new_terminal(cx);
                            if this.terminal_visible {
                                this.focus_terminal(window);
                            }
                        }),
                    ))
                    .child(div().flex_1().h_full().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if event.click_count == 2 {
                                this.new_terminal(cx);
                                this.focus_terminal(window);
                                cx.stop_propagation();
                            }
                        }),
                    ))
                    .children(find_bar)
                    .child(
                        div()
                            .relative()
                            .child(Self::icon_button(
                                "chevron-down",
                                "Seleccionar shell",
                                cx.listener(|this, _, _, cx| {
                                    this.terminal_shell_menu = !this.terminal_shell_menu;
                                    cx.notify();
                                }),
                            ))
                            .when(self.terminal_shell_menu, |menu| {
                                menu.child(
                                    div()
                                        .absolute()
                                        .bottom(px(26.))
                                        .right(px(0.))
                                        .w(px(175.))
                                        .p_1()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(0x273549))
                                        .bg(rgb(self.settings.panel()))
                                        .shadow_lg()
                                        .occlude()
                                        .children(
                                            [
                                                ("Shell predeterminada", None),
                                                ("bash", Some("/bin/bash")),
                                                ("zsh", Some("/bin/zsh")),
                                                ("sh", Some("/bin/sh")),
                                            ]
                                            .into_iter()
                                            .map(
                                                |(label, path)| {
                                                    div()
                                                        .px_2()
                                                        .py_1()
                                                        .cursor_pointer()
                                                        .hover(move |style| {
                                                            style.bg(rgb(if gere {
                                                                0x22252e
                                                            } else {
                                                                0x3e4451
                                                            }))
                                                        })
                                                        .on_mouse_up(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                this.terminal_shell =
                                                                    path.map(str::to_owned);
                                                                this.terminal_shell_menu = false;
                                                                cx.notify();
                                                            }),
                                                        )
                                                        .child(format!(
                                                            "{} {label}",
                                                            if self.terminal_shell.as_deref()
                                                                == path
                                                            {
                                                                "✓"
                                                            } else {
                                                                " "
                                                            }
                                                        ))
                                                },
                                            ),
                                        ),
                                )
                            }),
                    )
                    .child(Self::icon_button(
                        "close",
                        "Ocultar terminales",
                        cx.listener(|this, _, _, cx| {
                            this.terminal_visible = false;
                            this.terminal_focused = false;
                            this.terminal_shell_menu = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .pl_2()
                            .pt_1()
                            .font_family(font)
                            .text_size(px(font_size))
                            .text_color(rgb(if self.settings.is_gere() {
                                0xd7dce2
                            } else {
                                0xd7dae0
                            }))
                            .cursor_text()
                            .on_scroll_wheel(cx.listener(
                                move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                    let delta = event.delta.pixel_delta(px(row_height));
                                    this.scroll_terminal(f32::from(delta.y) / row_height, cx);
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                    this.focus_terminal(window);
                                    let point = this.terminal_point(event.position, window);
                                    this.terminal_selection = Some((point, point));
                                    this.terminal_selecting = true;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .children(rows.into_iter().map(|row| {
                                        div().h(px(row_height)).whitespace_nowrap().child(row)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(10.))
                            .h_full()
                            .flex_shrink_0()
                            .bg(rgb(0x292d36))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    this.terminal_scroll_dragging = true;
                                    this.terminal_scroll_grab = thumb_height / 2.;
                                    this.drag_terminal_scrollbar(event.position.y, window, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(thumb_top))
                                    .w_full()
                                    .h(px(thumb_height))
                                    .rounded_sm()
                                    .bg(rgb(if self.terminal_scroll_dragging {
                                        0xabb2bf
                                    } else {
                                        0x5c6370
                                    }))
                                    .hover(|style| style.bg(rgb(0xabb2bf)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                let height = this.terminal_height.min(
                                                    (window.bounds().size.height - px(145.))
                                                        .max(px(110.)),
                                                );
                                                let track_top =
                                                    window.bounds().size.height - px(26.) - height
                                                        + px(36.);
                                                this.terminal_scroll_grab =
                                                    (f32::from(event.position.y - track_top)
                                                        - thumb_top)
                                                        .clamp(0., thumb_height);
                                                this.terminal_scroll_dragging = true;
                                                cx.stop_propagation();
                                            },
                                        ),
                                    ),
                            ),
                    ),
            )
    }
}

fn color(value: vt100::Color, default: u32) -> u32 {
    match value {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
        vt100::Color::Idx(index) if index < 16 => [
            0x21252b, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xd7dae0,
            0x5c6370, 0xff7b86, 0xb5d88a, 0xffd68a, 0x80c2ff, 0xe3a0f3, 0x79d2dc, 0xffffff,
        ][index as usize],
        vt100::Color::Idx(index) if index < 232 => {
            let n = index - 16;
            let channel = |c: u8| if c == 0 { 0 } else { u32::from(c) * 40 + 55 };
            (channel(n / 36) << 16) | (channel((n / 6) % 6) << 8) | channel(n % 6)
        }
        vt100::Color::Idx(index) => {
            let shade = u32::from(index - 232) * 10 + 8;
            (shade << 16) | (shade << 8) | shade
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_copies_forward_and_reverse_across_rows() {
        let mut parser = vt100::Parser::new(3, 12, 10);
        parser.process(b"first\r\nsecond\r\nthird");
        let screen = parser.screen();
        assert_eq!(selected_terminal_text(screen, (0, 2), (1, 3)), "rst\nsec");
        assert_eq!(selected_terminal_text(screen, (1, 3), (0, 2)), "rst\nsec");
        assert_eq!(selected_terminal_text(screen, (2, 0), (2, 5)), "third");
    }

    #[test]
    fn terminal_find_counts_every_occurrence_and_locates_scrollback_rows() {
        let mut parser = vt100::Parser::new(2, 20, 10);
        parser.process(b"found found\r\nother\r\nFOUND\r\nlast");
        let screen = parser.screen_mut();
        let (oldest, matches) = scan_terminal_matches(screen, "found");
        assert_eq!(oldest, 2);
        assert_eq!(
            matches,
            vec![
                TerminalMatch {
                    line: 0,
                    columns: 0..5
                },
                TerminalMatch {
                    line: 0,
                    columns: 6..11
                },
                TerminalMatch {
                    line: 2,
                    columns: 0..5
                },
            ]
        );
        screen.set_scrollback(oldest - matches[0].line);
        assert!(screen
            .rows(0, 20)
            .next()
            .unwrap()
            .starts_with("found found"));
        screen.set_scrollback(oldest - matches[2].line);
        assert!(screen.rows(0, 20).next().unwrap().starts_with("FOUND"));
    }

    #[test]
    fn terminal_find_highlight_uses_cell_columns_after_wide_text() {
        let mut parser = vt100::Parser::new(2, 20, 0);
        parser.process("界found".as_bytes());
        assert_eq!(
            line_matches(parser.screen(), 0, "found", 0),
            vec![TerminalMatch {
                line: 0,
                columns: 2..7
            }]
        );
    }

    #[test]
    fn terminal_scrollbar_tracks_the_full_history_and_grab_position() {
        let (thumb, top) = terminal_scrollbar_geometry(300., 20, 10_000, 10_000);
        assert_eq!(top, 0.);
        assert_eq!(thumb, 22.);
        assert_eq!(
            terminal_scrollbar_offset(thumb / 2., thumb / 2., 300., 20, 10_000),
            10_000
        );
        assert_eq!(
            terminal_scrollbar_offset(300. - thumb / 2., thumb / 2., 300., 20, 10_000),
            0
        );
        let (_, halfway) = terminal_scrollbar_geometry(300., 20, 10_000, 5_000);
        assert_eq!(
            terminal_scrollbar_offset(halfway + 6., 6., 300., 20, 10_000),
            5_000
        );
        assert_eq!(terminal_scrollbar_geometry(300., 20, 0, 0), (300., 0.));
    }
}
