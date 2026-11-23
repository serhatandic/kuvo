//! The Sessions area's sidebar (Draft B): the search, then what's
//! waiting on you across projects ("Needs you"), then the box's sessions: each project with its
//! sessions nested under it (pinned raised to the top once and marked with a pin, then newest
//! first), and chats (pinned raised to top). Under the pointer (or while a row's menu/popover is
//! open), separated Pin and Archive buttons appear on the row (Unarchive and Delete… when viewing
//! archived); right-click opens the full box-driven menu, and Rename / Delete confirm anchor as
//! popovers beneath the row.


use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::search::SEARCHED_FROM;
use kuvo_box::session_actions::{Action, Actions};
use kuvo_box::sessions::{Facts, Listing};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    menu::ContextMenuArea,
    row::{NAV_ROW_HEIGHT, NavRow, SectionHeader},
    small::{Chip, count_badge, spinner, status_dot},
    theme::{radius, space},
};

use crate::session_menu::{PopoverAnchor, row_anchored_popover};
use crate::time;
use crate::workspace::{Lists, Workspace};

actions!(sidebar, [WalkNext, WalkPrevious, WalkOpen, WalkLeave, FocusSearch]);

/// Default width of an area's sidebar.
pub const WIDTH: f32 = 272.;
pub const COMPACT_WIDTH: f32 = 64.;
const MAX_WIDTH: f32 = 440.;
const BOTS_COMPACT_BELOW: f32 = 216.;

pub(crate) fn preferred_width(saved: Option<f32>) -> Pixels {
    px(saved.filter(|width| width.is_finite()).unwrap_or(WIDTH).clamp(COMPACT_WIDTH, MAX_WIDTH))
}

pub(crate) fn fit_width(preferred: Pixels, viewport: Pixels, bots: bool) -> Pixels {
    let min = px(if bots { COMPACT_WIDTH } else { BOTS_COMPACT_BELOW });
    let room = viewport - crate::rail::WIDTH - px(1.) - crate::panel::CONVERSATION_NARROWEST;
    preferred.clamp(min, room.clamp(min, px(MAX_WIDTH)))
}

pub(crate) fn compact_bots(width: Pixels) -> bool {
    width < px(BOTS_COMPACT_BELOW)
}

pub(crate) fn visible_width(width: Pixels) -> Pixels {
    if compact_bots(width) { px(COMPACT_WIDTH) } else { width }
}

/// How many of a project's sessions show before "Show more".
pub const SHOWN: usize = 6;
/// The key context of the list while the keyboard walks it, and of the search around its field.
pub const CONTEXT: &str = "Sidebar";
pub const SEARCH: &str = "SidebarSearch";
pub(crate) const QUICK_SEARCH: &str = "SidebarQuickSearch";

/// The keys the sidebar takes: the list's while it has the keyboard, and the search field's arrows,
/// Return and Escape ahead of the field's own.
pub fn bind_keys(cx: &mut App) {
    let list = Some(CONTEXT);
    let search = format!("{SEARCH} > {}", kuvo_ui::text_field::CONTEXT);
    let search = Some(search.as_str());
    let quick = format!("{QUICK_SEARCH} > {}", kuvo_ui::text_field::CONTEXT);
    let quick = Some(quick.as_str());
    cx.bind_keys([
        KeyBinding::new("down", WalkNext, list),
        KeyBinding::new("up", WalkPrevious, list),
        KeyBinding::new("enter", WalkOpen, list),
        KeyBinding::new("escape", WalkLeave, list),
        KeyBinding::new("down", WalkNext, search),
        KeyBinding::new("up", WalkPrevious, search),
        KeyBinding::new("enter", WalkOpen, search),
        KeyBinding::new("escape", WalkLeave, search),
        KeyBinding::new("down", WalkNext, quick),
        KeyBinding::new("up", WalkPrevious, quick),
        KeyBinding::new("enter", WalkOpen, quick),
        KeyBinding::new("escape", WalkLeave, quick),
        KeyBinding::new("cmd-shift-f", FocusSearch, Some("Workspace")),
    ]);
}

/// One line of the sidebar's list, in the order it shows: what the keyboard walks, and what's
/// between (headers, quiet words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Header(&'static str),
    /// Quiet words: nothing matches, why a search couldn't be made.
    Quiet(String),
    /// A project with no sessions yet, under it.
    Empty(String),
    Project(String),
    /// A project header in the archived filter list (folder + name, non-collapsible).
    ArchivedProject { directory: String, title: String },
    /// A session; `section` tells apart the section where it shows (`project`, `chat`, `found`,
    /// `archived-project`, `archived-chat`), `nested`: under its project.
    Session { id: String, section: &'static str, nested: bool },
    /// "Show N more" (or, from a box before it paged lists, fewer) under a project.
    More(String),
    /// "Show fewer" under a project's list "Show more" opened.
    Fewer(String),
    /// Why a project's next sessions couldn't be shown, under it.
    PageFailed { directory: String, why: String },
    /// A conversation whose words have the search, with the words around them.
    Found { id: String, snippet: String },
    /// The box is searching conversations.
    Searching,
    /// A session waiting on you, on top, with what it asks.
    Needs { id: String, asks: String },
}

impl Line {
    /// Whether the keyboard stops on it.
    pub fn walkable(&self) -> bool {
        matches!(self, Line::Project(_) | Line::Session { .. } | Line::More(_) | Line::Fewer(_) | Line::Found { .. } | Line::Needs { .. })
    }

