//! Insights: looking back at where AI usage went over a period, as the box sums it
//! (`usage.insights.summary`, PROTOCOL.md "Insights"): the period's cost, tokens and cache rate with
//! their change, one trend line, a ranked list (bots, providers, models, computers) and up to three
//! standout sessions. Every grouping and figure is the box's; this view draws what it says. Clicking
//! a row narrows the whole screen to it.

use std::{collections::{HashMap, HashSet}, time::Duration};

use gpui::{
    AnyElement, App, Bounds, Context, Hsla, IntoElement, PathBuilder, Pixels, Render, ScrollHandle, SharedString,
    Task, Window, canvas, div, hsla, point, prelude::*, px, relative, size,
};
use kuvo_box::{BoxClient, areas::Look, paths};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    icon::{Mark, provider_mark as mark_asset},
    theme::{radius, scaled, space},
};
use serde::Deserialize;

use crate::areas::Reachability;

const ROUTE: &str = "kuvo/agents/api/kuvo/insights";
/// The box makes the whole screen (`insights/summary`). Without it, an older box's overview gives
/// the totals, the trend and its providers.
pub const SUMMARY: &str = "usage.insights.summary";
/// Rows a list shows before "Show all", and sessions an expanded bot shows.
const ROWS: usize = 5;
const SESSIONS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Period {
    Week,
    Month,
    Quarter,
}

impl Period {
    const ALL: [Period; 3] = [Period::Week, Period::Month, Period::Quarter];

