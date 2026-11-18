//! Insights displays the usage index on the selected computer.

use std::time::Duration;

use gpui::{AnyElement, App, Context, FocusHandle, IntoElement, Render, ScrollHandle, SharedString, Subscription, Task, Window, div, prelude::*, px};
use kuvo_box::{BoxClient, paths, transport::Request};
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
    providers: Vec<Row>,
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
            overview: None, sessions: Sessions::default(), selected: None, calls: Calls::default(), problem: None,
            visible: false, reading: false, task: None, pages: None, survey: None, chart_focus, chart_day: None, scroll: ScrollHandle::new(), session_list_offset: gpui::point(px(0.), px(0.)), _focus: focus,
        }
    }

    #[cfg(feature = "stand-in")]
    pub fn sample(&mut self, slug: &str, cx: &mut Context<Self>) {
        match slug {
            "insights-tokens" => self.metric = Metric::Tokens,
            "insights-computer" => self.filters.computer = "key_devbox".into(),
            "insights-breakdown-skill" => self.breakdown = Breakdown::Skill,
            "insights-ring-filter" => self.filters.ring = Some(crate::area::UsageFilter {
                title: "Claude Code".into(), provider: "claude".into(), account: Some("claude#sample".into()),
            }),
            _ => {}
        }
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
        div().flex().flex_wrap().items_center().justify_between().gap(space::S2)
            .child(div().text_size(px(20.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Insights"))
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
        let max = overview.daily.iter().filter_map(|row| value(&row.usage, metric)).fold(0.0f64, f64::max);
        let height = 126.;
        let date_at = |idx: usize| overview.daily.get(idx).map(|row| row.name.clone());
        let active = self.chart_day.and_then(|idx| overview.daily.get(idx));
        let selected = self.chart_day;
        let tracked = self.chart_focus.is_focused(window);
        card(cx).flex_1().min_w_0()
            .child(div().flex().items_center().justify_between().gap(space::S2)
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(if metric == Metric::Cost { "Daily cost" } else { "Daily tokens" }))
                .child(div().text_color(c.text_muted).text_ui(TypeSize::Caption).child(active.map_or_else(
                    || format_metric(Some(max), metric),
                    |row| format!("{} · {}", row.name, day_value(&row.usage, metric)),
                ))))
            .child(div().id("insights-chart").debug_selector(|| "insights-chart".into())
                .track_focus(&self.chart_focus).mt(space::S2).h(px(height)).relative()
                .focus_visible(move |d| d.shadow(kuvo_ui::button::focus_ring(c.focus_ring)))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    let len = this.overview.as_ref().map_or(0, |view| view.daily.len());
                    if len == 0 { return; }
                    let current = this.chart_day.unwrap_or(len - 1);
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
                .children((0..=3).map(|i| div().absolute().top(px(height * i as f32 / 3. - 1.)).w_full().border_b_1().border_color(c.border)))
                .child(div().absolute().inset_0().flex().items_end().children(overview.daily.iter().enumerate().map(|(idx, row)| {
                    let amount = value(&row.usage, metric).unwrap_or(0.0);
                    let h = if max > 0.0 { (amount / max).clamp(0.0, 1.0) as f32 * height } else { 0. };
                    let label = format!("{} · {}", row.name, day_value(&row.usage, metric));
                    div().id(("insights-day", idx)).min_w_0().flex_1().h_full().flex().items_end()
                        .when(selected == Some(idx), |d| d.bg(c.selected))
                        .on_hover(cx.listener(move |this, hovered, _, cx| {
                            if *hovered { this.chart_day = Some(idx); cx.notify(); }
                            else if !tracked && this.chart_day == Some(idx) { this.chart_day = None; cx.notify(); }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.chart_focus, cx);
                            this.chart_day = Some(idx);
                            cx.notify();
                        }))
                        .tooltip(move |_, cx| Tooltip::new(label.clone()).build(cx))
                        .child(div().w_full().mx(px(if overview.daily.len() <= 90 { 1. } else { 0. })).h(px(h)).rounded_t(px(2.)).bg(c.accent))
                }))))
            .child(div().flex().justify_between().mt(space::S1).text_color(c.text_muted).text_ui(TypeSize::Caption)
                .children(date_at(0)).children(date_at(overview.daily.len().saturating_sub(1))))
    }

    fn overview(&self, overview: &Overview, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let totals = &overview.totals;
        let metric = self.metric;
        let session_count = totals.sessions.unwrap_or_default();
        let known = if totals.calls == 0 { Some(0.) } else { value(totals, metric) };
        let providers = overview.providers.iter().map(|row| {
            div().flex().items_center().gap(space::S2).children(provider_mark(&row.name))
                .child(div().flex_1().min_w_0().truncate().child(provider_name(&row.name)))
                .child(format_metric(value(&row.usage, metric), metric))
        });
        let summary = card(cx).w(px(252.)).flex_none().gap(space::S2)
            .child(div().text_color(c.text_muted).child(if metric == Metric::Cost { "Estimated API cost · USD" } else { "Tokens without cache reads" }))
            .child(div().text_size(px(30.)).font_weight(gpui::FontWeight::SEMIBOLD).child(format_metric(known, metric)))
            .child(div().text_color(c.text_muted).child(format!("{session_count} {}", if session_count == 1 { "session" } else { "sessions" })))
            .children(providers)
            .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Estimates use current API rates."))
            .when(totals.unpriced > 0, |d| d.child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(format!("Cost unavailable for {} calls", totals.unpriced))));
        let counts = [
            ("Without cache reads", totals.tokens), ("With cache reads", totals.processed),
            ("Cached input", totals.cached), ("Uncached input", totals.input),
            ("Cache creation", totals.creation), ("Output", totals.output), ("User messages", Some(totals.messages as f64)),
        ];
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
                .child(div().flex().gap(space::S3).child(summary).child(self.chart(overview, window, cx)))
                .child(card(cx).gap(space::S2).child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Totals"))
                    .child(div().flex().flex_wrap().gap(space::S3).children(counts.into_iter().map(|(title, count)| {
                        div().flex_1().min_w(px(95.)).child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(title))
                            .child(div().mt(space::S1).medium().child(format_count(count.or_else(|| (totals.calls == 0).then_some(0.)))))
                    }))))
                .child(self.breakdown(overview, cx))
                .child(self.session_list(cx)))
    }

    fn breakdown(&self, overview: &Overview, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        let (title, rows) = match self.breakdown {
            Breakdown::Model => ("Model", &overview.breakdowns.model),
            Breakdown::Agent => ("Agent", &overview.breakdowns.agent),
            Breakdown::Day => ("Day", &overview.breakdowns.day),
            Breakdown::Skill => ("Skill", &overview.breakdowns.skill),
        };
        let modes = [(Breakdown::Model, "Model"), (Breakdown::Agent, "Agent"), (Breakdown::Day, "Day"), (Breakdown::Skill, "Skill")];
        card(cx)
            .child(div().flex().items_center().justify_between().gap(space::S2)
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Breakdown"))
                .child(div().flex().gap(space::S1).children(modes.into_iter().map(|(mode, label)| {
                    div().when(self.breakdown == mode, |d| d.rounded(radius::NAV).bg(c.selected)).child(
                        Button::new(SharedString::from(format!("insights-by-{label}")), label).ghost().size(ButtonSize::Small)
                            .on_click(cx.listener(move |this, _, _, cx| { this.breakdown = mode; cx.notify(); })),
                    )
                }))))
            .when(self.breakdown == Breakdown::Skill, |d| d.child(div().text_color(c.text_muted).child("A call can appear under more than one skill.")))
            .child(table_header([title, "Estimated cost", "Share", "Tokens"], cx))
            .children(rows.iter().filter(|row| row.usage.calls > 0).map(|row| {
                let share = if self.metric == Metric::Cost { row.cost_share } else { row.token_share };
                table_row(
                    label_with_mark(row.provider.as_deref(), &row.name),
                    [format_cost(row.usage.cost), share.map_or_else(|| "—".into(), |p| format!("{:.1}%", p * 100.)), format_count(row.usage.tokens)], cx,
                )
            }))
            .when(rows.iter().all(|row| row.usage.calls == 0), |d| d.child(div().py(space::S2).text_color(c.text_muted).child(
                if self.breakdown == Breakdown::Skill { "No skill use was recorded." } else { "No calls recorded." }
            )))
    }

    fn session_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        card(cx).child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Sessions"))
            .child(table_header(["Session", "Last active", "Estimated cost", "Tokens"], cx))
            .children(self.sessions.sessions.iter().enumerate().map(|(i, session)| {
                let words = session.title.as_deref().filter(|title| !title.is_empty()).unwrap_or("Untitled session");
                let selected = session.clone();
                div().id(("insights-session", i)).cursor_pointer().hover(|d| d.bg(c.hover)).tab_index(0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.calls = Calls::default();
                        this.session_list_offset = this.scroll.offset();
                        this.scroll.set_offset(gpui::point(px(0.), px(0.)));
                        this.selected = Some(selected.clone());
                        this.problem = None;
                        this.page(0, cx);
                    }))
                    .child(table_row(div().truncate().child(words.to_owned()), [
                        crate::time::when(session.at, crate::time::now_ms()), format_cost(session.usage.cost), format_count(session.usage.tokens),
                    ], cx))
            }))
            .when(self.sessions.sessions.is_empty(), |d| d.child(div().py(space::S2).text_color(c.text_muted).child("No sessions in this range.")))
            .when_some(self.sessions.next, |d, next| d.child(self.more("insights-more-sessions", next, cx)))
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
                let label = label_with_mark(Some(&call.provider), call.model.as_deref().unwrap_or("Unknown model"))
                    .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(crate::time::when(call.at, crate::time::now_ms())));
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
            .child(div().max_w(px(1240.)).mx_auto().p(space::S4).flex().flex_col().gap(space::S3)
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

fn card(cx: &App) -> gpui::Div {
    let c = &cx.theme().colors;
    div().min_w_0().flex().flex_col().p(space::S3).rounded(radius::CARD).border_1().border_color(c.border).bg(c.card)
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
        if value >= scale { return format!("{:.2}{unit}", value / scale); }
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
    }
}
