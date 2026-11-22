//! What a key sends to a terminal's shell, as xterm sends it (what `TERM=xterm-256color`, which
//! the engine sets, promises): arrows and Home/End as the cursor keys' mode says, the modifiers'
//! code on them, function keys, Control's control characters, and Option as Meta (Esc first), but
//! where your keyboard types a character of its own with Option (`@`, `{`, `|` on Turkish and German
//! layouts), that character. ⌘ is the app's: none of it goes to the shell.

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// What `keystroke` sends, in the terminal's `mode`; none for one the app keeps (⌘, a modifier
/// alone).
pub(crate) fn sent(keystroke: &Keystroke, mode: TermMode) -> Option<String> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    // xterm's modifier code: 1, plus Shift 1, Alt 2, Control 4.
    let code = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let modified = code > 1;
    let cursor = |letter: char| {
        Some(if modified {
            format!("\x1b[1;{code}{letter}")
        } else if app_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        })
    };
    let tilde = |number: u8| Some(if modified { format!("\x1b[{number};{code}~") } else { format!("\x1b[{number}~") });
    let meta = |text: &str| if m.alt { format!("\x1b{text}") } else { text.to_owned() };
    let key = keystroke.key.as_str();
    match key {
        "up" => return cursor('A'),
        "down" => return cursor('B'),
        "right" => return cursor('C'),
        "left" => return cursor('D'),
        "home" => return cursor('H'),
        "end" => return cursor('F'),
        "insert" => return tilde(2),
        "delete" => return tilde(3),
        "pageup" => return tilde(5),
        "pagedown" => return tilde(6),
        "enter" => return Some(meta("\r")),
        "escape" => return Some(meta("\x1b")),
        "tab" if m.shift => return Some("\x1b[Z".to_owned()),
        "tab" => return Some(meta("\t")),
        // Control-Backspace, as the shells' word rubout takes it; Backspace is DEL.
        "backspace" if m.control => return Some(meta("\x08")),
        "backspace" => return Some(meta("\x7f")),
        "space" if m.control => return Some(meta("\0")),
        _ => {}
    }
    if let Some(number) = key.strip_prefix('f').and_then(|number| number.parse::<u8>().ok()) {
        return function(number, code);
    }
    if m.control {
        return control(key).map(|byte| meta(&(byte as char).to_string()));
    }
    let typed = keystroke.key_char.as_deref();
    if m.alt {
        // Option types a character of its own on this keyboard (plain ASCII, not the key's own):
        // that character. Otherwise it's Meta: Esc, then the key (shifted, where Shift is down).
        if let Some(own) = typed.filter(|own| own.len() == 1 && own.bytes().all(|byte| (0x20..0x7f).contains(&byte)) && *own != key && !own.eq_ignore_ascii_case(key)) {
            return Some(own.to_owned());
        }
        let base = if key == "space" { " ".to_owned() } else if m.shift { key.to_uppercase() } else { key.to_owned() };
        return (base.chars().count() == 1).then(|| format!("\x1b{base}"));
    }
    match typed {
        Some(text) if !text.is_empty() => Some(text.to_owned()),
        _ if key == "space" => Some(" ".to_owned()),
        _ => None,
    }
}

/// F1–F12, as xterm sends them, with the modifiers' code.
fn function(number: u8, code: u8) -> Option<String> {
    let modified = code > 1;
    let letter = match number {
        1 => Some('P'),
        2 => Some('Q'),
        3 => Some('R'),
        4 => Some('S'),
        _ => None,
    };
    if let Some(letter) = letter {
        return Some(if modified { format!("\x1b[1;{code}{letter}") } else { format!("\x1bO{letter}") });
    }
    let tilde = match number {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return None,
    };
    Some(if modified { format!("\x1b[{tilde};{code}~") } else { format!("\x1b[{tilde}~") })
}