    fn days(self) -> u16 {
        match self {
            Period::Week => 7,
            Period::Month => 30,
            Period::Quarter => 90,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Period::Week => "7d",
            Period::Month => "30d",
            Period::Quarter => "90d",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Metric {
    #[default]
    Cost,
    Tokens,
}

#[derive(Deserialize)]
struct Answer<T> {
    data: T,
}

/// Cost, tokens and cache rate, as the box gives them at every level.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Measures {
    #[serde(default)]
    cost: f64,
    #[serde(default)]
    tokens: f64,
    #[serde(default)]
    cache_rate: Option<f64>,
    /// Well under the period's, as the box says.
    #[serde(default)]
    low_cache: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Change {
    cost: Option<f64>,
    tokens: Option<f64>,
    cache_rate: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
struct Row {
    kind: String,
    id: String,
    name: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    look: Option<Look>,
    /// A bot's: the sessions it's over, to expand to.
    #[serde(default)]
    sessions: Vec<Row>,
    #[serde(flatten)]
    measures: Measures,
}

#[derive(Clone, Debug, Deserialize)]
struct List {
    key: String,
    name: String,
    rows: Vec<Row>,
}

#[derive(Clone, Debug, Deserialize)]
struct Standout {
    id: String,
    name: String,
    reason: String,
    kind: String,
    #[serde(flatten)]
    measures: Measures,
}

#[derive(Clone, Debug, Deserialize)]
struct Day {
    day: String,
    #[serde(default)]
    cost: f64,
    #[serde(default)]
    tokens: f64,
}

#[derive(Clone, Debug, Deserialize)]
struct Summary {
    state: String,
    totals: Measures,
    #[serde(default)]
    change: Option<Change>,
    trend: Vec<Day>,
    lists: Vec<List>,
    #[serde(default)]
    standouts: Vec<Standout>,
    /// The box's last summary for it, served at once while it reads it afresh: when it was read.
    #[serde(default, rename = "asOf")]
    as_of: Option<f64>,
    /// When the box last counted its usage (an older box doesn't say).
    #[serde(default, rename = "countedAt")]
    counted_at: Option<f64>,
}

/// An older box's overview: totals, days and providers, without cache rates, change or bots.
#[derive(Deserialize)]
struct Overview {
    state: String,
    totals: OverviewMeasures,
    daily: Vec<OverviewRow>,
    #[serde(default)]
    providers: Vec<OverviewRow>,
}

#[derive(Deserialize)]
struct OverviewMeasures {
    cost: Option<f64>,
    processed: Option<f64>,
}

#[derive(Deserialize)]
struct OverviewRow {
    name: String,
    cost: Option<f64>,
    processed: Option<f64>,
}

impl From<Overview> for Summary {
    fn from(overview: Overview) -> Self {
        let measures = |cost: Option<f64>, tokens: Option<f64>| Measures { cost: cost.unwrap_or_default(), tokens: tokens.unwrap_or_default(), ..Measures::default() };
        Summary {
            state: if matches!(overview.state.as_str(), "loading" | "empty") { overview.state } else { "ready".into() },
            totals: measures(overview.totals.cost, overview.totals.processed),
            change: None,
            trend: overview.daily.into_iter().map(|day| Day { day: day.name, cost: day.cost.unwrap_or_default(), tokens: day.processed.unwrap_or_default() }).collect(),
            lists: vec![List {
                key: "providers".into(),
                name: "Providers".into(),
                rows: overview.providers.into_iter().map(|row| Row {
                    kind: "provider".into(), id: row.name.clone(), name: row.name.clone(), note: None, provider: Some(row.name),
                    look: None, sessions: Vec::new(), measures: measures(row.cost, row.processed),
                }).collect(),
            }],
            standouts: Vec::new(),
            as_of: None,
            counted_at: None,
        }
    }
}

/// What the screen is narrowed to: a row clicked (by its kind and id, drawn as it was), or a usage
/// ring's account.
#[derive(Clone, Debug, PartialEq)]
struct Narrow {
    kind: String,
    id: String,
    name: String,
    look: Option<Look>,
    provider: Option<String>,
}

impl Narrow {
    fn of(row: &Row) -> Self {
        Narrow { kind: row.kind.clone(), id: row.id.clone(), name: row.name.clone(), look: row.look.clone(), provider: row.provider.clone() }
    }

    fn session(id: &str, name: &str) -> Self {
        Narrow { kind: "session".into(), id: id.into(), name: name.into(), look: None, provider: None }
    }
}

pub struct InsightsView {
    client: BoxClient,
    zone: String,
    period: Period,
    metric: Metric,
    /// The list chosen, by its key; the first while none is.
    list: Option<String>,
    /// What it's narrowed to, each step in from the last (a bot, then one of its sessions).
    narrowing: Vec<Narrow>,
    expanded: HashSet<String>,
    expanded_all: HashSet<String>,
    show_all: bool,
    hovered_day: Option<usize>,
    summary: Option<Summary>,
    /// What `summary` was read for (its query) and its period's days: another's while the box reads
    /// the one chosen now, and drawn as updating then, never as this one's.
    shown_for: Option<(String, u16)>,
    /// The last of each period and narrowing the box answered, by its query, drawn at once when it's
    /// chosen again while the box reads it afresh.
    kept: HashMap<String, Summary>,
    /// The box said it's still reading what's chosen now (`state` "loading").
    waiting: bool,
    /// A usage ring's title, for words about what it couldn't read.
    ring: Option<String>,
    problem: Option<String>,
    reachability: Reachability,
    visible: bool,
    reading: bool,
    /// The refresh button was clicked: the box counts again, and its answer for this (the query
    /// it was clicked on) is awaited.
    refreshing: Option<String>,
    /// The clock "Updated … ago" counts from (fixed in tests and shots).
    now: Option<f64>,
    task: Option<Task<()>>,
    survey: Option<Task<()>>,
    scroll: ScrollHandle,
}

impl InsightsView {
    pub fn new(client: BoxClient, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            client,
            zone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
            period: Period::Month,
            metric: Metric::Cost,
            list: None,
            narrowing: Vec::new(),
            expanded: HashSet::new(),
            expanded_all: HashSet::new(),
            show_all: false,
            hovered_day: None,
            summary: None,
            shown_for: None,
            kept: HashMap::new(),
            waiting: false,
            ring: None,
            problem: None,
            reachability: Reachability::Unanswered,
            visible: false,
            reading: false,
            refreshing: None,
            now: None,
            task: None,
            survey: None,
            scroll: ScrollHandle::new(),
        }
    }

    /// The shot runner's states, read at once from the stand-in.
    #[cfg(feature = "stand-in")]
    pub fn sample(&mut self, slug: &str, cx: &mut Context<Self>) {
        match slug {
            "insights-expanded" => { self.expanded.insert("bot_foreman".into()); }
            "insights-providers" => self.list = Some("providers".into()),
            "insights-refreshing" => self.refreshing = Some(self.query()),
            "insights-narrowed" => self.narrowing = vec![Narrow {
                kind: "bot".into(), id: "bot_foreman".into(), name: "Foreman".into(), provider: None,
                look: Some(Look { color: "indigo".into(), shape: "squircle".into(), face: "calm".into() }),
            }],
            _ => {}
        }
        let read = Self::read(&self.client, self.query(), false);
        self.reachability = Reachability::of(&read);
        self.summary = read.ok();
        self.shown_for = Some((self.query(), self.period.days()));
        cx.notify();
    }

    pub(crate) fn reachability(&self) -> Reachability {
        self.reachability
    }

    pub(crate) fn read_again(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    /// Counts "Updated … ago" from `now` (tests and shots, so they read the same each time).
    pub fn set_now(&mut self, now: f64) {
        self.now = Some(now);
    }

    /// The refresh button: the box counts its usage again, and what's shown stays until its new
    /// numbers replace it.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing.is_some() { return; }
        self.refreshing = Some(self.query());
        self.read_now(true, cx);
    }

    pub fn shown(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible { return; }
        self.visible = visible;
        if !visible {
            self.survey = None;
            return;
        }
        if self.summary.is_none() { self.load(cx); }
        self.keep_reading(cx);
    }

    /// Read again now and then while it's shown, sooner while the box reads what's chosen (from the
    /// next tick at that pace: started again when that begins).
    fn keep_reading(&mut self, cx: &mut Context<Self>) {
        self.survey = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(delay) = this.read_with(cx, |this, _| {
                    if this.waiting { 2 } else { 20 }
                }) else { break };
                cx.background_executor().timer(Duration::from_secs(delay)).await;
                if this.update(cx, |this, cx| if !this.reading { this.load(cx) }).is_err() { break; }
            }
        }));
    }

    /// Opened from a usage ring: narrowed to its account (or its provider, for one without).
    pub fn ring(&mut self, filter: Option<crate::area::UsageFilter>, cx: &mut Context<Self>) {
        self.ring = filter.as_ref().map(|filter| filter.title.clone());
        self.narrowing = filter.into_iter().map(|filter| match filter.account {
            Some(account) => Narrow { kind: "account".into(), id: account, name: filter.title, look: None, provider: Some(filter.provider) },
            None => Narrow { kind: "provider".into(), id: filter.provider.clone(), name: filter.title, look: None, provider: Some(filter.provider) },
        }).collect();
        self.narrowed(cx);
    }

    fn narrow_to(&mut self, narrow: Narrow, cx: &mut Context<Self>) {
        if self.narrowing.last().is_some_and(|last| last.kind == narrow.kind && last.id == narrow.id) { return; }
        // (A session inside a session is a step in; anything else replaces what it was narrowed by.)
        if narrow.kind != "session" && let Some(at) = self.narrowing.iter().position(|each| each.kind == narrow.kind) {
            self.narrowing.truncate(at);
        }
        self.narrowing.push(narrow);
        self.narrowed(cx);
    }

    fn back_to(&mut self, steps: usize, cx: &mut Context<Self>) {
        self.narrowing.truncate(steps);
        if self.narrowing.is_empty() { self.ring = None; }
        self.narrowed(cx);
    }

    fn narrowed(&mut self, cx: &mut Context<Self>) {
        self.list = None;
        self.show_all = false;
        self.expanded.clear();
        self.expanded_all.clear();
        self.hovered_day = None;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.load(cx);
    }

    fn query(&self) -> String {
        let mut query = format!("?period={}&zone={}", self.period.days(), paths::query_value(&self.zone));
        let mut by: Vec<(&str, &str)> = Vec::new();
        for narrow in &self.narrowing {
            by.retain(|(kind, _)| *kind != narrow.kind);
            by.push((narrow.kind.as_str(), narrow.id.as_str()));
        }
        for (kind, id) in by {
            query.push_str(&format!("&{kind}={}", paths::query_value(id)));
        }
        query
    }

    /// `recount`: the box counts again first (`refresh=1`; an older box reads it again as it is).
    fn read(client: &BoxClient, query: String, recount: bool) -> kuvo_box::Result<Summary> {
        if client.advertises(SUMMARY) {
            let recount = if recount { "&refresh=1" } else { "" };
            client.get::<Answer<Summary>>(&format!("{ROUTE}/summary{query}{recount}")).map(|answer| answer.data)
        } else {
            client.get::<Answer<Overview>>(&format!("{ROUTE}{query}")).map(|answer| answer.data.into())
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        // (Read again while a refresh of the same is counted: that refresh's answer is the newer.
        // Another period or narrowing chosen meanwhile is read at once, the refresh let go.)
        if self.refreshing.as_ref().is_some_and(|refreshing| *refreshing == self.query()) { return; }
        self.refreshing = None;
        self.read_now(false, cx);
    }

    fn read_now(&mut self, recount: bool, cx: &mut Context<Self>) {
        if !self.client.gate().has("usage.insights") {
            self.reading = false;
            self.refreshing = None;
            self.problem = None;
            return;
        }
        self.reading = true;
        let client = self.client.clone();
        let query = self.query();
        let days = self.period.days();
        // (What's chosen now, as the box last answered it, at once, while it's read afresh.)
        if self.shown_for.as_ref().is_none_or(|(shown, _)| *shown != query) && let Some(kept) = self.kept.get(&query) {
            self.summary = Some(kept.clone());
            self.shown_for = Some((query.clone(), days));
            self.waiting = false;
        }
        let asked = query.clone();
        let read = cx.background_executor().spawn(async move { Self::read(&client, asked, recount) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                if recount { this.refreshing = None; }
                // (Another period or narrowing chosen since: not this answer's to draw.)
                if this.query() != query { return; }
                this.reading = false;
                this.reachability = Reachability::of(&result);
                match result {
                    // Still reading: what's on screen stays (drawn as updating where it's another's).
                    Ok(summary) if summary.state == "loading" => {
                        if !this.waiting && this.visible { this.waiting = true; this.keep_reading(cx); }
                        this.waiting = true;
                        this.problem = None;
                        if this.summary.is_none() {
                            this.summary = Some(summary);
                            this.shown_for = Some((query, days));
                        }
                    }
                    Ok(summary) => {
                        // (A handful at most: three periods, and what was narrowed to lately.)
                        if this.kept.len() >= 24 { this.kept.clear(); }
                        this.kept.insert(query.clone(), summary.clone());
                        // (The box's last, while it reads it afresh: asked again soon.)
                        let waiting = summary.as_of.is_some();
                        if waiting && !this.waiting && this.visible { this.waiting = true; this.keep_reading(cx); }
                        this.waiting = waiting;
                        this.summary = Some(summary);
                        this.shown_for = Some((query, days));
                        this.problem = None;
                    }
                    Err(error) => this.problem = Some(Self::error_words(&error, this.ring.as_deref())),
                }
                cx.notify();
            }).ok();
        }));
        cx.notify();
    }

    fn error_words(error: &kuvo_box::Error, ring: Option<&str>) -> String {
        let words = kuvo_box::accounts::said(error);
        if words == "Not available for this agent." {
            return match ring {
                Some(title) => format!("Usage data isn\u{2019}t available for {title} on this computer."),
                None => "Insights isn\u{2019}t available on this computer.".into(),
            };
        }
        words
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let title = |muted: bool| div().text_size(scaled(px(20.))).line_height(scaled(px(26.)))
            .when(muted, |d| d.regular().text_color(c.text_muted)).when(!muted, |d| d.medium().text_color(c.text));
        let place = match self.narrowing.split_last() {
            None => title(false).child("Insights").into_any_element(),
            Some((last, before)) => {
                let chevron = || Icon::new(IconName::ChevronRight).size(IconSize::Nav).color(c.text_faint);
                div().flex().items_center().gap(space::S2).min_w_0()
                    .child(title(true).id("insights-crumb-root").debug_selector(|| "insights-crumb-root".into()).flex_none().cursor_pointer()
                        .hover(|d| d.text_color(c.text)).on_click(cx.listener(|this, _, _, cx| this.back_to(0, cx))).child("Insights"))
                    .children(before.iter().enumerate().map(|(i, narrow)| div().flex().items_center().gap(space::S2).min_w_0().child(chevron())
                        .child(div().id(("insights-crumb", i)).debug_selector(move || format!("insights-crumb-{i}").into())
                            .min_w_0().max_w(px(180.)).truncate().cursor_pointer().text_ui(TypeSize::Small).text_color(c.text_muted)
                            .hover(|d| d.text_color(c.text)).on_click(cx.listener(move |this, _, _, cx| this.back_to(i + 1, cx)))
                            .child(narrow.name.clone()))))
                    .child(chevron())
                    .child(div().id("insights-narrow-chip").debug_selector(|| "insights-narrow-chip".into())
                        .flex().items_center().gap(space::S1_5).min_w_0().h(scaled(px(30.))).px(space::S2).rounded(radius::PILL)
                        .bg(pill_bg(dark)).border_1().border_color(pill_border(dark)).shadow(cx.theme().card_shadow())
                        .cursor_pointer().on_click(cx.listener(|this, _, _, cx| { let back = this.narrowing.len() - 1; this.back_to(back, cx) }))
                        .children(narrow_icon(last, &c))
                        .child(div().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(last.name.clone()))
                        .child(div().pl(space::S0_5).flex().items_center().child(Icon::new(IconName::Close).size(IconSize::XSmall).color(c.text_muted))))
                    .into_any_element()
            }
        };
        let periods = segmented(dark, Period::ALL.into_iter().map(|period| {
            let id = format!("insights-period-{}", period.label());
            (SharedString::from(id), period.label().into(), self.period == period, cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                if this.period != period {
                    this.period = period;
                    this.hovered_day = None;
                    this.load(cx);
                }
            }))
        }).collect(), TypeSize::Caption, cx);
        // (Quiet, beside the periods: what's below is another's, read afresh for this one now.)
        let refreshing = self.refreshing.is_some();
        let updating = !refreshing && self.summary.as_ref().is_some_and(|summary| summary.state != "loading"
            && (summary.as_of.is_some() || self.shown_for.as_ref().is_some_and(|(shown, _)| *shown != self.query())));
        // (When the box last counted what's shown, as it says; quiet, and not while "Updating…" is.)
        let counted = self.summary.as_ref().filter(|_| !updating).and_then(|summary| summary.counted_at).map(|at| {
            let now = self.now.unwrap_or_else(crate::time::now_ms);
            match crate::time::ago(at, now).as_str() {
                "now" => "Updated just now".to_owned(),
                ago => format!("Updated {ago} ago"),
            }
        });
        // (Refresh and the periods act on figures: they come with them, never over "Reading usage…".)
        let figures = self.summary.as_ref().is_some_and(|summary| summary.state != "loading");
        let refresh = figures.then(|| Button::icon_only("insights-refresh", IconName::Refresh, "Refresh").ghost().quiet().size(ButtonSize::Small)
            .icon_size(IconSize::XSmall).loading(refreshing)
            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))));
        div().flex().items_center().justify_between().gap(space::S4)
            .child(div().flex_1().min_w_0().flex().child(place))
            .child(div().flex_none().flex().items_center().gap(space::S2)
                .when(updating, |d| d.child(div().debug_selector(|| "insights-updating-mark".into()).flex().items_center().gap(space::S1_5)
                    .text_ui(TypeSize::Caption).text_color(c.text_muted)
                    .child(kuvo_ui::small::spinner("insights-updating-spin", IconSize::XSmall, c.text_muted))
                    .child("Updating…")))
                .when_some(counted, |d, words| d.child(div().debug_selector(|| "insights-counted".into())
                    .text_ui(TypeSize::Caption).text_color(c.text_muted).whitespace_nowrap()
                    .child(div().debug_selector({ let words = words.clone(); move || format!("insights-counted-{words}") }).child(words))))
                .children(refresh)
                .children(figures.then_some(periods)))
            .into_any_element()
    }

    fn totals(&self, summary: &Summary, cx: &App) -> AnyElement {
        let c = &cx.theme().colors;
        let change = summary.change.clone().unwrap_or_default();
        let tint = change.cache_rate.map(|rate| (rate * 100.).round() as i64).and_then(|points| match points.signum() {
            1 => Some(c.success),
            -1 => Some(c.warning),
            _ => None,
        });
        let block = |id: &'static str, label: &'static str, value: String, delta: Option<String>, tint: Option<Hsla>| {
            div().flex_1().min_w_0().flex().flex_col().gap(px(4.)).debug_selector(move || id.into())
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child(label))
                .child(div().flex().items_end().gap(space::S2)
                    .child(div().text_size(scaled(px(28.))).line_height(scaled(px(32.))).medium().text_color(c.text).truncate().child(value))
                    .children(delta.map(|delta| div().flex_none().pb(px(3.)).text_ui(TypeSize::Caption).medium()
                        .text_color(tint.unwrap_or(c.text_faint)).child(delta))))
        };
        let rule = || div().w(px(1.)).mx(space::S6).my(px(4.)).bg(c.border);
        div().flex().items_stretch().py(space::S1)
            .child(block("insights-total-cost", "Cost", cost(summary.totals.cost), percent_change(change.cost), None))
            .child(rule())
            .child(block("insights-total-tokens", "Tokens", tokens(summary.totals.tokens), percent_change(change.tokens), None))
            .child(rule())
            .child(block("insights-total-cache", "Cache", cache(summary.totals.cache_rate), percent_change(change.cache_rate), tint))
            .into_any_element()
    }

    fn trend(&self, summary: &Summary, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let metric = self.metric;
        let samples: Vec<f64> = summary.trend.iter().map(|day| match metric { Metric::Cost => day.cost, Metric::Tokens => day.tokens }).collect();
        let axis = Axis::new(samples.iter().copied().fold(0., f64::max), metric);
        let accent = chart_accent(&c, dark);
        let grid = if dark { hsla(0., 0., 1., 0.06) } else { hsla(0., 0., 0., 0.055) };
        let selected = self.hovered_day.filter(|&i| i < samples.len());
        let caption = match selected.and_then(|i| summary.trend.get(i)) {
            Some(day) => format!("{} · {}", date(&day.day), match metric { Metric::Cost => cost(day.cost), Metric::Tokens => tokens(day.tokens) }),
            None => match (summary.trend.first(), summary.trend.last()) {
                (Some(first), Some(last)) => format!("{} – {}", date(&first.day), date(&last.day)),
                _ => String::new(),
            },
        };
        let metrics = segmented(dark, [(Metric::Cost, "Cost"), (Metric::Tokens, "Tokens")].into_iter().map(|(each, label)| {
            (SharedString::from(format!("insights-metric-{label}")), label.into(), metric == each, cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.metric = each;
                cx.notify();
            }))
        }).collect(), TypeSize::Caption, cx);
        let days = summary.trend.len();
        let last = days.saturating_sub(1);
        let axis_width = scaled(px(48.));
        div().flex().flex_col().gap(space::S2).debug_selector(|| "insights-trend".into())
            .child(div().flex().items_center().justify_between().gap(space::S2)
                .child(div().debug_selector(|| "insights-trend-caption".into()).text_ui(TypeSize::Caption)
                    .text_color(if selected.is_some() { c.text_muted } else { c.text_faint }).child(caption))
                .child(metrics))
            .child(div().flex().items_stretch().gap(space::S2).h(px(118.))
                .child(div().w(axis_width).flex_none().relative().text_ui(TypeSize::Caption).text_color(c.text_faint)
                    .children((0..=axis.intervals).map(|i| div().absolute().right_0().top(relative(axis.position(i))).mt(px(-7.)).child(axis.label(i, metric)))))
                .child(div().id("insights-chart").debug_selector(|| "insights-chart".into()).relative().flex_1().min_w_0()
                    .child(canvas(|_, _, _| {}, move |bounds, (), window, _| paint_trend(bounds, &samples, axis, selected, accent, grid, dark, window))
                        .absolute().size_full())
                    .children((0..days).map(|i| {
                        let span = last.max(1) as f32;
                        let left = ((i as f32 - 0.5) / span).max(0.);
                        let right = if i == last { 1. } else { (i as f32 + 0.5) / span };
                        div().id(("insights-day", i)).debug_selector(move || format!("insights-day-{i}").into())
                            .absolute().top_0().h_full().left(relative(left)).w(relative(right - left))
                            .on_hover(cx.listener(move |this, hovered, _, cx| {
                                if *hovered {
                                    this.hovered_day = Some(i);
                                    cx.notify();
                                } else if this.hovered_day == Some(i) {
                                    this.hovered_day = None;
                                    cx.notify();
                                }
                            }))
                    }))))
            .child(div().h(px(16.)).ml(axis_width + space::S2).relative().text_ui(TypeSize::Caption).text_color(c.text_faint)
                .children(ticks(days).into_iter().filter_map(|i| {
                    let label = div().absolute().top_0().whitespace_nowrap().child(date(&summary.trend.get(i)?.day));
                    Some(if i == 0 { label.left_0() } else if i == last { label.right_0() }
                        else { label.left(relative(i as f32 / last.max(1) as f32)).w(px(52.)).ml(px(-26.)).text_center() })
                })))
            .into_any_element()
    }

    fn ranked(&self, summary: &Summary, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let chosen = self.list.as_deref().and_then(|key| summary.lists.iter().find(|list| list.key == key)).or(summary.lists.first());
        let Some(list) = chosen else { return div().into_any_element() };
        let tabs = segmented(dark, summary.lists.iter().map(|each| {
            let key = each.key.clone();
            (SharedString::from(format!("insights-tab-{}", each.key)), each.name.clone().into(), each.key == list.key, cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.list = Some(key.clone());
                this.show_all = false;
                cx.notify();
            }))
        }).collect(), TypeSize::Small, cx);
        let headings = div().flex().items_center().gap(space::S2).text_ui(TypeSize::Caption).text_color(c.text_faint)
            .when(wide, |d| d.child(div().w(px(52.))))
            .child(cell(76.).child("Cost")).child(cell(68.).child("Tokens")).child(cell(52.).child("Cache"));
        let most = list.rows.iter().map(|row| row.measures.cost).fold(0., f64::max).max(0.01);
        let shown = if self.show_all { list.rows.len() } else { list.rows.len().min(ROWS) };
        let nested = list.rows.iter().any(|row| !row.sessions.is_empty());
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, row) in list.rows.iter().take(shown).enumerate() {
            let open = !row.sessions.is_empty() && self.expanded.contains(&row.id);
            rows.push(self.row(row, i, most, wide, nested, open, cx));
            if open { rows.push(self.sessions(row, wide, cx)); }
        }
        div().flex().flex_col().debug_selector(|| "insights-ranked".into())
            .child(div().flex().flex_wrap().items_center().justify_between().gap(space::S3).pb(space::S3).child(tabs).child(headings))
            .child(div().flex().flex_col().border_t_1().border_color(c.border).children(rows))
            .when(list.rows.len() > ROWS, |d| d.child(div().pt(space::S2).flex().child(
                div().id("insights-show-all").debug_selector(|| "insights-show-all".into()).cursor_pointer().px(space::S2).py(px(5.))
                    .rounded(radius::NAV).text_ui(TypeSize::Caption).medium().text_color(c.text_muted)
                    .hover(|d| d.bg(c.hover).text_color(c.text))
                    .on_click(cx.listener(|this, _, _, cx| { this.show_all = !this.show_all; cx.notify(); }))
                    .child(if self.show_all { "Show less" } else { "Show all" }))))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn row(&self, row: &Row, i: usize, most: f64, wide: bool, nested: bool, open: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        let share = (row.measures.cost / most).clamp(0.03, 1.) as f32;
        let narrow = Narrow::of(row);
        let toggle: AnyElement = if !row.sessions.is_empty() {
            let id = row.id.clone();
            div().id(("insights-expand", i)).debug_selector(move || format!("insights-expand-{i}").into())
                .size(px(20.)).flex_none().flex().items_center().justify_center().rounded(px(5.)).cursor_pointer().hover(|d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if !this.expanded.remove(&id) { this.expanded.insert(id.clone()); }
                    cx.notify();
                }))
                .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(IconSize::XSmall).color(c.icon))
                .into_any_element()
        } else if nested {
            // (In line with the rows that open.)
            div().w(px(20.)).flex_none().into_any_element()
        } else {
            div().into_any_element()
        };
        div().id(("insights-row", i)).debug_selector(move || format!("insights-row-{i}").into())
            .flex().items_center().gap(space::S2).min_h(scaled(px(40.))).px(px(6.)).mx(px(-6.)).rounded(radius::NAV)
            .border_b_1().border_color(if open { hsla(0., 0., 0., 0.) } else { c.border })
            .cursor_pointer().hover(|d| d.bg(c.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.narrow_to(narrow.clone(), cx)))
            .child(div().flex_1().min_w_0().flex().items_center().gap(space::S2)
                .child(toggle)
                .child(row_icon(row, &c))
                .child(div().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(row.name.clone()))
                .children(row.note.clone().map(|note| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(note))))
            .when(wide, |d| d.child(div().w(px(52.)).h(px(3.)).flex_none().rounded(radius::PILL)
                .bg(if dark { hsla(0., 0., 1., 0.07) } else { hsla(0., 0., 0., 0.06) }).overflow_hidden()
                .child(div().h_full().w(relative(share)).rounded(radius::PILL).bg(chart_accent(&c, dark).opacity(if dark { 0.72 } else { 0.62 })))))
            .child(figures(&row.measures, TypeSize::Small, true, &c))
            .into_any_element()
    }

    fn sessions(&self, row: &Row, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let all = self.expanded_all.contains(&row.id);
        let shown = if all { row.sessions.len() } else { row.sessions.len().min(SESSIONS) };
        let bot = row.id.clone();
        div().pb(space::S2).mb(px(1.)).border_b_1().border_color(c.border).flex()
            .debug_selector(|| "insights-sessions".into())
            .child(div().w(px(28.)).flex_none().flex().justify_center().py(px(4.)).child(div().w(px(1.)).h_full().bg(c.border_strong)))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .children(row.sessions.iter().take(shown).map(|session| {
                    let narrow = Narrow::session(&session.id, &session.name);
                    let id = session.id.clone();
                    div().id(SharedString::from(format!("insights-session-{id}"))).debug_selector(move || format!("insights-session-{id}").into())
                        .flex().items_center().gap(space::S2).min_h(scaled(px(30.))).px(px(6.)).rounded(px(6.)).cursor_pointer().hover(|d| d.bg(c.hover))
                        .on_click(cx.listener(move |this, _, _, cx| this.narrow_to(narrow.clone(), cx)))
                        .child(div().flex_1().min_w_0().flex().items_center().gap(space::S2)
                            .child(div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(session.name.clone()))
                            .children(session.note.clone().map(|note| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child(note))))
                        .when(wide, |d| d.child(div().w(px(52.)).flex_none()))
                        .child(figures(&session.measures, TypeSize::Caption, false, &c))
                }))
                .when(row.sessions.len() > shown, |d| d.child(div().id(SharedString::from(format!("insights-sessions-all-{bot}")))
                    .debug_selector(|| "insights-sessions-all".into())
                    .cursor_pointer().h(scaled(px(28.))).px(px(6.)).rounded(px(6.)).flex().items_center()
                    .text_ui(TypeSize::Caption).text_color(c.text_faint).hover(|d| d.text_color(c.text_muted))
                    .on_click(cx.listener(move |this, _, _, cx| { this.expanded_all.insert(bot.clone()); cx.notify(); }))
                    .child("Show all"))))
            .into_any_element()
    }

    fn standouts(&self, summary: &Summary, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let dark = cx.theme().appearance.is_dark();
        div().flex().flex_col().gap(space::S2).debug_selector(|| "insights-standouts".into())
            .child(div().text_ui(TypeSize::Caption).medium().text_color(c.text_muted).child("Standout"))
            .child(div().flex().flex_col().children(summary.standouts.iter().enumerate().map(|(i, standout)| {
                let narrow = Narrow::session(&standout.id, &standout.name);
                let cold = standout.kind == "cache";
                let measures = Measures { low_cache: standout.measures.low_cache || cold, ..standout.measures };
                div().id(("insights-standout", i)).debug_selector(move || format!("insights-standout-{i}").into())
                    .flex().items_center().gap(space::S2).min_h(scaled(px(36.))).px(px(6.)).mx(px(-6.)).rounded(radius::NAV)
                    .border_t_1().border_color(c.border).cursor_pointer().hover(|d| d.bg(c.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.narrow_to(narrow.clone(), cx)))
                    .child(div().flex_1().min_w_0().flex().items_center().gap(space::S2)
                        .child(div().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(standout.name.clone()))
                        .child(reason(&standout.reason, cold, dark, &c)))
                    .child(figures(&measures, TypeSize::Small, true, &c))
            })))
            .into_any_element()
    }

    /// Plain words in the content's place, and what to do about them where there's something.
    fn said(&self, words: String, again: bool, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        div().flex().flex_col().items_start().gap(space::S2).debug_selector(|| "insights-said".into())
            .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(words))
            .when(again, |d| d.child(Button::new("insights-try-again", "Try Again").ghost().size(ButtonSize::Small)
                .on_click(cx.listener(|this, _, _, cx| this.load(cx)))))
            .into_any_element()
    }
}

