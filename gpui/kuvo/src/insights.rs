//! Insights displays the usage index on the selected computer.

use std::time::Duration;

use gpui::{AnyElement, App, Context, FocusHandle, IntoElement, Render, ScrollHandle, SharedString, Subscription, Task, Window, div, prelude::*, px};
use kuvo_box::{BoxClient, models::OfferedModel, paths, transport::Request};
use kuvo_ui::{ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    icon::{Mark, provider_mark as mark_asset}, menu::{Menu, MenuEntry, PopoverMenu, Tooltip}, theme::{radius, space}};
use serde::Deserialize;

const ROUTE: &str = "kuvo/agents/api/kuvo/insights";

#[derive(Clone, Default, PartialEq, Eq)]
struct Filters {
    days: u16,
    computer: String,
    bot: String,
    ring: Option<crate::area::UsageFilter>,
}

impl Filters {
    fn query(&self, zone: &str, offset: usize) -> String {
        let mut query = format!("?period={}&zone={}&offset={offset}", self.days, paths::query_value(zone));
        for (key, value) in [("computer", self.computer.as_str()), ("bot", self.bot.as_str())] {
            if !value.is_empty() {
                query.push_str(&format!("&{key}={}", paths::query_value(value)));
            }
        }
        if let Some(filter) = &self.ring {
            let (key, value) = filter.account.as_deref().map_or(("provider", filter.provider.as_str()), |account| ("account", account));
            query.push_str(&format!("&{key}={}", paths::query_value(value)));
        }
        query
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Metric {
    #[default]
    Cost,
    Tokens,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Breakdown {
    #[default]
    Model,
    Agent,
    Day,
    Skill,
    Session,
}

#[derive(Deserialize)]
struct Answer<T> {
    data: T,
}

#[derive(Clone, Default, Deserialize)]
struct Measures {
    cost: Option<f64>,
    tokens: Option<f64>,
    cached: Option<f64>,
    input: Option<f64>,
    creation: Option<f64>,
    output: Option<f64>,
    processed: Option<f64>,
    messages: u64,
    sessions: Option<u64>,
    unpriced: u64,
    calls: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    name: String,
    provider: Option<String>,
    cost_share: Option<f64>,
    token_share: Option<f64>,
    #[serde(flatten)]
    usage: Measures,
}

#[derive(Default, Deserialize)]
struct Breakdowns {
    model: Vec<Row>,
    agent: Vec<Row>,
    day: Vec<Row>,
    skill: Vec<Row>,
}

#[derive(Clone, Deserialize)]
struct Choice {
    key: String,
    name: String,
}

#[derive(Deserialize)]
struct Coverage {
    name: String,
    scanned: Option<f64>,
    stale: bool,
    issues: Vec<String>,
}

#[derive(Deserialize)]
struct Overview {
    state: String,
    scanning: bool,
    computers: Vec<Choice>,
    bots: Vec<Choice>,
    totals: Measures,
    daily: Vec<Row>,
    breakdowns: Breakdowns,
    coverage: Vec<Coverage>,
    unavailable: Vec<String>,
}

#[derive(Clone, Deserialize)]
struct Session {
    session: String,
    title: Option<String>,
    computer: String,
    home: Option<String>,
    at: f64,
    #[serde(flatten)]
    usage: Measures,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sessions {
    sessions: Vec<Session>,
    next: Option<usize>,
    local_computers: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Call {
    at: f64,
    model: Option<String>,
    provider: String,
    token_count: Option<f64>,
    computer: String,
    cost: Option<f64>,
}

#[derive(Default, Deserialize)]
struct Calls {
    calls: Vec<Call>,
    next: Option<usize>,
}

#[derive(Clone)]
pub struct OpenSession {
    pub session: String,
    pub computer: Option<String>,
}
impl gpui::EventEmitter<OpenSession> for InsightsView {}

pub struct InsightsView {
    client: BoxClient,
    filters: Filters,
    metric: Metric,
    breakdown: Breakdown,
    zone: String,
    overview: Option<Overview>,
    sessions: Sessions,
    catalog: Vec<OfferedModel>,
    catalog_task: Option<Task<()>>,
    selected: Option<Session>,
    calls: Calls,
    problem: Option<String>,
    visible: bool,
    reading: bool,
    task: Option<Task<()>>,
    pages: Option<Task<()>>,
    survey: Option<Task<()>>,
    chart_focus: FocusHandle,
    scroll: ScrollHandle,
    session_list_offset: gpui::Point<gpui::Pixels>,
    chart_day: Option<usize>,
    _focus: Subscription,
}

impl InsightsView {
    pub fn new(client: BoxClient, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chart_focus = cx.focus_handle().tab_index(0).tab_stop(true);
        let focus = cx.on_focus(&chart_focus, window, |this, _, cx| {
            this.chart_day = this.overview.as_ref().and_then(|view| view.daily.len().checked_sub(1));
            cx.notify();
        });
        Self {
            client, filters: Filters { days: 30, ..Filters::default() },
            metric: Metric::Cost, breakdown: Breakdown::Model, zone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
            overview: None, sessions: Sessions::default(), catalog: Vec::new(), catalog_task: None, selected: None, calls: Calls::default(), problem: None,
            visible: false, reading: false, task: None, pages: None, survey: None, chart_focus, chart_day: None, scroll: ScrollHandle::new(), session_list_offset: gpui::point(px(0.), px(0.)), _focus: focus,
        }
    }

    #[cfg(feature = "stand-in")]
    pub fn sample(&mut self, slug: &str, cx: &mut Context<Self>) {
        match slug {
            "insights-tokens" => self.metric = Metric::Tokens,
            "insights-computer" => self.filters.computer = "key_devbox".into(),
            "insights-breakdown-skill" => self.breakdown = Breakdown::Skill,
            "insights-session-drilldown" => self.breakdown = Breakdown::Session,
            "insights-ring-filter" => self.filters.ring = Some(crate::area::UsageFilter {
                title: "Claude Code".into(), provider: "claude".into(), account: Some("claude#sample".into()),
            }),
            _ => {}
        }
        self.catalog = self.client.offered_models(None, None).map(|offered| offered.models).unwrap_or_default();
        let query = self.filters.query(&self.zone, 0);
        self.overview = self.client.get::<Answer<Overview>>(&format!("{ROUTE}{query}")).ok().map(|answer| answer.data);
        self.sessions = self.client.get::<Answer<Sessions>>(&format!("{ROUTE}/sessions{query}")).map(|answer| answer.data).unwrap_or_default();
        if slug == "insights-session-drilldown" {
            self.selected = self.sessions.sessions.first().cloned();
            if let Some(session) = self.selected.as_ref() {
                let route = format!("{ROUTE}/sessions/{}{query}", paths::segment(&session.session));
                self.calls = self.client.get::<Answer<Calls>>(&route).map(|answer| answer.data).unwrap_or_default();
            }
        }
        cx.notify();
    }

    pub fn shown(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible { return; }
        self.visible = visible;
        if !visible {
            self.survey = None;
            return;
        }
        if self.overview.is_none() { self.load(false, cx); }
        if self.catalog_task.is_none() {
            let client = self.client.clone();
            let read = cx.background_executor().spawn(async move { client.offered_models(None, None) });
            self.catalog_task = Some(cx.spawn(async move |this, cx| {
                let offered = read.await;
                this.update(cx, |this, cx| {
                    this.catalog_task = None;
                    if let Ok(offered) = offered {
                        this.catalog = offered.models;
                        cx.notify();
                    }
                }).ok();
            }));
        }
        self.survey = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(delay) = this.read_with(cx, |this, _| {
                    if this.overview.as_ref().is_some_and(|view| view.scanning || view.state == "loading") { 2 } else { 20 }
                }) else { break };
                cx.background_executor().timer(Duration::from_secs(delay)).await;
                if this.update(cx, |this, cx| {
                    if !this.reading && this.selected.is_none() && this.sessions.sessions.len() <= 50 {
                        this.load(false, cx);
                    }
                }).is_err() { break; }
            }
        }));
    }

    pub fn ring(&mut self, filter: Option<crate::area::UsageFilter>, cx: &mut Context<Self>) {
        self.filters.ring = filter;
        self.filters.computer.clear();
        self.filters.bot.clear();
        self.reset(cx);
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.pages = None;
        self.sessions = Sessions::default();
        self.calls = Calls::default();
        self.chart_day = None;
        self.overview = None;
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.load(false, cx);
    }

    fn load(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.reading = true;
        let client = self.client.clone();
        let query = self.filters.query(&self.zone, 0);
        let read = cx.background_executor().spawn(async move {
            if refresh {
                client.value::<serde_json::Value>(&Request::post(format!("{ROUTE}/refresh"), serde_json::json!({})))?;
            }
            let overview = client.get::<Answer<Overview>>(&format!("{ROUTE}{query}"))?.data;
            let sessions = client.get::<Answer<Sessions>>(&format!("{ROUTE}/sessions{query}"))?.data;
            Ok::<_, kuvo_box::Error>((overview, sessions))
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.reading = false;
                match result {
                    Ok((overview, sessions)) => {
                        this.overview = Some(overview);
                        if this.sessions.sessions.len() <= 50 && this.pages.is_none() {
                            this.sessions = sessions;
                        }
                        this.problem = None;
                    }
                    Err(error) => this.problem = Some(kuvo_box::accounts::said(&error)),
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    fn page(&mut self, offset: usize, cx: &mut Context<Self>) {
        let query = self.filters.query(&self.zone, offset);
        let client = self.client.clone();
        let selected = self.selected.as_ref().map(|session| session.session.clone());
        let read = cx.background_executor().spawn(async move {
            match selected {
                Some(id) => client.get::<Answer<Calls>>(&format!("{ROUTE}/sessions/{}{query}", paths::segment(&id))).map(|answer| (Some(answer.data), None)),
                None => client.get::<Answer<Sessions>>(&format!("{ROUTE}/sessions{query}")).map(|answer| (None, Some(answer.data))),
            }
        });
        self.pages = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.pages = None;
                match result {
                    Ok((Some(mut calls), _)) => {
                        this.calls.calls.append(&mut calls.calls);
                        this.calls.next = calls.next;
                    }
                    Ok((_, Some(mut sessions))) => {
                        this.sessions.sessions.append(&mut sessions.sessions);
                        this.sessions.next = sessions.next;
                    }
                    Ok(_) => {}
                    Err(error) => this.problem = Some(kuvo_box::accounts::said(&error)),
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    fn picker(
        &self, id: &'static str, title: &str, choices: Vec<Choice>, chosen: &str,
        pick: fn(&mut Self, &str, &mut Context<Self>), cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &cx.theme().colors;
        let shown = choices.iter().find(|item| item.key == chosen).map_or(title, |item| item.name.as_str());
        let trigger = div().id(id).flex().items_center().gap(space::S1_5).px(space::S2).h(px(28.)).rounded(radius::NAV)
            .cursor_pointer().border_1().border_color(c.border).bg(c.card).hover(|d| d.bg(c.hover))
            .child(div().truncate().max_w(px(170.)).child(shown.to_owned()))
            .child(Icon::new(IconName::ChevronDown).color(c.icon).size(IconSize::XSmall)).into_any_element();
        let this = cx.entity().downgrade();
        let chosen = chosen.to_owned();
        PopoverMenu::new(SharedString::from(format!("{id}-menu")), trigger, move |_, cx| {
            cx.new(|cx| choices.iter().fold(Menu::new(cx).width(px(210.)), |menu, item| {
                let (this, key) = (this.clone(), item.key.clone());
                menu.entry(MenuEntry::new(item.name.clone()).checked(key == chosen).on_select(move |_, cx| {
                    this.update(cx, |this, cx| pick(this, &key, cx)).ok();
                }))
            }))
        }).into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let choices = |title: &str, options: Option<&Vec<Choice>>| {
            let mut list = vec![Choice { key: String::new(), name: title.into() }];
            list.extend(options.into_iter().flatten().cloned());
            list
        };
        let overview = self.overview.as_ref();
        let computers = self.picker("insights-computer", "All computers", choices("All computers", overview.map(|v| &v.computers)), &self.filters.computer,
            |this, key, cx| { if this.filters.computer != key { this.filters.computer = key.into(); this.reset(cx); } }, cx);
        let bots = self.picker("insights-bot", "All activity", choices("All activity", overview.map(|v| &v.bots)), &self.filters.bot,
            |this, key, cx| { if this.filters.bot != key { this.filters.bot = key.into(); this.reset(cx); } }, cx);
        let metric = self.picker("insights-metric", "Cost", ["Cost", "Tokens"].into_iter().map(|name| Choice { key: name.into(), name: name.into() }).collect(),
            if self.metric == Metric::Cost { "Cost" } else { "Tokens" }, |this, key, cx| { this.metric = if key == "Cost" { Metric::Cost } else { Metric::Tokens }; cx.notify(); }, cx);
        let days = self.picker("insights-period", "30 days", [7, 30, 90, 365].into_iter().map(|days| Choice { key: days.to_string(), name: format!("{days} days") }).collect(),
            &self.filters.days.to_string(), |this, key, cx| { if let Ok(days) = key.parse() && this.filters.days != days { this.filters.days = days; this.reset(cx); } }, cx);
        let busy = overview.is_some_and(|view| view.scanning);
        div().flex().flex_wrap().items_center().justify_end().gap(space::S2)
            .child(div().flex().flex_wrap().items_center().gap(space::S1_5).child(computers).child(bots).child(metric).child(days)
                .child(Button::icon_only("insights-refresh", IconName::Refresh, "Refresh insights").ghost().size(ButtonSize::Small)
                    .disabled(busy || self.reading).on_click(cx.listener(|this, _, _, cx| this.load(true, cx)))))
            .when_some(self.filters.ring.as_ref(), |d, filter| d.child(
                div().w_full().child(Button::new("insights-ring-filter", filter.title.clone()).icon(IconName::Close).ghost().size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| { this.filters.ring = None; this.reset(cx); })))
            ))
            .when(busy && overview.is_some_and(|view| view.state != "loading"), |d| d.child(div().w_full().text_color(c.text_muted).child("Refreshing…")))
    }

    fn chart(&self, overview: &Overview, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let metric = self.metric;
        let samples: Vec<_> = overview.daily.iter().map(|row| value(&row.usage, metric).unwrap_or(0.)).collect();
        let axis = ChartAxis::new(samples.iter().copied().fold(0.0f64, f64::max), metric);
        let active = self.chart_day.and_then(|i| overview.daily.get(i));
        let selected = self.chart_day.filter(|&i| i < samples.len());
        let days = samples.len();
        let tint = gpui::Hsla {
            h: c.accent.h * 0.16 + 0.76 * 0.84,
            s: c.accent.s * 0.56,
            ..c.accent
        };
        let axis_width = px(58.);
        let step = chart_date_stride(days, f32::from(window.viewport_size().width).min(1240.));
        let last = days.saturating_sub(1);
        card(cx).flex_1()
            .debug_selector(|| "insights-chart-card".into())
            .child(div().flex().items_center().justify_between().gap(space::S2)
                .child(div().flex_none().font_weight(gpui::FontWeight::SEMIBOLD).child(if metric == Metric::Cost { "Daily cost" } else { "Daily tokens" }))
                .child(div().flex_1().min_w_0().truncate().text_align(gpui::TextAlign::Right).text_color(c.text_muted).text_ui(TypeSize::Caption)
                    .children(active.map(|row| div().debug_selector(|| "insights-chart-reading".into())
                        .child(format!("{} · {}", date_label(&row.name, false), day_value(&row.usage, metric))))))
                .child(Button::new("insights-view-daily", "View daily data").ghost().quiet().size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| { this.breakdown = Breakdown::Day; cx.notify(); }))))
            .child(div().flex().flex_1().min_h(px(152.)).my(px(7.)).gap(space::S2)
                .child(div().w(axis_width).flex_none().relative().text_color(c.text_muted).text_ui(TypeSize::Caption)
                    .children((0..=axis.intervals).map(|i| div().absolute().right_0().top(gpui::relative(axis.position(i)))
                        .mt(px(-8.)).child(axis.label(i, metric)))))
                .child(div().id("insights-chart").debug_selector(|| "insights-chart".into())
                    .relative().flex_1().min_w_0().track_focus(&self.chart_focus)
                    .focus_visible(move |d| d.shadow(kuvo_ui::button::focus_ring(c.focus_ring)))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        let len = this.overview.as_ref().map_or(0, |view| view.daily.len());
                        if len == 0 { return; }
                        let current = this.chart_day.unwrap_or(len - 1).min(len - 1);
                        let day = match event.keystroke.key.as_str() {
                            "left" => current.saturating_sub(1),
                            "right" => (current + 1).min(len - 1),
                            "home" => 0,
                            "end" => len - 1,
                            _ => return,
                        };
                        this.chart_day = Some(day);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(gpui::canvas(|_, _, _| {}, move |bounds, (), window, _| {
                        paint_chart(bounds, &samples, axis, selected, tint, c.border, window);
                    }).absolute().size_full())
                    .children(overview.daily.iter().enumerate().map(|(idx, row)| {
                        let denominator = last.max(1) as f32;
                        let left = ((idx as f32 - 0.5) / denominator).max(0.);
                        let right = if idx == last { 1. } else { (idx as f32 + 0.5) / denominator };
                        let label = format!("{} · {}", date_label(&row.name, false), day_value(&row.usage, metric));
                        div().id(("insights-day", idx)).absolute().top_0().h_full().left(gpui::relative(left)).w(gpui::relative(right - left))
                            .debug_selector(move || format!("insights-day-{idx}").into())
                            .on_hover(cx.listener(move |this, hovered, window, cx| {
                                if *hovered {
                                    this.chart_day = Some(idx);
                                    cx.notify();
                                } else if !this.chart_focus.is_focused(window) && this.chart_day == Some(idx) {
                                    this.chart_day = None;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.chart_focus, cx);
                                this.chart_day = Some(idx);
                                cx.notify();
                            }))
                            .tooltip(move |_, cx| Tooltip::new(label.clone()).build(cx))
                    }))))
            .child(div().h(px(18.)).ml(axis_width + space::S2).relative().text_color(c.text_muted).text_ui(TypeSize::Caption)
                .children(overview.daily.iter().enumerate().rev().step_by(step).map(|(idx, row)| {
                    let label = div().absolute().top_0().whitespace_nowrap().child(date_label(&row.name, true));
                    if idx == last { label.right_0() }
                    else if idx == 0 { label.left_0() }
                    else { label.left(gpui::relative(idx as f32 / last as f32)).w(px(54.)).ml(px(-27.)).text_center() }
                })))
    }

    fn overview(&self, overview: &Overview, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let totals = &overview.totals;
        let metric = self.metric;
        let session_count = totals.sessions.unwrap_or_default();
        let known = if totals.calls == 0 { Some(0.) } else { value(totals, metric) };
        let counts = [
            ("Without cache reads", totals.tokens), ("With cache reads", totals.processed),
            ("Cached input", totals.cached), ("Uncached input", totals.input),
            ("Cache creation", totals.creation), ("Output", totals.output), ("User messages", Some(totals.messages as f64)),
        ];
        let agents = overview.breakdowns.agent.iter().map(|row| {
            let name = agent_label(row);
            div().flex().items_center().gap(space::S1_5).children(row.provider.as_deref().and_then(provider_mark))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(format_metric(value(&row.usage, metric), metric))
        });
        let narrow = window.viewport_size().width < px(640.);
        let summary = card(cx).when(!narrow, |d| d.w(px(292.)).flex_none())
            .debug_selector(|| "insights-summary".into())
            .child(div().debug_selector(|| "insights-total-label".into()).text_color(c.text_muted).child(if metric == Metric::Cost { "Estimated API cost · USD" } else { "Tokens without cache reads" }))
            .child(div().debug_selector(|| "insights-total-value".into()).text_size(px(30.)).line_height(px(36.)).my(space::S0_5).font_weight(gpui::FontWeight::SEMIBOLD).child(format_metric(known, metric)))
            .child(div().debug_selector(|| "insights-total-sessions".into()).text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("{session_count} {}", if session_count == 1 { "session" } else { "sessions" })))
            .when(!overview.breakdowns.agent.is_empty(), |d| d.child(div().flex().flex_col().gap(space::S0_5).mt(space::S2).children(agents)))
            .child(div().mt(space::S2).pt(space::S1_5).border_t_1().border_color(c.border).flex().flex_col().gap(space::S0_5)
                .children(counts.into_iter().map(|(title, count)| {
                    div().flex().justify_between().items_center().gap(space::S2)
                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(title))
                        .child(div().medium().child(format_count(count.or_else(|| (totals.calls == 0).then_some(0.)))))
                })))
            .child(div().mt(space::S1_5).text_ui(TypeSize::Caption).text_color(c.text_muted).child("Estimates use current API rates."))
            .when(totals.unpriced > 0, |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("Cost unavailable for {} calls", totals.unpriced))));
        let status = match overview.state.as_str() {
            "loading" => Some("Reading usage…"),
            "empty" => Some("No Kuvo activity for these filters."),
            "partial" => Some("Some usage details are unavailable."),
            "stale" => Some("Some computers have older readings."),
            _ => None,
        };
        div().flex().flex_col().gap(space::S3)
            .when_some(status, |d, status| d.child(card(cx).gap(space::S1)
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(status))
                .children(overview.coverage.iter().filter_map(|entry| {
                    let words = if entry.stale { "Last reading is out of date".to_owned() }
                        else if entry.scanned.is_none() { "Waiting for its first reading".to_owned() }
                        else if !entry.issues.is_empty() { format!("Couldn’t read all {} usage", entry.issues.iter().map(|p| provider_name(p)).collect::<Vec<_>>().join(", ")) }
                        else { return None };
                    Some(div().text_color(c.text_muted).child(format!("{} · {words}", entry.name)))
                }))
                .when(matches!(overview.state.as_str(), "partial" | "stale"), |d| {
                    d.children(overview.unavailable.iter().map(|words| div().text_color(c.text_muted).child(words.clone())))
                })
            ))
            .when(overview.state != "loading", |d| d
                .child(div().flex().when(narrow, |d| d.flex_col()).gap(space::S3).child(summary).child(self.chart(overview, window, cx)))
                .child(self.breakdown(overview, cx)))
    }

    fn breakdown(&self, overview: &Overview, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let (title, rows) = match self.breakdown {
            Breakdown::Model => ("Model", &overview.breakdowns.model[..]),
            Breakdown::Agent => ("Agent", &overview.breakdowns.agent[..]),
            Breakdown::Day => ("Date", &overview.breakdowns.day[..]),
            Breakdown::Skill => ("Skill", &overview.breakdowns.skill[..]),
            Breakdown::Session => ("Session", &[][..]),
        };
        let modes = [
            (Breakdown::Model, "Model"), (Breakdown::Agent, "Agent"), (Breakdown::Day, "Day"),
            (Breakdown::Skill, "Skill"), (Breakdown::Session, "Session"),
        ];
        card(cx).debug_selector(|| "insights-table".into())
            .child(div().flex().flex_wrap().gap(space::S1).children(modes.into_iter().map(|(mode, label)| {
                div().when(self.breakdown == mode, |d| d.rounded(radius::NAV).bg(c.selected)
                    .debug_selector(move || format!("insights-active-tab-{label}").into())).child(
                    Button::new(SharedString::from(format!("insights-by-{label}")), label).ghost().size(ButtonSize::Small)
                        .on_click(cx.listener(move |this, _, _, cx| { this.breakdown = mode; cx.notify(); })),
                )
            })))
            .when(self.breakdown == Breakdown::Skill, |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_muted)
                .child("A call can appear under more than one skill.")))
            .child(table_header([title, "Estimated cost", "Share", "Tokens"], cx))
            .when(self.breakdown == Breakdown::Session, |d| d
                .children(self.session_rows(&overview.totals, cx))
                .when(self.sessions.sessions.is_empty(), |d| d.child(div().py(space::S2).text_color(c.text_muted).child("No sessions in this range.")))
                .when_some(self.sessions.next, |d, next| d.child(self.more("insights-more-sessions", next, cx))))
            .when(self.breakdown != Breakdown::Session, |d| d
                .children(rows.iter().filter(|row| self.breakdown == Breakdown::Day || row.usage.calls > 0).map(|row| {
                    let share = if self.metric == Metric::Cost { row.cost_share } else { row.token_share };
                    let name = match self.breakdown {
                        Breakdown::Model => model_label(&self.catalog, row.provider.as_deref(), &row.name),
                        Breakdown::Agent => agent_label(row),
                        Breakdown::Day => date_label(&row.name, false),
                        _ => row.name.clone(),
                    };
                    let shown = |metric| if self.breakdown == Breakdown::Day { day_value(&row.usage, metric) }
                        else { format_metric(value(&row.usage, metric), metric) };
                    table_row(label_with_mark(row.provider.as_deref(), &name),
                        [shown(Metric::Cost), format_share(share), shown(Metric::Tokens)], cx)
                }))
                .when(rows.is_empty(), |d| d.child(div().py(space::S2).text_color(c.text_muted).child(
                    if self.breakdown == Breakdown::Skill { "No skill use was recorded." } else { "No calls recorded." }
                ))))
    }

    fn session_rows(&self, totals: &Measures, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let total = value(totals, self.metric).filter(|total| *total > 0.);
        self.sessions.sessions.iter().enumerate().map(|(i, session)| {
            let words = session.title.as_deref().filter(|title| !title.is_empty()).unwrap_or("Untitled session");
            let selected = session.clone();
            let share = value(&session.usage, self.metric).zip(total).map(|(amount, total)| amount / total);
            div().id(("insights-session", i)).debug_selector(move || format!("insights-session-{i}").into())
                .cursor_pointer().hover(|d| d.bg(c.hover)).tab_index(0)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.calls = Calls::default();
                    this.session_list_offset = this.scroll.offset();
                    this.scroll.set_offset(gpui::point(px(0.), px(0.)));
                    this.selected = Some(selected.clone());
                    this.problem = None;
                    this.page(0, cx);
                }))
                .child(table_row(div().py(space::S0_5).flex().flex_col()
                    .child(div().truncate().child(words.to_owned()))
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(short_timestamp(session.at))),
                    [format_cost(session.usage.cost), format_share(share), format_count(session.usage.tokens)], cx))
                .into_any_element()
        }).collect()
    }

    fn more(&self, id: &'static str, next: usize, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new(id, "Show more").ghost().size(ButtonSize::Small).disabled(self.pages.is_some())
            .on_click(cx.listener(move |this, _, _, cx| this.page(next, cx)))
    }

    fn detail(&self, session: &Session, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let home = session.home.as_ref().unwrap_or(&session.computer);
        let open = OpenSession {
            session: session.session.clone(),
            computer: (!self.sessions.local_computers.contains(home)).then(|| home.clone()),
        };
        card(cx)
            .child(div().flex().items_center().justify_between().gap(space::S2)
                .child(Button::new("insights-back", "Sessions").icon(IconName::ArrowLeft).ghost().size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| { this.selected = None; this.pages = None; this.problem = None; this.scroll.set_offset(this.session_list_offset); cx.notify(); })))
                .child(Button::new("insights-open-session", "Open session").icon(IconName::External).ghost().size(ButtonSize::Small)
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(open.clone())))))
            .child(div().py(space::S2).text_size(px(18.)).font_weight(gpui::FontWeight::SEMIBOLD).child(session.title.clone().filter(|name| !name.is_empty()).unwrap_or_else(|| "Untitled session".into())))
            .child(table_header(["Call", "Computer", "Estimated cost", "Tokens"], cx))
            .children(self.calls.calls.iter().map(|call| {
                let words = self.overview.as_ref().and_then(|view| view.computers.iter().find(|each| each.key == call.computer)).map(|each| each.name.clone())
                    .or_else(|| self.sessions.local_computers.contains(&call.computer).then(|| "This computer".into()))
                    .unwrap_or_else(|| "Another computer".into());
                let name = model_label(&self.catalog, Some(&call.provider), call.model.as_deref().unwrap_or("Unknown model"));
                let label = label_with_mark(Some(&call.provider), &name)
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("{} · {}", short_timestamp(call.at), crate::time::clock(call.at))));
                table_row(label, [words, format_cost(call.cost), format_count(call.token_count)], cx)
            }))
            .when(self.calls.calls.is_empty(), |d| d.child(div().py(space::S2).text_color(c.text_muted)
                .child(if self.pages.is_some() { "Reading calls…" } else { "No calls in this range." })))
            .when_some(self.calls.next, |d, next| d.child(self.more("insights-more-calls", next, cx)))
    }
}

