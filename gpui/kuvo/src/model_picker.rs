//! The one model picker, wherever a model is picked (the composer, a side chat, a bot's settings
//! and New Bot, Marketplace › Add Agent, Settings › Models), laid out as T3 Code's (#975):
//! - down the left, ★ your favourites, then each provider's mark, the one shown marked in blue;
//! - in the middle, "Search models…" over every provider's, the provider's models with ⌘1… on the
//!   first rows, ✓ on the current one, ☆ on a row under the pointer, and "Show more" for the rest;
//!   your accounts with a coding agent switch above its list (#833), Kuvo's way rather than T3's;
//! - on the right, where a choice takes an effort, "Reasoning": the current model's efforts;
//! - along the bottom, where they apply (the composer's), Standard | Fast for a provider with a fast
//!   mode, and the conversation's permissions.
//!
//! What it offers, your favourites, the speeds and the modes' words are the box's
//! (`kuvo/models/offered`, `kuvo/models/starred`, `kuvo/models/speed`, the approval's `options`):
//! every client's picker shows the same. What's drawn is worked out once per list ([`Catalog`]),
//! and only the rows in sight are drawn (`uniform_list`), so it opens at once and scrolls through
//! hundreds; what's typed is matched off the UI thread, and a newer search drops the one before it.

use std::{
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{
    AnyElement, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, UniformListScrollHandle, Window, actions, div, prelude::FluentBuilder as _, px, uniform_list,
};
use kuvo_box::{
    approval::{Approval, Shown as ApprovalShown},
    client::BoxClient,
    models::{Offered, OfferedModel, Side, Speed, Starred, effort_title},
    typed::ModelRef,
};
use kuvo_ui::{
    button::keycap,
    icon::{Icon, IconName, IconSize},
    text_field::{TextField, TextFieldEvent},
    theme::{ActiveTheme, StyledType, TypeSize, radius, scaled, space},
};

actions!(model_picker, [Next, Previous, Pick, Close, NextRail, PreviousRail, Row1, Row2, Row3, Row4, Row5, Row6, Row7, Row8, Row9]);

const CONTEXT: &str = "ModelPicker";

/// The picker's keys, ahead of its search field's own while it has the keyboard.
pub fn bind_keys(cx: &mut App) {
    let within = format!("{CONTEXT} > {}", kuvo_ui::text_field::CONTEXT);
    for context in [CONTEXT, within.as_str()] {
        let c = Some(context);
        cx.bind_keys([
            KeyBinding::new("down", Next, c),
            KeyBinding::new("up", Previous, c),
            KeyBinding::new("enter", Pick, c),
            KeyBinding::new("escape", Close, c),
            KeyBinding::new("alt-down", NextRail, c),
            KeyBinding::new("alt-up", PreviousRail, c),
            KeyBinding::new("cmd-1", Row1, c),
            KeyBinding::new("cmd-2", Row2, c),
            KeyBinding::new("cmd-3", Row3, c),
            KeyBinding::new("cmd-4", Row4, c),
            KeyBinding::new("cmd-5", Row5, c),
            KeyBinding::new("cmd-6", Row6, c),
            KeyBinding::new("cmd-7", Row7, c),
            KeyBinding::new("cmd-8", Row8, c),
            KeyBinding::new("cmd-9", Row9, c),
        ]);
    }
}

/// The rail's width, the models' column's, and Reasoning's.
const RAIL: f32 = 52.;
const MIDDLE: f32 = 320.;
const RIGHT: f32 = 188.;
/// The picker's width without Reasoning (a setting that takes no effort).
pub const WIDTH: f32 = RAIL + MIDDLE;
/// A row's height: a model on one line, its provider's mark ahead of it.
const ROW: f32 = 34.;
/// How many rows show before the list scrolls.
const MOST_ROWS: usize = 8;
/// How many of a provider's models show before "Show more".
const FIRST: usize = 7;
/// How many rows have a ⌘ number.
const NUMBERED: usize = 9;
/// The bottom bar's height.
const BAR: f32 = 44.;
/// How many accounts the switch's list has before it offers a filter.
const FILTERED: usize = 7;
/// An account's row in the switch's list, and how many show before it scrolls.
const ACCOUNT_ROW: f32 = 30.;
const MOST_ACCOUNTS: usize = 7;

// MARK: What it offers

pub enum RefreshModels {
    Offered { folder: Option<String>, session: Option<String>, side: Option<Side> },
    Defaults,
}

/// A model as the picker draws it, worked out once per list.
#[derive(Clone, Debug)]
pub struct Entry {
    pub reference: ModelRef,
    pub name: SharedString,
    /// Where it's from: its provider (with its account's label), and "Free" for one that needs no
    /// account.
    pub line: SharedString,
    /// What a row says after its name, where its mark alone doesn't say where it's from: its
    /// account ("Work · Max") where its agent has two or more, its provider where its mark is
    /// shared (OpenCode Zen and Go), "Free".
    pub detail: Option<SharedString>,
    /// It's under its rail's account switch (two or more accounts), which names its account.
    pub switched: bool,
    pub provider: SharedString,
    /// Its place in the rail.
    pub rail: usize,
    pub efforts: Vec<String>,
    /// The speeds it runs at besides Standard, as the box offers them.
    pub speeds: Vec<Speed>,
    aliases: Vec<String>,
    /// Its name, and the rest it's found by (its id, its provider), lowercased for search.
    name_key: String,
    rest_key: String,
}

impl Entry {
    /// Whether it's `model`: the same provider, and its id or one it's known by.
    pub fn is(&self, provider: &str, id: &str) -> bool {
        self.reference.provider_id == provider && (self.reference.id == id || self.aliases.iter().any(|alias| alias == id))
    }

    fn starred(&self) -> Starred {
        Starred { provider_id: self.reference.provider_id.clone(), id: self.reference.id.clone() }
    }
}

/// One of the rail's providers: those drawn with one mark sit together (OpenCode Zen and OpenCode
/// Go).
#[derive(Clone, Debug)]
pub struct RailEntry {
    pub id: SharedString,
    pub name: SharedString,
    pub mark: Option<&'static str>,
    pub color: Option<Hsla>,
    /// Your accounts with its coding agent, in the box's order, where there are two or more: its
    /// list shows one at a time, switched between above it.
    pub accounts: Vec<AccountChoice>,
}

/// One of your accounts with a coding agent, as the box names it (`models.offered.accounts`).
#[derive(Clone, Debug, PartialEq)]
pub struct AccountChoice {
    /// Its provider (`claude-code@w0rk`): its models', and what a model picked under it runs on.
    pub provider: SharedString,
    pub name: SharedString,
    pub plan: Option<SharedString>,
}

impl AccountChoice {
    /// "Work · Max".
    pub fn line(&self) -> String {
        match &self.plan {
            Some(plan) => format!("{} · {plan}", self.name),
            None => self.name.to_string(),
        }
    }
}

/// The choice ahead of the models where a setting can be left to something else ("The engine's
/// choice", "Same as the conversation").
#[derive(Clone, Debug)]
pub struct Unset {
    pub title: SharedString,
    pub line: SharedString,
    /// Whether it's what's set now.
    pub chosen: bool,
}

/// What a picker offers, in the box's order, and the rail of their providers.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub rails: Vec<RailEntry>,
    pub unset: Option<Unset>,
    /// The speed picked for each provider on its box (`models.speed`; none: Standard).
    pub speeds: std::collections::HashMap<String, String>,
}