    /// What it's known by while the list changes under it.
    pub fn key(&self) -> String {
        match self {
            Line::Project(directory) => format!("project:{directory}"),
            Line::ArchivedProject { directory, .. } => format!("archived-project:{directory}"),
            Line::Session { id, section, .. } => format!("session-{section}:{id}"),
            Line::More(directory) => format!("more:{directory}"),
            Line::Fewer(directory) => format!("fewer:{directory}"),
            Line::PageFailed { directory, .. } => format!("page-failed:{directory}"),
            Line::Found { id, .. } => format!("found:{id}"),
            Line::Header(title) => format!("header:{title}"),
            Line::Quiet(words) => format!("quiet:{words}"),
            Line::Empty(directory) => format!("empty:{directory}"),
            Line::Searching => "searching".into(),
            Line::Needs { id, .. } => format!("needs:{id}"),
        }
    }
}

/// Reorders `ids` so pinned sessions come first (keeping their relative newest-first order).
fn raise_pinned(listing: &Listing, ids: &[String]) -> Vec<String> {
    let is_pinned = |id: &String| listing.facts(id).is_some_and(Facts::is_pinned);
    ids.iter().filter(|id| is_pinned(id)).chain(ids.iter().filter(|id| !is_pinned(id))).cloned().collect()
}

/// The list's lines as it shows them now: active sessions, archived sessions, or what a search finds.
pub fn lines(workspace: &Workspace, cx: &App) -> Vec<Line> {
    let query = workspace.search_text(cx);
    if !query.trim().is_empty() {
        return found_lines(workspace, query.trim(), workspace.archived_filter());
    }
    if workspace.archived_filter() {
        return archived_lines(workspace);
    }
    let listing = workspace.listing();
    let sidebar = listing.sidebar();
    let mut lines = Vec::new();
    // The sessions waiting on you across projects, as the box's list says them (`needsYou`): its
    // order, and what each asks in its words.
    if !listing.needs_you().is_empty() {
        lines.push(Line::Header("Needs you"));
        lines.extend(listing.needs_you().iter().map(|needs| Line::Needs { id: needs.session.clone(), asks: needs.asks.summary.clone() }));
    }
    // Draft B: a pinned session keeps its place once inside its project (or Chats) and is raised to its top.
    // (Its heading shows with none too: its + adds the first.)
    lines.push(Line::Header("Projects"));
    for group in &sidebar.projects {
        lines.push(Line::Project(group.directory.clone()));
        if workspace.is_collapsed(&group.directory) {
            continue;
        }
        if group.sessions.is_empty() {
            lines.push(Line::Empty(group.directory.clone()));
            continue;
        }
        // Its list as its box orders and pages it (pinned, then at work or waiting on you, then
        // newest; "Show more" adding the box's next page); a box before that: newest first, six,
        // then all.
        if let Some(shown) = workspace.project_list(&group.directory) {
            for id in shown {
                lines.push(Line::Session { id: id.clone(), section: "project", nested: true });
                lines.extend(subagent_lines(workspace, id));
            }
            if workspace.more_of(&group.directory).is_some() {
                lines.push(Line::More(group.directory.clone()));
            }
            if workspace.shows_more(&group.directory) {
                lines.push(Line::Fewer(group.directory.clone()));
            }
            if let (_, Some(why)) = workspace.paging(&group.directory) {
                lines.push(Line::PageFailed { directory: group.directory.clone(), why: why.to_owned() });
            }
            continue;
        }
        let ordered = raise_pinned(listing, &group.sessions);
        let shown = if workspace.shows_all(&group.directory) { ordered.len() } else { ordered.len().min(SHOWN) };
        for id in &ordered[..shown] {
            lines.push(Line::Session { id: id.clone(), section: "project", nested: true });
            lines.extend(subagent_lines(workspace, id));
        }
        if ordered.len() > SHOWN {
            lines.push(Line::More(group.directory.clone()));
        }
    }
    if !sidebar.chats.is_empty() {
        lines.push(Line::Header("Chats"));
        let ordered = raise_pinned(listing, &sidebar.chats);
        lines.extend(ordered.into_iter().map(|id| Line::Session { id, section: "chat", nested: false }));
    }
    lines
}

/// A session's subagents, under it: while it or one of them is open, each of them; else those at
/// work or waiting on you. In its box's order.
fn subagent_lines(workspace: &Workspace, id: &str) -> Vec<Line> {
    let listing = workspace.listing();
    let Some(subagents) = listing.facts(id).and_then(|facts| facts.subagents.as_ref()) else { return Vec::new() };
    let opened = workspace.opened();
    let open = opened == Some(id) || opened.is_some_and(|open| subagents.iter().any(|sub| sub == open));
    subagents
        .iter()
        .filter(|sub| open || listing.facts(sub).is_some_and(Facts::is_ongoing))
        .map(|sub| Line::Session { id: sub.clone(), section: "subagent", nested: true })
        .collect()
}