impl Render for InsightsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors.clone();
        // (Room for the share bars beside the figures.)
        let wide = window.viewport_size().width >= px(760.);
        let summary = self.summary.clone();
        // What's drawn was read for another period or narrowing (the box reads this one now): it
        // stays in place, faded and marked as updating, never as this one's figures.
        let query = self.query();
        let other = self.shown_for.as_ref().filter(|(shown, _)| *shown != query).map(|(_, days)| *days);
        let content: Vec<AnyElement> = match (&summary, &self.problem) {
            (None, Some(problem)) => vec![self.said(problem.clone(), true, cx)],
            (None, None) => vec![self.said("Reading usage…".into(), false, cx)],
            (Some(summary), _) if summary.state == "loading" => vec![self.said("Reading usage…".into(), false, cx)],
            (Some(summary), problem) => {
                let mut parts = Vec::new();
                if let Some(problem) = problem { parts.push(self.said(problem.clone(), true, cx)); }
                let mut figures = vec![self.totals(summary, cx), self.trend(summary, cx)];
                if summary.state == "empty" {
                    figures.push(self.said(format!("No usage in the last {} days.", other.unwrap_or(self.period.days())), false, cx));
                } else {
                    figures.push(self.ranked(summary, wide, cx));
                    if !summary.standouts.is_empty() { figures.push(self.standouts(summary, cx)); }
                }
                match other {
                    None => parts.extend(figures),
                    Some(_) => parts.push(div().debug_selector(|| "insights-updating".into()).flex().flex_col().gap(px(28.)).opacity(0.4)
                        .children(figures).into_any_element()),
                }
                parts
            }
        };
        div().id("insights").debug_selector(|| "insights".into()).size_full().bg(c.surface)
            .text_ui(TypeSize::Small).text_color(c.text).overflow_y_scroll().track_scroll(&self.scroll)
            .child(div().w_full().max_w(px(860.)).mx_auto().px(px(40.)).py(px(32.)).flex().flex_col().gap(px(28.))
                .child(self.header(cx))
                .children(content))
    }
}