/// Control and a key: its control character, as a terminal has them.
fn control(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(only), None) = (chars.next(), chars.next()) else { return None };
    Some(match only.to_ascii_lowercase() {
        letter @ 'a'..='z' => letter as u8 - b'a' + 1,
        '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: &'static str) -> Keystroke {
        Keystroke::parse(source).unwrap()
    }

    fn says(source: &'static str) -> Option<String> {
        sent(&key(source), TermMode::default())
    }

    #[test]
    fn the_keys_a_shell_reads_send_what_xterm_sends() {
        assert_eq!(says("enter").as_deref(), Some("\r"));
        assert_eq!(says("backspace").as_deref(), Some("\x7f"));
        assert_eq!(says("tab").as_deref(), Some("\t"));
        assert_eq!(says("shift-tab").as_deref(), Some("\x1b[Z"));
        assert_eq!(says("escape").as_deref(), Some("\x1b"));
        assert_eq!(says("up").as_deref(), Some("\x1b[A"));
        assert_eq!(says("left").as_deref(), Some("\x1b[D"));
        assert_eq!(says("delete").as_deref(), Some("\x1b[3~"));
        assert_eq!(says("pageup").as_deref(), Some("\x1b[5~"));
        assert_eq!(says("f1").as_deref(), Some("\x1bOP"));
        assert_eq!(says("f5").as_deref(), Some("\x1b[15~"));
        assert_eq!(says("f12").as_deref(), Some("\x1b[24~"));
        assert_eq!(says("space").as_deref(), Some(" "));
    }

    #[test]
    fn modifiers_go_on_the_cursor_and_function_keys() {
        assert_eq!(says("shift-up").as_deref(), Some("\x1b[1;2A"));
        assert_eq!(says("alt-left").as_deref(), Some("\x1b[1;3D"));
        assert_eq!(says("ctrl-right").as_deref(), Some("\x1b[1;5C"));
        assert_eq!(says("ctrl-f5").as_deref(), Some("\x1b[15;5~"));
        assert_eq!(says("shift-f1").as_deref(), Some("\x1b[1;2P"));
    }

    #[test]
    fn an_application_cursor_mode_sends_the_arrows_as_it_asks() {
        // (vim, less and the like turn it on.)
        assert_eq!(sent(&key("up"), TermMode::APP_CURSOR).as_deref(), Some("\x1bOA"));
        assert_eq!(sent(&key("home"), TermMode::APP_CURSOR).as_deref(), Some("\x1bOH"));
        assert_eq!(sent(&key("shift-up"), TermMode::APP_CURSOR).as_deref(), Some("\x1b[1;2A"));
    }

    #[test]
    fn control_sends_control_characters() {
        assert_eq!(says("ctrl-c").as_deref(), Some("\x03"));
        assert_eq!(says("ctrl-d").as_deref(), Some("\x04"));
        assert_eq!(says("ctrl-a").as_deref(), Some("\x01"));
        assert_eq!(says("ctrl-[").as_deref(), Some("\x1b"));
        assert_eq!(says("ctrl-space").as_deref(), Some("\0"));
        assert_eq!(says("ctrl-alt-b").as_deref(), Some("\x1b\x02"));
    }

    #[test]
    fn option_is_meta_except_where_the_keyboard_types_a_character_with_it() {
        assert_eq!(says("alt-b").as_deref(), Some("\x1bb"), "word back in a shell");
        assert_eq!(says("alt-backspace").as_deref(), Some("\x1b\x7f"));
        // On a US keyboard ⌥B types "∫", which isn't wanted in a shell: Meta.
        let mut integral = key("alt-b");
        integral.key_char = Some("∫".into());
        assert_eq!(sent(&integral, TermMode::default()).as_deref(), Some("\x1bb"));
        // On a Turkish one ⌥Q types "@": that's what's meant.
        let mut at = key("alt-q");
        at.key_char = Some("@".into());
        assert_eq!(sent(&at, TermMode::default()).as_deref(), Some("@"));
    }

    #[test]
    fn typed_characters_go_as_they_are_and_command_keys_stay_the_apps() {
        let mut typed = key("a");
        typed.key_char = Some("a".into());
        assert_eq!(sent(&typed, TermMode::default()).as_deref(), Some("a"));
        let mut accented = key("e");
        accented.key_char = Some("é".into());
        assert_eq!(sent(&accented, TermMode::default()).as_deref(), Some("é"));
        assert_eq!(says("cmd-c"), None);
        assert_eq!(says("cmd-v"), None);
    }
}