/// Archived sessions grouped by project (folder header + nested rows) and Chats, newest first.
fn archived_lines(workspace: &Workspace) -> Vec<Line> {
    let mut lines = Vec::new();
    for group in workspace.listing().archived_groups() {
        if group.kind == "chat" {
            lines.push(Line::Header("Chats"));
        } else {
            lines.push(Line::ArchivedProject { directory: group.directory.clone(), title: group.title.clone() });
        }
        lines.extend(group.sessions.iter().map(|id| Line::Session {
            id: id.clone(),
            section: if group.kind == "chat" { "archived-chat" } else { "archived-project" },
            nested: group.kind != "chat",
        }));
    }
    lines
}

/// What a search finds: sessions by their names, then (from three characters) the conversations
/// the box found the words in, less those found by name. When `archived_only` is on, only
/// archived sessions show.
fn found_lines(workspace: &Workspace, query: &str, archived_only: bool) -> Vec<Line> {
    let (named, matches) = matched_sessions(workspace.listing(), query, archived_only, workspace.finds());
    let searched = query.chars().count() >= SEARCHED_FROM;
    let in_conversations: Vec<Line> = matches.into_iter()
        .map(|found| Line::Found { id: found.session.clone(), snippet: found.snippet.clone() })
        .collect();
    let mut lines = Vec::new();
    if !named.is_empty() || in_conversations.is_empty() {
        lines.push(Line::Header("Results"));
    }
    if named.is_empty() && in_conversations.is_empty() && !workspace.is_finding() {
        lines.push(Line::Quiet("Nothing matches.".into()));
    }
    lines.extend(named.into_iter().map(|id| Line::Session { id, section: "found", nested: false }));
    let failed = workspace.find_failed();
    if searched && (workspace.is_finding() || !in_conversations.is_empty() || failed.is_some()) {
        lines.push(Line::Header("In Conversations"));
        if workspace.is_finding() {
            lines.push(Line::Searching);
        }
        lines.extend(in_conversations);
        if let Some(why) = failed {
            lines.push(Line::Quiet(format!("Conversations couldn’t be searched: {why}")));
        }
    }
    lines
}

/// The same title, project and conversation matches for both sidebar searches.
pub(crate) fn matched_sessions<'a>(
    listing: &Listing,
    query: &str,
    archived_only: bool,
    finds: &'a [kuvo_box::search::Found],
) -> (Vec<String>, Vec<&'a kuvo_box::search::Found>) {
    let in_scope = |id: &str| listing.facts(id).is_some_and(|facts| facts.is_archived() == archived_only);
    let named: Vec<_> = listing.named(query).into_iter().filter(|id| in_scope(id)).collect();
    let conversations = finds.iter()
        .filter(|found| !named.contains(&found.session) && in_scope(&found.session))
        .collect();
    (named, conversations)
}

pub fn render(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    if workspace.client().gate().is_held() {
        return div().debug_selector(|| "held-sessions".into()).size_full().into_any_element();
    }
    let lists: Vec<AnyElement> = match workspace.lists() {
        Lists::Loading => vec![quiet(Some("Loading sessions…"), true, cx)],
        Lists::Waiting(why) => vec![quiet(Some(&format!("Loading sessions… {why}")), true, cx)],
        Lists::Held => Vec::new(),
        Lists::Failed(why) if !workspace.listing().is_read() => vec![quiet(Some(why), false, cx)],
        _ => sessions(workspace, window, cx),
    };
    let c = cx.theme().colors.clone();
    let archived = workspace.archived_filter();
    let action_error = workspace.session_menu().action_error.clone();
    let workspace_handle = cx.entity();

    div()
        .id("sidebar")
        .on_action(cx.listener(|this, _: &WalkNext, window, cx| this.walk(1, window, cx)))
        .on_action(cx.listener(|this, _: &WalkPrevious, window, cx| this.walk(-1, window, cx)))
        .on_action(cx.listener(|this, _: &WalkOpen, window, cx| this.open_walked(window, cx)))
        .on_action(cx.listener(|this, _: &WalkLeave, window, cx| this.leave_walk(window, cx)))
        .flex()
        .flex_col()
        .size_full()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .px(space::S2)
                .pt(space::S2)
                .pb(space::S2)
                .child(search_field(workspace, cx))
                .when(archived, |d| {
                    let handle = workspace_handle.clone();
                    d.child(
                        div().flex().items_center().child(
                            Chip::new("showing-archived", "Archived")
                                .selected(true)
                                .on_remove(move |window, cx| {
                                    handle.update(cx, |this, cx| {
                                        this.set_archived_filter(false, window, cx);
                                    });
                                }),
                        ),
                    )
                })
                .when_some(action_error, |d, error| {
                    // What you asked of a session that didn't happen: it stays until you've seen
                    // it, so its one action is Dismiss.
                    d.child(
                        div().px(space::S2).child(
                            kuvo_ui::Notice::said("session-action-error", kuvo_ui::Tone::Error, error)
                                .trailing(Button::new("dismiss-session-error", "Dismiss").ghost().size(ButtonSize::Small).on_click(cx.listener(|this, _, _, cx| this.clear_action_error(cx)))),
                        ),
                    )
                }),
        )
        .child(
            div()
                .id("sidebar-lists")
                .key_context(CONTEXT)
                .track_focus(workspace.list_focus())
                .track_scroll(workspace.list_scroll())
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .px(space::S2)
                .pb(space::S4)
                .children(lists),
        )
        .into_any_element()
}