impl Render for InsightsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        div().id("insights").debug_selector(|| "insights".into()).size_full().bg(c.sidebar).overflow_y_scroll().track_scroll(&self.scroll)
            .child(div().max_w(px(1240.)).mx_auto().p(space::S3).flex().flex_col().gap(space::S3)
                .child(self.toolbar(cx))
                .children(self.problem.as_ref().map(|problem| card(cx).text_color(c.text_muted).child(problem.clone())))
                .child(match (&self.selected, &self.overview) {
                    (Some(session), _) => self.detail(session, cx).into_any_element(),
                    (_, Some(overview)) => self.overview(overview, window, cx).into_any_element(),
                    _ if self.problem.is_none() => card(cx).child("Reading usage…").into_any_element(),
                    _ => div().into_any_element(),
                }))
    }
}

#[derive(Clone, Copy)]
struct ChartAxis {
    step: f64,
    intervals: usize,
}

impl ChartAxis {
    fn new(max: f64, metric: Metric) -> Self {
        if max <= 0. || !max.is_finite() {
            return Self { step: if metric == Metric::Cost { 0.01 } else { 1. }, intervals: 3 };
        }
        let rough = max / 3.;
        let magnitude = 10_f64.powf(rough.log10().floor());
        let step = [1., 2., 2.5, 5., 10.].into_iter()
            .map(|multiple| multiple * magnitude).find(|&step| step >= rough).unwrap_or(10. * magnitude);
        let step = if metric == Metric::Tokens { step.ceil() } else { step };
        Self { step, intervals: ((max / step - 1e-10).ceil() as usize).clamp(1, 3).max(2) }
    }

