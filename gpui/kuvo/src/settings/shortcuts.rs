//! Read-only descriptions of the bindings installed in this running app.
use gpui::App;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub area: String,
    pub title: String,
    pub keys: String,
    pub context: String,
}

pub fn read(cx: &App) -> Vec<Shortcut> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let mut rows: Vec<_> = keymap.bindings().filter_map(|binding| {
        let name = binding.action().name();
        if standard(name) { return None; }
        let (area, title) = title(name).unwrap_or(("Kuvo", "Shortcut"));
        Some(Shortcut {
            area: area.into(),
            title: title.into(),
            keys: binding.keystrokes().iter().map(|key| key.inner().display(gpui::OperatingSystem::Mac).to_string()).collect::<Vec<_>>().join(" "),
            context: binding.predicate().map(|p| p.to_string()).unwrap_or_default(),
        })
    }).collect();
    rows.sort_by(|a, b| (&a.area, &a.title, &a.keys, &a.context).cmp(&(&b.area, &b.title, &b.keys, &b.context)));
    rows
}

/// Standard macOS text and menu navigation stays out of the app shortcut list.
pub fn standard(name: &str) -> bool {
    name.starts_with("text_field::") || name.starts_with("menu::")
}

/// Words for actions, independent of the live keymap that supplies their keys and contexts.
/// Tests require a title for every installed action, including standard editing actions.
pub fn title(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "kuvo::NewSession" => ("General", "New session"),
        "kuvo::OpenFolder" => ("General", "Open a folder"),
        "kuvo::ToggleAppearance" => ("General", "Switch light and dark"),
        "kuvo::Quit" => ("General", "Quit Kuvo"),
        "kuvo::TogglePanel" => ("General", "Show or hide the side panel"),
        "kuvo::ToggleSidebar" => ("General", "Show or hide the sidebar"),
        "kuvo::ShowChanges" => ("General", "Show Changes"),
        "kuvo::ShowFiles" => ("General", "Show Files"),
        "area::ShowSettings" => ("Navigation", "Open Settings"),
        "area::ShowAccounts" => ("Navigation", "Open Accounts"),
        "area::ShowQueue" => ("Navigation", "Show Work Queue"),
        "area::ShowBots" => ("Navigation", "Show Bots"),
        "area::ShowSessions" => ("Navigation", "Show Sessions"),
        "area::ShowComputers" => ("Navigation", "Show Computers"),
        "area::ShowUsage" => ("Navigation", "Show Insights"),
        "area::ShowLibrary" => ("Navigation", "Show Library"),
        "area::ShowMarketplace" => ("Navigation", "Show Marketplace"),
        "area::Back" => ("Navigation", "Go back"),
        "area::Forward" => ("Navigation", "Go forward"),
        "app_menu::Hide" => ("App menu", "Hide Kuvo"),
        "app_menu::HideOthers" => ("App menu", "Hide other apps"),
        "app_menu::Minimize" => ("App menu", "Minimize the window"),
        "composer::SendQueued" => ("Writing messages", "Queue for after the run"),
        "composer::ChooseModel" => ("Writing messages", "Choose model"),
        "model_picker::Next" => ("Choosing models", "Next model"),
        "model_picker::Previous" => ("Choosing models", "Previous model"),
        "model_picker::Pick" => ("Choosing models", "Choose model"),
        "model_picker::Close" => ("Choosing models", "Close model picker"),
        "model_picker::NextRail" => ("Choosing models", "Next model group"),
        "model_picker::PreviousRail" => ("Choosing models", "Previous model group"),
        "model_picker::Row1" => ("Choosing models", "Choose model 1"),
        "model_picker::Row2" => ("Choosing models", "Choose model 2"),
        "model_picker::Row3" => ("Choosing models", "Choose model 3"),
        "model_picker::Row4" => ("Choosing models", "Choose model 4"),
        "model_picker::Row5" => ("Choosing models", "Choose model 5"),
        "model_picker::Row6" => ("Choosing models", "Choose model 6"),
        "model_picker::Row7" => ("Choosing models", "Choose model 7"),
        "model_picker::Row8" => ("Choosing models", "Choose model 8"),
        "model_picker::Row9" => ("Choosing models", "Choose model 9"),
        "composer::ListNext" => ("Writing messages", "Next suggestion"),
        "composer::ListPrevious" => ("Writing messages", "Previous suggestion"),
        "composer::ListPick" => ("Writing messages", "Choose suggestion"),
        "composer::ListDismiss" => ("Writing messages", "Close suggestions"),
        "sidebar::WalkNext" => ("Sidebar", "Next item"),
        "sidebar::WalkPrevious" => ("Sidebar", "Previous item"),
        "sidebar::WalkOpen" => ("Sidebar", "Open selected item"),
        "sidebar::WalkLeave" => ("Sidebar", "Leave the list"),
        "sidebar::FocusSearch" => ("Sidebar", "Search the sidebar"),
        "session_menu::RenameSession" => ("Sessions", "Rename session"),
        "session_menu::ArchiveSession" => ("Sessions", "Archive session"),
        "session_menu::DeleteSession" => ("Sessions", "Delete session"),
        "queue_editor::NextField" => ("Work Queue", "Focus description"),
        "queue_editor::PreviousField" => ("Work Queue", "Focus title"),
        "queue_editor::SelectReference" => ("Work Queue", "Insert selected item reference"),
        "queue_editor::NextReference" => ("Work Queue", "Next item reference"),
        "queue_editor::PreviousReference" => ("Work Queue", "Previous item reference"),
        "queue_editor::HideReferences" => ("Work Queue", "Close item suggestions"),
        "wakes::Close" => ("Wake-ups", "Close wake-up schedule"),
        "changes::Commit" => ("Changes", "Commit changes"),
        "changes::Back" => ("Changes", "Go back in Changes"),
        "files::CloseFile" => ("Files", "Close file"),
        "menu::SelectNext" => ("Editing text", "Next menu item"),
        "menu::SelectPrevious" => ("Editing text", "Previous menu item"),
        "menu::Confirm" => ("Editing text", "Choose menu item"),
        "menu::Dismiss" => ("Editing text", "Close menu"),
        "text_field::Backspace" => ("Editing text", "Delete before the caret"),
        "text_field::Delete" => ("Editing text", "Delete after the caret"),
        "text_field::DeleteWordLeft" => ("Editing text", "Delete previous word"),
        "text_field::DeleteWordRight" => ("Editing text", "Delete next word"),
        "text_field::DeleteToLineStart" => ("Editing text", "Delete to start of line"),
        "text_field::Left" => ("Editing text", "Move left"),
        "text_field::Right" => ("Editing text", "Move right"),
        "text_field::Up" => ("Editing text", "Move up"),
        "text_field::Down" => ("Editing text", "Move down"),
        "text_field::SelectLeft" => ("Editing text", "Select left"),
        "text_field::SelectRight" => ("Editing text", "Select right"),
        "text_field::SelectUp" => ("Editing text", "Select up"),
        "text_field::SelectDown" => ("Editing text", "Select down"),
        "text_field::WordLeft" => ("Editing text", "Previous word"),
        "text_field::WordRight" => ("Editing text", "Next word"),
        "text_field::SelectWordLeft" => ("Editing text", "Select previous word"),
        "text_field::SelectWordRight" => ("Editing text", "Select next word"),
        "text_field::LineStart" => ("Editing text", "Start of line"),
        "text_field::LineEnd" => ("Editing text", "End of line"),
        "text_field::SelectLineStart" => ("Editing text", "Select to start of line"),
        "text_field::SelectLineEnd" => ("Editing text", "Select to end of line"),
        "text_field::DocStart" => ("Editing text", "Start of text"),
        "text_field::DocEnd" => ("Editing text", "End of text"),
        "text_field::SelectDocStart" => ("Editing text", "Select to start of text"),
        "text_field::SelectDocEnd" => ("Editing text", "Select to end of text"),
        "text_field::SelectAll" => ("Editing text", "Select all"),
        "text_field::Copy" => ("Editing text", "Copy"),
        "text_field::Cut" => ("Editing text", "Cut"),
        "text_field::Paste" => ("Editing text", "Paste"),
        "text_field::Undo" => ("Editing text", "Undo"),
        "text_field::Redo" => ("Editing text", "Redo"),
        "text_field::Enter" => ("Editing text", "Submit"),
        "text_field::NewLine" => ("Editing text", "Insert a line"),
        "text_field::Cancel" => ("Editing text", "Cancel editing"),
        "text_field::ShowCharacterPalette" => ("Editing text", "Emoji and symbols"),
        _ => return None,
    })
}

/// Where a binding works, without the keymap's internal predicate syntax.
pub fn description(context: &str) -> &'static str {
    if context.contains("MentionList") { "While choosing a suggestion" }
    else if context.contains("Composer") { "While writing a message" }
    else if context.contains("KuvoMenu") { "While a menu is open" }
    else if context.contains("QueueReferences") { "While choosing an item reference" }
    else if context.contains("QueueEditor") { "While editing a Work Queue item" }
    else if context.contains("WakeSchedule") { "In the wake-up schedule" }
    else if context.contains("Sidebar") { "In the sidebar" }
    else if context.contains("!KuvoTextField") { "Outside text fields" }
    else if context.contains("KuvoTextField") { "While editing text" }
    else if context.contains("Files") { "In Files" }
    else if context.contains("Changes") { "In Changes" }
    else if context.contains("Sidebar") { "In the sidebar" }
    else if context.is_empty() { "Anywhere in Kuvo" }
    else { "In the main window" }
}
