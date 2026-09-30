//! Terminal panel UI and focus/keyboard routing.
use super::*;
use gpui::{HighlightStyle, StyledText};

impl Reviewer {
    fn focus_terminal(&mut self, window: &mut Window) {
        self.terminal_focused = true;
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
        match terminal::Terminal::new(&self.root, format!("Terminal {id}"), self.terminal_size) {
            Ok(terminal) => {
                self.next_terminal_id += 1;
                self.terminals.push(terminal);
                self.terminal_active = Some(self.terminals.len() - 1);
                self.terminal_visible = true;
                self.terminal_focused = true;
                self.terminal_revision = 0;
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
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn terminal_view(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let font_size = self.settings.font_size as f32;
        let row_height = (font_size + 5.).max(18.);
        let font = self.settings.font_name();
        let font_id = cx.text_system().resolve_font(&gpui::font(font));
        let cell_width = cx
            .text_system()
            .ch_advance(font_id, px(font_size))
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
            ((f32::from(width) - 20.) / f32::from(cell_width))
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
                    .bg(rgb(if self.terminal_active == Some(index) {
                        0x3e4451
                    } else {
                        PANEL
                    }))
                    .child(
                        div()
                            .px_2()
                            .cursor_pointer()
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
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
                            .child("×"),
                    )
            })
            .collect::<Vec<_>>();

        let rows = self
            .terminal_active
            .and_then(|index| self.terminals.get(index))
            .and_then(|terminal| {
                terminal.screen.lock().ok().map(|parser| {
                    let screen = parser.screen();
                    let (count, cols) = screen.size();
                    let (cursor_row, cursor_col) = screen.cursor_position();
                    (0..count)
                        .map(|row| {
                            let mut text = String::new();
                            let mut highlights = Vec::new();
                            let end = (0..cols)
                                .rev()
                                .find(|&col| {
                                    screen.cell(row, col).is_some_and(|cell| {
                                        cell.has_contents()
                                            || cell.bgcolor() != vt100::Color::Default
                                            || (!screen.hide_cursor()
                                                && row == cursor_row
                                                && col == cursor_col
                                                && self.terminal_focused
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
                                let foreground = color(cell.fgcolor(), 0xd7dae0);
                                let background = color(cell.bgcolor(), 0x21252b);
                                let (foreground, background) = if cell.inverse() {
                                    (background, foreground)
                                } else {
                                    (foreground, background)
                                };
                                if foreground != 0xd7dae0 {
                                    style.color = Some(rgb(foreground).into());
                                }
                                if background != 0x21252b {
                                    style.background_color = Some(rgb(background).into());
                                }
                                if !screen.hide_cursor()
                                    && row == cursor_row
                                    && col == cursor_col
                                    && self.terminal_focused
                                    && self.cursor_blink_visible
                                {
                                    style.background_color = Some(rgb(0x61afef).into());
                                    style.color = Some(rgb(0x21252b).into());
                                }
                                if style.color.is_some() || style.background_color.is_some() {
                                    highlights.push((start..text.len(), style));
                                }
                            }
                            StyledText::new(text).with_highlights(highlights)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();

        div()
            .h(height)
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(0x21252b))
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
                    .bg(rgb(PANEL))
                    .children(tabs)
                    .child(
                        div()
                            .cursor_pointer()
                            .px_2()
                            .hover(|style| style.bg(rgb(0x3e4451)))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.new_terminal(cx);
                                    this.focus_terminal(window);
                                }),
                            )
                            .child("+"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .cursor_pointer()
                            .px_2()
                            .hover(|style| style.bg(rgb(0x3e4451)))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.terminal_visible = false;
                                    this.terminal_focused = false;
                                    cx.notify();
                                }),
                            )
                            .child("×"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .pl_2()
                    .pt_1()
                    .font_family(font)
                    .text_size(px(font_size))
                    .text_color(rgb(0xd7dae0))
                    .cursor_text()
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                            let delta = event.delta.pixel_delta(px(row_height));
                            this.scroll_terminal(f32::from(delta.y) / row_height, cx);
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.focus_terminal(window);
                            cx.notify();
                        }),
                    )
                    .child(
                        div().flex().flex_col().children(
                            rows.into_iter()
                                .map(|row| div().h(px(row_height)).whitespace_nowrap().child(row)),
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