/// The search: a sunken field with its glass, and × to clear while it has words. (Archived
/// sessions are listed and restored in Settings › Archived Sessions.)
fn search_field(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let has_words = !workspace.search_text(cx).is_empty();
    div()
        .key_context(SEARCH)
        .debug_selector(|| "sidebar-search".into())
        .flex()
        .items_center()
        .gap(space::S1_5)
        .h(px(30.))
        .pl(space::S2)
        .pr(space::S1)
        .rounded(radius::NAV)
        .bg(c.sunken)
        .child(Icon::new(IconName::Search).size(IconSize::Nav).color(c.text_faint))
        .child(div().flex_1().min_w_0().child(workspace.search().clone()))
        .when(has_words, |d| {
            d.child(
                Button::icon_only("clear-search", IconName::Close, "Clear the search")
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, window, cx| this.clear_search(window, cx))),
            )
        })
        .into_any_element()
}

/// The list's rows, one for each of its lines, so the keyboard can bring any into view.
fn sessions(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let lines = lines(workspace, cx);
    if lines.is_empty() {
        let message = if workspace.archived_filter() { "No archived sessions." } else { "No sessions yet." };
        return vec![quiet(Some(message), false, cx)];
    }
    if lines == [Line::Header("Projects")] {
        // (Nothing yet but the Projects heading, with its +.)
        return vec![line_row(workspace, &lines[0], false, cx), quiet(Some("No sessions yet."), false, cx)];
    }
    let cursor = workspace.walked().map(str::to_owned);
    lines
        .iter()
        .map(|line| line_row(workspace, line, cursor.as_deref() == Some(line.key().as_str()), cx))
        .collect()
}

/// What "Show more" under a project says: the whole list where its next page is the rest of it
/// ("Show all 16"), otherwise the next page and how many there are in all ("Show 10 more of 44").
fn more_words(shown: u64, count: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if shown + count >= total => format!("Show all {total}"),
        Some(total) => format!("Show {count} more of {total}"),
        None => format!("Show {count} more"),
    }
}

