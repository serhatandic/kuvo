use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Point, Style, Window, point, px, size,
};

pub(super) const CARD_WIDTH: f32 = 344.;
const CARD_GAP: f32 = 12.;
const CARD: Pixels = px(CARD_WIDTH);
const GAP: Pixels = px(CARD_GAP);
pub(super) const PAGE_WIDTH: f32 = 3. * CARD_WIDTH + 2. * CARD_GAP;

#[derive(Default)]
pub(super) struct Placement {
    columns: usize,
    assigned: HashMap<Rc<str>, usize>,
    measurements: HashMap<Rc<str>, (CardMeasure, Pixels)>,
    positions: Option<PositionCache>,
}

#[derive(Clone, PartialEq)]
pub(super) struct CardMeasure {
    pub(super) width: Pixels,
    pub(super) text_scale: Pixels,
    pub(super) meter_count: usize,
    pub(super) status: Option<String>,
}

struct PositionCache {
    columns: usize,
    cards: Vec<(Rc<str>, Pixels)>,
    origins: Rc<[Point<Pixels>]>,
    height: Pixels,
}

impl Placement {
    /// Keep each returning card in its column. New cards join the shortest column, in reading order.
    fn place(
        &mut self,
        columns: usize,
        cards: &[(Rc<str>, Pixels)],
    ) -> (Rc<[Point<Pixels>]>, Pixels) {
        if let Some(cache) = &self.positions
            && cache.columns == columns
            && cache.cards == cards
        {
            return (cache.origins.clone(), cache.height);
        }
        if self.columns != columns {
            self.columns = columns;
            self.assigned.clear();
        }

        let mut heights = vec![px(0.); columns];
        for (key, height) in cards {
            if let Some(&column) = self.assigned.get(key) {
                heights[column] += *height + GAP;
            }
        }
        for (key, height) in cards {
            self.assigned.entry(key.clone()).or_insert_with(|| {
                let column = (0..columns)
                    .min_by_key(|&column| heights[column])
                    .expect("at least one column");
                heights[column] += *height + GAP;
                column
            });
        }

        // Lay out each column in the order the accounts arrived.
        let mut bottom = vec![px(0.); columns];
        let positions: Rc<[_]> = cards
            .iter()
            .map(|(key, height)| {
                let column = self.assigned[key];
                let origin = point((CARD + GAP) * column as f32, bottom[column]);
                bottom[column] += *height + GAP;
                origin
            })
            .collect();
        let height = (bottom.into_iter().max().unwrap_or_default() - GAP).max(px(0.));
        self.positions = Some(PositionCache {
            columns,
            cards: cards.to_vec(),
            origins: positions.clone(),
            height,
        });
        (positions, height)
    }

    fn measure(
        &mut self,
        key: &Rc<str>,
        signature: &CardMeasure,
        measure: impl FnOnce() -> Pixels,
    ) -> Pixels {
        if let Some((last, height)) = self.measurements.get(key)
            && last == signature
        {
            return *height;
        }
        let height = measure();
        self.measurements
            .insert(key.clone(), (signature.clone(), height));
        height
    }
}

pub(super) struct UsageColumns {
    pub(super) width: Pixels,
    pub(super) placement: Rc<RefCell<Placement>>,
    pub(super) cards: Vec<(Rc<str>, CardMeasure, AnyElement)>,
}

impl Element for UsageColumns {
    type RequestLayoutState = (Rc<[Point<Pixels>]>, Vec<Pixels>, Vec<bool>);
    type PrepaintState = Vec<usize>;

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
        let mut placement = self.placement.borrow_mut();
        let mut laid_out = vec![false; self.cards.len()];
        let measured: Vec<_> = self
            .cards
            .iter_mut()
            .zip(&mut laid_out)
            .map(|((key, signature, card), laid_out)| {
                let height = placement.measure(key, signature, || {
                    *laid_out = true;
                    card.layout_as_root(card_space, window, cx).height
                });
                (key.clone(), height)
            })
            .collect();
        let (positions, height) = placement.place(columns, &measured);
        let style = Style {
            size: size(self.width.into(), height.into()),
            ..Style::default()
        };
        (
            window.request_layout(style, [], cx),
            (
                positions,
                measured.into_iter().map(|(_, height)| height).collect(),
                laid_out,
            ),
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (positions, heights, laid_out): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let width = self.width.min(CARD);
        let visible = window.content_mask().bounds;
        let mut painted = Vec::new();
        for (index, (_, _, card)) in self.cards.iter_mut().enumerate() {
            let origin = bounds.origin + positions[index];
            if !visible.intersects(&Bounds {
                origin,
                size: size(width, heights[index]),
            }) {
                continue;
            }
            if !laid_out[index] {
                card.layout_as_root(
                    size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                    window,
                    cx,
                );
                laid_out[index] = true;
            }
            card.prepaint_at(origin, window, cx);
            painted.push(index);
        }
        painted
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        painted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for &index in painted.iter() {
            self.cards[index].2.paint(window, cx);
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
        let (initial, height) =
            placement.place(2, &cards.map(|(key, height)| (Rc::from(key), height)));
        assert_eq!(
            initial.as_ref(),
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
        let refreshed = refreshed.map(|(key, height)| (Rc::from(key), height));
        let (kept, _) = placement.place(2, &refreshed);
        assert_eq!(
            kept.as_ref(),
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

    #[test]
    fn usage_caches_heights_by_width_and_content_and_reuses_positions() {
        let mut placement = Placement::default();
        let key: Rc<str> = Rc::from("claude");
        let mut signature = CardMeasure {
            width: CARD,
            text_scale: px(12.),
            meter_count: 2,
            status: None,
        };
        assert_eq!(placement.measure(&key, &signature, || px(80.)), px(80.));
        assert_eq!(
            placement.measure(&key, &signature, || panic!("remeasured")),
            px(80.)
        );
        let cards = [(key.clone(), px(80.))];
        let (first, _) = placement.place(2, &cards);
        assert!(Rc::ptr_eq(&first, &placement.place(2, &cards).0));
        assert!(!Rc::ptr_eq(&first, &placement.place(3, &cards).0));
        signature.meter_count = 3;
        assert_eq!(placement.measure(&key, &signature, || px(120.)), px(120.));
        signature.width = px(250.);
        assert_eq!(placement.measure(&key, &signature, || px(130.)), px(130.));
    }
}