type Pick = Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static>;

/// A row of choices on a glass track, the chosen one a lifted pill (the period, the trend's measure
/// and the lists).
pub(crate) fn segmented(dark: bool, choices: Vec<(SharedString, SharedString, bool, impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static)>, text: TypeSize, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let shadow = cx.theme().card_shadow();
    let tall = if text == TypeSize::Small { 26. } else { 24. };
    div().p(px(2.)).flex().items_center().gap(px(2.)).rounded(radius::NAV)
        .bg(if dark { hsla(0., 0., 1., 0.035) } else { hsla(0., 0., 0., 0.035) })
        .border_1().border_color(if dark { hsla(0., 0., 1., 0.07) } else { hsla(0., 0., 0., 0.055) })
        .children(choices.into_iter().map(|(id, label, on, pick)| {
            let pick: Pick = Box::new(pick);
            let selector = id.clone();
            div().id(id).debug_selector(move || selector.to_string()).cursor_pointer().h(scaled(px(tall))).px(px(if text == TypeSize::Small { 11. } else { 10. }))
                .rounded(px(6.)).flex().items_center().text_ui(text)
                .when(on, |d| d.bg(pill_bg(dark)).border_1().border_color(pill_border(dark)).shadow(shadow.clone()).text_color(c.text).medium())
                .when(!on, |d| d.text_color(c.text_muted).hover(|d| d.text_color(c.text)))
                .on_click(move |event, window, cx| pick(event, window, cx))
                // (Which is chosen, for tests: `insights-on-30d`.)
                .child(div().when(on, |d| d.debug_selector({ let label = label.clone(); move || format!("insights-on-{label}") })).child(label))
        }))
        .into_any_element()
}

