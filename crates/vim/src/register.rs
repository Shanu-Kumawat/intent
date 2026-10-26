/// Clipboard abstraction (SPEC §4.5). The engine never touches a real OS
/// clipboard directly; hosts provide this trait. Implementations must be
/// usable from the UI thread.
pub trait Clipboard {
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str);
}

/// In-memory clipboard for tests and headless use.
#[derive(Default)]
pub struct MemClipboard {
    pub content: std::cell::RefCell<Option<String>>,
}

impl MemClipboard {
    pub fn new() -> MemClipboard {
        MemClipboard::default()
    }
}

impl Clipboard for MemClipboard {
    fn read(&self) -> Option<String> {
        self.content.borrow().clone()
    }
    fn write(&self, text: &str) {
        *self.content.borrow_mut() = Some(text.to_string());
    }
}

/// A yank/delete register entry. `linewise` text always ends with '\n'.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Register {
    pub linewise: bool,
    pub text: String,
}

impl Register {
    pub fn charwise(text: impl Into<String>) -> Register {
        Register {
            linewise: false,
            text: text.into(),
        }
    }

    pub fn linewise(text: impl Into<String>) -> Register {
        let mut text = text.into();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        Register {
            linewise: true,
            text,
        }
    }
}
