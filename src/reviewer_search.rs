//! Debounced project-wide search.
use super::*;

impl Reviewer {
    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        self.search_id += 1;
        self.matches.clear();
        self.search_rows.clear();
        if self.query.text.is_empty() {
            self.message.clear();
            cx.notify();
            return;
        }
        let id = self.search_id;
        let root = self.root.clone();
        let query = self.query.text.clone();
        let options = self.search_options;
        self.message = "Buscando…".into();
        let executor = cx.background_executor().clone();
        cx.spawn(
            move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    gpui::Timer::after(Duration::from_millis(180)).await;
                    if !weak
                        .update(&mut cx, |this, _| this.search_id == id)
                        .unwrap_or(false)
                    {
                        return;
                    }
                    let result = executor
                        .spawn(async move { project::search(&root, &query, options) })
                        .await;
                    let _ = weak.update(&mut cx, |this, cx| {
                        if this.search_id == id {
                            match result {
                                Ok(matches) => {
                                    this.matches = matches;
                                    this.search_rows =
                                        search_rows(&this.matches, &this.collapsed_search);
                                    this.message =
                                        format!("{} coincidencias (máx. 300)", this.matches.len());
                                }
                                Err(error) => {
                                    this.matches.clear();
                                    this.search_rows.clear();
                                    this.message = error;
                                }
                            }
                            cx.notify();
                        }
                    });
                }
            },
        )
        .detach();
        cx.notify();
    }
}