impl Catalog {
    /// The models offered that `keep` keeps (and a picker shows: none retiring or hidden), in the
    /// box's order.
    pub fn new(offered: &Offered, keep: impl Fn(&OfferedModel) -> bool) -> Catalog {
        let mut catalog = Catalog::default();
        for model in offered.models.iter().filter(|model| model.is_offered() && keep(model)) {
            let provider = offered.providers.iter().find(|each| each.id == model.provider_id);
            let name = provider.map(|each| each.name.clone()).unwrap_or_else(|| model.provider_id.clone());
            let label = provider.and_then(|each| each.label.clone());
            let (group, mark) = match kuvo_ui::icon::provider_mark(&model.provider_id) {
                Some((group, path)) => (group.to_owned(), Some(path)),
                None => (model.provider_id.clone(), None),
            };
            let rail = match catalog.rails.iter().position(|rail| rail.id.as_ref() == group) {
                Some(at) => at,
                None => {
                    catalog.rails.push(RailEntry { id: group.into(), name: name.clone().into(), mark, color: kuvo_ui::icon::provider_mark_color(&model.provider_id), accounts: Vec::new() });
                    catalog.rails.len() - 1
                }
            };
            let mut line = name.clone();
            if let Some(label) = &label {
                line.push_str(" · ");
                line.push_str(label);
            }
            if model.free {
                line.push_str(" · Free");
            }
            if let Some(account) = provider.and_then(|each| each.account.as_ref()) {
                let accounts = &mut catalog.rails[rail].accounts;
                if !accounts.iter().any(|each| each.provider.as_ref() == model.provider_id) {
                    accounts.push(AccountChoice { provider: model.provider_id.clone().into(), name: account.name.clone().into(), plan: account.plan.clone().map(Into::into) });
                }
            }
            catalog.entries.push(Entry {
                reference: model.reference(),
                name_key: model.name.to_lowercase(),
                name: model.name.clone().into(),
                line: line.into(),
                detail: None,
                switched: false,
                provider: model.provider_id.clone().into(),
                rail,
                efforts: model.efforts.clone(),
                speeds: model.speeds.clone(),
                aliases: model.aliases.clone(),
                rest_key: String::new(),
            });
        }
        // A rail with two or more accounts switches between them: each of its models says which
        // account it's on wherever it's shown with others' (a search, Starred), and its agent's
        // name under the switch. One with a single account is as it was.
        for rail in &mut catalog.rails {
            if rail.accounts.len() < 2 {
                rail.accounts.clear();
            }
        }
        // (Whether a rail's mark stands for more than one provider: OpenCode Zen and Go.)
        let shared: Vec<bool> = (0..catalog.rails.len()).map(|rail| {
            let mut providers = catalog.entries.iter().filter(|entry| entry.rail == rail).map(|entry| entry.provider.clone());
            let first = providers.next();
            providers.any(|each| Some(&each) != first.as_ref())
        }).collect();
        for entry in &mut catalog.entries {
            let rail = &catalog.rails[entry.rail];
            let free = offered.models.iter().any(|model| model.free && model.provider_id == entry.provider.as_ref() && model.id == entry.reference.id);
            let mut detail = None;
            if let Some(account) = rail.accounts.iter().find(|each| each.provider == entry.provider) {
                entry.line = account.line().into();
                entry.switched = true;
                detail = Some(account.line());
            } else if shared[entry.rail] {
                let provider = offered.providers.iter().find(|each| each.id == entry.provider.as_ref());
                let mut named = provider.map(|each| each.name.clone()).unwrap_or_else(|| entry.provider.to_string());
                if let Some(label) = provider.and_then(|each| each.label.as_ref()) {
                    named = format!("{named} · {label}");
                }
                detail = Some(named);
            }
            if free {
                detail = Some(detail.map_or_else(|| "Free".to_owned(), |detail| format!("{detail} · Free")));
            }
            entry.detail = detail.map(Into::into);
            let model = &entry.reference;
            entry.rest_key = format!("{} {} {} {}", model.id, model.provider_id, entry.line, entry.aliases.join(" ")).to_lowercase();
        }
        catalog.speeds = offered.providers.iter().filter_map(|each| Some((each.id.clone(), each.speed.clone()?))).collect();
        catalog
    }

    /// With a choice ahead of the models, for a setting that can be left to something else.
    pub fn with_unset(mut self, title: impl Into<SharedString>, line: impl Into<SharedString>, chosen: bool) -> Catalog {
        self.unset = Some(Unset { title: title.into(), line: line.into(), chosen });
        self
    }

    /// Where `model` is among them.
    pub fn position(&self, model: &ModelRef) -> Option<usize> {
        self.entries.iter().position(|entry| entry.is(&model.provider_id, &model.id))
    }
}

/// The models matching `query`, best first: every word of it must be found, in the name (the
/// letters in order, as a fuzzy finder does: "cop" finds "Claude Opus") or as written in where
/// it's from (its id, its provider). A match in the name, from a word's start, and the letters
/// together come first; the box's order otherwise. `None` once `cancel` is set (a newer search).
pub fn matching(catalog: &Catalog, query: &str, cancel: &AtomicBool) -> Option<Vec<usize>> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Some((0..catalog.entries.len()).collect());
    }
    let mut found: Vec<(i64, usize)> = Vec::new();
    for (ix, entry) in catalog.entries.iter().enumerate() {
        if ix % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        let mut total = 0;
        let mut all = true;
        for word in &words {
            match word_score(&entry.name_key, &entry.rest_key, word) {
                Some(score) => total += score,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            found.push((total, ix));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    Some(found.into_iter().map(|(_, ix)| ix).collect())
}

/// How well `word` is found in a model's name, or in the rest it's found by; none where it isn't.
fn word_score(name: &str, rest: &str, word: &str) -> Option<i64> {
    let starts_word = |text: &str, at: usize| at == 0 || text[..at].chars().next_back().is_some_and(|c| !c.is_alphanumeric());
    if let Some(at) = name.find(word) {
        let start = name.match_indices(word).find(|(at, _)| starts_word(name, *at)).map(|(at, _)| at);
        return Some(match start {
            Some(0) => 1000,
            Some(_) => 900,
            None => 700 - at.min(50) as i64,
        });
    }
    if let Some(score) = in_order(name, word) {
        return Some(300 + score);
    }
    if let Some(at) = rest.find(word) {
        return Some(if starts_word(rest, at) { 200 } else { 100 });
    }
    None
}

/// `word`'s letters found in order in `text`: more for letters together and at words' starts.
fn in_order(text: &str, word: &str) -> Option<i64> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let mut score = 0i64;
    let mut last: Option<usize> = None;
    for wanted in word.chars() {
        let found = (at..chars.len()).find(|&ix| chars[ix] == wanted)?;
        if last.is_some_and(|last| last + 1 == found) {
            score += 8;
        }
        if found == 0 || !chars[found - 1].is_alphanumeric() {
            score += 10;
        }
        score -= (found - at).min(10) as i64;
        last = Some(found);
        at = found + 1;
    }
    Some(score)
}

// MARK: The picker

/// What's picked: a model, or the choice ahead of them.
pub type OnEffort = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;
pub type OnPick = Rc<dyn Fn(Option<ModelRef>, &mut Window, &mut App)>;
/// Your starred models, as they are now.
pub type OnStarred = Rc<dyn Fn(Vec<Starred>, &mut App)>;
/// A mode picked in the permissions.
pub type OnApproval = Rc<dyn Fn(Approval, &mut Window, &mut App)>;

/// Reasoning, on the right, where the choice takes an effort: what picking one does, and whether
/// it can be left to the model ("Default", a bot's).
#[derive(Clone)]
pub struct Reasoning {
    pub on_effort: OnEffort,
    pub default: bool,
}

/// The conversation's permissions, at the bottom's right (the composer's): its mode as its box
/// shows it, and what picking one does.
#[derive(Clone)]
pub struct Permissions {
    pub shown: ApprovalShown,
    pub pick: OnApproval,
}

/// Which list shows when nothing's typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    Starred,
    Rail(usize),
}

/// One of the list's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Unset,
    Model(usize),
    /// "Show more": the rest of the provider's.
    More,
}

pub struct ModelPicker {
    catalog: Arc<Catalog>,
    /// Your starred models, in the order starred; `None` where the box keeps none.
    starred: Option<Vec<Starred>>,
    current: Option<ModelRef>,
    /// Where a star is kept (none: a picker that can't keep one).
    client: Option<BoxClient>,
    field: Entity<TextField>,
    query: String,
    shown: Shown,
    rows: Vec<Row>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    /// The providers whose every model shows ("Show more"), by rail id.
    expanded: std::collections::HashSet<SharedString>,
    /// A search on its way, dropped (and stopped, `cancel`) as a newer one starts.
    searching: Option<Task<()>>,
    cancel: Arc<AtomicBool>,
    /// Return (or a ⌘ number: which row) was pressed before the search was found: that row of what
    /// it finds is picked.
    pick_when_found: Option<usize>,
    /// Said instead of the list: why nothing can be picked now ("Finding where this conversation
    /// runs…").
    notice: Option<SharedString>,
    /// Said under the list (models this app can't put the conversation on yet).
    note: Option<SharedString>,
    /// A star or a speed the box couldn't keep, and why.
    problem: Option<SharedString>,
    height: f32,
    /// The account each rail with an account switch shows, by rail id (none chosen: the current
    /// model's, else the first).
    accounts: std::collections::HashMap<SharedString, SharedString>,
    /// The account switch's list is open, with this one highlighted; its filter (from
    /// `FILTERED` accounts up), and what's typed in it.
    accounts_open: bool,
    account_highlight: usize,
    account_filter: Entity<TextField>,
    account_query: String,
    on_pick: OnPick,
    reasoning: Option<Reasoning>,
    on_starred: Option<OnStarred>,
    /// Standard | Fast shows, where the current model has a speed.
    speed: bool,
    /// The speed picked here for each provider since it opened (`None`: Standard), shown at once.
    speeds: std::collections::HashMap<String, Option<String>>,
    speed_sending: Option<Task<()>>,
    permissions: Option<Permissions>,
    permissions_open: bool,
    /// Star changes waiting to go to the box, one at a time in the order made; the one on its
    /// way; and the stars as the box last said them.
    star_queue: std::collections::VecDeque<(Vec<Starred>, bool)>,
    star_sending: Option<Task<()>>,
    box_starred: Vec<Starred>,
    _field: Subscription,
    _account_filter: Subscription,
}

impl EventEmitter<DismissEvent> for ModelPicker {}

impl Focusable for ModelPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.focus_handle(cx)
    }
}

/// How many rows the list is tall for `catalog`.
fn list_rows(catalog: &Catalog) -> usize {
    (catalog.entries.len() + usize::from(catalog.unset.is_some())).clamp(3, MOST_ROWS)
}