    fn max(self) -> f64 { self.step * self.intervals as f64 }

    fn position(self, index: usize) -> f32 { 1. - index as f32 / self.intervals as f32 }

    fn label(self, index: usize, metric: Metric) -> String {
        let amount = self.step * index as f64;
        match metric {
            Metric::Cost => {
                let decimals = (2..=9).find(|&places| {
                    let scaled = self.step * 10_f64.powi(places as i32);
                    (scaled - scaled.round()).abs() < 1e-8 && scaled >= 1.
                }).unwrap_or(9);
                format!("${amount:.decimals$}")
            }
            Metric::Tokens => format_count(Some(amount)),
        }
    }
}

fn chart_date_stride(days: usize, width: f32) -> usize {
    let ticks = (((width - 410.) / 72.) as usize).clamp(2, 7);
    days.saturating_sub(1).div_ceil(ticks - 1).max(if days > 10 { 5 } else { 1 })
}

fn paint_chart(
    bounds: gpui::Bounds<gpui::Pixels>, samples: &[f64], axis: ChartAxis, selected: Option<usize>,
    tint: gpui::Hsla, grid: gpui::Hsla, window: &mut Window,
) {
    use gpui::{PathBuilder, point};
    let bottom = bounds.bottom();
    let left = bounds.left();
    let right = bounds.right();
    for i in 0..=axis.intervals {
        let y = bounds.top() + bounds.size.height * axis.position(i);
        let mut path = PathBuilder::stroke(px(1.));
        path.move_to(point(left, y));
        path.line_to(point(right, y));
        if let Ok(path) = path.build() { window.paint_path(path, grid); }
    }
    let points: Vec<_> = samples.iter().enumerate().map(|(i, &sample)| point(
        left + bounds.size.width * (i as f32 / samples.len().saturating_sub(1).max(1) as f32),
        bottom - bounds.size.height * (sample / axis.max()).clamp(0., 1.) as f32,
    )).collect();
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return };
    let curve = |path: &mut PathBuilder| {
        path.move_to(*first);
        for pair in points.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            let mid = a.x + (b.x - a.x) * 0.5;
            // Horizontal tangents keep the curve between the two measured values.
            path.cubic_bezier_to(b, point(mid, a.y), point(mid, b.y));
        }
    };
    let mut fill = PathBuilder::fill();
    curve(&mut fill);
    fill.line_to(point(last.x, bottom));
    fill.line_to(point(first.x, bottom));
    fill.close();
    if let Ok(path) = fill.build() { window.paint_path(path, tint.opacity(0.16)); }

    let mut line = PathBuilder::stroke(px(1.5));
    curve(&mut line);
    if let Ok(path) = line.build() { window.paint_path(path, tint); }

    if let Some(active) = selected.and_then(|index| points.get(index)) {
        let mut marker = PathBuilder::stroke(px(1.));
        marker.move_to(point(active.x, bounds.top()));
        marker.line_to(point(active.x, bottom));
        if let Ok(path) = marker.build() { window.paint_path(path, tint.opacity(0.42)); }
        window.paint_quad(gpui::fill(gpui::Bounds {
            origin: point(active.x - px(2.), active.y - px(2.)),
            size: gpui::size(px(4.), px(4.)),
        }, tint).corner_radii(px(2.)));
    }
}

