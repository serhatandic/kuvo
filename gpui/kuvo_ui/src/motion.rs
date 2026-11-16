//! Shared motion for transient surfaces and active progress. IDs belong to the appearing item;
//! callers must give each element a stable ID and apply entry motion to its inner card,
//! leaving the anchored outer container responsible for placement.
use std::time::Duration;
use gpui::{Animation, AnimationElement, AnimationExt, Element, ElementId, Styled, px};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionDuration { Instant, Fast, Slow }
impl MotionDuration {
    pub const fn duration(self) -> Duration {
        Duration::from_millis(match self { Self::Instant => 50, Self::Fast => 150, Self::Slow => 300 })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Direction { FromTop, FromBottom, FromLeft, FromRight }

fn entry_values(direction: Direction, progress: f32) -> (f32, f32, f32) {
    let distance = 4. * (1. - progress);
    let (x, y) = match direction {
        Direction::FromTop => (0., -distance), Direction::FromBottom => (0., distance),
        Direction::FromLeft => (-distance, 0.), Direction::FromRight => (distance, 0.),
    };
    (0.4 + 0.6 * progress, x, y)
}
fn entry_animation() -> Animation {
    Animation::new(MotionDuration::Fast.duration()).with_easing(gpui::ease_out_quint())
}
pub(crate) fn pulse_animation() -> Animation {
    Animation::new(Duration::from_secs(2)).repeat().with_easing(gpui::pulsating_between(0.4, 0.8))
}
pub(crate) fn spin_animation() -> Animation {
    Animation::new(Duration::from_millis(900)).repeat()
}
/// Entry motion is finite; GPUI retains completion under the element's stable ID and
/// stops requesting frames. All helpers inherit GPUI's Reduce Motion behavior.
pub trait MotionExt: Element + Styled + Sized + 'static {
    fn animate_in(self, direction: Direction, fade: bool) -> AnimationElement<Self> {
        let id: ElementId = (self.id().expect("entry motion requires a stable element ID"), "motion-entry").into();
        self.with_animation(id, entry_animation(), move |element, progress| {
            let (opacity, x, y) = entry_values(direction, progress);
            let element = element.relative().left(px(x)).top(px(y));
            if fade { element.opacity(opacity) } else { element }
        })
    }
    fn pulsing(self) -> AnimationElement<Self> {
        let id: ElementId = (self.id().expect("progress motion requires a stable element ID"), "motion-pulse").into();
        self.with_animation(id, pulse_animation(), |element, opacity| element.opacity(opacity))
    }
}
impl<E: Element + Styled + 'static> MotionExt for E {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timings_and_easing() {
        for (duration, millis) in [(MotionDuration::Instant,50),(MotionDuration::Fast,150),(MotionDuration::Slow,300)] {
            assert_eq!(duration.duration(), Duration::from_millis(millis));
        }
        let animation = entry_animation();
        assert!(animation.oneshot);
        assert_eq!((animation.easing)(0.), 0.);
        assert_eq!((animation.easing)(1.), 1.);
        assert_eq!((animation.easing)(0.5), 0.96875);
    }
    #[test]
    fn entry_endpoints() {
        for direction in [Direction::FromTop,Direction::FromBottom,Direction::FromLeft,Direction::FromRight] {
            let (opacity,x,y) = entry_values(direction,0.);
            assert_eq!(opacity,0.4); assert_eq!(x.abs()+y.abs(),4.);
            assert_eq!(entry_values(direction,1.),(1.,0.,0.));
        }
    }
    #[test]
    fn pulse_range() {
        let animation = pulse_animation();
        assert!(!animation.oneshot); assert_eq!(animation.duration,Duration::from_secs(2));
        for step in 0..=1000 { assert!((0.4..=0.8).contains(&(animation.easing)(step as f32 / 1000.))); }
        assert_eq!((animation.easing)(0.25),0.8);
        assert_eq!((animation.easing)(0.75),0.4);
    }
}
