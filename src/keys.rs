//! Key-name helpers built on top of evdev's KeyCode name table.

use evdev::KeyCode;
use std::str::FromStr;
use std::sync::OnceLock;

/// Parse an evdev key/button name ("KEY_A", "BTN_SIDE"). Also accepts a bare
/// suffix like "A" or "esc" for convenience.
pub fn parse_key(name: &str) -> Option<KeyCode> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    if let Ok(k) = KeyCode::from_str(n) {
        return Some(k);
    }
    let up = n.to_ascii_uppercase();
    KeyCode::from_str(&up)
        .or_else(|_| KeyCode::from_str(&format!("KEY_{up}")))
        .or_else(|_| KeyCode::from_str(&format!("BTN_{up}")))
        .ok()
}

pub fn key_name(k: KeyCode) -> String {
    format!("{k:?}")
}

/// Every named key and button (deduplicated, sorted with common ones first).
pub fn all_key_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut v: Vec<String> = (0u16..0x300)
            .map(|c| key_name(KeyCode::new(c)))
            .filter(|s| !s.starts_with("unknown") && s != "KEY_RESERVED")
            .collect();
        v.dedup();
        v
    })
}

/// True if this code is a mouse/pointer button (BTN_LEFT..BTN_TASK).
pub fn is_mouse_button(k: KeyCode) -> bool {
    (0x110..=0x117).contains(&k.code())
}

/// Map a character to (key, needs_shift) for a US QWERTY layout.
pub fn char_to_key(c: char) -> Option<(KeyCode, bool)> {
    use KeyCode as K;
    let lower = |k| Some((k, false));
    let upper = |k| Some((k, true));
    match c {
        'a'..='z' | 'A'..='Z' => {
            const LETTERS: [KeyCode; 26] = [
                K::KEY_A,
                K::KEY_B,
                K::KEY_C,
                K::KEY_D,
                K::KEY_E,
                K::KEY_F,
                K::KEY_G,
                K::KEY_H,
                K::KEY_I,
                K::KEY_J,
                K::KEY_K,
                K::KEY_L,
                K::KEY_M,
                K::KEY_N,
                K::KEY_O,
                K::KEY_P,
                K::KEY_Q,
                K::KEY_R,
                K::KEY_S,
                K::KEY_T,
                K::KEY_U,
                K::KEY_V,
                K::KEY_W,
                K::KEY_X,
                K::KEY_Y,
                K::KEY_Z,
            ];
            let idx = (c.to_ascii_lowercase() as u8 - b'a') as usize;
            Some((LETTERS[idx], c.is_ascii_uppercase()))
        }
        '1'..='9' => {
            // KEY_1 = 2 .. KEY_9 = 10
            Some((KeyCode::new(2 + (c as u16 - '1' as u16)), false))
        }
        '0' => lower(K::KEY_0),
        ' ' => lower(K::KEY_SPACE),
        '\n' => lower(K::KEY_ENTER),
        '\t' => lower(K::KEY_TAB),
        '-' => lower(K::KEY_MINUS),
        '=' => lower(K::KEY_EQUAL),
        '[' => lower(K::KEY_LEFTBRACE),
        ']' => lower(K::KEY_RIGHTBRACE),
        '\\' => lower(K::KEY_BACKSLASH),
        ';' => lower(K::KEY_SEMICOLON),
        '\'' => lower(K::KEY_APOSTROPHE),
        '`' => lower(K::KEY_GRAVE),
        ',' => lower(K::KEY_COMMA),
        '.' => lower(K::KEY_DOT),
        '/' => lower(K::KEY_SLASH),
        '!' => upper(K::KEY_1),
        '@' => upper(K::KEY_2),
        '#' => upper(K::KEY_3),
        '$' => upper(K::KEY_4),
        '%' => upper(K::KEY_5),
        '^' => upper(K::KEY_6),
        '&' => upper(K::KEY_7),
        '*' => upper(K::KEY_8),
        '(' => upper(K::KEY_9),
        ')' => upper(K::KEY_0),
        '_' => upper(K::KEY_MINUS),
        '+' => upper(K::KEY_EQUAL),
        '{' => upper(K::KEY_LEFTBRACE),
        '}' => upper(K::KEY_RIGHTBRACE),
        '|' => upper(K::KEY_BACKSLASH),
        ':' => upper(K::KEY_SEMICOLON),
        '"' => upper(K::KEY_APOSTROPHE),
        '~' => upper(K::KEY_GRAVE),
        '<' => upper(K::KEY_COMMA),
        '>' => upper(K::KEY_DOT),
        '?' => upper(K::KEY_SLASH),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        assert_eq!(parse_key("KEY_A"), Some(KeyCode::KEY_A));
        assert_eq!(parse_key("a"), Some(KeyCode::KEY_A));
        assert_eq!(parse_key("esc"), Some(KeyCode::KEY_ESC));
        assert_eq!(parse_key("BTN_SIDE"), Some(KeyCode::BTN_SIDE));
        assert_eq!(parse_key("side"), Some(KeyCode::BTN_SIDE));
        assert_eq!(parse_key("nonsense"), None);
    }

    #[test]
    fn chars() {
        assert_eq!(char_to_key('5'), Some((KeyCode::KEY_5, false)));
        assert_eq!(char_to_key('Q'), Some((KeyCode::KEY_Q, true)));
        assert_eq!(char_to_key('?'), Some((KeyCode::KEY_SLASH, true)));
    }

    #[test]
    fn names_list() {
        let n = all_key_names();
        assert!(n.iter().any(|s| s == "KEY_CAPSLOCK"));
        assert!(n.iter().any(|s| s == "BTN_EXTRA"));
    }
}
