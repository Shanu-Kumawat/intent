use intent_vim::Key;

/// Printable character for a key event (insert-style). Handles the "space"
/// named key and shift-letters.
pub fn plain_char(key: &Key) -> Option<char> {
    if key.ctrl {
        return None;
    }
    if key.key == "space" {
        return Some(' ');
    }
    let mut it = key.key.chars();
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    if key.shift && c.is_ascii_lowercase() {
        Some(c.to_ascii_uppercase())
    } else {
        Some(c)
    }
}

/// Normalize a GPUI keystroke into an engine Key: single characters are
/// lowercased with the shift flag set (both layout conventions), and
/// "escape" maps to the engine's "esc".
pub fn from_display(key: &str, ctrl: bool, shift: bool) -> Key {
    let mut shift = shift;
    let normalized: String = if key == "escape" || key == "esc" {
        "esc".to_string()
    } else if key.len() == 1 {
        let c = key.chars().next().unwrap();
        if c.is_ascii_uppercase() {
            shift = true;
            c.to_ascii_lowercase().to_string()
        } else {
            key.to_string()
        }
    } else {
        // GPUI uses "space" already; normalize a few platform spellings.
        match key {
            "return" | "cr" => "enter".to_string(),
            "del" => "backspace".to_string(),
            _ => key.to_string(),
        }
    };
    Key {
        key: normalized,
        ctrl,
        shift,
    }
}