impl ModelPicker {
    /// A picker of what `catalog` offers, `current` marked (and its provider's list shown first),
    /// your stars as the box said them (`None`: it keeps none), kept through `client`.
    pub fn new(catalog: Arc<Catalog>, starred: Option<Vec<Starred>>, current: Option<ModelRef>, client: Option<BoxClient>, on_pick: OnPick, window: &mut Window, cx: &mut Context<Self>) -> ModelPicker {
        let field = cx.new(|cx| TextField::new(cx).bare().placeholder("Search models…"));
        let subscription = cx.subscribe_in(&field, window, |this: &mut ModelPicker, field, event: &TextFieldEvent, window, cx| {
            if let TextFieldEvent::Changed = event {
                let query = field.read(cx).text().to_owned();
                this.search(query, window, cx);
            }
        });
        let account_filter = cx.new(|cx| TextField::new(cx).bare().placeholder("Filter accounts…"));
        let filter_subscription = cx.subscribe_in(&account_filter, window, |this: &mut ModelPicker, field, event: &TextFieldEvent, _, cx| {
            if let TextFieldEvent::Changed = event {
                this.account_query = field.read(cx).text().to_owned();
                this.account_highlight = 0;
                cx.notify();
            }
        });
        let starred = starred.filter(|_| client.is_some());
        // Its list first: the current model's provider, else your starred ones, else the first.
        let at = current.as_ref().and_then(|current| catalog.position(current));
        let shown = match at {
            Some(at) => Shown::Rail(catalog.entries[at].rail),
            None if starred.as_ref().is_some_and(|starred| !starred.is_empty()) => Shown::Starred,
            None if catalog.rails.is_empty() && starred.is_some() => Shown::Starred,
            None => Shown::Rail(0),
        };
        let mut picker = ModelPicker {
            height: list_rows(&catalog) as f32 * ROW + 8.,
            catalog,
            starred,
            current,
            client,
            field,
            query: String::new(),
            shown,
            rows: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            expanded: Default::default(),
            searching: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pick_when_found: None,
            notice: None,
            note: None,
            problem: None,
            on_pick,
            reasoning: None,
            on_starred: None,
            speed: false,
            speeds: Default::default(),
            speed_sending: None,
            permissions: None,
            permissions_open: false,
            star_queue: Default::default(),
            star_sending: None,
            box_starred: Vec::new(),
            accounts: Default::default(),
            accounts_open: false,
            account_highlight: 0,
            account_filter,
            account_query: String::new(),
            _field: subscription,
            _account_filter: filter_subscription,
        };
        if let Some(entry) = at.map(|at| &picker.catalog.entries[at]) {
            let rail = &picker.catalog.rails[entry.rail];
            // Its account's list, where the current model is on one of several.
            if rail.accounts.iter().any(|each| each.provider == entry.provider) {
                picker.accounts.insert(rail.id.clone(), entry.provider.clone());
            }
            // And every one of its provider's, where it's past the first few.
            let before = picker.catalog.entries[..at.unwrap_or(0)].iter().filter(|each| each.rail == entry.rail && (!each.switched || each.provider == entry.provider)).count();
            if before >= FIRST {
                picker.expanded.insert(rail.id.clone());
            }
        }
        picker.box_starred = picker.starred.clone().unwrap_or_default();
        picker.show_list();
        // The current one highlighted, in sight.
        let current_row = picker.rows.iter().position(|row| matches!(row, Row::Model(ix) if Some(*ix) == at));
        picker.selected = current_row.or(if picker.rows.is_empty() { None } else { Some(0) });
        if let Some(row) = picker.selected {
            picker.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        picker
    }

    /// Reasoning on the right: the current model's efforts, `on_effort` told the one picked
    /// (`default`: "Default", leaving it to the model, ahead of them).
    pub fn reasoning(mut self, on_effort: Option<OnEffort>, default: bool) -> Self {
        self.reasoning = on_effort.map(|on_effort| Reasoning { on_effort, default });
        self
    }

    /// Standard | Fast at the bottom's left, where the current model's provider has a fast mode
    /// (the composer's and a side chat's: the speed is the box's, for that provider).
    pub fn with_speed(mut self) -> Self {
        self.speed = true;
        self
    }

    /// The conversation's permissions at the bottom's right (the composer's).
    pub fn permissions(mut self, permissions: Option<Permissions>) -> Self {
        self.permissions = permissions;
        self
    }

    /// Keep the last catalog visible while the host reads it again.
    pub fn refresh_when_opened(self, before: Option<Offered>, scope: RefreshModels, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Some(client) = self.client.clone() else { return self };
        let wanted = self.current.clone();
        let side = match &scope { RefreshModels::Offered { side, .. } => *side, RefreshModels::Defaults => None };
        let read = cx.background_executor().spawn(async move {
            match scope {
                RefreshModels::Offered { folder, session, .. } => client.refresh_offered_models(wanted.as_ref(), folder.as_deref(), session.as_deref()),
                RefreshModels::Defaults => client.models_for_defaults(true),
            }
                .map(|offered| offered.keeping(before.as_ref()))
                .ok()
                .filter(|offered| Some(offered) != before.as_ref())
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(offered) = read.await else { return };
            this.update_in(cx, |this, window, cx| {
                let mut catalog = Catalog::new(&offered, |model| offered.scoped || side.is_none_or(|side| model.side == side));
                catalog.unset = this.catalog.unset.clone();
                this.replace_catalog(catalog, window, cx);
            }).ok();
        }).detach();
        self
    }

    pub(crate) fn replace_catalog(&mut self, catalog: Catalog, window: &mut Window, cx: &mut Context<Self>) {
        if let Shown::Rail(at) = self.shown {
            let rail = self.catalog.rails.get(at);
            self.shown = Shown::Rail(rail.and_then(|rail| catalog.rails.iter().position(|entry| entry.id == rail.id)).unwrap_or(0));
        }
        self.height = list_rows(&catalog) as f32 * ROW + 8.;
        self.catalog = Arc::new(catalog);
        self.rows.clear();
        self.run_search(self.query.clone(), window, cx);
    }

    /// Told each time a star changes, with your starred models as they are now.
    pub fn on_starred(mut self, on_starred: OnStarred) -> Self {
        self.on_starred = Some(on_starred);
        self
    }

    /// Said instead of the list: why nothing can be picked now.
    pub fn notice(mut self, notice: Option<SharedString>) -> Self {
        self.notice = notice;
        self
    }

    /// Said under the list.
    pub fn note(mut self, note: Option<SharedString>) -> Self {
        self.note = note;
        self
    }

    /// Types `query` into its search (shots and tests).
    pub fn type_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.set_text(query, cx));
    }

    /// Shows a provider's list, by its rail id, or "starred" (shots and tests).
    pub fn show_rail(&mut self, id: &str, cx: &mut Context<Self>) {
        if id == "starred" && self.starred.is_some() {
            self.choose(Shown::Starred, cx);
        } else if let Some(at) = self.catalog.rails.iter().position(|rail| rail.id.as_ref() == id) {
            self.choose(Shown::Rail(at), cx);
        }
    }

    /// Highlights row `row`, as the pointer over it does (shots).
    pub fn highlight(&mut self, row: usize, cx: &mut Context<Self>) {
        self.selected = (row < self.rows.len()).then_some(row);
        cx.notify();
    }

    /// Opens the permissions' popover (shots).
    pub fn open_permissions(&mut self, cx: &mut Context<Self>) {
        if self.permissions.is_some() {
            self.permissions_open = true;
            cx.notify();
        }
    }

    /// The rows shown now, by name (tests).
    pub fn shown_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| match row {
                Row::Unset => self.catalog.unset.as_ref().map(|unset| unset.title.to_string()).unwrap_or_default(),
                Row::Model(ix) => self.catalog.entries[*ix].name.to_string(),
                Row::More => "Show more".to_owned(),
            })
            .collect()
    }

    /// Whether a search is still on its way (tests).
    pub fn is_searching(&self) -> bool {
        self.searching.is_some()
    }

    fn is_starred(&self, entry: &Entry) -> bool {
        self.starred.as_ref().is_some_and(|starred| starred.iter().any(|each| entry.is(&each.provider_id, &each.id)))
    }

    /// The current model, as the catalog has it.
    fn current_entry(&self) -> Option<&Entry> {
        let current = self.current.as_ref()?;
        self.catalog.position(current).map(|at| &self.catalog.entries[at])
    }

    /// The rows of the list shown while nothing's typed.
    fn show_list(&mut self) {
        let unset = self.catalog.unset.is_some().then_some(Row::Unset);
        let models: Vec<Row> = match self.shown {
            // Your starred ones that are offered here, in the order starred.
            Shown::Starred => self.starred.iter().flatten().filter_map(|each| self.catalog.entries.iter().position(|entry| entry.is(&each.provider_id, &each.id))).map(Row::Model).collect(),
            // (Under its account switch, the account shown's; the first few, then "Show more".)
            Shown::Rail(rail) => {
                let account = self.account_shown(rail).cloned();
                let mut all: Vec<Row> = self.catalog.entries.iter().enumerate().filter(|(_, entry)| entry.rail == rail && account.as_ref().is_none_or(|account| !entry.switched || entry.provider == account.provider)).map(|(ix, _)| Row::Model(ix)).collect();
                let expanded = self.catalog.rails.get(rail).is_some_and(|each| self.expanded.contains(&each.id));
                if !expanded && all.len() > FIRST + 1 {
                    all.truncate(FIRST);
                    all.push(Row::More);
                }
                all
            }
        };
        self.rows = unset.into_iter().chain(models).collect();
    }

    /// "Show more": every one of the provider's shown, the first of the rest highlighted.
    fn show_more(&mut self, cx: &mut Context<Self>) {
        let Shown::Rail(rail) = self.shown else { return };
        let Some(id) = self.catalog.rails.get(rail).map(|each| each.id.clone()) else { return };
        let at = self.rows.iter().position(|row| *row == Row::More);
        self.expanded.insert(id);
        self.show_list();
        self.selected = at.filter(|at| *at < self.rows.len());
        if let Some(at) = self.selected {
            self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// The account `rail`'s list shows, where it has a switch.
    fn account_shown(&self, rail: usize) -> Option<&AccountChoice> {
        let rail = self.catalog.rails.get(rail)?;
        let chosen = self.accounts.get(&rail.id);
        rail.accounts.iter().find(|each| Some(&each.provider) == chosen).or(rail.accounts.first())
    }

    /// The rail whose account switch shows now: a provider's list with two or more accounts, while
    /// nothing's typed.
    fn switch_rail(&self) -> Option<usize> {
        match self.shown {
            Shown::Rail(rail) if self.query.trim().is_empty() && self.notice.is_none() && self.catalog.rails.get(rail).is_some_and(|each| each.accounts.len() >= 2) => Some(rail),
            _ => None,
        }
    }

    /// The accounts the switch's list shows, as filtered: their places in the rail's.
    fn accounts_listed(&self, rail: usize) -> Vec<usize> {
        let query = self.account_query.trim().to_lowercase();
        self.catalog.rails[rail].accounts.iter().enumerate().filter(|(_, each)| query.is_empty() || each.line().to_lowercase().contains(&query)).map(|(ix, _)| ix).collect()
    }

    fn toggle_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.accounts_open {
            self.close_accounts(window, cx);
            return;
        }
        let Some(rail) = self.switch_rail() else { return };
        self.accounts_open = true;
        self.permissions_open = false;
        self.account_filter.update(cx, |field, cx| field.set_text("", cx));
        self.account_query.clear();
        let shown = self.account_shown(rail).map(|each| each.provider.clone());
        self.account_highlight = self.catalog.rails[rail].accounts.iter().position(|each| Some(&each.provider) == shown.as_ref()).unwrap_or(0);
        if self.catalog.rails[rail].accounts.len() >= FILTERED {
            let focus = self.account_filter.focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn close_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.accounts_open = false;
        let focus = self.field.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Shows the account at `at` (its place in the rail's) in `rail`'s list.
    fn choose_account(&mut self, rail: usize, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.catalog.rails.get(rail).and_then(|each| each.accounts.get(at)) else { return };
        self.accounts.insert(self.catalog.rails[rail].id.clone(), account.provider.clone());
        self.close_accounts(window, cx);
        self.show_list();
        // The current model, where it's on this account; else the first.
        let current = self.current.as_ref().and_then(|current| self.catalog.position(current));
        self.selected = self.rows.iter().position(|row| matches!(row, Row::Model(ix) if Some(*ix) == current)).or((!self.rows.is_empty()).then_some(0));
        self.scroll.scroll_to_item(self.selected.unwrap_or(0), ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Shows the account named `name` in the list shown (shots and tests).
    pub fn show_account(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Shown::Rail(rail) = self.shown else { return };
        if let Some(at) = self.catalog.rails[rail].accounts.iter().position(|each| each.name.as_ref() == name) {
            self.choose_account(rail, at, window, cx);
        }
    }

    /// Opens the account switch's list (shots).
    pub fn open_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accounts_open {
            self.toggle_accounts(window, cx);
        }
    }

    fn choose(&mut self, shown: Shown, cx: &mut Context<Self>) {
        self.shown = shown;
        self.accounts_open = false;
        self.permissions_open = false;
        if !self.query.is_empty() {
            // (Clearing the search shows the list.)
            self.field.update(cx, |field, cx| field.set_text("", cx));
        }
        self.show_list();
        self.selected = (!self.rows.is_empty()).then_some(0);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Finds what `query` names, off the UI thread; a search already on its way is dropped.
    fn search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        if query == self.query {
            return;
        }
        self.run_search(query, window, cx);
    }

    fn run_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.query = query.clone();
        if !query.trim().is_empty() {
            self.accounts_open = false;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.searching = None;
        // (A Return pressed for the search before is for that one only.)
        self.pick_when_found = None;
        if query.trim().is_empty() {
            self.show_list();
            self.selected = (!self.rows.is_empty()).then_some(0);
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let catalog = self.catalog.clone();
        let found = cx.background_executor().spawn(async move { matching(&catalog, &query, &cancel) });
        self.searching = Some(cx.spawn_in(window, async move |this, cx| {
            let Some(found) = found.await else { return };
            this.update_in(cx, |this, window, cx| {
                this.searching = None;
                this.rows = found.into_iter().map(Row::Model).collect();
                this.selected = (!this.rows.is_empty()).then_some(0);
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                if let Some(row) = this.pick_when_found.take() {
                    this.pick_row(row, window, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(rail) = self.switch_rail().filter(|_| self.accounts_open) {
            let listed = self.accounts_listed(rail);
            if let Some(at) = listed.iter().position(|ix| *ix == self.account_highlight).or((!listed.is_empty()).then_some(0)) {
                self.account_highlight = listed[(at as isize + by).clamp(0, listed.len() as isize - 1) as usize];
                cx.notify();
            }
            return;
        }
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        let next = match self.selected {
            None if by > 0 => 0,
            None => last,
            Some(at) => (at as isize + by).clamp(0, last),
        } as usize;
        self.selected = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn step_rail(&mut self, by: isize, cx: &mut Context<Self>) {
        // The rail: your starred ones (where the box keeps them), then each provider.
        let mut all: Vec<Shown> = Vec::new();
        if self.starred.is_some() {
            all.push(Shown::Starred);
        }
        all.extend((0..self.catalog.rails.len()).map(Shown::Rail));
        if all.is_empty() {
            return;
        }
        let at = all.iter().position(|each| *each == self.shown).unwrap_or(0) as isize;
        let next = (at + by).clamp(0, all.len() as isize - 1) as usize;
        self.choose(all[next], cx);
    }

    /// Picks row `row` of the list shown, or, while a search is on its way, of what it finds.
    fn pick_number(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.searching.is_some() {
            self.pick_when_found = Some(row);
        } else {
            self.pick_row(row, window, cx);
        }
    }

    fn pick_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(row).copied() else { return };
        let picked = match row {
            Row::Unset => None,
            Row::Model(ix) => Some(self.catalog.entries[ix].reference.clone()),
            Row::More => return self.show_more(cx),
        };
        let on_pick = self.on_pick.clone();
        cx.emit(DismissEvent);
        on_pick(picked, window, cx);
    }

    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rail) = self.switch_rail().filter(|_| self.accounts_open) {
            let listed = self.accounts_listed(rail);
            if let Some(at) = listed.iter().find(|ix| **ix == self.account_highlight).or(listed.first()).copied() {
                self.choose_account(rail, at, window, cx);
            }
            return;
        }
        // Return before the search has found anything: what it finds first.
        if self.searching.is_some() {
            self.pick_when_found = Some(0);
            return;
        }
        if let Some(row) = self.selected {
            self.pick_row(row, window, cx);
        }
    }

    /// Escape: the permissions' popover first, then the account list, then the picker.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.permissions_open {
            self.permissions_open = false;
            cx.notify();
        } else if self.accounts_open {
            self.close_accounts(window, cx);
        } else {
            cx.emit(DismissEvent);
        }
    }

    fn pick_effort(&mut self, effort: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reasoning) = self.reasoning.clone() else { return };
        let unchanged = self.current.as_ref().is_some_and(|current| current.variant == effort);
        cx.emit(DismissEvent);
        if !unchanged {
            (reasoning.on_effort)(effort, window, cx);
        }
    }

    fn toggle_star(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(starred) = self.starred.as_mut() else { return };
        let entry = &self.catalog.entries[ix];
        let model = entry.starred();
        let on = !starred.iter().any(|each| entry.is(&each.provider_id, &each.id));
        // (Its id as the box keeps it: one starred by another name is unstarred by that.)
        let sent: Vec<Starred> = if on { vec![model.clone()] } else { starred.iter().filter(|each| entry.is(&each.provider_id, &each.id)).cloned().collect() };
        // Shown at once; kept by the box in the order made.
        if on {
            starred.push(model);
        } else {
            starred.retain(|each| !entry.is(&each.provider_id, &each.id));
        }
        self.problem = None;
        self.star_queue.push_back((sent, on));
        self.starred_changed(cx);
        if self.star_sending.is_none() {
            self.send_star(cx);
        }
    }

    /// Sends the next star change waiting; once none waits, what the box says is shown.
    fn send_star(&mut self, cx: &mut Context<Self>) {
        let (Some((sent, on)), Some(client)) = (self.star_queue.pop_front(), self.client.clone()) else {
            self.star_sending = None;
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let mut last = Ok(Vec::new());
            for model in sent {
                last = client.set_starred(&model, on);
                if last.is_err() {
                    break;
                }
            }
            last
        });
        self.star_sending = Some(cx.spawn(async move |this, cx| {
            let kept = task.await;
            this.update(cx, |this, cx| {
                match kept {
                    Ok(now) => {
                        this.box_starred = now;
                        if this.star_queue.is_empty() {
                            this.starred = Some(this.box_starred.clone());
                        }
                    }
                    // What the box has, and none of the changes after this one.
                    Err(error) => {
                        this.star_queue.clear();
                        this.starred = Some(this.box_starred.clone());
                        this.problem = Some(format!("The star wasn’t kept: {error}").into());
                    }
                }
                this.starred_changed(cx);
                this.send_star(cx);
            })
            .ok();
        }));
    }

    fn starred_changed(&mut self, cx: &mut Context<Self>) {
        if self.query.trim().is_empty() && self.shown == Shown::Starred {
            self.show_list();
            self.selected = self.selected.map(|at| at.min(self.rows.len().saturating_sub(1))).filter(|_| !self.rows.is_empty());
        }
        if let (Some(on_starred), Some(starred)) = (self.on_starred.clone(), self.starred.clone()) {
            on_starred(starred, cx);
        }
        cx.notify();
    }

    /// The speed `provider`'s models run at now: as picked here, else as its box said.
    fn speed_of(&self, provider: &str) -> Option<String> {
        match self.speeds.get(provider) {
            Some(picked) => picked.clone(),
            None => self.catalog.speeds.get(provider).cloned(),
        }
    }

    /// Picks the speed for the current model's provider: shown at once, kept by its box.
    fn pick_speed(&mut self, speed: Option<String>, cx: &mut Context<Self>) {
        let (Some(entry), Some(client)) = (self.current_entry(), self.client.clone()) else { return };
        let provider = entry.provider.to_string();
        let before = self.speed_of(&provider);
        if before == speed {
            return;
        }
        self.speeds.insert(provider.clone(), speed.clone());
        self.problem = None;
        cx.notify();
        let task = cx.background_executor().spawn({
            let provider = provider.clone();
            async move { client.set_speed(&provider, speed.as_deref()) }
        });
        self.speed_sending = Some(cx.spawn(async move |this, cx| {
            let kept = task.await;
            this.update(cx, |this, cx| {
                match kept {
                    Ok(now) => {
                        this.speeds.insert(provider, now);
                    }
                    Err(error) => {
                        this.speeds.insert(provider, before);
                        this.problem = Some(format!("The speed wasn’t changed: {error}").into());
                    }
                }
                this.speed_sending = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Picks a mode in the permissions: set (the popover closes), or, for Full Access not yet
    /// confirmed on this computer, the picker closes so the composer asks.
    fn pick_approval(&mut self, approval: Approval, window: &mut Window, cx: &mut Context<Self>) {
        let Some(permissions) = self.permissions.as_mut() else { return };
        let choice = permissions.shown.choice(&approval).cloned();
        let why = choice.as_ref().and_then(|choice| choice.unavailable.as_ref()).map(|why| why.reason.clone());
        match why.as_deref() {
            // Not to be had here, as its box says: shown, not picked.
            Some(reason) if reason != "fullAccessOff" => return,
            Some(_) => {
                let pick = permissions.pick.clone();
                cx.emit(DismissEvent);
                pick(approval, window, cx);
            }
            None => {
                permissions.shown.approval = approval.clone();
                self.permissions_open = false;
                let pick = permissions.pick.clone();
                cx.notify();
                pick(approval, window, cx);
            }
        }
    }

    fn rows_in(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = cx.theme().colors.clone();
        let stars = self.starred.is_some();
        // (Under the account switch, which names the account, a model says no more.)
        let switched = self.switch_rail().is_some();
        // (Listed with other providers', a row says its own: OpenAI's and Codex's marks look alike.)
        let mixed = !self.query.trim().is_empty() || self.shown == Shown::Starred;
        range
            .filter_map(|row| {
                let kind = *self.rows.get(row)?;
                let selected = self.selected == Some(row);
                if kind == Row::More {
                    return Some(
                        div()
                            .id(("model-row", row))
                            .debug_selector(|| "model-show-more".into())
                            .flex()
                            .items_center()
                            .w_full()
                            .h(scaled(px(ROW)))
                            .px(space::S3)
                            .rounded(radius::NAV)
                            .cursor_pointer()
                            .when(selected, |d| d.bg(c.hover))
                            .hover(|d| d.bg(c.hover))
                            .text_ui(TypeSize::Body)
                            .text_color(c.text_muted)
                            .on_click(cx.listener(|this, _, _, cx| this.show_more(cx)))
                            .child("Show more")
                            .into_any_element(),
                    );
                }
                let number = (row < NUMBERED).then(|| SharedString::from(format!("⌘{}", row + 1)));
                let (selector, name, detail, mark, current, star) = match kind {
                    Row::Unset => {
                        let unset = self.catalog.unset.as_ref()?;
                        let line = (!unset.line.is_empty()).then(|| unset.line.clone());
                        ("model-row-unset".to_owned(), unset.title.clone(), line, None, unset.chosen, None)
                    }
                    Row::Model(ix) => {
                        let entry = &self.catalog.entries[ix];
                        let rail = &self.catalog.rails[entry.rail];
                        let current = self.current.as_ref().is_some_and(|current| entry.is(&current.provider_id, &current.id));
                        let star = stars.then(|| (ix, self.is_starred(entry)));
                        let detail = entry.detail.clone().filter(|_| !(switched && entry.switched)).or_else(|| mixed.then(|| rail.name.clone()));
                        (format!("model-row-{}/{}", entry.reference.provider_id, entry.reference.id), entry.name.clone(), detail, Some((rail.mark, rail.color)), current, star)
                    }
                    Row::More => unreachable!(),
                };
                let star_selector = selector.replacen("model-row-", "model-star-", 1);
                let mark = match mark {
                    Some((Some(path), color)) => kuvo_ui::icon::Mark::new(path).icon_size(IconSize::Nav).color(color.unwrap_or(c.text_muted)).into_any_element(),
                    Some((None, _)) => Icon::new(IconName::Sparkle).size(IconSize::Nav).color(c.text_faint).into_any_element(),
                    None => div().size(px(16.)).into_any_element(),
                };
                Some(
                    div()
                        .id(("model-row", row))
                        .group("model-row")
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .w_full()
                        .h(scaled(px(ROW)))
                        .pl(space::S3)
                        .pr(space::S1_5)
                        .rounded(radius::NAV)
                        .cursor_pointer()
                        .when(selected, |d| d.bg(c.hover))
                        .hover(|d| d.bg(c.hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.pick_row(row, window, cx)))
                        .child(div().flex().flex_none().items_center().justify_center().size(px(16.)).child(mark))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .items_center()
                                .gap(space::S1_5)
                                .min_w_0()
                                .child(div().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(name))
                                .when(current, |d| d.child(div().flex_none().debug_selector(|| "model-current".into()).child(Icon::new(IconName::Check).size(IconSize::XSmall).color(c.text_muted))))
                                .children(detail.map(|detail| div().min_w_0().truncate().text_ui(TypeSize::Caption).text_color(c.text_faint).child(detail))),
                        )
                        // ☆ on the row under the pointer (or highlighted); ★ always where starred.
                        .children(star.map(|(ix, on)| {
                            div()
                                .id(("model-star", ix))
                                .debug_selector(move || star_selector)
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(24.))
                                .rounded(radius::SMALL)
                                .when(!on && !selected, |d| d.opacity(0.).group_hover("model-row", |s| s.opacity(1.)))
                                .hover(|d| d.bg(c.pressed))
                                .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(if on { "Remove from Favourites" } else { "Add to Favourites" }).build(cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_star(ix, cx);
                                }))
                                .child(Icon::new(if on { IconName::StarFilled } else { IconName::Star }).size(IconSize::Nav).color(if on { c.text } else { c.text_muted }))
                        }))
                        .children(number.map(|keys| div().flex_none().child(keycap(keys, c.border, c.text_faint))))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors.clone();
        let searching = !self.query.trim().is_empty();
        let button = |id: String, shown: Shown, tip: SharedString, mark: AnyElement, cx: &mut Context<Self>| {
            // (While searching, all of them are searched: none is marked.)
            let on = !searching && self.shown == shown;
            let selector = id.clone();
            div()
                .relative()
                .flex()
                .justify_center()
                .w_full()
                .child(
                    div()
                        .id(SharedString::from(id))
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(34.))
                        .rounded(radius::NAV)
                        .cursor_pointer()
                        .when(on, |d| d.bg(c.selected))
                        .hover(|d| d.bg(c.hover))
                        .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(tip.clone()).build(cx))
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(shown, cx)))
                        .child(mark),
                )
                // The provider shown, marked in blue at the rail's edge.
                .when(on, |d| d.child(div().absolute().right_0().top(px(9.)).w(px(3.)).h(px(16.)).rounded_l(px(2.)).bg(c.accent)))
        };
        let mut items: Vec<AnyElement> = Vec::new();
        if self.starred.is_some() {
            items.push(button("model-rail-starred".into(), Shown::Starred, "Favourites".into(), Icon::new(IconName::StarFilled).size(IconSize::Medium).color(c.text).into_any_element(), cx).into_any_element());
            if !self.catalog.rails.is_empty() {
                items.push(div().mx(space::S3).my(space::S1).h(px(1.)).bg(c.border).into_any_element());
            }
        }
        for (ix, rail) in self.catalog.rails.iter().enumerate() {
            let mark = match rail.mark {
                Some(path) => kuvo_ui::icon::Mark::new(path).icon_size(IconSize::Medium).color(rail.color.unwrap_or(c.text)).into_any_element(),
                None => div().text_ui(TypeSize::Small).medium().text_color(c.text_muted).child(rail.name.chars().next().map(|first| first.to_uppercase().to_string()).unwrap_or_default()).into_any_element(),
            };
            items.push(button(format!("model-rail-{}", rail.id), Shown::Rail(ix), rail.name.clone(), mark, cx).into_any_element());
        }
        div()
            .id("model-rail")
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(space::S1)
            .w(px(RAIL))
            .py(space::S2)
            .overflow_y_scroll()
            .border_r_1()
            .border_color(c.border)
            .children(items)
            .into_any_element()
    }

    /// The account switch over a provider's list with two or more accounts: the account shown, by
    /// name and plan, and ▾ for the others. Its edges, corners and text line up with the rows'
    /// highlight below it.
    fn account_switch(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rail = self.switch_rail()?;
        let account = self.account_shown(rail)?.clone();
        let c = cx.theme().colors.clone();
        let open = self.accounts_open;
        let count = self.catalog.rails[rail].accounts.len();
        Some(
            div()
                .px(space::S1)
                .pt(space::S1)
                .child(
                    div()
                        .id("model-account-switch")
                        .debug_selector(|| "model-account-switch".into())
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .h(scaled(px(34.)))
                        .pl(space::S3)
                        .pr(space::S2)
                        .rounded(radius::NAV)
                        .border_1()
                        .border_color(if open { c.accent } else { c.border })
                        .bg(if open { c.elevated } else { c.hover })
                        .cursor_pointer()
                        .hover(|d| d.border_color(c.border_strong))
                        .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(format!("Switch between your {count} accounts")).build(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_accounts(window, cx)))
                        .child(div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_faint).child("Account"))
                        .child(div().min_w_0().truncate().text_ui(TypeSize::Caption).medium().text_color(c.text).child(account.name.clone()))
                        .children(account.plan.clone().map(|plan| div().flex().flex_none().gap(space::S1_5).text_ui(TypeSize::Caption).child(div().text_color(c.text_faint).child("·")).child(div().text_color(c.text_muted).child(plan))))
                        .child(div().flex_1())
                        .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(IconSize::XSmall).color(c.icon)),
                )
                .into_any_element(),
        )
    }

    /// The account switch's list, open over the models: each account by name and plan, the one
    /// shown checked, and a filter from `FILTERED` accounts up.
    fn account_list(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rail = self.switch_rail().filter(|_| self.accounts_open)?;
        let theme = cx.theme().clone();
        let c = theme.colors.clone();
        let accounts = self.catalog.rails[rail].accounts.clone();
        let shown = self.account_shown(rail).map(|each| each.provider.clone());
        let listed = self.accounts_listed(rail);
        let filter = accounts.len() >= FILTERED;
        // (As many as fit over the list, up to `MOST_ACCOUNTS`.)
        let fit = ((self.height - 12. - if filter { 31. } else { 0. }) / ACCOUNT_ROW).floor().max(2.) as usize;
        let visible = listed.len().clamp(1, MOST_ACCOUNTS.min(fit));
        let rows = listed.iter().map(|&at| {
            let account = &accounts[at];
            let current = Some(&account.provider) == shown.as_ref();
            let highlighted = at == self.account_highlight;
            let selector = format!("model-account-{}", account.provider);
            div()
                .id(("model-account", at))
                .debug_selector(move || selector)
                .flex()
                .flex_none()
                .items_center()
                .gap(space::S1_5)
                .h(scaled(px(ACCOUNT_ROW)))
                .pl(space::S2)
                .pr(space::S2)
                .rounded(radius::SMALL)
                .cursor_pointer()
                .when(highlighted, |d| d.bg(c.hover))
                .hover(|d| d.bg(c.hover))
                .on_click(cx.listener(move |this, _, window, cx| this.choose_account(rail, at, window, cx)))
                .child(div().flex_none().w(px(14.)).when(current, |d| d.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(c.accent))))
                .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text).child(account.name.clone()))
                .children(account.plan.clone().map(|plan| div().flex_none().text_ui(TypeSize::Caption).text_color(c.text_muted).child(plan)))
                .into_any_element()
        });
        let empty = listed.is_empty().then(|| div().h(scaled(px(ACCOUNT_ROW))).px(space::S2).flex().items_center().text_ui(TypeSize::Small).text_color(c.text_muted).child("No accounts match."));
        Some(
            div()
                .id("model-account-list")
                .debug_selector(|| "model-account-list".into())
                .absolute()
                .top(space::S1)
                .left(space::S1)
                .right(space::S1)
                .flex()
                .flex_col()
                .p(space::S1)
                .rounded(radius::NAV)
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .shadow(theme.elevated_shadow())
                .occlude()
                .when(filter, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::S1_5)
                            .h(scaled(px(28.)))
                            .px(space::S2)
                            .mb(px(3.))
                            .border_b_1()
                            .border_color(c.border)
                            .child(Icon::new(IconName::Search).size(IconSize::XSmall).color(c.text_faint))
                            .child(div().flex_1().min_w_0().child(self.account_filter.clone())),
                    )
                })
                .child(div().id("model-account-rows").flex().flex_col().max_h(scaled(px(ACCOUNT_ROW * visible as f32))).overflow_y_scroll().children(rows).children(empty))
                .into_any_element(),
        )
    }

    /// Reasoning, on the right: the current model's efforts (and "Default" where the choice can be
    /// left to it), the one picked checked; the model's name over them.
    fn reasoning_column(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reasoning = self.reasoning.clone()?;
        let c = cx.theme().colors.clone();
        let entry = self.current_entry().cloned();
        let picked = self.current.as_ref().and_then(|current| current.variant.clone());
        let mut rows: Vec<AnyElement> = Vec::new();
        match &entry {
            Some(entry) if !entry.efforts.is_empty() => {
                let choices = reasoning.default.then_some(None).into_iter().chain(entry.efforts.iter().cloned().map(Some));
                for choice in choices {
                    let id = choice.clone().unwrap_or_else(|| "default".to_owned());
                    let selector = format!("model-effort-{id}");
                    let title = choice.as_deref().map(effort_title).unwrap_or_else(|| "Default".into());
                    let on = picked == choice;
                    rows.push(
                        div()
                            .id(SharedString::from(selector.clone()))
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .h(scaled(px(ROW)))
                            .px(space::S2)
                            .rounded(radius::NAV)
                            .cursor_pointer()
                            .hover(|d| d.bg(c.hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.pick_effort(choice.clone(), window, cx)))
                            .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(title))
                            .when(on, |d| d.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(c.text_muted)))
                            .into_any_element(),
                    );
                }
            }
            Some(_) => rows.push(div().px(space::S2).py(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child("This model has no reasoning levels.").into_any_element()),
            None => rows.push(div().px(space::S2).py(space::S2).text_ui(TypeSize::Small).text_color(c.text_muted).child("Pick a model to choose how hard it thinks.").into_any_element()),
        }
        Some(
            div()
                .debug_selector(|| "model-reasoning".into())
                .flex()
                .flex_col()
                .flex_none()
                .w(px(RIGHT))
                .border_l_1()
                .border_color(c.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S2)
                        .h(px(44.))
                        .mx(space::S3)
                        .border_b_1()
                        .border_color(c.border)
                        .child(div().flex_none().text_ui(TypeSize::Body).text_color(c.text_muted).child("Reasoning"))
                        .child(div().flex_1())
                        .children(entry.map(|entry| div().min_w_0().truncate().text_ui(TypeSize::Small).text_color(c.text_faint).child(entry.name))),
                )
                .child(div().id("model-efforts").flex().flex_col().p(space::S1).px(space::S1_5).overflow_y_scroll().children(rows))
                .into_any_element(),
        )
    }

    /// Standard | Fast: the speeds the current model runs at, where it has more than Standard.
    fn speed_switch(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.speed || self.client.is_none() {
            return None;
        }
        let entry = self.current_entry()?.clone();
        if entry.speeds.is_empty() {
            return None;
        }
        let c = cx.theme().colors.clone();
        let picked = self.speed_of(&entry.provider);
        let choices = std::iter::once((None, "Standard".to_owned(), None)).chain(entry.speeds.iter().map(|speed| (Some(speed.id.clone()), speed.name.clone(), speed.description.clone())));
        Some(
            div()
                .debug_selector(|| "model-speed".into())
                .flex()
                .items_center()
                .p(px(2.))
                .rounded(radius::NAV)
                .bg(c.sunken)
                .children(choices.map(|(id, name, description)| {
                    let on = picked == id;
                    let selector = format!("model-speed-{}", id.as_deref().unwrap_or("standard"));
                    let fast = id.is_some();
                    let tip: SharedString = description.unwrap_or_else(|| name.clone()).into();
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .h(px(26.))
                        .px(space::S2)
                        .rounded(radius::SMALL)
                        .cursor_pointer()
                        .when(on, |d| d.bg(c.selected))
                        .when(!on, |d| d.hover(|d| d.bg(c.hover)))
                        .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(tip.clone()).build(cx))
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_speed(id.clone(), cx)))
                        .when(fast, |d| d.child(Icon::new(IconName::Bolt).size(IconSize::XSmall).color(if on { c.text } else { c.text_muted })))
                        .child(div().text_ui(TypeSize::Small).text_color(if on { c.text } else { c.text_muted }).child(name))
                }))
                .into_any_element(),
        )
    }

    /// The conversation's mode, at the bottom's right: its icon and name, opening the permissions.
    fn permissions_button(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let permissions = self.permissions.as_ref()?;
        let c = cx.theme().colors.clone();
        let shown = &permissions.shown;
        let title = shown.title_of(&shown.approval).to_owned();
        let open = self.permissions_open;
        let holding = shown.holding().is_some();
        let fixed = shown.parent.is_some();
        let tip: SharedString = if fixed { "A subagent works with the permissions of the conversation that started it. Change them there.".into() } else { format!("Permissions: {}", shown.detail_of(&shown.approval)).into() };
        Some(
            div()
                .id("model-permissions")
                .debug_selector(|| "model-permissions".into())
                .relative()
                .flex()
                .items_center()
                .gap(space::S1_5)
                .h(px(30.))
                .px(space::S2)
                .rounded(radius::NAV)
                .when(open, |d| d.bg(c.hover))
                .when(!fixed, |d| d.cursor_pointer().hover(|d| d.bg(c.hover)).on_click(cx.listener(|this, _, _, cx| {
                    this.permissions_open = !this.permissions_open;
                    this.accounts_open = false;
                    cx.notify();
                })))
                .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(tip.clone()).build(cx))
                .child(Icon::new(approval_icon(&shown.approval)).size(IconSize::Nav).color(c.icon))
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(title))
                .when(!fixed, |d| d.child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronUp }).size(IconSize::XSmall).color(c.text_faint)))
                .children(holding.then(|| div().absolute().top(px(4.)).left(px(18.)).size(px(6.)).rounded_full().bg(c.warning)))
                .into_any_element(),
        )
    }

    /// The permissions' popover, over the picker's bottom right: the four modes in a row, and the
    /// one picked's name and what it does, as its box words them.
    fn permissions_popover(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.permissions_open {
            return None;
        }
        let shown = self.permissions.as_ref()?.shown.clone();
        let theme = cx.theme().clone();
        let c = theme.colors.clone();
        let modes: Vec<Approval> = if shown.options.is_empty() { Approval::ALL.to_vec() } else { shown.options.iter().map(|choice| choice.approval.clone()).collect() };
        let held = shown.holding().map(|why| why.why.clone());
        Some(
            div()
                .id("model-permissions-popover")
                .debug_selector(|| "model-permissions-popover".into())
                .absolute()
                .right(space::S2)
                .bottom(px(BAR + 4.))
                .w(px(300.))
                .flex()
                .flex_col()
                .gap(space::S2)
                .p(space::S3)
                .rounded(radius::POPOVER)
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .shadow(theme.elevated_shadow())
                .occlude()
                .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child("Permissions"))
                .child(div().flex().items_center().p(px(2.)).rounded(radius::NAV).bg(c.sunken).children(modes.into_iter().map(|approval| {
                    let on = approval == shown.approval;
                    let choice = shown.choice(&approval);
                    let refused = choice.and_then(|choice| choice.unavailable.as_ref()).filter(|why| why.reason != "fullAccessOff").map(|why| why.why.clone());
                    let title = shown.title_of(&approval).to_owned();
                    let tip: SharedString = match &refused {
                        Some(why) => format!("{title}: {why}").into(),
                        None => title.into(),
                    };
                    let selector = format!("model-permission-{}", approval.name());
                    let icon = approval_icon(&approval);
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector)
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .h(px(28.))
                        .rounded(radius::SMALL)
                        .when(on, |d| d.bg(c.selected))
                        .when(refused.is_none() && !on, |d| d.cursor_pointer().hover(|d| d.bg(c.hover)))
                        .when(refused.is_some(), |d| d.opacity(0.4))
                        .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(tip.clone()).build(cx))
                        .on_click(cx.listener(move |this, _, window, cx| this.pick_approval(approval.clone(), window, cx)))
                        .child(Icon::new(icon).size(IconSize::Nav).color(if on { c.text } else { c.icon }))
                })))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child({
                            let title = shown.title_of(&shown.approval).to_owned();
                            let selector = format!("model-permission-title-{title}");
                            div().debug_selector(move || selector).text_ui(TypeSize::Body).text_color(c.text).child(title)
                        })
                        .child({
                            let detail = shown.detail_of(&shown.approval).to_owned();
                            let selector = format!("model-permission-detail-{detail}");
                            div().debug_selector(move || selector).text_ui(TypeSize::Small).text_color(c.text_muted).child(detail)
                        })
                        .children(held.map(|why| div().text_ui(TypeSize::Small).text_color(c.warning).child(format!("Asks first for now. {why}")))),
                )
                .into_any_element(),
        )
    }

    /// What the list says where it has no rows.
    fn empty_words(&self) -> SharedString {
        if let Some(notice) = &self.notice {
            return notice.clone();
        }
        if !self.query.trim().is_empty() {
            return format!("No models match “{}”.", self.query.trim()).into();
        }
        match self.shown {
            Shown::Starred => "Add a model with ☆ to keep it here, whichever provider it’s from.".into(),
            Shown::Rail(_) => "No models to offer yet.".into(),
        }
    }
}

