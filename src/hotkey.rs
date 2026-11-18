//! Hotkey strings: converting a key press captured in the settings window into the
//! `global-hotkey` string format stored in the config, and back into a ⌘⇧2-style label.

use eframe::egui::{Key, Modifiers};
use global_hotkey::hotkey::HotKey;
use std::str::FromStr;

pub fn parse(s: &str) -> Result<HotKey, String> {
    HotKey::from_str(s).map_err(|e| e.to_string())
}

/// Do two shortcut strings mean the same keys (`shift+cmd+1` is `cmd+shift+1`)?
pub fn same(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Ok(a), Ok(b)) => a.id() == b.id(),
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// Build a hotkey string from a key press. Returns `None` for keys we can't register or when
/// a typing key is pressed without ⌘, ⌃ or ⌥ — a bare letter as a global hotkey would hijack
/// typing. Keys that never type anything (F13–F24, Insert) may be used on their own, so a
/// Windows keyboard's PrtSc works like it does on Windows.
pub fn from_key_press(key: Key, mods: Modifiers) -> Option<String> {
    if !(mods.mac_cmd || mods.ctrl || mods.alt || works_alone(key)) {
        return None;
    }
    Some(build(key_code(key)?, mods))
}

/// Record an in-overlay tool shortcut. Unlike the global hotkey, a bare key is fine (it only
/// applies while the overlay is open). Keys the overlay already uses are refused.
pub fn tool_key_from_press(key: Key, mods: Modifiers) -> Result<String, &'static str> {
    match key {
        Key::Escape => return Err("Esc closes the capture"),
        Key::Enter => return Err("↩ copies the whole screen"),
        Key::Z if mods.mac_cmd || mods.ctrl => return Err("⌘Z is undo"),
        _ => {}
    }
    let code = key_code(key).ok_or("That key can't be used")?;
    Ok(build(code, mods))
}

/// Does a key press match a shortcut string such as `P` or `cmd+2`? Modifiers must match
/// exactly (⇧P is not P), so tools never fire while you hold ⇧ to draw a square.
pub fn matches(shortcut: &str, key: Key, mods: Modifiers) -> bool {
    let Some(code) = key_code(key) else { return false };
    let (mut cmd, mut ctrl, mut alt, mut shift, mut want) = (false, false, false, false, None);
    for part in shortcut.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "super" | "meta" => cmd = true,
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            _ => want = Some(part),
        }
    }
    want.is_some_and(|k| k.eq_ignore_ascii_case(code))
        && (mods.mac_cmd, mods.ctrl, mods.alt, mods.shift) == (cmd, ctrl, alt, shift)
}

fn build(code: &str, mods: Modifiers) -> String {
    let mut parts = Vec::new();
    for (on, name) in [(mods.mac_cmd, "cmd"), (mods.ctrl, "ctrl"), (mods.alt, "alt"), (mods.shift, "shift")] {
        if on {
            parts.push(name);
        }
    }
    parts.push(code);
    parts.join("+")
}

fn works_alone(key: Key) -> bool {
    use Key::*;
    matches!(key, F13 | F14 | F15 | F16 | F17 | F18 | F19 | F20 | F21 | F22 | F23 | F24 | Insert)
}

/// egui key → the key name `global-hotkey` parses.
fn key_code(key: Key) -> Option<&'static str> {
    use Key::*;
    Some(match key {
        A => "A", B => "B", C => "C", D => "D", E => "E", F => "F", G => "G", H => "H",
        I => "I", J => "J", K => "K", L => "L", M => "M", N => "N", O => "O", P => "P",
        Q => "Q", R => "R", S => "S", T => "T", U => "U", V => "V", W => "W", X => "X",
        Y => "Y", Z => "Z",
        Num0 => "0", Num1 => "1", Num2 => "2", Num3 => "3", Num4 => "4",
        Num5 => "5", Num6 => "6", Num7 => "7", Num8 => "8", Num9 => "9",
        F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6",
        F7 => "F7", F8 => "F8", F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12",
        F13 => "F13", F14 => "F14", F15 => "F15", F16 => "F16", F17 => "F17",
        F18 => "F18", F19 => "F19", F20 => "F20", F21 => "F21", F22 => "F22",
        F23 => "F23", F24 => "F24",
        // On a Mac, a Windows keyboard's Insert arrives as Insert (the old Help key) and
        // PrtSc / ScrLk / Pause arrive as F13 / F14 / F15.
        Insert => "Insert",
        Space => "Space", Enter => "Enter", Tab => "Tab", Backspace => "Backspace",
        Delete => "Delete", Home => "Home", End => "End", PageUp => "PageUp",
        PageDown => "PageDown", ArrowUp => "ArrowUp", ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft", ArrowRight => "ArrowRight",
        Minus => "Minus", Equals => "Equal", Comma => "Comma", Period => "Period",
        Slash => "Slash", Backslash => "Backslash", Semicolon => "Semicolon",
        Quote => "Quote", Backtick => "Backquote", OpenBracket => "BracketLeft",
        CloseBracket => "BracketRight",
        _ => return None,
    })
}

