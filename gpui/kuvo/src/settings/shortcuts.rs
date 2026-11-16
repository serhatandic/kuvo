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
    let mut rows: Vec<_> = keymap.bindings().map(|binding| {
        let name = binding.action().name();
        let (namespace, action) = name.rsplit_once("::").unwrap_or(("Kuvo", name));
        let area = match namespace.rsplit("::").next().unwrap_or(namespace) {
            "area" => "Navigation".into(), "app_menu" => "App menu".into(),
            name => words(name),
        };
        Shortcut {
            area,
            title: words(action),
            keys: binding.keystrokes().iter().map(|key| key.inner().unparse(gpui::OperatingSystem::Mac)).collect::<Vec<_>>().join(" "),
            context: binding.predicate().map(|p| p.to_string()).unwrap_or_default(),
        }
    }).collect();
    rows.sort_by(|a, b| (&a.area, &a.title, &a.keys, &a.context).cmp(&(&b.area, &b.title, &b.keys, &b.context)));
    rows
}

fn words(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch == '_' { out.push(' '); }
        else { if i > 0 && ch.is_uppercase() { out.push(' '); } out.push(ch); }
    }
    if let Some(first) = out.get_mut(..1) { first.make_ascii_uppercase(); }
    out
}

/// Where a binding works, without the keymap's internal predicate syntax.
pub fn description(context: &str) -> &'static str {
    if context.contains("ComposerSlash") { "While choosing a command" }
    else if context.contains("Composer") { "While writing a message" }
    else if context.contains("KuvoMenu") { "While a menu is open" }
    else if context.contains("KuvoTextField") { "While editing text" }
    else if context.contains("Files") { "In Files" }
    else if context.contains("Changes") { "In Changes" }
    else if context.contains("Sidebar") { "In the sidebar" }
    else if context.is_empty() { "Anywhere in Kuvo" }
    else { "In the main window" }
}
