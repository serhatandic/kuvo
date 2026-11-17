//! Bot wake-up chips and their schedule popover. Timing, notes and active wake turns come from
//! the bot's computer. Relative dates are formatted here like other times in the app.

use gpui::{
    AnyElement, App, AppContext as _, Context, DismissEvent, ElementId, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render,
    StatefulInteractiveElement, Styled, Task, Window, actions, div, px,
};
use kuvo_box::{
    BoxClient, Follow, Timing,
    areas::Bot,
    follow::Followed,
    wakes::{Cancel, Schedule},
};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, IconName, StyledType, TypeSize,
    menu::{PopoverMenu, PopoverMenuHandle},
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
        'm' => "minute",
        'h' => "hour",
        'd' => "day",
        'w' => "week",
        _ => return "Repeat".into(),
    };
    match every[..every.len() - last.len_utf8()].parse::<u32>() {
        Ok(1) => format!("Every {unit}"),
        Ok(count) => format!("Every {count} {unit}s"),
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

pub(crate) fn chip(
    id: impl Into<ElementId>,
    label: String,
    source: Source,
    above: bool,
    handle: PopoverMenuHandle,
) -> AnyElement {
    let trigger = Button::new("open-wake-schedule", label)
        .icon(IconName::Clock)
        .pill()
        .size(ButtonSize::Small)
        .tooltip("Show wake-ups and watches");
    let popover = PopoverMenu::view(id, trigger, move |_, cx| {
        cx.new(|cx| WakeSchedule::new(source.clone(), cx))
    });
    let popover = popover.with_handle(handle);
    if above {
        popover.above().into_any_element()
    } else {
        popover.align_right().into_any_element()
    }
}

pub(crate) struct WakeSchedule {
    focus: FocusHandle,
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
        div()
            .id(gpui::SharedString::from(key.clone()))
            .debug_selector(move || key)
            .flex()
            .flex_none()
            .items_start()
            .gap(space::S2)
            .px(space::S3)
            .py(space::S2)
            .border_t_1()
            .border_color(c.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(space::S0_5)
                    .child(
                        div()
                            .text_ui(TypeSize::Small)
                            .text_color(c.text)
                            .child(summary.to_owned()),
                    )
                    .child(
                        div()
                            .text_ui(TypeSize::Caption)
                            .text_color(c.text_muted)
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
}

impl Render for WakeSchedule {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let now = self.source.now.unwrap_or_else(time::now_ms);
        let mut rows = Vec::new();
        if let Some(schedule) = &self.schedule {
            for wake in &schedule.wakes {
                let detail = if schedule.paused {
                    "Paused".into()
                } else {
                    format!("Wake-up {}", relative(wake.due, now))
                };
                rows.push(self.row(
                    format!("wake-{}", wake.id),
                    &wake.summary,
                    format!("{detail} · {}", repeat(wake.every.as_deref())),
                    false,
                    wake.cancel_key.clone().map(Cancel::Wake),
                    cx,
                ));
            }
            for watch in &schedule.watches {
                let detail = if schedule.paused {
                    "Watch paused".into()
                } else {
                    watch.next.map_or_else(
                        || "Watch · First check pending".into(),
                        |next| format!("Next check {}", relative(next, now)),
                    )
                };
                rows.push(self.row(
                    format!("watch-{}", watch.letters),
                    &watch.summary,
                    format!("{detail} · {}", repeat(Some(&watch.every))),
                    true,
                    Some(Cancel::Watch(watch.letters.clone())),
                    cx,
                ));
            }
        }
        let note = if self.failed {
            Some(format!(
                "Couldn’t read wake-ups from Kuvo on {}. Check the connection. We’ll retry.",
                self.source.computer
            ))
        } else if self.cancel_failed {
            Some("Couldn’t cancel this entry. Check the schedule and try again.".into())
        } else if self.busy {
            Some("Canceling…".into())
        } else if rows.is_empty() {
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
                .px(space::S3)
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
        div()
            .id("wake-schedule")
            .debug_selector(|| "wake-schedule".into())
            .key_context("WakeSchedule")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Close, _, cx| cx.emit(DismissEvent)))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .occlude()
            .w(px(360.).min(window.viewport_size().width - px(16.)))
            .flex()
            .flex_col()
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
            .children(note.map(|note| {
                div()
                    .debug_selector(move || note_selector.into())
                    .px(space::S3)
                    .pb(space::S2)
                    .text_ui(TypeSize::Caption)
                    .text_color(c.text_muted)
                    .child(note)
            }))
            .child(
                div()
                    .id("wake-entries")
                    .debug_selector(|| "wake-entries".into())
                    .flex()
                    .flex_col()
                    .max_h(px(288.).min(window.viewport_size().height / 2.))
                    .overflow_y_scroll()
                    .children(rows),
            )
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
