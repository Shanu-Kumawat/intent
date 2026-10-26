/// A normalized key event, decoupled from any UI framework (SPEC §6).
///
/// `key` uses GPUI-style names: single characters ("a", "5", "d") or named
/// keys ("space", "enter", "esc", "backspace", "tab", "up", "down", "left",
/// "right", "delete", "home", "end", "pageup", "pagedown").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub ctrl: bool,
    pub shift: bool,
}

impl Key {
    pub fn new(key: &str) -> Key {
        Key { key: key.to_string(), ctrl: false, shift: false }
    }

    pub fn ctrl(key: &str) -> Key {
        Key { key: key.to_string(), ctrl: true, shift: false }
    }

    pub fn shifted(key: &str) -> Key {
        Key { key: key.to_string(), ctrl: false, shift: true }
    }

    /// The command character: a single-char key, uppercased when shift is
    /// held. This is what the normal-mode grammar matches on.
    pub fn chr(&self) -> Option<char> {
        if self.ctrl {
            return None;
        }
        let mut it = self.key.chars();
        let c = it.next()?;
        if it.next().is_some() {
            return None;
        }
        if self.shift && c.is_ascii_lowercase() {
            Some(c.to_ascii_uppercase())
        } else {
            Some(c)
        }
    }

    pub fn is_enter(&self) -> bool {
        !self.ctrl && self.key == "enter"
    }

    pub fn is_esc(&self) -> bool {
        !self.ctrl && (self.key == "esc" || self.key == "escape")
    }

    pub fn is_backspace(&self) -> bool {
        !self.ctrl && self.key == "backspace"
    }

    pub fn is_tab(&self) -> bool {
        !self.ctrl && self.key == "tab"
    }

    /// Compact display for the status line, e.g. `2diw`, `f`, `ctrl-r`.
    pub fn display(&self) -> String {
        let base = if self.key.len() == 1 {
            self.chr().map(|c| c.to_string()).unwrap_or_else(|| self.key.clone())
        } else {
            self.key.clone()
        };
        if self.ctrl {
            format!("ctrl-{base}")
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chars() {
        assert_eq!(Key::new("d").chr(), Some('d'));
        assert_eq!(Key::shifted("d").chr(), Some('D'));
        assert_eq!(Key::new("5").chr(), Some('5'));
        assert_eq!(Key::ctrl("r").chr(), None);
        assert_eq!(Key::new("enter").chr(), None);
        assert_eq!(Key::new(" ").chr(), Some(' '));
    }

    #[test]
    fn display() {
        assert_eq!(Key::ctrl("r").display(), "ctrl-r");
        assert_eq!(Key::shifted("d").display(), "D");
        assert_eq!(Key::new("space").display(), "space");
    }
}
