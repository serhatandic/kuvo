//! Your settings: this app's own, on this device (Settings' "Your settings" side). One small typed
//! file keeps them all, `settings.json` in the app's own folder (`keeper::folder_of`:
//! `~/.kuvo-code/desktop`), one object per page:
//!
//! ```json
//! {"appearance": {"theme": "dark", "textSize": "large"}, "general": {"editor": "dev.zed.Zed"},
//!  "notifications": {"sessions": true, "sound": true, "dock": true}, "quitNoteSeen": true}
//! ```
//!
//! (And some that are no page's: whether the first Quit's note was seen, `quit_note`; and where
//! the window was, `place`, so it opens there again: [`Place`].)
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

/// The file's key for whether the first Quit's note was seen.
const QUIT_NOTE_SEEN: &str = "quitNoteSeen";

/// Light, dark, or as the system is (following it as it changes), as picked in Appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    System,
}

/// How big the app's text is, as picked in Appearance (see [`kuvo_ui::TextSize`] for the sizes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSize {
    pub const ALL: [TextSize; 4] = [TextSize::Small, TextSize::Default, TextSize::Large, TextSize::Larger];

    pub fn ui(self) -> kuvo_ui::TextSize {
        match self {
            TextSize::Small => kuvo_ui::TextSize::Small,
            TextSize::Default => kuvo_ui::TextSize::Default,
            TextSize::Large => kuvo_ui::TextSize::Large,
            TextSize::Larger => kuvo_ui::TextSize::Larger,
        }
    }
}

/// Appearance.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// None (as nothing was picked yet): as the system is.
    #[serde(deserialize_with = "or_default")]
    pub theme: Option<Theme>,
    #[serde(rename = "textSize", deserialize_with = "or_default")]
    pub text_size: TextSize,
}

/// A field read on its own: one this app can't read (a newer app's choice) is its default, and
/// the page's other fields are still read. (A change on the page then writes that default.)
fn or_default<'de, D: serde::Deserializer<'de>, T: DeserializeOwned + Default>(deserializer: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(deserializer)?).unwrap_or_default())
}

impl Appearance {
    /// Whether the app's colours follow the system's light or dark.
    pub fn follows_system(&self) -> bool {
        matches!(self.theme, None | Some(Theme::System))
    }
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

/// The app's anonymous usage preference. No telemetry is sent yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Privacy {
    pub share_anonymous_usage: bool,
}
impl Default for Privacy {
    fn default() -> Self { Self { share_anonymous_usage: true } }
}

/// The first view in a newly opened side panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelStart {
    #[default]
    Changes,
    Files,
    Branches,
}

impl PanelStart {
    pub const ALL: [Self; 3] = [Self::Changes, Self::Files, Self::Branches];

    pub fn title(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Files => "Files",
            Self::Branches => "Branches",
        }
    }
}

/// Side panel choices on this device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Panel {
    #[serde(rename = "startsWith", deserialize_with = "or_default")]
    pub starts_with: PanelStart,
}

/// Where the window was, kept as you go (so a quit, a closed window or a crash leaves it), to
/// open there again: the computer it showed, whether the area's sidebar was folded away, and on
/// each computer the area in front and what was open in each area. Nothing here is a computer's:
/// it's only where this device's window was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Place {
    /// The computer shown, by its key (none: this computer's own, else the first it reaches).
    #[serde(deserialize_with = "or_default")]
    pub computer: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub sidebar_folded: bool,
    /// Each computer's, by its key ([`NEAREST`] for this computer's own).
    #[serde(deserialize_with = "or_default")]
    pub computers: std::collections::BTreeMap<String, Spot>,
}

/// The key this computer's own box's [`Spot`] is kept under (its key may not be known yet).
pub const NEAREST: &str = "nearest";

/// Where a computer's window was: the area in front (`sessions`, `bots`, `queue`, `computers`, as
/// [`crate::area::Area::slug`] names them), the conversation open in Sessions, the bot whose
/// conversation was open in Bots, and the row picked in each other area, by the area's slug.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spot {
    #[serde(deserialize_with = "or_default")]
    pub area: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub session: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub bot: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub picked: std::collections::BTreeMap<String, String>,
}

/// All of your settings, a field a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Yours {
    pub queue_last_opened: std::collections::BTreeMap<String, f64>,
    pub appearance: Appearance,
    pub general: General,
    pub panel: Panel,
    pub notifications: Notifications,
    pub privacy: Privacy,
    /// The first Quit's note was seen on this device (`quit_note`).
    pub quit_note_seen: bool,
    pub place: Place,
}

