//! Exercise the actual animation wrapper, including its frame scheduling and keyed state.
use std::{cell::Cell, rc::Rc, time::Duration};
use gpui::{div, px, size, Context, IntoElement, InteractiveElement, Render, Styled, TestAppContext, Window, WindowHandle};
use crate::motion::{Direction, MotionExt};

struct Popover {
    renders: Rc<Cell<usize>>,
    pulse: bool,
}

impl Render for Popover {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let card = div().id("usage-card").w(px(120.)).h(px(80.));
        if self.pulse {
            card.pulsing().into_any_element()
        } else {
            card.animate_in(Direction::FromBottom, true).into_any_element()
        }
    }
}

fn frame(window: WindowHandle<Popover>, cx: &mut TestAppContext) -> usize {
    let count = window.update(cx, |_, window, cx| window.simulate_next_frame(cx)).unwrap();
    cx.run_until_parked();
    count
}

#[gpui::test]
fn completed_popover_stops_requesting_frames_and_does_not_restart(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let window = cx.open_window(size(px(200.), px(200.)), {
        let renders = renders.clone();
        move |_, _| Popover { renders, pulse: false }
    });
    cx.run_until_parked();
    // A real animation must be running before we test that it comes to rest.
    assert!(frame(window, cx) > 0);
    cx.executor().advance_clock(Duration::from_millis(200));
    assert!(frame(window, cx) > 0);
    assert_eq!(frame(window, cx), 0);
    let settled_renders = renders.get();
    cx.executor().advance_clock(Duration::from_secs(10));
    assert_eq!(frame(window, cx), 0);
    assert_eq!(renders.get(), settled_renders);

    // An unrelated update retains the animation's stable identity and completed state.
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.run_until_parked();
    assert!(renders.get() > settled_renders);
    assert_eq!(frame(window, cx), 0);
}

#[gpui::test]
fn reduced_motion_schedules_no_entrance_or_pulse_frames(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    for pulse in [false, true] {
        let window = cx.open_window(size(px(200.), px(200.)), move |_, _| Popover {
            renders: Rc::new(Cell::new(0)), pulse,
        });
        cx.run_until_parked();
        assert_eq!(frame(window, cx), 0);
        cx.executor().advance_clock(Duration::from_secs(3));
        assert_eq!(frame(window, cx), 0);
    }
}