fn date_label(day: &str, axis: bool) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let Some(day_number) = crate::time::parse_day(day) else { return day.to_owned() };
    let (_, month, date) = crate::time::civil(day_number);
    let month = MONTHS[(month - 1) as usize];
    if axis { format!("{} {date}", month.to_ascii_uppercase()) } else { format!("{date} {month}") }
}

fn short_timestamp(at: f64) -> String {
    date_label(&crate::time::day(at, crate::pasted::utc_offset((at / 1000.) as i64), 0), false)
}

fn card(cx: &App) -> gpui::Div {
    let c = &cx.theme().colors;
    div().min_w_0().flex().flex_col().p(px(10.)).rounded(radius::CARD).border_1().border_color(c.border).bg(c.card)
}

fn format_share(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |share| format!("{:.1}%", share * 100.))
}

fn agent_label(row: &Row) -> String {
    if row.provider.as_deref() == Some(&row.name) { provider_name(&row.name) } else { row.name.clone() }
}

fn model_label(catalog: &[OfferedModel], provider: Option<&str>, id: &str) -> String {
    let provider = provider.map(|name| match name {
        "claude" => "claude-code",
        "codex" => "codex-cli",
        "grok" => "grok-cli",
        name => name,
    });
    let same_provider = |item: &OfferedModel| provider.is_some_and(|provider| item.provider_id.split('@').next() == Some(provider));
    if let Some(model) = catalog.iter()
        .filter(|item| item.id == id || (same_provider(item) && item.aliases.iter().any(|alias| alias == id)))
        .min_by_key(|item| (!same_provider(item), item.id != id))
    {
        return model.name.clone();
    }
    let id = id.rsplit('/').next().unwrap_or(id);
    if let Some(version) = id.strip_prefix("gpt-") {
        return format!("GPT-{}", version.replace('-', " "));
    }
    id.split(['-', '_', ' ']).filter(|part| !part.is_empty()).map(|part| {
        let mut letters = part.chars();
        letters.next().map_or_else(String::new, |first| format!("{}{}", first.to_uppercase(), letters.as_str()))
    }).collect::<Vec<_>>().join(" ")
}