/// One change to your settings, as a row makes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Theme(Theme),
    TextSize(TextSize),
    Editor(Option<String>),
    PanelStart(PanelStart),
    Sessions(bool),
    Sound(bool),
    Dock(bool),
    ShareAnonymousUsage(bool),
    QuitNoteSeen,
    QueueOpened(String, f64),
    /// The computer the window shows now (none: this computer's own).
    ShownComputer(Option<String>),
    SidebarFolded(bool),
    /// Where a computer's window is now, by its key.
    Spot(String, Spot),
}

impl Change {
    fn apply(self, yours: &mut Yours) {
        match self {
            Change::Theme(theme) => yours.appearance.theme = Some(theme),
            Change::TextSize(size) => yours.appearance.text_size = size,
            Change::Editor(editor) => yours.general.editor = editor,
            Change::PanelStart(start) => yours.panel.starts_with = start,
            Change::Sessions(on) => yours.notifications.sessions = on,
            Change::Sound(on) => yours.notifications.sound = on,
            Change::Dock(on) => yours.notifications.dock = on,
            Change::ShareAnonymousUsage(on) => yours.privacy.share_anonymous_usage = on,
            Change::QueueOpened(computer, at) => { yours.queue_last_opened.insert(computer, at); }
            Change::QuitNoteSeen => yours.quit_note_seen = true,
            Change::ShownComputer(computer) => yours.place.computer = computer,
            Change::SidebarFolded(folded) => yours.place.sidebar_folded = folded,
            Change::Spot(computer, spot) => {
                yours.place.computers.insert(computer, spot);
            }
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
        let quit_note_seen = value.get(QUIT_NOTE_SEEN).and_then(Value::as_bool).unwrap_or(false);
        let now = Yours { queue_last_opened: page(&value, "queueLastOpened"), appearance: page(&value, "appearance"), general: page(&value, "general"), panel: page(&value, "panel"), notifications: page(&value, "notifications"), privacy: page(&value, "privacy"), quit_note_seen, place: page(&value, "place") };
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
        let pages = [("panel", serde_json::to_value(&self.now.panel)), ("privacy", serde_json::to_value(&self.now.privacy)), ("appearance", serde_json::to_value(&self.now.appearance)), ("general", serde_json::to_value(&self.now.general)), ("notifications", serde_json::to_value(&self.now.notifications))];
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
        self.file.insert("queueLastOpened".into(), serde_json::to_value(&self.now.queue_last_opened).unwrap());
        self.file.insert("place".into(), serde_json::to_value(&self.now.place).unwrap());
        if self.now.quit_note_seen {
            self.file.insert(QUIT_NOTE_SEEN.into(), Value::Bool(true));
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

/// Reads your settings from `path` (none: kept only while the app runs), for every window, and
/// sets the text size they pick (the theme is set as each window opens: [`appearance`]).
pub fn init(path: Option<PathBuf>, cx: &mut App) {
    let store = Store::read(path);
    let size = store.now.appearance.text_size.ui();
    cx.set_global(store);
    kuvo_ui::TextSize::set(size, cx);
}

/// Your settings now (the defaults where nothing read them yet).
pub fn now(cx: &App) -> Yours {
    cx.try_global::<Store>().map(|store| store.now.clone()).unwrap_or_default()
}

/// Makes a change to your settings and keeps it, and makes what it changes for every window at
/// once (the theme, the text size); whatever else follows them hears it
/// (`cx.observe_global::<Store>`).
pub fn change(change: Change, cx: &mut App) {
    if !cx.has_global::<Store>() {
        init(None, cx);
    }
    // (Where the window is is kept as you go: written only when it moved.)
    let place = &cx.global::<Store>().now.place;
    let same = match &change {
        Change::ShownComputer(computer) => &place.computer == computer,
        Change::SidebarFolded(folded) => place.sidebar_folded == *folded,
        Change::Spot(computer, spot) => place.computers.get(computer) == Some(spot),
        _ => false,
    };
    if same {
        return;
    }
    let effect = change.clone();
    cx.update_global::<Store, _>(|store, _| store.change(change));
    match effect {
        Change::Theme(theme) => {
            let system = kuvo_ui::Appearance::of_system(cx.window_appearance());
            let appearance = match theme {
                Theme::Light => kuvo_ui::Appearance::Light,
                Theme::Dark => kuvo_ui::Appearance::Dark,
                Theme::System => system,
            };
            show_in(appearance, cx);
        }
        Change::TextSize(size) => kuvo_ui::TextSize::set(size.ui(), cx),
        _ => {}
    }
}

/// Light or dark, for every window, kept as your pick (⌘⇧L: from System too).
pub fn set_theme(appearance: kuvo_ui::Appearance, cx: &mut App) {
    change(Change::Theme(if appearance.is_dark() { Theme::Dark } else { Theme::Light }), cx);
}

/// The appearance a window opens in: your pick, else the system's (`system`, the window's).
pub fn appearance(system: kuvo_ui::Appearance, cx: &App) -> kuvo_ui::Appearance {
    match now(cx).appearance.theme {
        Some(Theme::Light) => kuvo_ui::Appearance::Light,
        Some(Theme::Dark) => kuvo_ui::Appearance::Dark,
        None | Some(Theme::System) => system,
    }
}

/// Follows the system's light or dark in `window` as it changes, while your theme is System. Call
/// as each window opens.
pub fn follow_system(window: &mut gpui::Window) {
    window.observe_window_appearance(|window, cx| system_changed(kuvo_ui::Appearance::of(window), cx)).detach();
}

/// The system turned light or dark (`system`): every window follows it while your theme is System.
pub fn system_changed(system: kuvo_ui::Appearance, cx: &mut App) {
    if now(cx).appearance.follows_system() {
        show_in(system, cx);
    }
}

/// Every window in `appearance`, where it isn't yet.
fn show_in(appearance: kuvo_ui::Appearance, cx: &mut App) {
    if cx.try_global::<kuvo_ui::Theme>().map(|theme| theme.appearance) != Some(appearance) {
        kuvo_ui::Theme::set_appearance(appearance, cx);
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
    fn appearance_follows_the_system_at_the_default_size_until_picked_and_keeps_its_picks() {
        let folder = folder();
        let path = folder.path().join(FILE);
        // Nothing picked, or a file from before System was a choice (none written as null).
        for file in ["{}", r#"{"appearance": {"theme": null}}"#] {
            std::fs::write(&path, file).unwrap();
            let now = Store::read(Some(path.clone())).now().appearance.clone();
            assert!(now.follows_system(), "{file}");
            assert_eq!(now.text_size, TextSize::Default);
        }
        let mut store = Store::read(Some(path.clone()));
        store.change(Change::TextSize(TextSize::Large));
        store.change(Change::Theme(Theme::Light));
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["appearance"], serde_json::json!({"theme": "light", "textSize": "large"}));
        store.change(Change::Theme(Theme::System));
        let again = Store::read(Some(path.clone())).now().appearance.clone();
        assert!(again.follows_system() && again.text_size == TextSize::Large);
        // A size this app doesn't know (a newer app's) is the default, and the theme stays.
        std::fs::write(&path, r#"{"appearance": {"theme": "dark", "textSize": "huge"}}"#).unwrap();
        let odd = Store::read(Some(path)).now().appearance.clone();
        assert_eq!((odd.theme, odd.text_size), (Some(Theme::Dark), TextSize::Default));
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

/// The newest local item stamp is a cutoff in the queue computer's clock, not this Mac's.
pub fn queue_watermark(items: &[kuvo_box::work_queue::Item]) -> f64 {
    items.iter().filter(|item| item.computer.is_none()).map(|item| item.updated.at).fold(0.0, f64::max)
}

/// No status-entry stamp is supplied by the box; updated is the closest available time.
pub fn queue_badge(items: &[kuvo_box::work_queue::Item], opened: Option<f64>) -> usize {
    items.iter().filter(|item| item.computer.is_none() && item.status == "needs-you" && opened.is_none_or(|at| item.updated.at > at)).count()
}

#[cfg(test)]
mod queue_badge_tests {
    use super::*;

    #[test]
    fn opening_clears_only_the_badge_and_persists_per_computer() {
        let path = std::env::temp_dir().join(format!("kuvo-queue-opened-{}.json", std::process::id()));
        let mut store = Store::read(Some(path.clone()));
        let mut items: Vec<kuvo_box::work_queue::Item> = serde_json::from_value(serde_json::json!([
            {"id":"a", "status":"needs-you", "updated":{"at":10}},
            {"id":"b", "status":"needs-you", "updated":{"at":20}}
        ])).unwrap();
        assert_eq!(queue_watermark(&items), 20.);
        assert_eq!(queue_watermark(&[]), 0.);
        let before = items.clone();
        assert_eq!(queue_badge(&items, None), 2);
        store.change(Change::QueueOpened("one".into(), queue_watermark(&items)));
        let restarted = Store::read(Some(path.clone()));
        let opened = restarted.now().queue_last_opened.get("one").copied();
        assert_eq!(queue_badge(&items, opened), 0);
        assert_eq!(items, before);
        assert_eq!(queue_badge(&items, restarted.now().queue_last_opened.get("two").copied()), 2);
        items[0].updated.at = 21.;
        assert_eq!(queue_badge(&items, opened), 1);
        std::fs::remove_file(path).unwrap();
    }
}