fn line_row(workspace: &mut Workspace, line: &Line, cursor: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let row: AnyElement = match line {
        Line::Header("Needs you") => {
            let count = workspace.listing().needs_you().len();
            div()
                .debug_selector(|| "header-Needs you".into())
                .flex()
                .items_center()
                .gap(space::S1_5)
                .h(px(28.))
                .pl(space::S2)
                .pt(space::S2)
                .text_ui(TypeSize::Caption)
                .child(div().text_color(c.text_muted).medium().child("Needs you"))
                .child(div().text_color(c.text_faint).child(count.to_string()))
                .into_any_element()
        }
        Line::Header("Projects") => {
            // Add Project: this Mac's folders, through its own picker; another computer's can't be
            // picked here yet.
            let here = workspace.can_add_project();
            let add = Button::icon_only("add-project", IconName::Plus, "Add Project")
                .ghost()
                .size(ButtonSize::Small)
                .icon_size(IconSize::Medium)
                .tooltip(if here { "Add Project" } else { "Projects are added on the computer they’re on" })
                .disabled(!here)
                .on_click(cx.listener(|this, _, window, cx| this.add_project(window, cx)));
            div()
                .debug_selector(|| "header-Projects".into())
                .child(SectionHeader::new("header-Projects", "Projects").action(div().debug_selector(|| "add-project".into()).child(add)))
                .into_any_element()
        }
        Line::Header(title) => {
            let selector = format!("header-{title}");
            div()
                .debug_selector(move || selector)
                .child(SectionHeader::new(SharedString::from(format!("header-{title}")), *title))
                .into_any_element()
        }
        Line::Quiet(words) => div()
            .px(space::S2)
            .py(space::S1_5)
            .text_ui(TypeSize::Small)
            .text_color(c.text_muted)
            .child(words.clone())
            .into_any_element(),
        Line::Searching => kuvo_ui::small::LoadingLabel::new("session-search", "Searching conversations…").compact().into_any_element(),
        Line::Empty(_) => div()
            .h(px(26.))
            .flex()
            .items_center()
            .pl(space::S2)
            .text_ui(TypeSize::Caption)
            .text_color(c.text_faint)
            .child("No sessions yet")
            .into_any_element(),
        Line::ArchivedProject { title, .. } => {
            let name = title.clone();
            let selector = format!("archived-project-{name}");
            div()
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(space::S2)
                .h(NAV_ROW_HEIGHT)
                .px(space::S2)
                .text_role(TextRole::RowLabel)
                .text_color(c.text)
                .child(Icon::new(IconName::Folder).size(IconSize::Nav).color(c.icon))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .into_any_element()
        }
        Line::Project(directory) => {
            let listing = workspace.listing();
            let sidebar = listing.sidebar();
            let Some(group) = sidebar.projects.iter().find(|group| &group.directory == directory) else {
                return div().into_any_element();
            };
            let collapsed = workspace.is_collapsed(directory);
            // The project's sums are the box's (`projects[<dir>].{asking, working}`): how many of
            // its sessions wait on you, and how many are at work.
            let (asking, working) = listing.project(directory).map_or((0, 0), |project| (project.asking, project.working));
            let name = group.name.clone();
            // (Named with its computer only where another project has the same name.)
            let label = match &group.computer {
                Some(computer) => format!("{} · {computer}", group.name),
                None => group.name.clone(),
            };
            let elsewhere = group.elsewhere;
            let trailing: Option<AnyElement> = if asking > 0 {
                Some(div().debug_selector(move || format!("project-asking-{name}")).child(count_badge(asking, cx)).into_any_element())
            } else if collapsed && working > 0 {
                Some(
                    div()
                        .debug_selector(move || format!("project-working-{name}"))
                        .child(spinner(SharedString::from(format!("project-spin-{directory}")), IconSize::Nav, c.text_muted))
                        .into_any_element(),
                )
            } else {
                None
            };
            let (toggle, start_in) = (directory.clone(), directory.clone());
            let header = NavRow::new(SharedString::from(format!("project-{directory}")), label)
                .icon(project_icon(collapsed))
                .cursor(cursor);
            // (Another computer's project: a new session there starts from Runs on.)
            let mut header = if elsewhere { header } else { header.hover_trailing(
                    Button::icon_only(
                        SharedString::from(format!("new-in-{directory}")),
                        IconName::Plus,
                        format!("New session in {}", group.name),
                    )
                    .disabled(workspace.client().gate().is_held())
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.new_session(Some(start_in.clone()), window, cx);
                    })),
                ) }
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(&toggle, cx)));
            if let Some(trailing) = trailing {
                header = header.trailing(trailing);
            }
            let selector = format!("project-{}", group.name);
            div().debug_selector(move || selector).child(header).into_any_element()
        }
        Line::Session { id, section, .. } => session_row(workspace, id, section, cursor, cx),
        Line::More(directory) if workspace.project_list(directory).is_some() => {
            let count = workspace.more_of(directory).map_or(0, |more| more.count);
            let shown = workspace.project_list(directory).map_or(0, |list| list.len() as u64);
            let (asking, _) = workspace.paging(directory);
            let label = if asking { "Showing more…".to_owned() } else { more_words(shown, count, workspace.total_of(directory)) };
            let more = directory.clone();
            let selector = format!("more-{directory}");
            let said = format!("more-words-{label}");
            let row = NavRow::new(SharedString::from(format!("more-{directory}")), label)
                .muted(true)
                .cursor(cursor)
                .on_click(cx.listener(move |this, _, _, cx| this.show_more(&more, cx)));
            div().debug_selector(move || said).child(div().debug_selector(move || selector).child(row)).into_any_element()
        }
        Line::Fewer(directory) => {
            let fewer = directory.clone();
            let selector = format!("fewer-{directory}");
            let row = NavRow::new(SharedString::from(format!("fewer-{directory}")), "Show fewer")
                .muted(true)
                .cursor(cursor)
                .on_click(cx.listener(move |this, _, _, cx| this.show_fewer(&fewer, cx)));
            div().debug_selector(move || selector).child(row).into_any_element()
        }
        Line::PageFailed { directory, why } => div()
            .debug_selector({ let selector = format!("page-failed-{directory}"); move || selector })
            .px(space::S2)
            .py(space::S1)
            .text_ui(TypeSize::Caption)
            .text_color(c.text_muted)
            .child(format!("Couldn’t show more: {why}"))
            .into_any_element(),
        Line::More(directory) => {
            let sidebar = workspace.listing().sidebar();
            let count = sidebar.projects.iter().find(|group| &group.directory == directory).map_or(0, |group| group.sessions.len());
            let label = if workspace.shows_all(directory) { "Show fewer".to_owned() } else { format!("Show all {count}") };
            let toggle = directory.clone();
            NavRow::new(SharedString::from(format!("more-{directory}")), label)
                .muted(true)
                .cursor(cursor)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_all(&toggle, cx)))
                .into_any_element()
        }
        Line::Found { id, snippet } => found_row(workspace, id, snippet, cursor, cx),
        Line::Needs { id, asks } => needs_row(workspace, id, asks, cursor, cx),
    };
    // A project's own rows hang off a thin line under it, unbroken from one to the next.
    let nested = matches!(line, Line::Session { nested: true, .. } | Line::More(_) | Line::Fewer(_) | Line::PageFailed { .. } | Line::Empty(_));
    // (A subagent sits under the session that started it, a step further in.)
    let subagent = matches!(line, Line::Session { section: "subagent", .. });
    div()
        .pb(space::S0_5)
        .when(nested, |d| d.ml(px(15.)).pl(space::S1).border_l_1().border_color(c.border))
        .child(div().when(subagent, |d| d.pl(space::S4)).child(row))
        .into_any_element()
}

fn active_row_popover(workspace: &Workspace, row_key: &str) -> Option<AnyElement> {
    let popover = workspace.session_menu().active_popover.as_ref()?;
    if !popover.is_at_row(row_key) {
        return None;
    }
    let content = match popover {
        crate::session_menu::ActivePopover::Rename { view, .. } => view.clone().into_any_element(),
        crate::session_menu::ActivePopover::Delete { view, .. } => view.clone().into_any_element(),
    };
    Some(row_anchored_popover(content))
}

fn hover_action_specs(actions: &Actions) -> Vec<Action> {
    if actions.held.is_some() {
        return Vec::new();
    }
    let has_archive = actions.actions.iter().any(|action| action.id == "archive");
    actions
        .actions
        .iter()
        .filter(|action| match action.id.as_str() {
            "pin" | "unpin" | "archive" | "unarchive" => true,
            "delete" => !has_archive,
            _ => false,
        })
        .cloned()
        .collect()
}

