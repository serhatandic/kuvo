//! Bot wake-up chips and their schedule popover. Timing, notes and active wake turns come from
//! the bot's computer. Relative dates are formatted here like other times in the app.

use gpui::{
    AnyElement, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, StatefulInteractiveElement,
    Styled, Task, Window, actions, div, prelude::FluentBuilder as _, px,
};
use kuvo_box::{
    BoxClient, Follow, Timing,
    areas::Bot,
    follow::Followed,
    wakes::{Cancel, Schedule},
};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    theme::{radius, space},
};

use crate::time;

actions!(wakes, [Close]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Close, Some("WakeSchedule"))]);
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Status {
    pub next: Option<f64>,
    pub watching: usize,
    pub paused: bool,
    pub waking: bool,
}

impl From<&Bot> for Status {
    fn from(bot: &Bot) -> Self {
        Self {
            next: bot.wakes,
            watching: bot.watching,
            paused: bot.paused,
            waking: bot.now.waking,
        }
    }
}

impl Status {
    pub fn scheduled(&self, now: f64) -> Option<String> {
        if self.next.is_none() && self.watching == 0 {
            return None;
        }
        Some(if self.paused {
            "Wake-ups paused".into()
        } else if let Some(next) = self.next {
            if next * 1_000. <= now {
                "Wake-up due".into()
            } else {
                format!("Wakes {}", relative(next, now))
            }
        } else {
            "Watches".into()
        })
    }

    pub fn chip_label(&self, now: f64) -> Option<String> {
        if self.waking {
            Some("Waking up…".into())
        } else {
            self.scheduled(now)
        }
    }
}

fn relative(seconds: f64, now: f64) -> String {
    let due = seconds * 1_000.;
    if due <= now {
        "due now".into()
    } else {
        format!("in {}", time::until(due, now))
    }
}

fn relative_capitalized(seconds: f64, now: f64) -> String {
    let due = seconds * 1_000.;
    if due <= now {
        "Due now".into()
    } else {
        format!("In {}", time::until(due, now))
    }
}

fn repeat(every: Option<&str>) -> String {
    let Some(every) = every else {
        return "Once".into();
    };
    if every == "weekdays" {
        return "Weekdays".into();
    }
    let Some(last) = every.chars().next_back() else {
        return "Repeat".into();
    };
    let unit = match last {
        'm' => "min",
        'h' => "hour",
        'd' => "day",
        'w' => "week",
        _ => return "Repeat".into(),
    };
    match every[..every.len() - last.len_utf8()].parse::<u32>() {
        Ok(1) => format!("Every {unit}"),
        Ok(count) => {
            if unit == "min" {
                format!("Every {count} min")
            } else {
                format!("Every {count} {unit}s")
            }
        }
        Err(_) => "Repeat".into(),
    }
}

#[derive(Clone)]
pub(crate) struct Source {
    pub client: BoxClient,
    pub session: String,
    pub computer: String,
    pub live: bool,
    pub now: Option<f64>,
}

/// How many rows of each group an embedded schedule shows before Show All.
const EMBEDDED_ROWS: usize = 3;

pub(crate) struct WakeSchedule {
    focus: FocusHandle,
    embedded: bool,
    /// Embedded in another popover, which scrolls as one: each group shows its first few rows
    /// until Show All.
    show_all: bool,
    source: Source,
    schedule: Option<Schedule>,
    confirm: Option<(Cancel, bool, String)>,
    busy: bool,
    failed: bool,
    cancel_failed: bool,
    _follow: Option<(Follow<Followed<Schedule>>, Task<()>)>,
}