fn pill_bg(dark: bool) -> Hsla {
    if dark { hsla(0., 0., 1., 0.105) } else { hsla(0., 0., 1., 0.94) }
}

fn pill_border(dark: bool) -> Hsla {
    if dark { hsla(0., 0., 1., 0.14) } else { hsla(0., 0., 0., 0.075) }
}

fn reason(words: &str, warn: bool, dark: bool, c: &kuvo_ui::theme::Colors) -> AnyElement {
    let (bg, border, text) = if warn {
        (c.warning.opacity(if dark { 0.14 } else { 0.11 }), c.warning.opacity(if dark { 0.25 } else { 0.20 }), c.warning)
    } else if dark {
        (hsla(0., 0., 1., 0.05), hsla(0., 0., 1., 0.09), c.text_muted)
    } else {
        (hsla(0., 0., 0., 0.035), hsla(0., 0., 0., 0.06), c.text_muted)
    };
    div().flex_none().px(px(7.)).py(px(1.)).rounded(radius::PILL).bg(bg).border_1().border_color(border)
        .text_ui(TypeSize::Caption).text_color(text).child(words.to_owned()).into_any_element()
}

/// A calm line in the accent's hue.
fn chart_accent(c: &kuvo_ui::theme::Colors, dark: bool) -> Hsla {
    Hsla { h: c.accent.h, s: if dark { 0.56 } else { 0.62 }, l: if dark { 0.66 } else { 0.50 }, a: 1. }
}