fn row_hover_actions(
    id: &str,
    row_key: &str,
    actions: &[Action],
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let actions_id = SharedString::from(format!("actions-{row_key}"));
    let mut row = div()
        .id(actions_id)
        .flex()
        .flex_none()
        .items_center()
        .pl(space::S1)
        .gap(space::S2)
        // Keeping buttons strictly pointer-driven and stopping left mouse_down prevents a narrow-row
        // slip between buttons from opening the row or triggering adjacent actions.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| cx.stop_propagation());

    for action in actions {
        let target_id = id.to_owned();
        let target_key = row_key.to_owned();
        let label = action.label.clone();
        match action.id.as_str() {
            "pin" | "unpin" => {
                let pinned = action.id == "unpin";
                row = row.child(
                    Button::icon_only(
                        SharedString::from(format!("pin-{row_key}")),
                        IconName::Pin,
                        label.clone(),
                    )
                    .ghost()
                    .size(ButtonSize::Small)
                    .selected(pinned)
                    .tooltip(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.pin_session(&target_id, !pinned, cx))),
                );
            }
            "archive" => {
                row = row.child(
                    Button::icon_only(
                        SharedString::from(format!("archive-{row_key}")),
                        IconName::Archive,
                        label.clone(),
                    )
                    .ghost()
                    .size(ButtonSize::Small)
                    .tooltip(format!("{label} ⌘⌫"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.archive_session(&target_id, true, window, cx)
                    })),
                );
            }
            "unarchive" => {
                row = row.child(
                    Button::icon_only(
                        SharedString::from(format!("unarchive-{row_key}")),
                        IconName::Undo,
                        label.clone(),
                    )
                    .ghost()
                    .size(ButtonSize::Small)
                    .tooltip(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.archive_session(&target_id, false, window, cx)
                    })),
                );
            }
            "delete" => {
                let display_label = format!("{label}…");
                let confirm = action.confirm.clone();
                row = row.child(
                    Button::icon_only(
                        SharedString::from(format!("delete-{row_key}")),
                        IconName::Trash,
                        display_label.clone(),
                    )
                    .ghost()
                    .size(ButtonSize::Small)
                    .tooltip(format!("{display_label} ⇧⌘⌫"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_delete_action(
                            &target_id,
                            confirm.clone(),
                            PopoverAnchor::Row(target_key.clone()),
                            window,
                            cx,
                        );
                    })),
                );
            }
            _ => {}
        }
    }

    row.into_any_element()
}

fn session_row(workspace: &mut Workspace, id: &str, section: &str, cursor: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let facts = workspace.listing().facts(id).cloned();
    let title = facts
        .as_ref()
        .and_then(|facts| facts.title.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "New Session".into());
    let running = facts.as_ref().is_some_and(Facts::is_running);
    let asked = facts.as_ref().is_some_and(|facts| facts.asked.is_some());
    let pinned = facts.as_ref().is_some_and(Facts::is_pinned);
    let archived = facts.as_ref().is_some_and(Facts::is_archived);
    let unsent = workspace.is_unsent(id);
    // With "Needs you" on top, the open session is highlighted there only, never twice.
    let selected = workspace.opened() == Some(id) && !needs_you(workspace, id);

    let row_key = format!("session-{section}:{id}");
    let row_key_id = format!("session-{section}-{id}");
    let menu_handle = workspace.session_menu_mut().row_menu_handle(&row_key);
    let menu_open = menu_handle.is_open();
    let popover = active_row_popover(workspace, &row_key);
    let is_hovered = workspace.session_menu().hovered_row.as_deref() == Some(row_key.as_str());
    let gate_held = workspace.client().gate().is_held();
    // Another computer's session: its menu is that computer's (not drawn here yet), and while that
    // computer is away, the row says since when, or that it's updating.
    let on = facts.as_ref().and_then(|facts| facts.on.clone());
    let hover_buttons = (!gate_held && on.is_none())
        .then(|| workspace.session_menu().cached_actions(id))
        .flatten()
        .map(hover_action_specs)
        .unwrap_or_default();
    let show_actions = (is_hovered || menu_open || popover.is_some()) && !hover_buttons.is_empty();

    let leading: AnyElement = if unsent {
        status_dot(c.danger).into_any_element()
    } else if running {
        spinner(SharedString::from(format!("spin-{section}-{id}")), IconSize::Nav, c.text_muted).into_any_element()
    } else if asked {
        status_dot(c.accent).into_any_element()
    } else if pinned {
        Icon::new(IconName::Pin).size(IconSize::Nav).color(if selected { c.icon_on } else { c.text_faint }).into_any_element()
    } else {
        div().into_any_element()
    };
    let when = match &on {
        Some(on) if on.updating => "Updating…".to_owned(),
        Some(on) if on.away => on.last_seen.map(|seen| format!("seen {}", time::ago(seen, workspace.now()))).unwrap_or_default(),
        // (In Chats, where nothing else says which computer it's on.)
        Some(on) if section == "chat" => format!("{} · {}", on.name, facts.as_ref().map(|facts| time::ago(facts.updated, workspace.now())).unwrap_or_default()),
        _ => facts.as_ref().map(|facts| time::ago(facts.updated, workspace.now())).unwrap_or_default(),
    };
    let opens = on.as_ref().is_none_or(|on| on.opens());
    let target = kuvo_box::moves::Target::new(kuvo_box::moves::Kind::Chat, id);
    let move_status = workspace.moves().read(cx).row_status(&target, cx);
    let end: AnyElement = if let Some(status) = move_status {
        div().max_w(px(120.)).child(status).into_any_element()
    } else if show_actions {
        row_hover_actions(id, &row_key, &hover_buttons, cx)
    } else {
        let when_selector = format!("session-when-{section}-{id}");
        // (What it says, for tests: "Updating…", "seen 2h".)
        let said_selector = format!("session-said-{id}-{when}");
        div()
            .debug_selector(move || when_selector)
            .child(div().debug_selector(move || said_selector))
            .flex_none()
            .text_role(TextRole::Meta)
            .text_color(c.text_faint)
            .child(when)
            .into_any_element()
    };

    let (hover, pressed, selected_bg, accent) = (c.hover, c.pressed, c.selected, c.accent);
    let text_color = if selected || !archived { c.text } else { c.text_muted };
    let open = id.to_owned();
    let hover_key = row_key.clone();
    let menu_session = id.to_owned();
    let menu_key = row_key.clone();
    let workspace_handle = cx.entity();

    let row = div()
        .id(SharedString::from(row_key.clone()))
        .flex()
        .items_center()
        .gap(space::S2)
        .h(NAV_ROW_HEIGHT)
        .pl(space::S2)
        .pr(if show_actions { space::S1 } else { space::S2 })
        .rounded(radius::NAV)
        .border_1()
        .border_color(if cursor { accent } else { gpui::transparent_black() })
        .text_role(TextRole::RowLabel)
        .text_color(text_color)
        .when(selected, |r| r.bg(selected_bg))
        .when(!selected && (cursor || is_hovered || menu_open || popover.is_some()), |r| r.bg(hover))
        .cursor_pointer()
        .tab_index(0)
        .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
        .when(!selected, |r| r.active(move |s| s.bg(pressed)))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.set_hovered_row(Some(hover_key.clone()), cx);
            } else if this.session_menu().hovered_row.as_deref() == Some(hover_key.as_str()) {
                this.set_hovered_row(None, cx);
            }
        }))
        .when(opens, |r| r.on_click(cx.listener(move |this, _, window, cx| this.open_session(&open, window, cx))))
        .when(!opens, |r| r.cursor_default())
        .child(div().flex().flex_none().justify_center().min_w(IconSize::Nav.pixels()).child(leading))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .child(end);

    if on.is_some() {
        return div().relative().debug_selector(move || row_key_id).child(row).into_any_element();
    }
    let with_menu = ContextMenuArea::new(SharedString::from(format!("ctx-{row_key}")), row, move |window, cx| {
        workspace_handle.update(cx, |workspace, cx| workspace.open_row_menu(&menu_session, &menu_key, window, cx))
    })
    .with_handle(menu_handle);

    div()
        .relative()
        .debug_selector(move || row_key_id)
        .child(with_menu)
        .children(popover)
        .into_any_element()
}

