//! Trello board picker, sidebar and kanban view. All network calls run off the UI thread.
use super::*;

#[derive(Clone)]
pub(super) enum EditCard {
    Rename(String),
    Create(String),
}

#[derive(Clone)]
pub(super) struct CardDrag(pub(super) String, pub(super) String);

impl Render for CardDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .rounded_md()
            .bg(rgb(0x3e4451))
            .text_color(rgb(FG))
            .shadow_md()
            .child(self.1.clone())
    }
}

enum Loaded {
    Credentials,
    Board(trello::Config, Vec<trello::List>),
    Choose(Vec<trello::Board>),
}

impl Reviewer {
    pub(super) fn load_trello(&mut self, cx: &mut Context<Self>) {
        if self.trello_credentials_open || self.trello_saving {
            return;
        }
        self.trello_request = self.trello_request.wrapping_add(1);
        let request = self.trello_request;
        self.trello_loading = true;
        self.trello_error = None;
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move {
                            if trello::credentials()?.is_none() {
                                return Ok(Loaded::Credentials);
                            }
                            match trello::config(&root)? {
                                Some(config) => {
                                    let lists = trello::load(&root)?;
                                    Ok(Loaded::Board(config, lists))
                                }
                                None => Ok(Loaded::Choose(trello::boards()?)),
                            }
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.trello_request != request {
                            return;
                        }
                        this.trello_loading = false;
                        match result {
                            Ok(Loaded::Credentials) => {
                                this.open_trello_credentials(cx);
                            }
                            Ok(Loaded::Board(config, lists)) => {
                                this.trello_board = Some(config.board_url);
                                this.trello_lists = lists;
                                this.trello_picker = false;
                                this.trello_error = None;
                            }
                            Ok(Loaded::Choose(boards)) => {
                                this.trello_boards = boards;
                                this.trello_picker = true;
                                this.trello_lists.clear();
                                this.trello_board = None;
                                this.trello_error = None;
                            }
                            Err(error) => this.trello_error = Some(error),
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn open_trello_credentials(&mut self, cx: &mut Context<Self>) {
        self.trello_request = self.trello_request.wrapping_add(1);
        self.trello_loading = false;
        self.trello_credentials_open = true;
        self.trello_credential_focus_token = false;
        self.trello_credential_key.set_text(String::new());
        self.trello_credential_token.set_text(String::new());
        self.trello_error = None;
        self.terminal_focused = false;
        cx.notify();
    }

    pub(super) fn save_trello_credentials(&mut self, cx: &mut Context<Self>) {
        if self.trello_saving {
            return;
        }
        let key = self.trello_credential_key.text.trim().to_owned();
        let token = self.trello_credential_token.text.trim().to_owned();
        if key.is_empty() || token.is_empty() {
            self.trello_error = Some("Pegá la API key y el token de Trello".into());
            cx.notify();
            return;
        }
        self.trello_saving = true;
        self.trello_error = None;
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move { trello::save_credentials(&key, &token) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.trello_saving = false;
                        match result {
                            Ok(()) => {
                                this.trello_credential_key.set_text(String::new());
                                this.trello_credential_token.set_text(String::new());
                                this.trello_credentials_open = false;
                                this.load_trello(cx);
                            }
                            Err(error) => {
                                this.trello_error = Some(error);
                                cx.notify();
                            }
                        }
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    fn cancel_trello_credentials(&mut self, cx: &mut Context<Self>) {
        self.trello_credentials_open = false;
        self.trello_credential_key.set_text(String::new());
        self.trello_credential_token.set_text(String::new());
        self.trello_error = None;
        cx.notify();
    }

    fn credential_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Mask the token in the UI without modifying the actual input/cursor state.
        let hidden = SingleLineInput {
            text: "•".repeat(self.trello_credential_token.text.len()),
            cursor: self.trello_credential_token.cursor * "•".len(),
            anchor: self.trello_credential_token.anchor.map(|at| at * "•".len()),
        };
        div()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(rgb(FG))
                    .whitespace_normal()
                    .child("Conectá tu cuenta de Trello. Se guarda solo en este equipo."),
            )
            .child(div().text_color(rgb(MUTED)).child("API key"))
            .child(
                input_view(
                    &self.trello_credential_key,
                    "API key",
                    !self.trello_credential_focus_token,
                    !self.trello_credential_focus_token,
                    30,
                    px(8.),
                    BG,
                    false,
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.trello_credential_focus_token = false;
                        window.focus(&this.focus);
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("Token (read,write para editar tarjetas)"),
            )
            .child(
                input_view(
                    &hidden,
                    "Token",
                    self.trello_credential_focus_token,
                    self.trello_credential_focus_token,
                    30,
                    px(8.),
                    BG,
                    false,
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.trello_credential_focus_token = true;
                        window.focus(&this.focus);
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Self::button(
                        "Guardar y conectar",
                        cx.listener(|this, _, _, cx| this.save_trello_credentials(cx)),
                    ))
                    .child(Self::button(
                        "Cancelar",
                        cx.listener(|this, _, _, cx| this.cancel_trello_credentials(cx)),
                    )),
            )
    }

    pub(super) fn choose_trello_board(&mut self, cx: &mut Context<Self>) {
        self.trello_request = self.trello_request.wrapping_add(1);
        let request = self.trello_request;
        self.trello_picker = true;
        self.trello_loading = true;
        self.trello_error = None;
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor.spawn(async move { trello::boards() }).await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.trello_request != request {
                            return;
                        }
                        this.trello_loading = false;
                        match result {
                            Ok(boards) => {
                                this.trello_boards = boards;
                                this.trello_error = None;
                            }
                            Err(error) => this.trello_error = Some(error),
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn save_trello_board(&mut self, url: String, cx: &mut Context<Self>) {
        if self.trello_saving {
            return;
        }
        self.trello_saving = true;
        self.trello_error = None;
        let root = self.root.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move {
                            trello::save_config(
                                &root,
                                &trello::Config {
                                    board_url: url,
                                    list_order: Vec::new(),
                                },
                            )
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.trello_saving = false;
                        match result {
                            Ok(()) => {
                                this.trello_selected = None;
                                this.load_trello(cx);
                            }
                            Err(error) => {
                                this.trello_error = Some(error);
                                cx.notify();
                            }
                        }
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn reorder_trello(
        &mut self,
        index: usize,
        direction: isize,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = index.checked_add_signed(direction) else {
            return;
        };
        if target >= self.trello_lists.len() || self.trello_saving {
            return;
        }
        let Some(url) = self.trello_board.clone() else {
            return;
        };
        self.trello_lists.swap(index, target);
        let order = self
            .trello_lists
            .iter()
            .map(|list| list.id.clone())
            .collect();
        let root = self.root.clone();
        self.trello_saving = true;
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move {
                            trello::save_config(
                                &root,
                                &trello::Config {
                                    board_url: url,
                                    list_order: order,
                                },
                            )
                        })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.trello_saving = false;
                        if let Err(error) = result {
                            this.trello_lists.swap(index, target);
                            this.trello_error = Some(error);
                        }
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn edit_trello_card(&mut self, edit: EditCard, cx: &mut Context<Self>) {
        let name = match &edit {
            EditCard::Rename(id) => self
                .trello_lists
                .iter()
                .flat_map(|list| &list.cards)
                .find(|card| &card.id == id)
                .map(|card| card.name.clone())
                .unwrap_or_default(),
            EditCard::Create(_) => String::new(),
        };
        self.trello_input.set_text(name);
        self.trello_edit = Some(edit);
        self.terminal_focused = false;
        cx.notify();
    }

    pub(super) fn submit_trello_card(&mut self, cx: &mut Context<Self>) {
        if self.trello_saving || self.trello_input.text.trim().is_empty() {
            return;
        }
        let Some(edit) = self.trello_edit.clone() else {
            return;
        };
        let name = self.trello_input.text.clone();
        let change = match edit {
            EditCard::Rename(id) => trello::CardChange::Rename { id, name },
            EditCard::Create(list) => trello::CardChange::Create { list, name },
        };
        self.trello_saving = true;
        self.trello_error = None;
        self.run_trello_change(change, cx);
    }

    pub(super) fn move_trello_card(&mut self, id: String, list: String, cx: &mut Context<Self>) {
        if self.trello_saving {
            return;
        }
        self.trello_saving = true;
        self.trello_error = None;
        self.run_trello_change(trello::CardChange::Move { id, list }, cx);
    }

    fn run_trello_change(&mut self, change: trello::CardChange, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let result = executor
                        .spawn(async move { trello::change_card(change) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        this.trello_saving = false;
                        match result {
                            Ok(()) => {
                                this.trello_edit = None;
                                this.load_trello(cx);
                            }
                            Err(error) => {
                                this.trello_error = Some(error);
                                cx.notify();
                            }
                        }
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }

    fn picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("trello-board-picker")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(rgb(FG))
                    .child("Elegí un tablero para este proyecto"),
            )
            .children(self.trello_boards.iter().map(|board| {
                let url = board.url.clone();
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(BG))
                    .cursor_pointer()
                    .text_color(rgb(FG))
                    .hover(|s| s.bg(rgb(0x3e4451)))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.save_trello_board(url.clone(), cx)),
                    )
                    .child(board.name.clone())
            }))
            .when(
                !self.trello_loading
                    && self.trello_boards.is_empty()
                    && self.trello_error.is_none(),
                |v| v.child("No hay tableros disponibles para esta cuenta"),
            )
    }

    pub(super) fn trello_view(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(if self.trello_picker {
                                "TABLEROS"
                            } else {
                                "LISTAS Y TARJETAS"
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(Self::icon_button(
                                "maximize",
                                "Abrir tablero en pestaña",
                                cx.listener(|this, _, _, cx| {
                                    this.trello_tab_open = true;
                                    this.trello_full = true;
                                    this.show_diff = false;
                                    cx.notify();
                                }),
                            ))
                            .child(Self::icon_button(
                                "refresh-cw",
                                "Actualizar Trello",
                                cx.listener(|this, _, _, cx| {
                                    if this.trello_picker {
                                        this.choose_trello_board(cx);
                                    } else {
                                        this.load_trello(cx);
                                    }
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .when(!self.trello_picker, |v| {
                        v.child(Self::button(
                            "Cambiar tablero",
                            cx.listener(|this, _, _, cx| this.choose_trello_board(cx)),
                        ))
                    })
                    .child(Self::button(
                        "Credenciales",
                        cx.listener(|this, _, window, cx| {
                            this.open_trello_credentials(cx);
                            window.focus(&this.focus);
                        }),
                    ))
                    .when(self.trello_picker && self.trello_board.is_some(), |v| {
                        v.child(Self::button(
                            "Volver",
                            cx.listener(|this, _, _, cx| this.load_trello(cx)),
                        ))
                    }),
            )
            .when(self.trello_loading, |v| {
                v.child(div().p_3().text_color(rgb(MUTED)).child("Cargando Trello…"))
            })
            .when_some(self.trello_error.clone(), |v, error| {
                v.child(
                    div()
                        .p_3()
                        .text_color(rgb(0xee938e))
                        .whitespace_normal()
                        .child(error),
                )
            })
            .when(self.trello_credentials_open, |v| {
                v.child(self.credential_form(cx))
            })
            .when(self.trello_picker && !self.trello_credentials_open, |v| {
                v.child(self.picker(cx))
            })
            .when(!self.trello_picker && !self.trello_credentials_open, |v| {
                v.child(
                    div()
                        .id("trello-lists")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .when(
                            !self.trello_loading
                                && self.trello_error.is_none()
                                && self.trello_lists.is_empty(),
                            |v| {
                                v.child(
                                    div()
                                        .p_3()
                                        .text_color(rgb(MUTED))
                                        .child("No hay listas abiertas"),
                                )
                            },
                        )
                        .children(self.trello_lists.iter().enumerate().map(|(index, list)| {
                            let list_id = list.id.clone();
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .border_b_1()
                                .border_color(rgb(0x3e4451))
                                .child(
                                    div()
                                        .px_2()
                                        .py_2()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_color(rgb(FG))
                                                .child(format!(
                                                    "{} ({})",
                                                    list.name,
                                                    list.cards.len()
                                                )),
                                        )
                                        .child(Self::icon_button(
                                            "arrow-up",
                                            "Subir lista",
                                            cx.listener(move |this, _, _, cx| {
                                                this.reorder_trello(index, -1, cx)
                                            }),
                                        ))
                                        .child(Self::icon_button(
                                            "arrow-down",
                                            "Bajar lista",
                                            cx.listener(move |this, _, _, cx| {
                                                this.reorder_trello(index, 1, cx)
                                            }),
                                        )),
                                )
                                .children(list.cards.iter().map(|card| {
                                    let id = card.id.clone();
                                    div()
                                        .mx_2()
                                        .mb_1()
                                        .p_2()
                                        .rounded_md()
                                        .bg(rgb(BG))
                                        .cursor_pointer()
                                        .text_color(rgb(FG))
                                        .hover(|s| s.bg(rgb(0x3e4451)))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.trello_selected = Some(id.clone());
                                                this.trello_tab_open = true;
                                                this.trello_full = true;
                                                cx.notify();
                                            }),
                                        )
                                        .child(card.name.clone())
                                }))
                                .child(Self::button(
                                    "+ Nueva tarjeta",
                                    cx.listener(move |this, _, window, cx| {
                                        this.trello_full = true;
                                        this.trello_tab_open = true;
                                        this.edit_trello_card(
                                            EditCard::Create(list_id.clone()),
                                            cx,
                                        );
                                        window.focus(&this.focus);
                                    }),
                                ))
                        })),
                )
            })
    }

    pub(super) fn trello_board_view(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .child(
                div()
                    .h(px(48.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .text_color(rgb(FG))
                            .child("Trello · Tablero del proyecto"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Self::button(
                                "Cambiar tablero",
                                cx.listener(|this, _, _, cx| {
                                    this.sidebar = Sidebar::Trello;
                                    this.sidebar_visible = true;
                                    this.choose_trello_board(cx);
                                }),
                            ))
                            .child(Self::button(
                                "Credenciales",
                                cx.listener(|this, _, window, cx| {
                                    this.open_trello_credentials(cx);
                                    window.focus(&this.focus);
                                }),
                            ))
                            .child(Self::icon_button(
                                "refresh-cw",
                                "Actualizar Trello",
                                cx.listener(|this, _, _, cx| this.load_trello(cx)),
                            )),
                    ),
            )
            .when(self.trello_loading, |v| {
                v.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(rgb(MUTED))
                        .child("Cargando Trello…"),
                )
            })
            .when_some(self.trello_error.clone(), |v, error| {
                v.child(div().px_4().py_2().text_color(rgb(0xee938e)).child(error))
            })
            .when(self.trello_credentials_open, |v| {
                v.child(self.credential_form(cx))
            })
            .when(self.trello_picker && !self.trello_credentials_open, |v| {
                v.child(self.picker(cx))
            })
            .when(!self.trello_picker && !self.trello_credentials_open, |v| {
                v.child(
                    div()
                        .id("trello-board-columns")
                        .flex_1()
                        .min_h_0()
                        .overflow_x_scroll()
                        .flex()
                        .items_start()
                        .gap_3()
                        .p_3()
                        .children(self.trello_lists.iter().enumerate().map(|(index, list)| {
                            let list_id = list.id.clone();
                            div()
                                .w(px(260.))
                                .flex_shrink_0()
                                .max_h_full()
                                .rounded_md()
                                .bg(rgb(PANEL))
                                .flex()
                                .flex_col()
                                .can_drop(|drag, _, _| drag.downcast_ref::<CardDrag>().is_some())
                                .drag_over::<CardDrag>(|style, _, _, _| style.bg(rgb(0x3e4451)))
                                .on_drop(cx.listener({
                                    let target = list_id.clone();
                                    move |this, drag: &CardDrag, _, cx| {
                                        if this.trello_lists.iter().any(|list| {
                                            list.id == target
                                                && list.cards.iter().any(|card| card.id == drag.0)
                                        }) {
                                            return;
                                        }
                                        this.move_trello_card(drag.0.clone(), target.clone(), cx);
                                    }
                                }))
                                .child(
                                    div()
                                        .p_3()
                                        .text_color(rgb(FG))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(format!("{}  ·  {}", list.name, list.cards.len())),
                                )
                                .child(
                                    div()
                                        .id(("trello-column-cards", index))
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .px_2()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .children(list.cards.iter().enumerate().map(
                                            |(card_index, card)| {
                                                let id = card.id.clone();
                                                div()
                                                    .id(("trello-card", card_index))
                                                    .w_full()
                                                    .p_3()
                                                    .rounded_md()
                                                    .bg(rgb(BG))
                                                    .cursor_pointer()
                                                    .text_color(rgb(FG))
                                                    .border_1()
                                                    .border_color(rgb(
                                                        if self.trello_selected.as_ref()
                                                            == Some(&card.id)
                                                        {
                                                            0x61afef
                                                        } else {
                                                            0x3e4451
                                                        },
                                                    ))
                                                    .hover(|s| s.bg(rgb(0x3e4451)))
                                                    .on_drag(
                                                        CardDrag(
                                                            card.id.clone(),
                                                            card.name.clone(),
                                                        ),
                                                        |drag, _, _, cx| cx.new(|_| drag.clone()),
                                                    )
                                                    .on_mouse_up(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            if cx.has_active_drag() {
                                                                return;
                                                            }
                                                            this.trello_selected = Some(id.clone());
                                                            this.trello_edit = None;
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(card.name.clone())
                                            },
                                        )),
                                )
                                .child(Self::button(
                                    "+ Añadir tarjeta",
                                    cx.listener(move |this, _, window, cx| {
                                        this.edit_trello_card(
                                            EditCard::Create(list_id.clone()),
                                            cx,
                                        );
                                        window.focus(&this.focus);
                                    }),
                                ))
                        })),
                )
            })
            .when(
                !self.trello_credentials_open
                    && (self.trello_selected.is_some() || self.trello_edit.is_some()),
                |v| v.child(self.trello_detail(cx)),
            )
    }

    fn trello_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = self.trello_selected.as_ref().and_then(|id| {
            self.trello_lists
                .iter()
                .flat_map(|list| &list.cards)
                .find(|card| &card.id == id)
        });
        let current_list = self
            .trello_selected
            .as_ref()
            .and_then(|id| {
                self.trello_lists
                    .iter()
                    .find(|list| list.cards.iter().any(|card| &card.id == id))
            })
            .map(|list| list.id.as_str());
        div()
            .id("trello-detail")
            .w_full()
            .max_h(px(300.))
            .overflow_y_scroll()
            .p_3()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(0x3e4451))
            .flex()
            .flex_col()
            .gap_2()
            .when_some(card, |v, card| {
                let id = card.id.clone();
                let url = card.url.clone();
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_color(rgb(FG)).child(card.name.clone()))
                        .child(Self::button(
                            "Abrir en Trello",
                            cx.listener(move |_, _, _, _| {
                                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
                            }),
                        ))
                        .child(Self::button(
                            "Renombrar",
                            cx.listener(move |this, _, window, cx| {
                                this.edit_trello_card(EditCard::Rename(id.clone()), cx);
                                window.focus(&this.focus);
                            }),
                        )),
                )
            })
            .when_some(self.trello_edit.clone(), |v, _| {
                v.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(
                            input_view(
                                &self.trello_input,
                                "Nombre de la tarjeta…",
                                true,
                                true,
                                40,
                                px(8.),
                                BG,
                                false,
                            )
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(BG))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    window.focus(&this.focus);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(Self::button(
                            "Guardar",
                            cx.listener(|this, _, _, cx| this.submit_trello_card(cx)),
                        ))
                        .child(Self::button(
                            "Cancelar",
                            cx.listener(|this, _, _, cx| {
                                this.trello_edit = None;
                                cx.notify();
                            }),
                        )),
                )
            })
            .when_some(self.trello_selected.clone(), |v, id| {
                v.child(
                    div().flex().flex_wrap().gap_1().children(
                        self.trello_lists
                            .iter()
                            .filter(|list| Some(list.id.as_str()) != current_list)
                            .map(|list| {
                                let target = list.id.clone();
                                let id = id.clone();
                                div().child(Self::button(
                                    format!("Mover a {}", list.name),
                                    cx.listener(move |this, _, _, cx| {
                                        this.move_trello_card(id.clone(), target.clone(), cx)
                                    }),
                                ))
                            }),
                    ),
                )
            })
    }
}