fn cell(width: f32) -> gpui::Div {
    div().w(scaled(px(width))).flex_none().text_align(gpui::TextAlign::Right)
}

/// Cost, tokens and cache rate in their columns.
fn figures(measures: &Measures, text: TypeSize, strong: bool, c: &kuvo_ui::theme::Colors) -> gpui::Div {
    let quiet = if strong { c.text_muted } else { c.text_faint };
    div().flex().flex_none().items_center().gap(space::S2).text_ui(text)
        .child(cell(76.).medium().text_color(if strong { c.text } else { c.text_muted }).child(cost(measures.cost)))
        .child(cell(68.).text_color(quiet).child(tokens(measures.tokens)))
        .child(cell(52.).text_color(if measures.low_cache { c.warning } else { quiet }).child(cache(measures.cache_rate)))
}

fn row_icon(row: &Row, c: &kuvo_ui::theme::Colors) -> AnyElement {
    let tile = || div().size(px(18.)).flex_none().flex().items_center().justify_center();
    match row.kind.as_str() {
        "bot" => match &row.look {
            Some(look) => crate::bot_face::still_bot_face(&row.id, look, px(18.)),
            None => tile().child(Icon::new(IconName::Bot).size(IconSize::Nav).color(c.icon)).into_any_element(),
        },
        "provider" | "model" => tile().children(row.provider.as_deref().and_then(provider_mark)).into_any_element(),
        "computer" => tile().child(Icon::new(IconName::Computer).size(IconSize::Nav).color(c.icon)).into_any_element(),
        _ => tile().child(Icon::new(IconName::Message).size(IconSize::Nav).color(c.icon)).into_any_element(),
    }
}