/// A mode's icon, wherever it's shown.
pub fn approval_icon(approval: &Approval) -> IconName {
    match approval {
        Approval::AskFirst => IconName::Hand,
        Approval::AcceptEdits => IconName::Pencil,
        Approval::FullAccess => IconName::Bolt,
        Approval::Auto | Approval::Other(_) => IconName::Shield,
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let c = &theme.colors;
        // (Tall enough for Reasoning's efforts too.)
        let rows_tall = if self.reasoning.is_some() { (self.height - 8.).max(6. * ROW) + 8. } else { self.height };
        let height = scaled(px(rows_tall));
        let rows = if self.notice.is_some() { 0 } else { self.rows.len() };
        let list = if rows == 0 {
            div()
                .debug_selector(|| "model-picker-empty".into())
                .h(height)
                .p(space::S4)
                .text_ui(TypeSize::Small)
                .text_color(c.text_muted)
                .child(self.empty_words())
                .into_any_element()
        } else {
            uniform_list("model-rows", rows, cx.processor(|this, range: Range<usize>, _, cx| this.rows_in(range, cx)))
                .track_scroll(&self.scroll)
                .w_full()
                .h(height)
                .p(space::S1)
                .into_any_element()
        };
        let searching = !self.query.trim().is_empty();
        let search = div()
            .flex()
            .items_center()
            .gap(space::S2)
            .h(px(44.))
            .mx(space::S3)
            .border_b_1()
            // (Blue while it's searching every provider's, as T3's.)
            .border_color(if searching { c.accent } else { c.border })
            .child(Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint))
            .child(div().flex_1().min_w_0().child(self.field.clone()));
        let middle = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(MIDDLE))
            .min_w_0()
            .child(search)
            .children(self.account_switch(cx))
            .child(div().relative().child(list).children(self.account_list(cx)))
            .children(self.problem.clone().map(|problem| div().px(space::S3).pb(space::S1).child(kuvo_ui::Notice::said("model-picker-problem", kuvo_ui::Tone::Warning, problem))))
            .children(self.note.clone().map(|note| div().px(space::S3).py(space::S2).border_t_1().border_color(c.border).text_ui(TypeSize::Caption).text_color(c.text_faint).child(note)));
        let reasoning = self.reasoning_column(cx);
        let width = RAIL + MIDDLE + if reasoning.is_some() { RIGHT } else { 0. };
        let speed = self.speed_switch(cx);
        let permissions = self.permissions_button(cx);
        let bar = (speed.is_some() || permissions.is_some()).then(|| {
            div()
                .debug_selector(|| "model-picker-bar".into())
                .flex()
                .items_center()
                .h(px(BAR))
                .px(space::S2)
                .border_t_1()
                .border_color(c.border)
                .children(speed)
                .child(div().flex_1())
                .children(permissions)
        });
        div()
            .id("model-picker")
            .debug_selector(|| "model-picker".into())
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Next, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &Previous, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &NextRail, _, cx| this.step_rail(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousRail, _, cx| this.step_rail(-1, cx)))
            .on_action(cx.listener(|this, _: &Pick, window, cx| this.pick(window, cx)))
            .on_action(cx.listener(|this, _: &Close, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &Row1, window, cx| this.pick_number(0, window, cx)))
            .on_action(cx.listener(|this, _: &Row2, window, cx| this.pick_number(1, window, cx)))
            .on_action(cx.listener(|this, _: &Row3, window, cx| this.pick_number(2, window, cx)))
            .on_action(cx.listener(|this, _: &Row4, window, cx| this.pick_number(3, window, cx)))
            .on_action(cx.listener(|this, _: &Row5, window, cx| this.pick_number(4, window, cx)))
            .on_action(cx.listener(|this, _: &Row6, window, cx| this.pick_number(5, window, cx)))
            .on_action(cx.listener(|this, _: &Row7, window, cx| this.pick_number(6, window, cx)))
            .on_action(cx.listener(|this, _: &Row8, window, cx| this.pick_number(7, window, cx)))
            .on_action(cx.listener(|this, _: &Row9, window, cx| this.pick_number(8, window, cx)))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .w(px(width))
            .rounded(radius::POPOVER)
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .shadow(theme.elevated_shadow())
            .overflow_hidden()
            .child(div().flex().child(self.rail(cx)).child(middle).children(reasoning))
            .children(bar)
            .children(self.permissions_popover(cx))
    }
}

