//! The rail's areas: the big switches (Sessions, Bots, Work Queue, Library, Marketplace, Computers,
//! Settings), each with
//! its own sidebar beside the rail and its own main view. ⌃1 to ⌃5 switch between them (⌘1 to ⌘8
//! are the tabs' keys, as on the Mac).

use gpui::{App, KeyBinding, actions};
use kuvo_ui::IconName;

actions!(area, [ShowSessions, ShowBots, ShowQueue, ShowComputers, ShowSettings, ShowUsage, ShowLibrary, ShowMarketplace, ShowAccounts, OpenHelp, Back, Forward]);

#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
#[action(namespace = area, no_json)]
pub struct UsageFilter {
    pub title: String,
    pub account: Option<String>,
    pub provider: String,
}

/// Where Help goes: Kuvo Code's documentation.
pub const HELP: &str = "https://www.kuvo.run/code/docs/";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Sessions,
    Bots,
    Queue,
    Computers,
    Settings,
    Insights,
    Library,
    Marketplace,
}

impl Area {
    pub const ALL: [Area; 8] = [Area::Bots, Area::Sessions, Area::Queue, Area::Library, Area::Marketplace, Area::Computers, Area::Insights, Area::Settings];

    pub fn name(self) -> &'static str {
        match self {
            Area::Sessions => "Sessions",
            Area::Bots => "Bots",
            Area::Queue => "Work Queue",
            Area::Computers => "Computers",
            Area::Settings => "Settings",
            Area::Insights => "Insights",
            Area::Library => "Library",
            Area::Marketplace => "Marketplace",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            Area::Sessions => IconName::Message,
            Area::Bots => IconName::Bot,
            Area::Queue => IconName::Queue,
            Area::Computers => IconName::Computer,
            Area::Settings => IconName::Settings,
            Area::Insights => IconName::Chart,
            Area::Library => IconName::Library,
            Area::Marketplace => IconName::Grid,
        }
    }

    /// Its key, as its tooltip shows it.
    pub fn keys(self) -> &'static str {
        match self {
            Area::Sessions => "⌃2",
            Area::Bots => "⌃1",
            Area::Queue => "⌃3",
            Area::Computers => "⌃4",
            Area::Settings => "⌃5",
            Area::Insights => "⌃6",
            Area::Library => "⌃7",
            Area::Marketplace => "⌃8",
        }
    }

    /// What its sidebar's search says before anything's typed.
    pub fn slug(self) -> &'static str {
        match self {
            Area::Sessions => "sessions",
            Area::Bots => "bots",
            Area::Queue => "queue",
            Area::Computers => "computers",
            Area::Settings => "settings",
            Area::Insights => "insights",
            Area::Library => "library",
            Area::Marketplace => "marketplace",
        }
    }
}

pub fn bind_keys(cx: &mut App) {
    let workspace = Some("Workspace");
    cx.bind_keys([
        // Settings… and Accounts…, as on the Mac (ahead of ⌃5: the menu shows an action's first key).
        KeyBinding::new("cmd-,", ShowSettings, workspace),
        KeyBinding::new("cmd-shift-k", ShowAccounts, workspace),
        // The Mac app's Show Work Queue (ahead of ⌃3, for the same reason).
        KeyBinding::new("cmd-alt-u", ShowQueue, workspace),
        KeyBinding::new("ctrl-1", ShowBots, workspace),
        KeyBinding::new("ctrl-2", ShowSessions, workspace),
        KeyBinding::new("ctrl-3", ShowQueue, workspace),
        KeyBinding::new("ctrl-4", ShowComputers, workspace),
        KeyBinding::new("ctrl-5", ShowSettings, workspace),
        KeyBinding::new("ctrl-6", ShowUsage, workspace),
        KeyBinding::new("ctrl-7", ShowLibrary, workspace),
        KeyBinding::new("ctrl-8", ShowMarketplace, workspace),
        // Back and forward, as a browser's and Xcode's.
        KeyBinding::new("cmd-[", Back, workspace),
        KeyBinding::new("cmd-]", Forward, workspace),
    ]);
}