fn provider_mark(provider: &str) -> Option<Mark> {
    Mark::of_service(provider).or_else(|| mark_asset(provider).map(|(_, path)| Mark::new(path))).map(|mark| mark.icon_size(IconSize::Nav))
}

fn provider_name(provider: &str) -> String {
    match provider {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "grok" => "Grok",
        "antigravity" => "Antigravity",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        "opencode-go" => "OpenCode Go",
        "openai" => "OpenAI",
        _ => provider,
    }.to_owned()
}

fn label_with_mark(provider: Option<&str>, words: &str) -> gpui::Div {
    div().flex().items_center().gap(space::S1_5).children(provider.and_then(provider_mark))
        .child(div().min_w_0().truncate().child(words.to_owned()))
}

fn cells(label: impl IntoElement, values: [impl Into<SharedString>; 3]) -> gpui::Div {
    div().flex().items_center().gap(space::S2).min_h(px(32.))
        .child(div().flex_1().min_w_0().child(label))
        .children(values.map(|value| div().min_w_0().w(gpui::relative(0.19)).text_align(gpui::TextAlign::Right).truncate().child(value.into())))
}

fn table_header(words: [&str; 4], cx: &App) -> gpui::Div {
    cells(words[0].to_owned(), [words[1].to_owned(), words[2].to_owned(), words[3].to_owned()])
        .text_ui(TypeSize::Caption).text_color(cx.theme().colors.text_muted)
}