impl Focusable for WakeSchedule {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<DismissEvent> for WakeSchedule {}

impl WakeSchedule {
    pub(crate) fn new(source: Source, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            focus: cx.focus_handle(),
            embedded: false,
            show_all: false,
            source,
            schedule: None,
            confirm: None,
            busy: false,
            failed: false,
            cancel_failed: false,
            _follow: None,
        };
        if view.source.live {
            let follow = view
                .source
                .client
                .follow_wake_schedule(&view.source.session, Timing::default());
            let updates = follow.updates().clone();
            let task = cx.spawn(async move |this, cx| {
                while let Ok(update) = updates.recv().await {
                    if this.update(cx, |this, cx| this.take(update, cx)).is_err() {
                        break;
                    }
                }
            });
            view._follow = Some((follow, task));
        } else {
            let answer = view.source.client.wake_schedule(&view.source.session);
            view.take(
                match answer {
                    Ok(schedule) => Followed::Now(schedule),
                    Err(error) => Followed::Failed(error),
                },
                cx,
            );
        }
        view
    }

    pub(crate) fn embedded(source: Source, cx: &mut Context<Self>) -> Self {
        let mut schedule = Self::new(source, cx);
        schedule.embedded = true;
        schedule
    }

    /// Plain-word count summary for the outer popover header (e.g. "2 wake-ups · 2 watches").
    pub(crate) fn summary_counts(&self) -> Option<String> {
        let schedule = self.schedule.as_ref()?;
        let wakes = schedule.wakes.len();
        let watches = schedule.watches.len();
        match (wakes, watches) {
            (0, 0) => None,
            (w, 0) => Some(if w == 1 { "1 wake-up".into() } else { format!("{w} wake-ups") }),
            (0, v) => Some(if v == 1 { "1 watch".into() } else { format!("{v} watches") }),
            (w, v) => Some(format!(
                "{} · {}",
                if w == 1 { "1 wake-up".into() } else { format!("{w} wake-ups") },
                if v == 1 { "1 watch".into() } else { format!("{v} watches") },
            )),
        }
    }

    fn take(&mut self, update: Followed<Schedule>, cx: &mut Context<Self>) {
        match update {
            Followed::Now(schedule) => {
                self.schedule = Some(schedule);
                self.failed = false;
            }
            Followed::Failed(_) => {
                self.failed = true;
                self.confirm = None;
            }
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.failed {
            return;
        }
        let Some((cancel, _, _)) = self.confirm.take() else {
            return;
        };
        self.busy = true;
        self.cancel_failed = false;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let (result, fresh) = cx
                .background_executor()
                .spawn(async move {
                    let result = source.client.cancel_wake(&source.session, &cancel);
                    let fresh =
                        (!source.live).then(|| source.client.wake_schedule(&source.session));
                    (result, fresh)
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.cancel_failed = result.is_err();
                if let Some(Ok(fresh)) = fresh {
                    this.schedule = Some(fresh);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn row(
        &self,
        key: String,
        summary: &str,
        detail: String,
        watch: bool,
        cancel: Option<Cancel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = cx.theme().colors.clone();
        let pad_x = if self.embedded { space::S2 } else { space::S3 };
        div()
            .id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || key)
            .flex()
            .flex_none()
            .items_start()
            .gap(space::S2)
            .px(pad_x)
            .py(space::S1_5)
            .rounded(radius::NAV)
            .hover(move |s| s.bg(c.hover))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .min_w_0()
                            .text_ui(TypeSize::Small)
                            .text_color(c.text)
                            .child(summary.to_owned()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(detail),
                    ),
            )
            .children(cancel.map(|cancel| {
                let summary = summary.to_owned();
                Button::icon_only(
                    "cancel-entry",
                    IconName::Close,
                    if watch {
                        "Remove watch"
                    } else {
                        "Cancel wake-up"
                    },
                )
                .ghost()
                .size(ButtonSize::Small)
                .tooltip(if watch {
                    "Remove watch…"
                } else {
                    "Cancel wake-up…"
                })
                .disabled(self.failed || self.busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.confirm = Some((cancel.clone(), watch, summary.clone()));
                    this.cancel_failed = false;
                    cx.notify();
                }))
            }))
            .into_any_element()
    }

    fn subgroup_header(
        &self,
        selector: &'static str,
        icon: IconName,
        icon_color: gpui::Hsla,
        title: &'static str,
        subtitle: &'static str,
        count: usize,
        cx: &App,
    ) -> AnyElement {
        let c = &cx.theme().colors;
        let pad_x = if self.embedded { space::S2 } else { space::S3 };
        div()
            .debug_selector(move || selector.into())
            .px(pad_x)
            .pt(px(2.))
            .pb(px(2.))
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S2)
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(space::S1_5)
                    .child(Icon::new(icon).size(IconSize::XSmall).color(icon_color))
                    .child(
                        div()
                            .flex_none()
                            .text_ui(TypeSize::Caption)
                            .medium()
                            .text_color(c.text)
                            .child(title),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_faint)
                            .child(format!("· {subtitle}")),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_faint)
                    .child(count.to_string()),
            )
            .into_any_element()
    }
}