fn narrow_icon(narrow: &Narrow, c: &kuvo_ui::theme::Colors) -> Option<AnyElement> {
    match narrow.kind.as_str() {
        "bot" => Some(match &narrow.look {
            Some(look) => crate::bot_face::still_bot_face(&narrow.id, look, px(16.)),
            None => Icon::new(IconName::Bot).size(IconSize::Nav).color(c.icon).into_any_element(),
        }),
        "provider" | "model" | "account" => narrow.provider.as_deref().and_then(provider_mark).map(IntoElement::into_any_element),
        "computer" => Some(Icon::new(IconName::Computer).size(IconSize::Nav).color(c.icon).into_any_element()),
        _ => Some(Icon::new(IconName::Message).size(IconSize::Nav).color(c.icon).into_any_element()),
    }
}

fn provider_mark(provider: &str) -> Option<Mark> {
    Mark::of_service(provider).or_else(|| mark_asset(provider).map(|(_, path)| Mark::new(path))).map(|mark| mark.icon_size(IconSize::Nav))
}

/// "Oct 23".
fn date(day: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let Some(number) = crate::time::parse_day(day) else { return day.to_owned() };
    let (_, month, date) = crate::time::civil(number);
    format!("{} {date}", MONTHS[(month - 1) as usize])
}

/// "$943.00", "$1,234.56".
fn cost(value: f64) -> String {
    let cents = (value.max(0.) * 100.).round() as u64;
    let (whole, cents) = (cents / 100, cents % 100);
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { grouped.push(','); }
        grouped.push(digit);
    }
    format!("${grouped}.{cents:02}")
}

/// "262M", "82.8M", "860K".
fn tokens(value: f64) -> String {
    let value = value.max(0.);
    for (scale, unit) in [(1e9, "B"), (1e6, "M"), (1e3, "K")] {
        if value >= scale {
            let amount = value / scale;
            return if amount >= 100. || unit == "K" { format!("{amount:.0}{unit}") } else { format!("{amount:.1}{unit}") };
        }
    }
    format!("{value:.0}")
}

/// "83%", or a dash where no input was counted.
fn cache(rate: Option<f64>) -> String {
    rate.map_or_else(|| "\u{2014}".into(), |rate| format!("{:.0}%", rate * 100.))
}

