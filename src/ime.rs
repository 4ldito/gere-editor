use gpui::{
    App, Bounds, Element, ElementId, ElementInputHandler, Entity, GlobalElementId, IntoElement,
    LayoutId, Pixels, Style, Window,
};

use crate::Reviewer;

/// Registers the editor's focused entity with GPUI's native text-input system.
/// XIM/Wayland composition commits are otherwise discarded before key handlers see them.
pub(super) struct ImeElement(pub(super) Entity<Reviewer>);

impl IntoElement for ImeElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ImeElement {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (window.request_layout(Style::default(), [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.0.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.0.clone()), cx);
    }
}