/// A conversation the search found words in: its title, and the words around them under it.
fn found_row(workspace: &mut Workspace, id: &str, snippet: &str, cursor: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let facts = workspace.listing().facts(id).cloned();
    let title = facts
        .as_ref()
        .and_then(|facts| facts.title.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "New Session".into());
    let archived = facts.as_ref().is_some_and(Facts::is_archived);
    let selected = workspace.opened() == Some(id);
    let open = id.to_owned();
    let row_key = format!("found:{id}");
    let selector = format!("found-{id}");
    let menu_handle = workspace.session_menu_mut().row_menu_handle(&row_key);
    let menu_open = menu_handle.is_open();
    let popover = active_row_popover(workspace, &row_key);
    let is_hovered = workspace.session_menu().hovered_row.as_deref() == Some(row_key.as_str());
    let hover_key = row_key.clone();
    let menu_session = id.to_owned();
    let menu_key = row_key.clone();
    let workspace_handle = cx.entity();

    let (hover, selected_bg) = (c.hover, c.selected);
    let row = div()
        .id(SharedString::from(format!("found-{id}")))
        .flex()
        .flex_col()
        .gap(space::S0_5)
        .px(space::S2)
        .py(space::S1_5)
        .rounded(radius::NAV)
        .border_1()
        .border_color(if cursor { c.accent } else { gpui::transparent_black() })
        .when(selected, |d| d.bg(selected_bg))
        .when(!selected && (cursor || is_hovered || menu_open || popover.is_some()), |d| d.bg(hover))
        .cursor_pointer()
        .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.set_hovered_row(Some(hover_key.clone()), cx);
            } else if this.session_menu().hovered_row.as_deref() == Some(hover_key.as_str()) {
                this.set_hovered_row(None, cx);
            }
        }))
        .on_click(cx.listener(move |this, _, window, cx| this.open_session(&open, window, cx)))
        .child(div().min_w_0().truncate().text_role(TextRole::RowTitle).text_color(if selected || !archived { c.text } else { c.text_muted }).child(title))
        .child(div().min_w_0().text_role(TextRole::RowDetail).text_color(c.text_muted).line_clamp(2).child(snippet.trim().to_owned()));

    let with_menu = ContextMenuArea::new(SharedString::from(format!("ctx-{row_key}")), row, move |window, cx| {
        workspace_handle.update(cx, |workspace, cx| workspace.open_row_menu(&menu_session, &menu_key, window, cx))
    })
    .with_handle(menu_handle);

    div()
        .relative()
        .debug_selector(move || selector)
        .child(with_menu)
        .children(popover)
        .into_any_element()
}

