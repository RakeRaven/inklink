// Key names used in shortcut combos like "ctrl+shift+z", with Windows VK and Linux evdev codes.
pub struct Key {
    pub vk: u16,
    pub evdev: u16,
}

const fn k(vk: u16, evdev: u16) -> Key {
    Key { vk, evdev }
}

const NAMED: &[(&str, Key)] = &[
    ("ctrl", k(0x11, 29)),
    ("shift", k(0x10, 42)),
    ("alt", k(0x12, 56)),
    ("win", k(0x5B, 125)),
    ("super", k(0x5B, 125)),
    ("space", k(0x20, 57)),
    ("tab", k(0x09, 15)),
    ("enter", k(0x0D, 28)),
    ("esc", k(0x1B, 1)),
    ("backspace", k(0x08, 14)),
    ("delete", k(0x2E, 111)),
    ("insert", k(0x2D, 110)),
    ("home", k(0x24, 102)),
    ("end", k(0x23, 107)),
    ("pageup", k(0x21, 104)),
    ("pagedown", k(0x22, 109)),
    ("up", k(0x26, 103)),
    ("down", k(0x28, 108)),
    ("left", k(0x25, 105)),
    ("right", k(0x27, 106)),
    ("[", k(0xDB, 26)),
    ("]", k(0xDD, 27)),
    ("-", k(0xBD, 12)),
    ("=", k(0xBB, 13)),
    (",", k(0xBC, 51)),
    (".", k(0xBE, 52)),
    ("/", k(0xBF, 53)),
    (";", k(0xBA, 39)),
    ("'", k(0xDE, 40)),
    ("\\", k(0xDC, 43)),
    ("`", k(0xC0, 41)),
];

// evdev codes for a..z (keyboard layout order differs from alphabet).
const LETTERS: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];

pub fn lookup(name: &str) -> Option<Key> {
    let name = name.trim().to_ascii_lowercase();
    let name = match name.as_str() {
        "control" | "cmd" => "ctrl",
        "option" => "alt",
        "escape" => "esc",
        "return" => "enter",
        "del" => "delete",
        "plus" => "=",
        "minus" => "-",
        other => other,
    };
    if let Some((_, key)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(k(key.vk, key.evdev));
    }
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0];
        if c.is_ascii_lowercase() {
            return Some(k(
                c.to_ascii_uppercase() as u16,
                LETTERS[(c - b'a') as usize],
            ));
        }
        if c.is_ascii_digit() {
            let evdev = if c == b'0' { 11 } else { (c - b'1') as u16 + 2 };
            return Some(k(c as u16, evdev));
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        let evdev = match n {
            1..=10 => 58 + n,
            11 => 87,
            12 => 88,
            _ => return None,
        };
        return Some(k(0x6F + n, evdev));
    }
    None
}

/// Parses "ctrl+shift+z"; unknown names are skipped with a warning.
pub fn parse_combo(combo: &str) -> Vec<Key> {
    // "+" alone or a trailing "++" means the plus key itself.
    let parts: Vec<&str> = if combo.ends_with("++") || combo == "+" {
        let mut p: Vec<&str> = combo
            .trim_end_matches('+')
            .split('+')
            .filter(|s| !s.is_empty())
            .collect();
        p.push("plus");
        p
    } else {
        combo.split('+').filter(|s| !s.trim().is_empty()).collect()
    };
    parts
        .into_iter()
        .filter_map(|p| {
            let key = lookup(p);
            if key.is_none() {
                log::warn!("unknown key name '{p}' in '{combo}'");
            }
            key
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos() {
        let keys = parse_combo("Ctrl+Shift+Z");
        assert_eq!(
            keys.iter().map(|k| k.evdev).collect::<Vec<_>>(),
            vec![29, 42, 44]
        );
        assert_eq!(parse_combo("f12")[0].vk, 0x7B);
        assert_eq!(parse_combo("1")[0].evdev, 2);
        assert_eq!(parse_combo("ctrl++").len(), 2);
    }
}
