//! Regression tests for bugs found in the line-by-line review.

use intent_core::{ops, NewTask, Store, TaskView};
use intent_gui::model::GuiState;
use intent_vim::{Clipboard as _, Key, MemClipboard};
use std::rc::Rc;

fn fx() -> GuiState {
    let store = Store::open_memory().unwrap();
    GuiState::new(store, Rc::new(MemClipboard::new()))
}

fn feed(s: &mut GuiState, keys: &str) {
    for t in keys.split(' ').filter(|t| !t.is_empty()) {
        match t {
            "<cr>" => s.handle_key(Key::new("enter")),
            "<esc>" => s.handle_key(Key::new("esc")),
            "<space>" => s.handle_key(Key::new("space")),
            "<c-d>" => s.handle_key(Key::ctrl("d")),
            _ => {
                for c in t.chars() {
                    let k = if c.is_ascii_uppercase() {
                        Key::shifted(&c.to_ascii_lowercase().to_string())
                    } else {
                        Key::new(&c.to_string())
                    };
                    s.handle_key(k);
                }
            }
        }
    }
}

fn typed(s: &mut GuiState, text: &str) {
    for c in text.chars() {
        let k = if c == ' ' {
            Key::new("space")
        } else {
            Key::new(&c.to_string())
        };
        s.handle_key(k);
    }
}

fn seed(s: &mut GuiState, title: &str) -> String {
    let id = ops::add(
        &s.store,
        NewTask {
            title: title.into(),
            ..Default::default()
        },
    )
    .unwrap()
    .task
    .unwrap()
    .id;
    s.switch_view(TaskView::Inbox);
    id
}

#[test]
fn editor_visual_esc_exits_visual_not_editor() {
    let mut s = fx();
    seed(&mut s, "hello");
    feed(&mut s, "<cr>");
    assert!(s.editor.is_some());
    feed(&mut s, "v");
    feed(&mut s, "l");
    assert_eq!(s.editor.as_ref().unwrap().title.mode_name(), "VISUAL");
    feed(&mut s, "<esc>");
    assert!(s.editor.is_some(), "editor must stay open after visual esc");
    assert_eq!(s.editor.as_ref().unwrap().title.mode_name(), "NORMAL");
    feed(&mut s, "<esc>"); // normal-mode esc closes the editor
    assert!(s.editor.is_none());
}

#[test]
fn editor_write_through_bumps_updated_at() {
    let mut s = fx();
    let id = seed(&mut s, "before");
    let t0 = s.store.get_task(&id).unwrap();
    feed(&mut s, "<cr>");
    feed(&mut s, "A");
    typed(&mut s, "!");
    feed(&mut s, "<esc>"); // leave insert; write-through already happened
    let t1 = s.store.get_task(&id).unwrap();
    assert_eq!(t1.title, "before!");
    assert!(
        t1.updated_at > t0.updated_at,
        "write-through bumps updated_at (SPEC §1)"
    );
    feed(&mut s, "<esc>");
}

#[test]
fn editor_never_persists_empty_title() {
    let mut s = fx();
    let id = seed(&mut s, "keep");
    feed(&mut s, "<cr>");
    feed(&mut s, "d$ x"); // delete the whole title
    feed(&mut s, "<esc>"); // (still insert→normal transitions; write-through refused)
    let t = s.store.get_task(&id).unwrap();
    assert_eq!(t.title, "keep", "empty title is refused, old title kept");
    assert!(
        s.status.contains("must not be empty"),
        "status: {:?}",
        s.status
    );
    feed(&mut s, "<esc>");
}

// MemClipboard read needs the Clipboard trait in scope for the yank tests.
#[allow(dead_code)]
fn _trait_in_scope(c: &MemClipboard) -> Option<String> {
    c.read()
}