/// "+11%", "−4%" (a cache rate's change in points reads the same).
fn percent_change(change: Option<f64>) -> Option<String> {
    let points = (change? * 100.).round() as i64;
    Some(match points.signum() {
        1 => format!("+{points}%"),
        -1 => format!("\u{2212}{}%", -points),
        _ => "0%".into(),
    })
}

fn ticks(days: usize) -> Vec<usize> {
    match days {
        0 => Vec::new(),
        1 => vec![0],
        2..=7 => vec![0, days / 2, days - 1],
        _ => {
            let last = days - 1;
            vec![0, last / 4, last / 2, last * 3 / 4, last]
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Axis {
    step: f64,
    intervals: usize,
}

impl Axis {
    fn new(max: f64, metric: Metric) -> Self {
        if max <= 0. || !max.is_finite() {
            return Self { step: if metric == Metric::Cost { 5. } else { 500_000. }, intervals: 2 };
        }
        let rough = max / 2.;
        let magnitude = 10_f64.powf(rough.log10().floor());
        let step = [1., 2., 2.5, 5., 10.].into_iter().map(|m| m * magnitude).find(|&step| step >= rough).unwrap_or(10. * magnitude);
        Self { step, intervals: ((max / step - 1e-9).ceil() as usize).clamp(2, 3) }
    }

    fn max(self) -> f64 { self.step * self.intervals as f64 }

    fn position(self, i: usize) -> f32 { 1. - i as f32 / self.intervals as f32 }

    fn label(self, i: usize, metric: Metric) -> String {
        let value = self.step * i as f64;
        match metric {
            Metric::Cost if i == 0 => "$0".into(),
            Metric::Cost if value >= 1. && (value - value.round()).abs() < 1e-6 => format!("${value:.0}"),
            Metric::Cost => format!("${value:.2}"),
            Metric::Tokens if i == 0 => "0".into(),
            Metric::Tokens => tokens(value),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_trend(bounds: Bounds<Pixels>, samples: &[f64], axis: Axis, selected: Option<usize>, accent: Hsla, grid: Hsla, dark: bool, window: &mut Window) {
    let (left, right, top, bottom) = (bounds.left(), bounds.right(), bounds.top(), bounds.bottom());
    let (width, height) = (bounds.size.width, bounds.size.height);
    for i in 0..=axis.intervals {
        let y = top + height * axis.position(i);
        let mut line = PathBuilder::stroke(px(1.));
        line.move_to(point(left, y));
        line.line_to(point(right, y));
        if let Ok(path) = line.build() { window.paint_path(path, grid); }
    }
    if samples.len() < 2 { return; }
    let span = (samples.len() - 1) as f32;
    let points: Vec<_> = samples.iter().enumerate()
        .map(|(i, &value)| point(left + width * (i as f32 / span), bottom - height * (value / axis.max()).clamp(0., 1.) as f32))
        .collect();
    let trace = |path: &mut PathBuilder| {
        path.move_to(points[0]);
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let middle = a.x + (b.x - a.x) * 0.5;
            // (Level tangents keep the curve between the two days' values.)
            path.cubic_bezier_to(b, point(middle, a.y), point(middle, b.y));
        }
    };
    let mut area = PathBuilder::fill();
    trace(&mut area);
    area.line_to(point(points[points.len() - 1].x, bottom));
    area.line_to(point(points[0].x, bottom));
    area.close();
    if let Ok(path) = area.build() { window.paint_path(path, accent.opacity(if dark { 0.13 } else { 0.095 })); }
    let mut stroke = PathBuilder::stroke(px(1.75));
    trace(&mut stroke);
    if let Ok(path) = stroke.build() { window.paint_path(path, accent); }
    if let Some(at) = selected.and_then(|i| points.get(i)) {
        let mut rule = PathBuilder::stroke(px(1.));
        rule.move_to(point(at.x, top));
        rule.line_to(point(at.x, bottom));
        if let Ok(path) = rule.build() { window.paint_path(path, accent.opacity(0.35)); }
        window.paint_quad(gpui::fill(Bounds::new(point(at.x - px(3.), at.y - px(3.)), size(px(6.), px(6.))), accent).corner_radii(px(3.)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insights_figures_read_as_the_drafts_do() {
        assert_eq!(cost(943.), "$943.00");
        assert_eq!(cost(1234.567), "$1,234.57");
        assert_eq!(cost(0.), "$0.00");
        assert_eq!(tokens(262_000_000.), "262M");
        assert_eq!(tokens(82_800_000.), "82.8M");
        assert_eq!(tokens(860_000.), "860K");
        assert_eq!(cache(Some(0.834)), "83%");
        assert_eq!(cache(None), "\u{2014}");
        assert_eq!(percent_change(Some(0.111)), Some("+11%".into()));
        assert_eq!(percent_change(Some(-0.04)), Some("\u{2212}4%".into()));
        assert_eq!(percent_change(None), None);
        assert_eq!(date("2026-10-23"), "Oct 23");
    }

    #[test]
    fn insights_axis_uses_round_steps() {
        let axis = Axis::new(61.3, Metric::Cost);
        let labels: Vec<_> = (0..=axis.intervals).map(|i| axis.label(i, Metric::Cost)).collect();
        assert_eq!(labels, ["$0", "$50", "$100"]);
        assert_eq!(ticks(30), [0, 7, 14, 21, 29]);
        assert_eq!(ticks(7), [0, 3, 6]);
    }

    #[test]
    fn insights_error_words_replace_the_generic_agent_404() {
        let missing = kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(404, br#"{"message":"Not available for this agent."}"#));
        assert_eq!(InsightsView::error_words(&missing, Some("Claude")), "Usage data isn\u{2019}t available for Claude on this computer.");
        assert_eq!(InsightsView::error_words(&missing, None), "Insights isn\u{2019}t available on this computer.");
        let other = kuvo_box::Error::Refused(kuvo_box::Refusal::from_answer(500, br#"{"message":"Usage database is locked."}"#));
        assert_eq!(InsightsView::error_words(&other, Some("Claude")), "Usage database is locked.");
    }
}
