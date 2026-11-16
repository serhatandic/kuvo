//! Your settings: this app's own, on this device (Settings' "Your settings" side). One small typed
//! file keeps them all, `settings.json` in the app's own folder (`keeper::folder_of`:
//! `~/.kuvo-code/desktop`), one object per page:
//!
//! ```json
//! {"appearance": {"theme": "dark"}, "general": {"editor": "dev.zed.Zed"},
//!  "notifications": {"sessions": true, "sound": true, "dock": true}}
//! ```
//!
//! Nothing here is a computer's: those are kept by each computer's Kuvo. A key left out is its
//! default; a page that can't be read is its defaults (the rest is read as it is); a file that
//! can't be read at all is all defaults, and the app opens as usual. What this app doesn't know (a
//! newer app's keys) is kept as it was when it writes.
//!
//! How to add a page: `.claude/notes/yours-settings-pattern.md`.

pub mod editors;
#[cfg(target_os = "macos")]
mod mac;

use std::path::{Path, PathBuf};

use gpui::{App, BorrowAppContext as _, Global};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The file's name in the app's folder.
pub const FILE: &str = "settings.json";

/// Light or dark, as picked in Appearance (none: as the Mac is when the app opens).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
}

/// Appearance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Option<Theme>,
}

/// General.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// The app code and text open in from Files, by its bundle identifier (none: the Mac's
    /// default text editor).
    pub editor: Option<String>,
}

/// Notifications: whether this device shows what your computers tell (each computer decides
/// what's news), with a sound, and the Dock's count. Each on unless turned off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    /// A session finished, or asks you something, while Kuvo isn't in front.
    pub sessions: bool,
    pub sound: bool,
    /// What waits for you, counted on the Dock icon.
    pub dock: bool,
}

impl Default for Notifications {
    fn default() -> Notifications {
        Notifications { sessions: true, sound: true, dock: true }
    }
}

/// All of your settings, a field a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Yours {
    pub appearance: Appearance,
    pub general: General,
    pub notifications: Notifications,
}

/// One change to your settings, as a row makes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Theme(Theme),
    Editor(Option<String>),
    Sessions(bool),
    Sound(bool),
    Dock(bool),
}

impl Change {
    fn apply(self, yours: &mut Yours) {
        match self {
            Change::Theme(theme) => yours.appearance.theme = Some(theme),
            Change::Editor(editor) => yours.general.editor = editor,
            Change::Sessions(on) => yours.notifications.sessions = on,
            Change::Sound(on) => yours.notifications.sound = on,
            Change::Dock(on) => yours.notifications.dock = on,
        }
    }
}

/// A page's object in the file: read on its own, so one that can't be read is its defaults alone.
fn page<T: DeserializeOwned + Default>(file: &Value, key: &str) -> T {
    file.get(key).cloned().and_then(|page| serde_json::from_value(page).ok()).unwrap_or_default()
}

/// Your settings as this app keeps them: where (none: only while it runs, as in tests and the
/// stand-in), what they are now, and the file as it was read, so what this app doesn't know stays.
pub struct Store {
    path: Option<PathBuf>,
    now: Yours,
    file: Map<String, Value>,
    /// Why the last change couldn't be written, where it couldn't.
    problem: Option<String>,
}

impl Global for Store {}