/// What the model chip says: the provider's mark, the model's name and its effort, as T3's
/// ("✲ Opus 5.5 High"); a dot where the conversation does less than its permissions say for now.
#[derive(Clone, Debug, Default)]
pub struct ChipWords {
    pub provider: Option<String>,
    pub name: String,
    pub effort: Option<String>,
    pub holding: Option<SharedString>,
}

/// The model chip, opening `picker` (`ModelPicker`) above it: the composer's and a side chat's.
pub fn model_chip<V: 'static>(words: ChipWords, disabled: bool, handle: &kuvo_ui::menu::PopoverMenuHandle, picker: impl Fn(&mut V, &mut Window, &mut Context<V>) -> Entity<ModelPicker> + 'static, cx: &mut Context<V>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let mark = words.provider.as_deref().and_then(|provider| {
        let (_, path) = kuvo_ui::icon::provider_mark(provider)?;
        Some(kuvo_ui::icon::Mark::new(path).icon_size(IconSize::Nav).color(kuvo_ui::icon::provider_mark_color(provider).unwrap_or(c.icon)).into_any_element())
    });
    let tip: SharedString = match (&words.effort, &words.holding) {
        (_, Some(why)) => format!("{}. {why}", words.name).into(),
        (Some(effort), None) => format!("{} · {effort}", words.name).into(),
        (None, None) => words.name.clone().into(),
    };
    let trigger = div()
        .id("model-chip")
        .debug_selector(|| "model-chip".into())
        .relative()
        .flex()
        .items_center()
        .gap(space::S1_5)
        .min_w_0()
        .h(px(24.))
        .px(space::S2)
        .rounded(radius::NAV)
        .text_ui(TypeSize::Small)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| d.cursor_pointer().hover(|d| d.bg(c.hover)))
        .tooltip(move |_, cx| kuvo_ui::menu::Tooltip::new(tip.clone()).shortcut("⇧⌘M").build(cx))
        .children(mark.map(|mark| div().flex_none().child(mark)))
        // In a tight row the name gives up its room first, then the effort (#975).
        .child(div().debug_selector({
            let name = format!("model-chip-name-{}", words.name);
            move || name
        }).min_w_0().max_w(px(220.)).flex_shrink_1().truncate().text_color(c.text).child(words.name.clone()))
        .children(words.effort.clone().map(|effort| div().debug_selector({
            let selector = format!("model-chip-effort-{effort}");
            move || selector
        }).min_w_0().flex_shrink(0.001).truncate().text_color(c.text_muted).child(effort)))
        .children(words.holding.is_some().then(|| div().absolute().top(px(3.)).right(px(3.)).size(px(6.)).rounded_full().bg(c.warning)));
    if disabled {
        // (Held: it says the model, and opens nothing.)
        return div().flex().flex_shrink_1().min_w_0().child(trigger).into_any_element();
    }
    let this = cx.entity().downgrade();
    kuvo_ui::menu::PopoverMenu::view("model-menu", trigger, move |window, cx| this.update(cx, |this, cx| picker(this, window, cx)).expect("its view opens its picker")).with_handle(handle.clone()).above().centered().shrinks().into_any_element()
}

