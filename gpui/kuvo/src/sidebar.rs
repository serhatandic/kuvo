//! The Sessions area's sidebar (Draft B): the search with an Archived filter toggle, then what's
//! waiting on you across projects ("Needs you"), then the box's sessions: each project with its
//! sessions nested under it (pinned raised to the top once and marked with a pin, then newest
//! first), and chats (pinned raised to top). Under the pointer (or while a row's menu/popover is
//! open), separated Pin and Archive buttons appear on the row (Unarchive and Delete… when viewing
//! archived); right-click opens the full box-driven menu, and Rename / Delete confirm anchor as
//! popovers beneath the row.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use kuvo_box::search::SEARCHED_FROM;
use kuvo_box::session_actions::{Action, Actions};
use kuvo_box::sessions::{Facts, Listing, finder_order};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    menu::ContextMenuArea,
    row::{NAV_ROW_HEIGHT, NavRow, SectionHeader},
    small::{Chip, count_badge, spinner, status_dot},
    theme::{radius, space},
};

use crate::session_menu::{PopoverAnchor, row_anchored_popover};
use crate::time;
use crate::workspace::{Lists, Workspace};

actions!(sidebar, [WalkNext, WalkPrevious, WalkOpen, WalkLeave, FocusSearch]);

/// How wide an area's sidebar is.
pub const WIDTH: f32 = 272.;
/// How many of a project's sessions show before "Show more".
pub const SHOWN: usize = 6;
/// The key context of the list while the keyboard walks it, and of the search around its field.
pub const CONTEXT: &str = "Sidebar";
pub const SEARCH: &str = "SidebarSearch";

