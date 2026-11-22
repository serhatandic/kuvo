//! What a key sends to a page: its `key`, `code`, the text it types and the key code Chrome edits
//! by, down then up, as CDP's `Input.dispatchKeyEvent` takes them. Keys are sent the same to every
//! computer, as on Linux (PROTOCOL.md, "The browser's live view"; CONTRIBUTING.md, "The browser on
//! a machine"): Control for ⌘ in the keys a page takes from it (⌘A, ⌘C, ⌘X, ⌘Z), Home and End for
//! ⌘← and ⌘→, Control for Option's word moves. The keeper makes them a Mac's where Chrome runs on
//! one. Every other ⌘ key is the app's; ⌘V pastes this Mac's clipboard, which the view does itself.

use gpui::Keystroke;
use kuvo_box::browser_view::{Key, KeyAction, Modifiers};

/// A named key: its CDP `key`, `code`, Windows key code, and the text it types.
fn named(key: &str) -> Option<(&'static str, &'static str, u32, Option<&'static str>)> {
    Some(match key {
        "enter" => ("Enter", "Enter", 13, Some("\r")),
        "backspace" => ("Backspace", "Backspace", 8, None),
        "tab" => ("Tab", "Tab", 9, None),
        "escape" => ("Escape", "Escape", 27, None),
        "delete" => ("Delete", "Delete", 46, None),
        "left" => ("ArrowLeft", "ArrowLeft", 37, None),
        "up" => ("ArrowUp", "ArrowUp", 38, None),
        "right" => ("ArrowRight", "ArrowRight", 39, None),
        "down" => ("ArrowDown", "ArrowDown", 40, None),
        "home" => ("Home", "Home", 36, None),
        "end" => ("End", "End", 35, None),
        "pageup" => ("PageUp", "PageUp", 33, None),
        "pagedown" => ("PageDown", "PageDown", 34, None),
        "space" => (" ", "Space", 32, Some(" ")),
        "f1" => ("F1", "F1", 112, None),
        "f2" => ("F2", "F2", 113, None),
        "f3" => ("F3", "F3", 114, None),
        "f4" => ("F4", "F4", 115, None),
        "f5" => ("F5", "F5", 116, None),
        "f6" => ("F6", "F6", 117, None),
        "f7" => ("F7", "F7", 118, None),
        "f8" => ("F8", "F8", 119, None),
        "f9" => ("F9", "F9", 120, None),
        "f10" => ("F10", "F10", 121, None),
        "f11" => ("F11", "F11", 122, None),
        "f12" => ("F12", "F12", 123, None),
        _ => return None,
    })
}

/// The physical key's `code` and Windows key code, for a key that types a character: letters and
/// digits, and the punctuation of a US layout.
fn printed(key: &str) -> (String, Option<u32>) {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return (String::new(), None) };
    if c.is_ascii_alphabetic() {
        let upper = c.to_ascii_uppercase();
        return (format!("Key{upper}"), Some(upper as u32));
    }
    if c.is_ascii_digit() {
        return (format!("Digit{c}"), Some(c as u32));
    }
    let (code, key_code) = match c {
        '-' => ("Minus", 189),
        '=' => ("Equal", 187),
        '[' => ("BracketLeft", 219),
        ']' => ("BracketRight", 221),
        '\\' => ("Backslash", 220),
        ';' => ("Semicolon", 186),
        '\'' => ("Quote", 222),
        ',' => ("Comma", 188),
        '.' => ("Period", 190),
        '/' => ("Slash", 191),
        '`' => ("Backquote", 192),
        _ => return (String::new(), None),
    };
    (code.to_owned(), Some(key_code))
}

/// A key down and up.
fn pressed(key: &str, code: &str, key_code: Option<u32>, text: Option<String>, modifiers: Modifiers) -> Vec<Key> {
    let down = if text.is_some() { KeyAction::Down } else { KeyAction::RawDown };
    vec![
        Key { action: down, key: key.to_owned(), code: code.to_owned(), text, key_code, modifiers },
        Key { action: KeyAction::Up, key: key.to_owned(), code: code.to_owned(), text: None, key_code, modifiers },
    ]
}