impl Store {
    /// Reads the file at `path`: what it can't read is the defaults, never an error.
    pub fn read(path: Option<PathBuf>) -> Store {
        let file = path.as_deref().and_then(|path| std::fs::read(path).ok()).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        });
        let file = file.unwrap_or_default();
        let value = Value::Object(file.clone());
        let now = Yours { appearance: page(&value, "appearance"), general: page(&value, "general"), notifications: page(&value, "notifications") };
        Store { path, now, file, problem: None }
    }

    pub fn now(&self) -> &Yours {
        &self.now
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Makes `change`, and writes the file (whole, through a file beside it, so a stop halfway
    /// leaves the one before).
    pub fn change(&mut self, change: Change) {
        change.apply(&mut self.now);
        self.problem = self.write().err();
    }

    fn write(&mut self) -> Result<(), String> {
        let pages = [("appearance", serde_json::to_value(&self.now.appearance)), ("general", serde_json::to_value(&self.now.general)), ("notifications", serde_json::to_value(&self.now.notifications))];
        for (key, page) in pages {
            let Ok(Value::Object(page)) = page else { continue };
            let kept = self.file.entry(key).or_insert_with(|| Value::Object(Map::new()));
            if !kept.is_object() {
                *kept = Value::Object(Map::new());
            }
            if let Value::Object(kept) = kept {
                for (field, value) in page {
                    // (A default left out reads as itself again: none is written as null.)
                    kept.insert(field, value);
                }
            }
        }
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_string_pretty(&self.file).map_err(|error| error.to_string())?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let beside = path.with_extension("json.new");
        std::fs::write(&beside, text).map_err(|error| error.to_string())?;
        std::fs::rename(&beside, path).map_err(|error| error.to_string())
    }
}

/// Reads your settings from `path` (none: kept only while the app runs), for every window.
pub fn init(path: Option<PathBuf>, cx: &mut App) {
    cx.set_global(Store::read(path));
}

/// Your settings now (the defaults where nothing read them yet).
pub fn now(cx: &App) -> Yours {
    cx.try_global::<Store>().map(|store| store.now.clone()).unwrap_or_default()
}

/// Makes a change to your settings and keeps it; whatever follows them hears it
/// (`cx.observe_global::<Store>`).
pub fn change(change: Change, cx: &mut App) {
    if !cx.has_global::<Store>() {
        init(None, cx);
    }
    cx.update_global::<Store, _>(|store, _| store.change(change));
}

/// Light or dark, for every window, kept as your pick.
pub fn set_theme(appearance: kuvo_ui::Appearance, cx: &mut App) {
    kuvo_ui::Theme::set_appearance(appearance, cx);
    change(Change::Theme(if appearance.is_dark() { Theme::Dark } else { Theme::Light }), cx);
}

/// The appearance a window opens in: your pick, else the Mac's (`system`).
pub fn appearance(system: kuvo_ui::Appearance, cx: &App) -> kuvo_ui::Appearance {
    match now(cx).appearance.theme {
        Some(Theme::Light) => kuvo_ui::Appearance::Light,
        Some(Theme::Dark) => kuvo_ui::Appearance::Dark,
        None => system,
    }
}

/// What only this device's system does for your settings: notifications, the Dock's count, and
/// this app's identifier for System Settings. Nothing where the system has none.
pub mod desk {
    /// The count on the Dock icon (none at 0).
    pub fn set_dock_count(count: usize) {
        #[cfg(target_os = "macos")]
        super::mac::set_dock_count(count);
        #[cfg(not(target_os = "macos"))]
        let _ = count;
    }

    /// Posts a notification (`sound`: the system's notification sound with it), off the UI's
    /// thread; where the system has none of its own, through GPUI.
    pub fn post(tag: String, title: String, body: String, sound: bool, cx: &mut gpui::App) {
        #[cfg(target_os = "macos")]
        {
            use gpui::AppContext as _;
            let _ = cx;
            cx.background_spawn(async move { super::mac::post(&tag, &title, &body, sound) }).detach();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = sound;
            cx.show_system_notification(gpui::SystemNotification { tag: tag.into(), title: title.into(), body: body.into(), actions: Vec::new() });
        }
    }

    /// Whether Kuvo is the app in front (everywhere else: whether a window is, as the window says).
    pub fn app_active() -> bool {
        #[cfg(target_os = "macos")]
        return super::mac::app_active();
        #[cfg(not(target_os = "macos"))]
        true
    }

    /// Where System Settings shows this app's notifications.
    pub fn notification_settings() -> String {
        let base = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";
        #[cfg(target_os = "macos")]
        if let Some(id) = super::mac::bundle_id() {
            return format!("{base}?id={id}");
        }
        base.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn nothing_kept_yet_is_every_default() {
        let folder = folder();
        let store = Store::read(Some(folder.path().join(FILE)));
        assert_eq!(store.now(), &Yours::default());
        assert!(store.now().notifications.sessions && store.now().notifications.sound && store.now().notifications.dock);
        assert_eq!(store.now().appearance.theme, None);
    }

    #[test]
    fn a_change_is_kept_and_read_again_by_the_next_run() {
        let folder = folder();
        let path = folder.path().join("desktop").join(FILE);
        let mut store = Store::read(Some(path.clone()));
        store.change(Change::Theme(Theme::Dark));
        store.change(Change::Editor(Some("dev.zed.Zed".into())));
        store.change(Change::Sound(false));
        assert_eq!(store.problem(), None);
        let again = Store::read(Some(path.clone()));
        assert_eq!(again.now().appearance.theme, Some(Theme::Dark));
        assert_eq!(again.now().general.editor.as_deref(), Some("dev.zed.Zed"));
        assert!(!again.now().notifications.sound && again.now().notifications.sessions);
        // Back to the default text editor: kept as none.
        let mut again = again;
        again.change(Change::Editor(None));
        assert_eq!(Store::read(Some(path)).now().general.editor, None);
    }

    #[test]
    fn a_file_that_cant_be_read_is_the_defaults_and_a_change_writes_a_good_one() {
        let folder = folder();
        let path = folder.path().join(FILE);
        for bad in ["{not json", "[1, 2]", "", "null"] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(Store::read(Some(path.clone())).now(), &Yours::default(), "{bad:?}");
        }
        let mut store = Store::read(Some(path.clone()));
        store.change(Change::Dock(false));
        assert!(!Store::read(Some(path)).now().notifications.dock);
    }

    #[test]
    fn a_page_that_cant_be_read_is_its_defaults_and_the_others_stay() {
        let folder = folder();
        let path = folder.path().join(FILE);
        std::fs::write(&path, r#"{"appearance": {"theme": "purple"}, "notifications": {"sound": false}}"#).unwrap();
        let store = Store::read(Some(path));
        assert_eq!(store.now().appearance.theme, None);
        assert!(!store.now().notifications.sound && store.now().notifications.sessions, "a key left out is its default");
    }

    #[test]
    fn what_this_app_doesnt_know_stays_as_it_was() {
        let folder = folder();
        let path = folder.path().join(FILE);
        std::fs::write(&path, r#"{"voice": {"style": "field"}, "notifications": {"which": "this"}}"#).unwrap();
        let mut store = Store::read(Some(path.clone()));
        store.change(Change::Sessions(false));
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["voice"]["style"], "field");
        assert_eq!(file["notifications"]["which"], "this");
        assert_eq!(file["notifications"]["sessions"], false);
    }

    #[test]
    fn a_folder_it_cant_write_says_why_and_the_change_holds_while_it_runs() {
        let folder = folder();
        // (A file where its folder would be.)
        std::fs::write(folder.path().join("desktop"), "").unwrap();
        let mut store = Store::read(Some(folder.path().join("desktop").join(FILE)));
        store.change(Change::Sound(false));
        assert!(store.problem().is_some());
        assert!(!store.now().notifications.sound);
    }
}
