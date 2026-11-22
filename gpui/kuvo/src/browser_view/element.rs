//! The page's picture, drawn as one element: as big as fits where it's laid out, keeping its shape,
//! centred on the panel's surface. Laying it out tells the view where it's drawn (the size the box
//! is asked for); drawing a new picture is what lets the box send the next.

use gpui::{App, Bounds, Corners, Element, ElementId, Entity, GlobalElementId, InspectorElementId, LayoutId, Pixels, Style, Window, relative};

use super::BrowserView;

pub(crate) struct PageElement {
    view: Entity<BrowserView>,
}

impl PageElement {
    pub fn new(view: Entity<BrowserView>) -> PageElement {
        PageElement { view }
    }
}

impl Element for PageElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) {
        let scale = window.scale_factor();
        self.view.update(cx, |view, cx| view.laid_out(bounds, scale, cx));
    }

    fn paint(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, cx: &mut App) {
        let (retired, shown) = self.view.update(cx, |view, _| (std::mem::take(&mut view.retired), view.shown_picture().map(|picture| (picture.frame_id, picture.width, picture.height, picture.image.clone()))));
        // Pictures replaced since: out of the window's atlas.
        for image in retired {
            let _ = window.drop_image(image);
        }
        let Some((frame_id, width, height, image)) = shown else { return };
        let rect = super::fitted(bounds, width, height);
        if window.paint_image(rect, rect, Corners::default(), image, 0, false).is_ok() {
            self.view.update(cx, |view, _| view.drawn(frame_id));
        }
    }
}