/// A setting's model, as a settings row draws a choice (Settings › Models, a bot's): what's chosen,
/// in a button found as `{key}-menu`, and the picker it opens (made only as it opens).
pub fn setting_button(key: &str, shown: String, open: impl Fn(&mut Window, &mut App) -> Entity<ModelPicker> + 'static, cx: &App) -> AnyElement {
    let shown = if shown.is_empty() { "—".to_owned() } else { shown };
    let trigger = setting_trigger(key, shown, cx);
    kuvo_ui::menu::PopoverMenu::view(SharedString::from(format!("{key}-popover")), trigger, open).align_right().into_any_element()
}

/// The shared trigger for a setting whose current value opens a picker.
pub(crate) fn setting_trigger(key: &str, shown: String, cx: &App) -> AnyElement {
    let c = cx.theme().colors.clone();
    let id = SharedString::from(format!("{key}-menu"));
    div()
        .id(id.clone())
        .debug_selector({
            let id = id.to_string();
            move || id
        })
        .flex()
        .items_center()
        .gap(space::S2)
        .min_w(px(160.))
        .max_w(px(280.))
        .h(px(32.))
        .px(space::S3)
        .rounded(radius::NAV)
        .border_1()
        .border_color(c.border_strong)
        .bg(c.card)
        .cursor_pointer()
        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Body).text_color(c.text).child(shown))
        .child(div().flex().flex_none().flex_col().child(Icon::new(IconName::ChevronUp).size(IconSize::Indicator).color(c.icon)).child(Icon::new(IconName::ChevronDown).size(IconSize::Indicator).color(c.icon)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Catalog {
        let offered: Offered = serde_json::from_value(json!({
            "models": [
                { "providerID": "anthropic", "id": "claude-opus-5-5", "name": "Claude Opus 5.5", "side": "engine", "efforts": ["low", "high"], "aliases": [] },
                { "providerID": "anthropic", "id": "claude-sonnet-5-5", "name": "Claude Sonnet 5.5", "side": "engine", "efforts": [], "aliases": [] },
                { "providerID": "openai", "id": "gpt-6", "name": "GPT-6", "side": "engine", "efforts": [], "aliases": [] },
                { "providerID": "opencode", "id": "big-pickle", "name": "Big Pickle", "side": "engine", "efforts": [], "aliases": [], "free": true },
                { "providerID": "opencode-go", "id": "beam", "name": "Beam", "side": "engine", "efforts": [], "aliases": [] },
                { "providerID": "openai", "id": "old", "name": "Old", "side": "engine", "efforts": [], "aliases": [], "retiring": true },
                { "providerID": "claude-code", "id": "opus", "name": "Opus", "side": "agents", "efforts": [], "aliases": ["default"] }
            ],
            "providers": [{ "id": "anthropic", "name": "Anthropic" }, { "id": "opencode", "name": "OpenCode Zen" }, { "id": "opencode-go", "name": "OpenCode Go" }, { "id": "claude-code", "name": "Claude Code", "label": "Work" }],
            "known": { "engine": true, "agents": true }
        }))
        .unwrap();
        Catalog::new(&offered, |_| true)
    }

    fn found(query: &str) -> Vec<String> {
        let catalog = catalog();
        matching(&catalog, query, &AtomicBool::new(false)).unwrap().into_iter().map(|ix| catalog.entries[ix].name.to_string()).collect()
    }

    #[test]
    fn each_model_says_where_its_from_and_providers_drawn_alike_share_the_rail() {
        let catalog = catalog();
        // (One its provider retires isn't offered.)
        assert_eq!(catalog.entries.iter().map(|entry| entry.line.to_string()).collect::<Vec<_>>(), ["Anthropic", "Anthropic", "openai", "OpenCode Zen · Free", "OpenCode Go", "Claude Code · Work"]);
        assert_eq!(catalog.rails.iter().map(|rail| rail.id.to_string()).collect::<Vec<_>>(), ["anthropic", "openai", "opencode", "claude"]);
        // A row says where it's from only where its mark doesn't: two providers under one mark.
        assert_eq!(catalog.entries.iter().map(|entry| entry.detail.as_deref().unwrap_or("").to_owned()).collect::<Vec<_>>(), ["", "", "", "OpenCode Zen · Free", "OpenCode Go", ""]);
        // A model named by another id it's known by is that one.
        assert_eq!(catalog.position(&ModelRef { provider_id: "claude-code".into(), id: "default".into(), variant: None }), Some(5));
    }

    #[test]
    fn a_search_finds_every_word_in_the_name_first_then_where_its_from() {
        // A name that starts with it first, then one with a word that does.
        assert_eq!(found("opus"), ["Opus", "Claude Opus 5.5"]);
        // The letters in order: "cs" is Claude Sonnet.
        assert_eq!(found("cl son"), ["Claude Sonnet 5.5"]);
        // By its provider: every one of Anthropic's, in the box's order.
        assert_eq!(found("anthropic"), ["Claude Opus 5.5", "Claude Sonnet 5.5"]);
        assert_eq!(found("free"), ["Big Pickle"]);
        assert!(found("nothing like it").is_empty());
        // A name's start comes before a match inside one.
        assert_eq!(found("o")[0], "Opus");
    }

    #[test]
    fn a_newer_search_stops_the_one_before() {
        let cancelled = AtomicBool::new(true);
        assert_eq!(matching(&catalog(), "claude", &cancelled), None);
    }
}