/// The keys the sidebar takes: the list's while it has the keyboard, and the search field's arrows,
/// Return and Escape ahead of the field's own.
pub fn bind_keys(cx: &mut App) {
    let list = Some(CONTEXT);
    let search = format!("{SEARCH} > {}", kuvo_ui::text_field::CONTEXT);
    let search = Some(search.as_str());
    cx.bind_keys([
        KeyBinding::new("down", WalkNext, list),
        KeyBinding::new("up", WalkPrevious, list),
        KeyBinding::new("enter", WalkOpen, list),
        KeyBinding::new("escape", WalkLeave, list),
        KeyBinding::new("down", WalkNext, search),
        KeyBinding::new("up", WalkPrevious, search),
        KeyBinding::new("enter", WalkOpen, search),
        KeyBinding::new("escape", WalkLeave, search),
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
    ArchivedProject(String),
    /// A session; `section` tells apart the section where it shows (`project`, `chat`, `found`,
    /// `archived-project`, `archived-chat`), `nested`: under its project.
    Session { id: String, section: &'static str, nested: bool },
    /// "Show N more" (or fewer) under a project.
    More(String),
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
        matches!(self, Line::Project(_) | Line::Session { .. } | Line::More(_) | Line::Found { .. } | Line::Needs { .. })
    }

    /// What it's known by while the list changes under it.
    pub fn key(&self) -> String {
        match self {
            Line::Project(directory) => format!("project:{directory}"),
            Line::ArchivedProject(directory) => format!("archived-project:{directory}"),
            Line::Session { id, section, .. } => format!("session-{section}:{id}"),
            Line::More(directory) => format!("more:{directory}"),
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
    if !sidebar.projects.is_empty() {
        lines.push(Line::Header("Projects"));
    }
    for group in &sidebar.projects {
        lines.push(Line::Project(group.directory.clone()));
        if workspace.is_collapsed(&group.directory) {
            continue;
        }
        if group.sessions.is_empty() {
            lines.push(Line::Empty(group.directory.clone()));
            continue;
        }
        let ordered = raise_pinned(listing, &group.sessions);
        let shown = if workspace.shows_all(&group.directory) { ordered.len() } else { ordered.len().min(SHOWN) };
        lines.extend(ordered[..shown].iter().map(|id| Line::Session { id: id.clone(), section: "project", nested: true }));
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

/// Archived sessions grouped by project (folder header + nested rows) and Chats, newest first.
fn archived_lines(workspace: &Workspace) -> Vec<Line> {
    let listing = workspace.listing();
    let mut by_project: BTreeMap<String, Vec<(&String, &Facts)>> = BTreeMap::new();
    let mut chats: Vec<(&String, &Facts)> = Vec::new();
    for (id, facts) in listing.sessions() {
        if !facts.is_archived() {
            continue;
        }
        if facts.kind.is_project() {
            let directory = facts.project.clone().unwrap_or_else(|| facts.directory.clone());
            by_project.entry(directory).or_default().push((id, facts));
        } else if facts.kind.is_chat() {
            chats.push((id, facts));
        }
    }
    let sort_newest = |rows: &mut Vec<(&String, &Facts)>| {
        rows.sort_by(|a, b| b.1.updated.partial_cmp(&a.1.updated).unwrap_or(Ordering::Equal).then_with(|| a.0.cmp(b.0)));
    };
    let mut groups: Vec<(f64, String, String, Vec<String>)> = by_project
        .into_iter()
        .map(|(directory, mut rows)| {
            sort_newest(&mut rows);
            let top = rows.first().map_or(0.0, |(_, facts)| facts.updated);
            let name = workspace.project_name(&directory);
            let ids = rows.into_iter().map(|(id, _)| id.clone()).collect();
            (top, name, directory, ids)
        })
        .collect();
    groups.sort_by(|(left, a_name, a_dir, _), (right, b_name, b_dir, _)| {
        right.partial_cmp(left).unwrap_or(Ordering::Equal).then_with(|| finder_order(a_name, b_name)).then_with(|| a_dir.cmp(b_dir))
    });
    sort_newest(&mut chats);

    let mut lines = Vec::new();
    for (_, _, directory, ids) in groups {
        lines.push(Line::ArchivedProject(directory));
        lines.extend(ids.into_iter().map(|id| Line::Session { id, section: "archived-project", nested: true }));
    }
    if !chats.is_empty() {
        lines.push(Line::Header("Chats"));
        lines.extend(chats.into_iter().map(|(id, _)| Line::Session { id: id.clone(), section: "archived-chat", nested: false }));
    }
    lines
}

/// What a search finds: sessions by their names, then (from three characters) the conversations
/// the box found the words in, less those found by name. When `archived_only` is on, only
/// archived sessions show.
fn found_lines(workspace: &Workspace, query: &str, archived_only: bool) -> Vec<Line> {
    let listing = workspace.listing();
    let named: Vec<String> = listing
        .named(query)
        .into_iter()
        .filter(|id| listing.facts(id).is_some_and(|facts| facts.is_archived() == archived_only))
        .collect();
    let searched = query.chars().count() >= SEARCHED_FROM;
    let finds = workspace.finds();
    let in_conversations: Vec<Line> = finds
        .iter()
        .filter(|found| {
            !named.contains(&found.session)
                && listing
                    .facts(&found.session)
                    .is_some_and(|facts| facts.is_archived() == archived_only)
        })
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
        .w(px(WIDTH))
        .h_full()
        .flex_none()
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
                    d.child(
                        div()
                            .debug_selector(|| "session-action-error".into())
                            .flex()
                            .items_start()
                            .gap(space::S1_5)
                            .px(space::S2)
                            .py(space::S1_5)
                            .rounded(radius::NAV)
                            .bg(c.danger.opacity(0.10))
                            .border_1()
                            .border_color(c.danger.opacity(0.25))
                            .child(div().flex_1().min_w_0().text_ui(TypeSize::Caption).text_color(c.danger).child(error))
                            .child(
                                Button::icon_only("dismiss-session-error", IconName::Close, "Dismiss")
                                    .ghost()
                                    .size(ButtonSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_action_error(cx))),
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

/// The search: a sunken field with its glass, × to clear while it has words, and the Archived
/// filter toggle at its right.
fn search_field(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let c = cx.theme().colors.clone();
    let has_words = !workspace.search_text(cx).is_empty();
    let archived = workspace.archived_filter();
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
        .child(
            Button::icon_only(
                "toggle-archived",
                IconName::Sliders,
                if archived { "Show active sessions" } else { "Show archived sessions" },
            )
            .ghost()
            .size(ButtonSize::Small)
            .selected(archived)
            .tooltip("Archived sessions")
            .on_click(cx.listener(|this, _, window, cx| this.toggle_archived_filter(window, cx))),
        )
        .into_any_element()
}

/// The list's rows, one for each of its lines, so the keyboard can bring any into view.
fn sessions(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let lines = lines(workspace, cx);
    if lines.is_empty() {
        let message = if workspace.archived_filter() { "No archived sessions." } else { "No sessions yet." };
        return vec![quiet(Some(message), false, cx)];
    }
    let cursor = workspace.walked().map(str::to_owned);
    lines
        .iter()
        .map(|line| line_row(workspace, line, cursor.as_deref() == Some(line.key().as_str()), cx))
        .collect()
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
        Line::Searching => div()
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S2)
            .py(space::S1_5)
            .text_ui(TypeSize::Small)
            .text_color(c.text_muted)
            .child(spinner("searching", IconSize::Nav, c.text_muted))
            .child("Searching conversations…")
            .into_any_element(),
        Line::Empty(_) => div()
            .h(px(26.))
            .flex()
            .items_center()
            .pl(space::S2)
            .text_ui(TypeSize::Caption)
            .text_color(c.text_faint)
            .child("No sessions yet")
            .into_any_element(),
        Line::ArchivedProject(directory) => {
            let name = workspace.project_name(directory);
            let selector = format!("archived-project-{name}");
            div()
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(space::S2)
                .h(NAV_ROW_HEIGHT)
                .px(space::S2)
                .text_ui(TypeSize::Body)
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
            let (asking, working) = listing.projects().get(directory).map_or((0, 0), |project| (project.asking, project.working));
            let name = group.name.clone();
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
            let mut header = NavRow::new(SharedString::from(format!("project-{directory}")), group.name.clone())
                .icon(if collapsed { IconName::ChevronRight } else { IconName::Folder })
                .cursor(cursor)
                .hover_trailing(
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
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(&toggle, cx)));
            if let Some(trailing) = trailing {
                header = header.trailing(trailing);
            }
            let selector = format!("project-{}", group.name);
            div().debug_selector(move || selector).child(header).into_any_element()
        }
        Line::Session { id, section, .. } => session_row(workspace, id, section, cursor, cx),
        Line::More(directory) => {
            let sidebar = workspace.listing().sidebar();
            let count = sidebar.projects.iter().find(|group| &group.directory == directory).map_or(0, |group| group.sessions.len());
            let label = if workspace.shows_all(directory) { "Show fewer".to_owned() } else { format!("Show {} more", count.saturating_sub(SHOWN)) };
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
    let nested = matches!(line, Line::Session { nested: true, .. } | Line::More(_) | Line::Empty(_));
    div()
        .pb(space::S0_5)
        .when(nested, |d| d.ml(px(15.)).pl(space::S1).border_l_1().border_color(c.border))
        .child(row)
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
    let away = facts.as_ref().is_some_and(|facts| facts.away == Some(true));
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
    let hover_buttons = (!gate_held && !away)
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
        Icon::new(IconName::Pin).size(IconSize::Nav).color(if selected { c.text } else { c.text_faint }).into_any_element()
    } else {
        div().into_any_element()
    };
    let when = facts.as_ref().map(|facts| time::ago(facts.updated, workspace.now())).unwrap_or_default();
    let end: AnyElement = if show_actions {
        row_hover_actions(id, &row_key, &hover_buttons, cx)
    } else {
        div()
            .flex_none()
            .text_ui(TypeSize::Caption)
            .text_color(if asked { c.accent } else { c.text_faint })
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
        .text_ui(TypeSize::Body)
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
        .on_click(cx.listener(move |this, _, window, cx| this.open_session(&open, window, cx)))
        .child(div().flex().flex_none().justify_center().min_w(IconSize::Nav.pixels()).child(leading))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .child(end);

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
        .child(div().truncate().text_ui(TypeSize::Body).text_color(if selected || !archived { c.text } else { c.text_muted }).child(title))
        .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).line_clamp(2).child(snippet.trim().to_owned()));

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
    let away = facts.as_ref().is_some_and(|facts| facts.away == Some(true));
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
    let hover_buttons = (!gate_held && !away)
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
        div().flex_none().text_ui(TypeSize::Caption).text_color(c.accent).child(when).into_any_element()
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
        .child(div().flex().flex_none().justify_center().w(IconSize::Nav.pixels()).pt(px(6.)).child(status_dot(c.accent)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().truncate().text_ui(TypeSize::Body).text_color(c.text).child(title))
                .child(div().truncate().text_ui(TypeSize::Small).text_color(c.text_muted).child(detail)),
        )
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
