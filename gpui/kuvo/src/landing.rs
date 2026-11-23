//! The landing view, for a new conversation: the composer in the middle, on a sky that follows the time of day,
//! with where it starts above it: the project (or no project, a chat). The computer it runs on is
//! the composer's Runs on, beside its model: one choice of computer, there.

use gpui::{AnyElement, AppContext as _, Context, IntoElement, ParentElement, Styled, Window, div, px};
use kuvo_box::models::StartIn;
use kuvo_ui::{
    Button, ButtonSize, IconName,
    menu::{Menu, MenuEntry, PopoverMenu},
    sky::sky,
    theme::space,
};

use crate::entries::COLUMN;
use crate::workspace::Workspace;

pub fn render(workspace: &Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let place = workspace.landing_place();
    let label = match &place {
        StartIn::Project(directory) => workspace.project_name(directory),
        StartIn::Chat => "No project".to_owned(),
    };
    // Every project the computer picked in Runs on has, newest first, then none (a chat).
    let on = workspace.landing().read(cx).runs_on().map(str::to_owned);
    let projects = workspace.landing_projects(on.as_deref());
    let this = cx.entity().downgrade();
    let picked = place.clone();
    let project_chip = PopoverMenu::new(
        "project-menu",
        Button::new("project-chip", label)
            .ghost()
            .size(ButtonSize::Small)
            .icon(if place == StartIn::Chat { IconName::Message } else { IconName::Folder })
            .icon_after(IconName::ChevronDown),
        move |_, cx| {
            let (projects, this, picked) = (projects.clone(), this.clone(), picked.clone());
            cx.new(|cx| {
                let mut menu = Menu::new(cx).width(px(240.)).header("Start in");
                for (directory, name) in projects {
                    let checked = picked == StartIn::Project(directory.clone());
                    let this = this.clone();
                    menu = menu.entry(MenuEntry::new(name).icon(IconName::Folder).checked(checked).on_select(move |_, cx| {
                        this.update(cx, |this, cx| this.pick_place(StartIn::Project(directory.clone()), cx)).ok();
                    }));
                }
                let this = this.clone();
                menu.separator().entry(MenuEntry::new("No project").detail("a chat").icon(IconName::Message).checked(picked == StartIn::Chat).on_select(move |_, cx| {
                    this.update(cx, |this, cx| this.pick_place(StartIn::Chat, cx)).ok();
                }))
            })
        },
    );
    div()
        .relative()
        .size_full()
        .child(sky(workspace.sky_hour(), cx))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px(space::S6)
                // A little above the middle, where the eye rests.
                .pb(px(96.))
                .child(
                    div()
                        .w_full()
                        .max_w(px(COLUMN))
                        .flex()
                        .flex_col()
                        .gap(space::S2)
                        .child(div().flex().items_center().gap(space::S1).ml(-space::S1).child(project_chip))
                        .child(workspace.landing().clone()),
                ),
        )
        .into_any_element()
}