/// Quiet words in the sidebar's place: loading, nothing yet, or what's wrong.
fn quiet(words: Option<&str>, busy: bool, cx: &App) -> AnyElement {
    if busy { return div().px(space::S2).text_center().child(kuvo_ui::small::LoadingLabel::new("sessions-loading", words.unwrap_or("Loading sessions…").to_owned()).compact()).into_any_element(); }
    let c = &cx.theme().colors;
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(space::S2)
        .px(space::S2)
        .pt(space::S3)
        .text_ui(TypeSize::Small)
        .text_color(c.text_muted)
        .children(busy.then(|| div().pt(px(3.)).child(spinner("lists-loading", IconSize::Nav, c.text_muted))))
        .children(words.map(|words| div().flex_1().min_w_0().child(words.to_owned())))
        .into_any_element()
}

/// Whether `session` shows in "Needs you".
fn needs_you(workspace: &Workspace, session: &str) -> bool {
    !workspace.archived_filter()
        && workspace.listing().needs_you().iter().any(|needs| needs.session == session)
        && workspace.search_is_empty()
}

/// A session waiting on you, on top: its title, then its project and what it asks, in the box's words.
fn needs_row(workspace: &mut Workspace, id: &str, asks: &str, cursor: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let facts = workspace.listing().facts(id).cloned();
    let title = facts
        .as_ref()
        .and_then(|facts| facts.title.clone())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "New Session".into());
    let project = facts.as_ref().and_then(|facts| facts.project.clone()).map(|project| workspace.project_name(&project));
    let detail = match project {
        Some(project) if !asks.is_empty() => format!("{project} · {asks}"),
        Some(project) => project,
        None => asks.to_owned(),
    };
    let when = facts.as_ref().and_then(|facts| facts.asked).map(|asked| time::ago(asked, workspace.now())).unwrap_or_default();
    let selected = workspace.opened() == Some(id);
    let (hover, selected_bg) = (c.hover, c.selected);
    let open = id.to_owned();
    let row_key = format!("needs:{id}");
    let selector = format!("needs-{id}");
    let menu_handle = workspace.session_menu_mut().row_menu_handle(&row_key);
    let menu_open = menu_handle.is_open();
    let popover = active_row_popover(workspace, &row_key);
    let is_hovered = workspace.session_menu().hovered_row.as_deref() == Some(row_key.as_str());
    let gate_held = workspace.client().gate().is_held();
    let hover_buttons = (!gate_held)
        .then(|| workspace.session_menu().cached_actions(id))
        .flatten()
        .map(hover_action_specs)
        .unwrap_or_default();
    let show_actions = (is_hovered || menu_open || popover.is_some()) && !hover_buttons.is_empty();
    let hover_key = row_key.clone();
    let menu_session = id.to_owned();
    let menu_key = row_key.clone();
    let workspace_handle = cx.entity();

    let end: AnyElement = if show_actions {
        row_hover_actions(id, &row_key, &hover_buttons, cx)
    } else {
        div().flex_none().text_role(TextRole::Meta).text_color(c.text_faint).child(when).into_any_element()
    };

    let row = div()
        .id(SharedString::from(format!("needs-{id}")))
        .flex()
        .gap(space::S2)
        .px(space::S2)
        .py(space::S1_5)
        .rounded(radius::NAV)
        .border_1()
        .border_color(if cursor { c.accent } else { gpui::transparent_black() })
        .cursor_pointer()
        .when(selected, |r| r.bg(selected_bg))
        .when(!selected && (cursor || is_hovered || menu_open || popover.is_some()), |r| r.bg(hover))
        .hover(move |s| s.bg(if selected { selected_bg } else { hover }))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.set_hovered_row(Some(hover_key.clone()), cx);
            } else if this.session_menu().hovered_row.as_deref() == Some(hover_key.as_str()) {
                this.set_hovered_row(None, cx);
            }
        }))
        .on_click(cx.listener(move |this, _, window, cx| this.open_session(&open, window, cx)))
        .child(div().flex().flex_none().justify_center().w(IconSize::Nav.pixels()).pt(px(5.)).child(status_dot(c.accent)))
        .child(crate::areas::two_lines(title, div().truncate().text_role(TextRole::RowDetail).text_color(c.text_muted).child(detail), &c))
        .child(end);

    let with_menu = ContextMenuArea::new(SharedString::from(format!("ctx-{row_key}")), row, move |window, cx| {
        workspace_handle.update(cx, |workspace, cx| workspace.open_row_menu(&menu_session, &menu_key, window, cx))
    })
    .with_handle(menu_handle);

    div()
        .relative()
        .debug_selector(move || selector)
        .child(with_menu)
        .children(popover)
        .into_any_element()
}

/// Keep the project recognizable as a folder in both disclosure states.
fn project_icon(collapsed: bool) -> IconName {
    if collapsed { IconName::Folder } else { IconName::FolderOpen }
}

#[cfg(test)]
mod project_icon_tests {
    use super::*;

    #[test]
    fn project_disclosure_keeps_a_folder_in_both_states() {
        assert_eq!(project_icon(true), IconName::Folder);
        assert_eq!(project_icon(false), IconName::FolderOpen);
    }
}
