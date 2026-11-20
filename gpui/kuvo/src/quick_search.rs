//! Search from the compact Bots sidebar without changing the sidebar's width or focus history.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, MouseButton, ParentElement, ScrollHandle,
    StatefulInteractiveElement as _, Styled, Subscription, Task, Window, deferred, div, px,
    relative,
};
use kuvo_box::search::{Found, SEARCHED_FROM};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, StyledType, TextField, TextFieldEvent, TextRole,
    icon::Mark,
    row::NavRow,
    theme::{radius, space},
};

use super::Workspace;
use crate::{area::Area, areas::Reading, sidebar};

const PROMPT: &str = "Search bots and sessions";
const LEADING: gpui::Pixels = px(20.);
// Results have the list's padding plus NavRow's border and padding.
const SEARCH_INSET: gpui::Pixels = px(17.);

fn leading_slot(selector: String, icon: impl IntoElement) -> gpui::Div {
    div()
        .debug_selector(move || selector)
        .size(LEADING)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .child(icon)
}

pub(super) struct QuickSearch {
    field: Entity<TextField>,
    before: Option<FocusHandle>,
    query: String,
    finds: Vec<Found>,
    finding: bool,
    failed: bool,
    selected: usize,
    scroll: ScrollHandle,
    task: Option<Task<()>>,
    provider_requests: Vec<String>,
    providers: HashMap<String, String>,
    reading_providers: Option<Task<()>>,
    _input: Subscription,
}

#[derive(Clone)]
enum Target {
    Bot(String),
    Session(String),
}

struct Match {
    label: String,
    snippet: Option<String>,
    bot: Option<String>,
    provider: Option<String>,
    target: Target,
}

impl Workspace {
    pub fn open_quick_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TextField>> {
        if !self.compact_sidebar_visible(window.viewport_size().width, cx) {
            return None;
        }
        if let Some(search) = &self.quick_search {
            window.focus(&search.field.focus_handle(cx), cx);
            return Some(search.field.clone());
        }
        let field = cx.new(|cx| TextField::new(cx).bare().placeholder(PROMPT));
        let before = window.focused(cx);
        let input = cx.subscribe(&field, |this, _, event: &TextFieldEvent, cx| {
            if matches!(event, TextFieldEvent::Changed) {
                this.quick_search_changed(cx);
            }
        });
        window.focus(&field.focus_handle(cx), cx);
        self.quick_search = Some(QuickSearch {
            field: field.clone(),
            before,
            query: String::new(),
            finds: Vec::new(),
            finding: false,
            failed: false,
            selected: 0,
            scroll: ScrollHandle::new(),
            task: None,
            provider_requests: Vec::new(),
            providers: HashMap::new(),
            reading_providers: None,
            _input: input,
        });
        self.refresh_quick_search_providers(cx);
        cx.notify();
        Some(field)
    }

