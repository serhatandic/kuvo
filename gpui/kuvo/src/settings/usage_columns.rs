use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Point, Style, Window, point, px, size,
};

const CARD_WIDTH: f32 = 344.;
const CARD_GAP: f32 = 12.;
const CARD: Pixels = px(CARD_WIDTH);
const GAP: Pixels = px(CARD_GAP);
pub(super) const PAGE_WIDTH: f32 = 3. * CARD_WIDTH + 2. * CARD_GAP;

#[derive(Default)]
pub(super) struct Placement {
    columns: usize,
    assigned: HashMap<String, usize>,
}

impl Placement {
    /// Keep each returning card in its column. New cards join the shortest column, in reading order.
    fn place(&mut self, columns: usize, cards: &[(&str, Pixels)]) -> (Vec<Point<Pixels>>, Pixels) {
        if self.columns != columns {
            self.columns = columns;
            self.assigned.clear();
        }

        let mut heights = vec![px(0.); columns];
        for &(key, height) in cards {
            if let Some(&column) = self.assigned.get(key) {
                heights[column] += height + GAP;
            }
        }
        for &(key, height) in cards {
            self.assigned.entry(key.to_owned()).or_insert_with(|| {
                let column = (0..columns)
                    .min_by_key(|&column| heights[column])
                    .expect("at least one column");
                heights[column] += height + GAP;
                column
            });
        }

        // Lay out each column in the order the accounts arrived.
        let mut bottom = vec![px(0.); columns];
        let positions = cards
            .iter()
            .map(|&(key, height)| {
                let column = self.assigned[key];
                let origin = point((CARD + GAP) * column as f32, bottom[column]);
                bottom[column] += height + GAP;
                origin
            })
            .collect();
        let height = (bottom.into_iter().max().unwrap_or_default() - GAP).max(px(0.));
        (positions, height)
    }
}

pub(super) struct UsageColumns {
    pub(super) width: Pixels,
    pub(super) placement: Rc<RefCell<Placement>>,
    pub(super) cards: Vec<(String, AnyElement)>,
}

impl Element for UsageColumns {
    type RequestLayoutState = Vec<Point<Pixels>>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let columns = ((f32::from(self.width + GAP) / f32::from(CARD + GAP)) as usize).clamp(1, 3);
        let card_space = size(
            AvailableSpace::Definite(self.width.min(CARD)),
            AvailableSpace::MinContent,
        );
        let measured: Vec<_> = self
            .cards
            .iter_mut()
            .map(|(key, card)| {
                (
                    key.as_str(),
                    card.layout_as_root(card_space, window, cx).height,
                )
            })
            .collect();
        let (positions, height) = self.placement.borrow_mut().place(columns, &measured);
        let style = Style {
            size: size(self.width.into(), height.into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), positions)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        positions: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for ((_, card), &position) in self.cards.iter_mut().zip(positions.iter()) {
            card.prepaint_at(bounds.origin + position, window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        for (_, card) in &mut self.cards {
            card.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_places_in_shortest_column_and_keeps_assignments_until_reflow() {
        let mut placement = Placement::default();
        let cards = [
            ("claude", px(120.)),
            ("cursor", px(40.)),
            ("codex", px(50.)),
        ];
        let right = CARD + GAP;
        let (initial, height) = placement.place(2, &cards);
        assert_eq!(
            initial,
            [
                point(px(0.), px(0.)),
                point(right, px(0.)),
                point(right, px(52.))
            ]
        );
        assert_eq!(height, px(120.));

        let refreshed = [
            ("claude", px(25.)),
            ("cursor", px(100.)),
            ("codex", px(50.)),
            ("grok", px(70.)),
        ];
        let (kept, _) = placement.place(2, &refreshed);
        assert_eq!(
            kept,
            [
                point(px(0.), px(0.)),
                point(right, px(0.)),
                point(right, px(112.)),
                point(px(0.), px(37.)),
            ]
        );
        let (reflowed, _) = placement.place(3, &refreshed);
        assert_eq!(reflowed[2], point(right * 2., px(0.)));
    }
}