fn table_row(label: impl IntoElement, values: [String; 3], cx: &App) -> gpui::Div {
    cells(label, values).border_t_1().border_color(cx.theme().colors.border)
}

fn value(usage: &Measures, metric: Metric) -> Option<f64> {
    match metric {
        Metric::Cost => usage.cost,
        Metric::Tokens => usage.tokens,
    }
}

fn day_value(usage: &Measures, metric: Metric) -> String {
    format_metric(value(usage, metric).or_else(|| (usage.calls == 0).then_some(0.)), metric)
}

fn format_metric(value: Option<f64>, metric: Metric) -> String {
    match metric {
        Metric::Cost => format_cost(value),
        Metric::Tokens => format_count(value),
    }
}

fn format_cost(value: Option<f64>) -> String {
    value.map_or_else(|| "Unavailable".into(), |value| {
        if value == 0. || value >= 0.01 { format!("${value:.2}") } else if value >= 0.0001 { format!("${value:.4}") } else { "< $0.0001".into() }
    })
}

fn format_count(value: Option<f64>) -> String {
    let Some(value) = value else { return "Unavailable".into(); };
    for (scale, unit) in [(1e9, "B"), (1e6, "M"), (1e3, "K")] {
        if value >= scale {
            let words = format!("{:.2}", value / scale);
            return format!("{}{unit}", words.trim_end_matches('0').trim_end_matches('.'));
        }
    }
    format!("{value:.0}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insights_filter_encodes_account_and_time_zone() {
        let filters = Filters {
            days: 365,
            ring: Some(crate::area::UsageFilter { title: "Claude".into(), account: Some("claude#a/b".into()), provider: "claude".into() }),
            ..Filters::default()
        };
        assert_eq!(filters.query("Europe/Istanbul", 50), "?period=365&zone=Europe%2FIstanbul&offset=50&account=claude%23a%2Fb");
    }

    #[test]
    fn insights_keeps_zero_and_unavailable_distinct() {
        assert_eq!(format_cost(Some(0.)), "$0.00");
        assert_eq!(format_cost(None), "Unavailable");
        assert_eq!(format_count(Some(1234.)), "1.23K");
        assert_eq!(format_count(Some(320_000.)), "320K");
        assert_eq!(format_count(Some(1_250_000.)), "1.25M");
    }

    #[test]
    fn insights_axes_use_round_steps_and_short_dates() {
        let axis = ChartAxis::new(0.0285, Metric::Cost);
        let labels: Vec<_> = (0..=axis.intervals).map(|index| axis.label(index, Metric::Cost)).collect();
        assert_eq!(labels, ["$0.00", "$0.01", "$0.02", "$0.03"]);
        assert_eq!(axis.position(0), 1.);
        assert_eq!(axis.position(axis.intervals), 0.);
        assert_eq!(chart_date_stride(30, 1280.), 5);
        assert_eq!(date_label("2026-10-16", false), "16 Oct");
        assert_eq!(date_label("2026-11-01", true), "NOV 1");
    }

    #[test]
    fn insights_uses_catalog_names_and_formats_unlisted_models() {
        let model: OfferedModel = serde_json::from_value(serde_json::json!({
            "id": "real-id", "providerID": "claude-code@work", "name": "Catalog Name",
            "aliases": ["another-id"], "side": "agents", "efforts": [],
        })).unwrap();
        assert_eq!(model_label(&[model], Some("claude"), "another-id"), "Catalog Name");
        assert_eq!(model_label(&[], Some("codex"), "gpt-5.5"), "GPT-5.5");
        assert_eq!(model_label(&[], Some("claude"), "claude-sonnet-5"), "Claude Sonnet 5");
    }

}