    pub(super) fn dismiss_hidden_quick_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Dropping the popup also cancels its outstanding search and subscription.
        if self.quick_search.take().is_none() {
            return;
        }
        let focus = if let Some(run) = self.first_run() {
            run.focus_handle(cx)
        } else if self.awaits_box() {
            self.focus.clone()
        } else {
            self.sidebar_toggle_focus().clone()
        };
        window.focus(&focus, cx);
        cx.notify();
    }

    fn close_quick_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = self.quick_search.take() {
            window.focus(search.before.as_ref().unwrap_or(&self.focus), cx);
            cx.notify();
        }
    }

    fn quick_search_changed(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.quick_search else {
            return;
        };
        let query = search.field.read(cx).text().trim().to_owned();
        if query == search.query {
            return;
        }
        search.query = query.clone();
        search.finds.clear();
        search.failed = false;
        search.selected = 0;
        search.scroll.scroll_to_item(0);
        search.task = None;
        search.finding = query.chars().count() >= SEARCHED_FROM;
        if search.finding {
            let task = self.search_conversations(query, Self::quick_search_found, cx);
            if let Some(search) = &mut self.quick_search {
                search.task = task;
            }
        }
        self.refresh_quick_search_providers(cx);
        cx.notify();
    }

    fn quick_search_found(
        &mut self,
        query: &str,
        found: kuvo_box::Result<Vec<Found>>,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = &mut self.quick_search else {
            return;
        };
        if search.query == query {
            search.finding = false;
            search.failed = found.is_err();
            search.finds = found.unwrap_or_default();
            self.refresh_quick_search_providers(cx);
            cx.notify();
        }
    }

    fn refresh_quick_search_providers(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &self.quick_search else {
            return;
        };
        let mut ids: Vec<String> = self
            .quick_matches(search, cx)
            .into_iter()
            .filter_map(|item| match item.target {
                Target::Session(id) if item.bot.is_none() => Some(id),
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        let Some(search) = &mut self.quick_search else {
            return;
        };
        if search.provider_requests == ids {
            return;
        }
        search.provider_requests = ids.clone();
        search.reading_providers = None;
        search.providers.retain(|id, _| ids.binary_search(id).is_ok());
        if ids.is_empty() {
            return;
        }
        let client = self.client.clone();
        let expected = ids.clone();
        let read = move || {
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            client.session_models(&refs).ok().map(|states| {
                states
                    .into_iter()
                    .filter_map(|(id, state)| {
                        let provider = crate::work_bars::provider_from_model_state(&state)?;
                        Some((id, provider))
                    })
                    .collect::<HashMap<String, String>>()
            })
        };
        if !self.live {
            if let Some(providers) = read() {
                search.providers = providers;
            }
            return;
        }
        let task = cx.background_executor().spawn(async move { read() });
        search.reading_providers = Some(cx.spawn(async move |this, cx| {
            let providers = task.await;
            this.update(cx, |this, cx| {
                let Some(search) = &mut this.quick_search else {
                    return;
                };
                if search.provider_requests == expected
                    && let Some(providers) = providers
                {
                    search.providers = providers;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn quick_matches(&self, search: &QuickSearch, cx: &App) -> Vec<Match> {
        let mut results = Vec::new();
        let bots = self.area_view(Area::Bots).and_then(|view| match view.read(cx).bots() {
            Reading::Ready(bots) => Some(bots),
            _ => None,
        });
        if let Some(bots) = bots {
            let needle = search.query.to_lowercase();
            let local = bots
                .bots
                .iter()
                .map(|bot| ("bot", bot.id.clone(), bot.name.clone()));
            let others = bots
                .there()
                .into_iter()
                .map(|bot| ("elsewhere", bot.id, bot.name));
            results.extend(local.chain(others).filter_map(|(kind, id, name)| {
                name.to_lowercase().contains(&needle).then_some(Match {
                    label: name,
                    snippet: None,
                    target: Target::Bot(format!("{kind}:{id}")),
                    bot: Some(id),
                    provider: None,
                })
            }));
        }
        let listing = self.listing();
        let (named, found) =
            sidebar::matched_sessions(listing, &search.query, false, &search.finds);
        results.extend(
            named
                .into_iter()
                .map(|id| (id, None))
                .chain(
                    found
                        .into_iter()
                        .map(|found| (found.session.clone(), Some(found.snippet.clone()))),
                )
                .map(|(id, snippet)| {
                    let facts = listing.facts(&id);
                    let label = facts
                        .and_then(|facts| facts.title.clone())
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or_else(|| "New Session".into());
                    // Only a bot's own conversation carries its face; sessions a bot started show
                    // their own agent/provider icon instead.
                    let own_bot = bots
                        .and_then(|bots| bots.bots.iter().find(|bot| bot.session == id).map(|bot| bot.id.clone()))
                        .or_else(|| {
                            facts
                                .filter(|facts| facts.kind.0 == "bot")
                                .and_then(|facts| facts.bot.clone())
                        });
                    let provider = own_bot
                        .is_none()
                        .then(|| search.providers.get(&id).cloned())
                        .flatten();
                    Match {
                        label,
                        snippet,
                        bot: own_bot,
                        provider,
                        target: Target::Session(id),
                    }
                }),
        );
        results
    }

    fn move_quick_search(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(search) = &self.quick_search else {
            return;
        };
        let count = self.quick_matches(search, cx).len();
        if count == 0 {
            return;
        }
        let search = self.quick_search.as_mut().unwrap();
        search.selected = (search.selected as isize + delta).rem_euclid(count as isize) as usize;
        search.scroll.scroll_to_item(search.selected);
        cx.notify();
    }

    fn open_quick_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = &self.quick_search else {
            return;
        };
        let results = self.quick_matches(search, cx);
        let selected = search.selected.min(results.len().saturating_sub(1));
        let Some(result) = results.into_iter().nth(selected) else {
            return;
        };
        self.open_quick_target(result.target, window, cx);
    }

    fn open_quick_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if !self.compact_sidebar_visible(window.viewport_size().width, cx) {
            self.dismiss_hidden_quick_search(window, cx);
            return;
        }
        self.close_quick_search(window, cx);
        match target {
            Target::Bot(key) => {
                if let Some(view) = self.area_view(Area::Bots).cloned() {
                    view.update(cx, |view, cx| view.pick(key, cx));
                }
            }
            Target::Session(id) => {
                self.session_with_bot_strip = true;
                self.open_session_now(&id, window, cx);
            }
        }
    }

    pub(super) fn render_quick_search(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.quick_search.as_ref()?;
        let theme = cx.theme().clone();
        let c = &theme.colors;
        let results = self.quick_matches(search, cx);
        let empty = results.is_empty();
        let selected = search.selected.min(results.len().saturating_sub(1));
        let bots = self.area_view(Area::Bots);
        let rows = results.into_iter().enumerate().map(|(index, result)| {
            let (kind, key, icon) = match &result.target {
                Target::Bot(key) => ("Bot", key, IconName::Bot),
                Target::Session(id) => ("Session", id, IconName::Message),
            };
            let selector = format!("sidebar-search-result-{key}");
            let leading = result.bot.as_deref().zip(bots);
            let leading = if let Some((bot, view)) = leading {
                let avatar = format!("{selector}-avatar");
                div()
                    .debug_selector(move || avatar)
                    .child(view.read(cx).still_bot_avatar(bot, LEADING))
                    .into_any_element()
            } else if let Some((service, path, color)) = result
                .provider
                .as_deref()
                .and_then(crate::work_bars::session_mark)
            {
                let mark = format!("{selector}-provider-{service}");
                div()
                    .debug_selector(move || mark)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(LEADING)
                    .rounded(radius::PILL)
                    .border_1()
                    .border_color(c.border)
                    .bg(c.sunken)
                    .child(Mark::new(path).size(px(11.)).color(color.unwrap_or(c.text)))
                    .into_any_element()
            } else {
                Icon::new(icon)
                    .size(kuvo_ui::IconSize::Nav)
                    .into_any_element()
            };
            let row = NavRow::new(selector.clone(), result.label)
                .leading(leading_slot(format!("{selector}-leading"), leading))
                .trailing(
                    div()
                        .text_role(TextRole::Meta)
                        .text_color(c.text_faint)
                        .child(kind),
                )
                .selected(index == selected)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_quick_target(result.target.clone(), window, cx)
                }));
            div()
                .w_full()
                .min_w_0()
                .debug_selector(move || selector)
                .child(row)
                .children(result.snippet.map(|snippet| {
                    div()
                        .min_w_0()
                        .pl(px(1.) + space::S2 + LEADING + space::S2)
                        .pr(space::S2)
                        .pb(space::S1)
                        .truncate()
                        .text_role(TextRole::RowDetail)
                        .text_color(c.text_muted)
                        .child(snippet.trim().to_owned())
                }))
        });
        let message = match (search.finding, search.failed, empty) {
            (true, _, _) => Some("Searching conversations…"),
            (_, true, _) => Some("Conversations couldn’t be searched."),
            (_, _, true) => Some("No matches."),
            _ => None,
        };
        let popup =
            div()
                .id("sidebar-quick-search")
                .debug_selector(|| "sidebar-quick-search".into())
                .key_context(sidebar::QUICK_SEARCH)
                .w(px(460.))
                .max_w(relative(0.9))
                .max_h(relative(0.7))
                .flex()
                .flex_col()
                .bg(c.elevated)
                .border_1()
                .border_color(c.border)
                .rounded(radius::POPOVER)
                .shadow(theme.elevated_shadow())
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_action(
                    cx.listener(|this, _: &sidebar::WalkNext, _, cx| this.move_quick_search(1, cx)),
                )
                .on_action(cx.listener(|this, _: &sidebar::WalkPrevious, _, cx| {
                    this.move_quick_search(-1, cx)
                }))
                .on_action(cx.listener(|this, _: &sidebar::WalkOpen, window, cx| {
                    this.open_quick_match(window, cx)
                }))
                .on_action(cx.listener(|this, _: &sidebar::WalkLeave, window, cx| {
                    this.close_quick_search(window, cx)
                }))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(space::S2)
                        .px(SEARCH_INSET)
                        .h(px(44.))
                        .border_b_1()
                        .border_color(c.border)
                        .child(leading_slot(
                            "sidebar-search-leading".into(),
                            Icon::new(IconName::Search),
                        ))
                        .child(
                            div()
                                .debug_selector(|| "sidebar-search-input".into())
                                .flex_1()
                                .min_w_0()
                                .child(search.field.clone()),
                        ),
                )
                .child(
                    div()
                        .id("sidebar-search-results")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&search.scroll)
                        .p(space::S2)
                        .children(rows)
                        .children(message.map(|words| {
                            div()
                                .debug_selector(move || format!("sidebar-search-{words}"))
                                .p(space::S2)
                                .text_color(c.text_muted)
                                .child(words)
                        })),
                );
        Some(
            deferred(
                div()
                    .id("sidebar-search-overlay")
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(78.))
                    .occlude()
                    .bg(crate::bots::dim(cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_quick_search(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(popup),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