/// What `keystroke` sends to the page, down then up; none for one that's the app's (most of ⌘'s,
/// Control's, a modifier alone).
pub(crate) fn sent(keystroke: &Keystroke) -> Option<Vec<Key>> {
    let m = keystroke.modifiers;
    let key = keystroke.key.as_str();
    if keystroke.is_modifier_key() {
        return None;
    }
    let held = Modifiers { alt: false, control: false, meta: false, shift: m.shift };
    if m.platform {
        if m.control || m.alt {
            return None;
        }
        let control = Modifiers { control: true, ..held };
        let (named_as, modifiers) = match key {
            // ⌘← and ⌘→ go to the line's ends; ⌘↑ and ⌘↓ to the page's.
            "left" => ("home", held),
            "right" => ("end", held),
            "up" => ("home", control),
            "down" => ("end", control),
            "a" | "c" | "x" | "z" => {
                let (code, key_code) = printed(key);
                return Some(pressed(key, &code, key_code, None, control));
            }
            _ => return None,
        };
        let (key, code, key_code, _) = named(named_as)?;
        return Some(pressed(key, code, Some(key_code), None, modifiers));
    }
    if m.control || m.function {
        return None;
    }
    if let Some((name, code, key_code, text)) = named(key) {
        // Option's word moves and deletes, as Control's on Linux.
        let word = m.alt && matches!(key, "left" | "right" | "backspace" | "delete");
        if m.alt && !word {
            return None;
        }
        let modifiers = Modifiers { control: word, ..held };
        return Some(pressed(name, code, Some(key_code), text.map(str::to_owned), modifiers));
    }
    // A character: the one this keyboard types (Option's own, Shift's), as its text.
    // (A key with a name of its own, not in the table, types nothing: never its name.)
    let typed = keystroke.key_char.as_deref().filter(|text| !text.is_empty() && !text.chars().any(char::is_control) && !(key.chars().count() > 1 && *text == key))?;
    let (code, key_code) = printed(key);
    Some(pressed(typed, &code, key_code, Some(typed.to_owned()), Modifiers::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(source: &'static str) -> Option<Vec<Key>> {
        sent(&Keystroke::parse(source).unwrap().with_simulated_ime())
    }

    #[test]
    fn a_character_is_typed_as_its_text_down_then_up() {
        let a = keys("A").unwrap();
        assert_eq!((a[0].action, a[0].key.as_str(), a[0].code.as_str(), a[0].text.as_deref(), a[0].key_code), (KeyAction::Down, "A", "KeyA", Some("A"), Some(65)));
        assert_eq!((a[1].action, a[1].text.as_deref()), (KeyAction::Up, None));
        assert_eq!(a[0].modifiers, Modifiers::default(), "Shift is in the character");
        // Option's own character on this keyboard, as typed.
        let typed = sent(&Keystroke { modifiers: gpui::Modifiers { alt: true, ..Default::default() }, key: "s".into(), key_char: Some("ß".into()) }).unwrap();
        assert_eq!((typed[0].key.as_str(), typed[0].text.as_deref()), ("ß", Some("ß")));
    }

    #[test]
    fn named_keys_carry_the_key_code_chrome_edits_by() {
        let enter = keys("enter").unwrap();
        assert_eq!((enter[0].key.as_str(), enter[0].text.as_deref(), enter[0].key_code), ("Enter", Some("\r"), Some(13)));
        let back = keys("backspace").unwrap();
        assert_eq!((back[0].action, back[0].key_code, back[0].text.as_deref()), (KeyAction::RawDown, Some(8), None));
        assert_eq!(keys("shift-tab").unwrap()[0].modifiers, Modifiers { shift: true, ..Modifiers::default() });
        assert_eq!(keys("alt-left").unwrap()[0].modifiers, Modifiers { control: true, ..Modifiers::default() }, "a word left, as on Linux");
    }

    #[test]
    fn the_keys_a_page_takes_from_cmd_go_as_control_and_the_rest_stay_the_apps() {
        let select_all = keys("cmd-a").unwrap();
        assert_eq!((select_all[0].key.as_str(), select_all[0].text.as_deref()), ("a", None));
        assert_eq!(select_all[0].modifiers, Modifiers { control: true, ..Modifiers::default() });
        assert_eq!(keys("cmd-left").unwrap()[0].key, "Home");
        assert_eq!(keys("cmd-down").unwrap()[0].modifiers, Modifiers { control: true, ..Modifiers::default() });
        assert_eq!(keys("cmd-shift-right").unwrap()[0].modifiers, Modifiers { shift: true, ..Modifiers::default() });
        for apps in ["cmd-w", "cmd-t", "cmd-l", "cmd-v", "ctrl-a", "shift"] {
            assert_eq!(keys(apps), None, "{apps}");
        }
    }
}