/// `cmd+shift+2` → `⌃⌥⇧⌘2` order as macOS menus show it: `⇧⌘2`.
pub fn pretty(s: &str) -> String {
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    let mut key = String::new();
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "cmd" | "command" | "super" | "meta" => cmd = true,
            _ => key = pretty_key(part),
        }
    }
    let mut out = String::new();
    for (on, sym) in [(ctrl, "⌃"), (alt, "⌥"), (shift, "⇧"), (cmd, "⌘")] {
        if on {
            out.push_str(sym);
        }
    }
    out + &key
}

fn pretty_key(k: &str) -> String {
    let k = k.strip_prefix("Key").or_else(|| k.strip_prefix("Digit")).unwrap_or(k);
    match k {
        "Space" => "Space",
        "Enter" => "↩",
        "Tab" => "⇥",
        "Backspace" => "⌫",
        "Delete" => "⌦",
        "Insert" => "Ins",
        "F13" => "F13 (PrtSc)",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "Minus" => "-",
        "Equal" => "=",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Backquote" => "`",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        other => return other.to_uppercase(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers { alt, ctrl, shift, mac_cmd: cmd, command: cmd }
    }

    #[test]
    fn key_press_round_trips_through_global_hotkey() {
        let s = from_key_press(Key::Num2, mods(true, false, false, true)).unwrap();
        assert_eq!(s, "cmd+shift+2");
        assert!(parse(&s).is_ok());

        for key in [Key::S, Key::F5, Key::F13, Key::Insert, Key::Backtick, Key::OpenBracket, Key::Space, Key::ArrowUp] {
            let s = from_key_press(key, mods(false, true, true, false)).unwrap();
            assert!(parse(&s).is_ok(), "{s} should parse");
        }
    }

    #[test]
    fn needs_a_real_modifier() {
        assert_eq!(from_key_press(Key::A, mods(false, false, false, false)), None);
        assert_eq!(from_key_press(Key::A, mods(false, false, false, true)), None);
        assert_eq!(from_key_press(Key::Escape, mods(true, false, false, false)), None);
    }

    #[test]
    fn tool_keys_allow_bare_letters_but_not_reserved_keys() {
        let none = mods(false, false, false, false);
        assert_eq!(tool_key_from_press(Key::P, none), Ok("P".to_string()));
        assert_eq!(tool_key_from_press(Key::Num2, mods(true, false, false, false)), Ok("cmd+2".to_string()));
        assert!(tool_key_from_press(Key::Escape, none).is_err());
        assert!(tool_key_from_press(Key::Enter, none).is_err());
        assert!(tool_key_from_press(Key::Z, mods(true, false, false, false)).is_err());
        assert_eq!(tool_key_from_press(Key::Z, none), Ok("Z".to_string()));
    }

    #[test]
    fn matching_needs_the_same_key_and_exact_modifiers() {
        let none = mods(false, false, false, false);
        assert!(matches("P", Key::P, none));
        assert!(matches("p", Key::P, none));
        assert!(!matches("P", Key::P, mods(false, false, false, true)), "⇧P is not P");
        assert!(!matches("P", Key::O, none));
        assert!(matches("cmd+2", Key::Num2, mods(true, false, false, false)));
        assert!(!matches("cmd+2", Key::Num2, none));
        assert!(matches("ctrl+alt+F5", Key::F5, mods(false, true, true, false)));
        for (tool_key, key) in [("S", Key::S), ("P", Key::P), ("A", Key::A), ("R", Key::R), ("E", Key::E)] {
            assert!(matches(tool_key, key, none));
        }
    }

    #[test]
    fn non_typing_keys_work_alone() {
        let none = mods(false, false, false, false);
        assert_eq!(from_key_press(Key::F13, none).as_deref(), Some("F13"));
        assert_eq!(from_key_press(Key::Insert, none).as_deref(), Some("Insert"));
        assert_eq!(from_key_press(Key::Insert, mods(true, false, false, false)).as_deref(), Some("cmd+Insert"));
        assert!(parse("F13").is_ok() && parse("cmd+Insert").is_ok());
    }

    #[test]
    fn default_hotkey_parses() {
        assert!(parse(crate::config::DEFAULT_HOTKEY).is_ok());
    }

    #[test]
    fn pretty_uses_mac_symbol_order() {
        assert_eq!(pretty("cmd+shift+2"), "⇧⌘2");
        assert_eq!(pretty("shift+ctrl+alt+cmd+s"), "⌃⌥⇧⌘S");
        assert_eq!(pretty("alt+BracketLeft"), "⌥[");
        assert_eq!(pretty("ctrl+KeyQ"), "⌃Q");
        assert_eq!(pretty("cmd+Insert"), "⌘Ins");
        assert_eq!(pretty("cmd+F13"), "⌘F13 (PrtSc)");
    }
}