impl Render for WakeSchedule {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let now = self.source.now.unwrap_or_else(time::now_ms);
        let mut wake_rows = Vec::new();
        let mut watch_rows = Vec::new();
        if let Some(schedule) = &self.schedule {
            for wake in &schedule.wakes {
                let timing = if schedule.paused {
                    "Paused".into()
                } else {
                    relative_capitalized(wake.due, now)
                };
                wake_rows.push(self.row(
                    format!("wake-{}", wake.id),
                    &wake.summary,
                    format!("{timing} · {}", repeat(wake.every.as_deref())),
                    false,
                    wake.cancel_key.clone().map(Cancel::Wake),
                    cx,
                ));
            }
            for watch in &schedule.watches {
                let detail = if schedule.paused {
                    format!("Paused · {}", repeat(Some(&watch.every)))
                } else {
                    match watch.next {
                        Some(next) => format!(
                            "{} · Next {}",
                            repeat(Some(&watch.every)),
                            relative(next, now)
                        ),
                        None => format!("{} · First check soon", repeat(Some(&watch.every))),
                    }
                };
                watch_rows.push(self.row(
                    format!("watch-{}", watch.letters),
                    &watch.summary,
                    detail,
                    true,
                    Some(Cancel::Watch(watch.letters.clone())),
                    cx,
                ));
            }
        }
        let has_wakes = !wake_rows.is_empty();
        let has_watches = !watch_rows.is_empty();
        let wakes_count = wake_rows.len();
        let watches_count = watch_rows.len();
        // In another popover, nothing scrolls on its own: a few rows of each, then Show All.
        let capped = self.embedded && !self.show_all;
        let hidden = if capped { wakes_count.saturating_sub(EMBEDDED_ROWS) + watches_count.saturating_sub(EMBEDDED_ROWS) } else { 0 };
        if capped {
            wake_rows.truncate(EMBEDDED_ROWS);
            watch_rows.truncate(EMBEDDED_ROWS);
        }
        let empty = !has_wakes && !has_watches;
        let note = if self.failed {
            Some(format!(
                "Couldn’t read wake-ups from Kuvo on {}. Check the connection. We’ll retry.",
                self.source.computer
            ))
        } else if self.cancel_failed {
            Some("Couldn’t cancel this entry. Check the schedule and try again.".into())
        } else if self.busy {
            Some("Canceling…".into())
        } else if empty {
            Some(
                if self.schedule.is_some() {
                    "No wake-ups or watches scheduled."
                } else {
                    "Loading wake-ups…"
                }
                .into(),
            )
        } else {
            None
        };
        let note_selector = if self.failed {
            "wake-schedule-error"
        } else {
            "wake-schedule-note"
        };
        let confirmation = self.confirm.as_ref().map(|(_, watch, summary)| {
            let (question, button) = if *watch {
                ("Remove this watch?", "Remove watch")
            } else {
                ("Cancel this wake-up?", "Cancel wake-up")
            };
            div()
                .border_t_1()
                .border_color(c.border)
                .px(if self.embedded { space::S2 } else { space::S3 })
                .py(space::S2)
                .flex()
                .flex_col()
                .gap(space::S2)
                .child(div().text_ui(TypeSize::Small).medium().child(question))
                .child(
                    div()
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(summary.clone()),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(space::S2)
                        .child(
                            Button::new("keep-entry", "Keep")
                                .ghost()
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirm = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("confirm-cancel-entry", button)
                                .destructive()
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        ),
                )
        });
        let pad_x = if self.embedded { space::S2 } else { space::S3 };
        div()
            .id("wake-schedule")
            .debug_selector(|| "wake-schedule".into())
            .flex()
            .flex_col()
            .when(!self.embedded, |d| {
                d.key_context("WakeSchedule")
                    .track_focus(&self.focus)
                    .on_action(cx.listener(|_, _: &Close, _, cx| cx.emit(DismissEvent)))
                    .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
                    .occlude()
                    .w(px(360.).min(window.viewport_size().width - px(16.)))
                    .overflow_hidden()
                    .rounded(radius::POPOVER)
                    .bg(c.elevated)
                    .border_1()
                    .border_color(c.border)
                    .shadow(cx.theme().elevated_shadow())
                    .child(
                        div()
                            .px(space::S3)
                            .py(space::S2)
                            .text_ui(TypeSize::Small)
                            .medium()
                            .text_color(c.text)
                            .child("Wake-ups & Watches"),
                    )
            })
            .children(note.map(|note| {
                div()
                    .debug_selector(move || note_selector.into())
                    .px(pad_x)
                    .py(space::S1)
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_muted)
                    .child(note)
            }))
            .when(!empty, |d| {
                d.child(
                    div()
                        .id("wake-entries")
                        .debug_selector(|| "wake-entries".into())
                        .flex()
                        .flex_col()
                        .gap(space::S1_5)
                        .when(!self.embedded, |d| d.max_h(px(288.).min(window.viewport_size().height / 2.)).overflow_y_scroll())
                        .when(has_wakes, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(self.subgroup_header(
                                        "wake-group-wakeups",
                                        IconName::Clock,
                                        c.accent,
                                        "Wake-ups",
                                        "Scheduled prompts",
                                        wakes_count,
                                        cx,
                                    ))
                                    .children(wake_rows),
                            )
                        })
                        .when(has_watches, |d| {
                            d.child(
                                div()
                                    .when(has_wakes, |d| {
                                        d.pt(space::S1_5).border_t_1().border_color(c.border)
                                    })
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(self.subgroup_header(
                                        "wake-group-watches",
                                        IconName::Eye,
                                        c.success,
                                        "Watches",
                                        "Recurring checks",
                                        watches_count,
                                        cx,
                                    ))
                                    .children(watch_rows),
                            )
                        }),
                )
            })
            .when(hidden > 0, |d| {
                d.child(
                    div().debug_selector(|| "wake-show-all".into()).px(pad_x).pt(space::S1).child(
                        Button::new("wake-show-all", format!("Show All {}", wakes_count + watches_count))
                            .ghost()
                            .quiet()
                            .size(ButtonSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_all = true;
                                cx.notify();
                            })),
                    ),
                )
            })
            .children(confirmation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_labels_distinguish_schedule_from_active_turn() {
        let now = 1_200_000.;
        assert_eq!(Status::default().chip_label(now), None);
        let mut status = Status {
            next: Some(1_380.),
            ..Status::default()
        };
        assert_eq!(status.chip_label(now).as_deref(), Some("Wakes in 3 min"));
        assert_eq!(
            status.chip_label(1_380_000.).as_deref(),
            Some("Wake-up due")
        );
        status.waking = true;
        assert_eq!(status.chip_label(now).as_deref(), Some("Waking up…"));
        status.waking = false;
        status.paused = true;
        assert_eq!(status.chip_label(now).as_deref(), Some("Wake-ups paused"));
        assert_eq!(
            Status {
                watching: 1,
                ..Status::default()
            }
            .chip_label(now)
            .as_deref(),
            Some("Watches")
        );
    }
}
