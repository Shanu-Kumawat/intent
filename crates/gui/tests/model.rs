//! Headless tests of the GUI interaction model — the same key stream the
//! GPUI layer delivers (SPEC §4, §5).

use intent_core::model::{NewTask, TaskView};
use intent_core::{ops, Store};
use intent_gui::model::GuiState;
use intent_vim::{Clipboard, Key, MemClipboard};
use std::rc::Rc;

struct Fixture {
    state: GuiState,
    #[allow(dead_code)]
    clip: Rc<MemClipboard>,
}

fn fx() -> Fixture {
    let store = Store::open_memory().unwrap();
    let clip = Rc::new(MemClipboard::new());
    let state = GuiState::new(store, clip.clone());
    Fixture { state, clip }
}

/// Seed one inbox task per name; returns ids in order. Leaves the view on
/// Inbox so the seeded tasks are under the cursor.
fn seed(f: &mut Fixture, names: &[&str]) -> Vec<String> {
    let mut ids = Vec::new();
    for n in names {
        let out = ops::add(
            &f.state.store,
            NewTask {
                title: (*n).to_string(),
                ..Default::default()
            },
        )
        .unwrap()
        .task
        .unwrap();
        ids.push(out.id);
    }
    f.state.switch_view(intent_core::TaskView::Inbox);
    ids
}

fn feed(f: &mut Fixture, s: &str) {
    for t in s.split(' ').filter(|t| !t.is_empty()) {
        for k in parse(t) {
            f.state.handle_key(k);
        }
    }
}

/// Parse one token into keys: "<esc>", "<cr>", "<space>", "<c-r>", "<c-v>",
/// uppercase = shift+letter, other multi-char tokens expand to characters.
fn parse(t: &str) -> Vec<Key> {
    if let Some(inner) = t.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
        let l = inner.to_ascii_lowercase();
        let key = match l.as_str() {
            "esc" => Key::new("esc"),
            "cr" => Key::new("enter"),
            "bs" => Key::new("backspace"),
            "space" => Key::new("space"),
            "up" => Key::new("up"),
            "down" => Key::new("down"),
            "tab" => Key::new("tab"),
            "stab" => Key::shifted("tab"),
            _ if l.starts_with("c-") => Key::ctrl(&l[2..]),
            _ => Key::new(inner),
        };
        return vec![key];
    }
    match t {
        "tab" => return vec![Key::new("tab")],
        "space" => return vec![Key::new("space")],
        "up" => return vec![Key::new("up")],
        "down" => return vec![Key::new("down")],
        "esc" => return vec![Key::new("esc")],
        _ => {}
    }
    t.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                Key::shifted(&c.to_ascii_lowercase().to_string())
            } else {
                Key::new(&c.to_string())
            }
        })
        .collect()
}

fn type_prompt_text(f: &mut Fixture, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            f.state.handle_key(Key::new("space"));
        } else {
            f.state.handle_key(Key::new(&c.to_string()));
        }
    }
}

// ---- capture ----------------------------------------------------------------

#[test]
fn capture_flow_and_undo() {
    let mut f = fx();
    feed(&mut f, "o");
    assert!(f.state.prompt.is_some());
    type_prompt_text(&mut f, "buy milk");
    feed(&mut f, "<cr>");

    assert!(f.state.prompt.is_none(), "prompt closes on enter");
    let all = f.state.store.all_tasks().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title, "buy milk");
    assert_eq!(all[0].inbox, true);
    assert!(f.state.status.contains("captured"), "status names the id");

    // focus returned to the list at the new task (inbox view shows it)
    f.state.switch_view(TaskView::Inbox);
    assert_eq!(f.state.rows.len(), 1);
    assert_eq!(f.state.cursor, 0);

    // undo deletes the capture
    feed(&mut f, "u");
    assert!(f.state.store.all_tasks().unwrap().is_empty());
    feed(&mut f, "<c-r>");
    assert_eq!(f.state.store.all_tasks().unwrap().len(), 1);
}

#[test]
fn capture_prompt_requires_title_and_esc_cancels() {
    let mut f = fx();
    feed(&mut f, "o");
    feed(&mut f, "<cr>"); // empty title
    assert!(f.state.prompt.is_some(), "empty title keeps prompt open");
    feed(&mut f, "<esc>");
    assert!(f.state.prompt.is_none());
    assert!(f.state.store.all_tasks().unwrap().is_empty());
}

// ---- views --------------------------------------------------------------------

#[test]
fn number_keys_switch_views_and_tab_cycles() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    feed(&mut f, "1");
    assert_eq!(f.state.view, TaskView::Inbox);
    feed(&mut f, "2");
    assert_eq!(f.state.view, TaskView::Today);
    feed(&mut f, "3");
    assert_eq!(f.state.view, TaskView::Upcoming);
    feed(&mut f, "4");
    assert_eq!(f.state.view, TaskView::Completed);
    feed(&mut f, "tab");
    assert_eq!(f.state.view, TaskView::Inbox, "tab wraps to first view");
    feed(&mut f, "<stab>");
    assert_eq!(f.state.view, TaskView::Completed);
    feed(&mut f, "tab");
    feed(&mut f, "tab");
    assert_eq!(
        f.state.view,
        TaskView::Today,
        "two tabs from Completed wrap through Inbox"
    );
}

#[test]
fn digits_five_nine_are_counts_not_views() {
    let mut f = fx();
    seed(&mut f, &["a", "b", "c", "d", "e", "f", "g", "h"]);
    f.state.switch_view(TaskView::Inbox);
    assert_eq!(f.state.cursor, 0);
    feed(&mut f, "5 j");
    assert_eq!(f.state.cursor, 5, "5j moves five rows");
    assert_eq!(
        f.state.view,
        TaskView::Inbox,
        "5 is a count, not a view key"
    );
    feed(&mut f, "G");
    let bottom = f.state.rows.len() - 1;
    assert_eq!(f.state.cursor, bottom);
    feed(&mut f, "g g");
    assert_eq!(f.state.cursor, 0);
}

// ---- completion, archive, delete ------------------------------------------------

#[test]
fn x_toggles_completion_and_undo_restores() {
    let mut f = fx();
    let ids = seed(&mut f, &["one"]);

    // Complete from the inbox; the task leaves the view.
    feed(&mut f, "x");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Done
    );
    assert!(f
        .state
        .store
        .get_task(&ids[0])
        .unwrap()
        .completed_at
        .is_some());

    // Reopen from the Completed view.
    feed(&mut f, "4");
    assert_eq!(f.state.rows.len(), 1);
    feed(&mut f, "x");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Active
    );

    // The reopened task is back in the inbox.
    feed(&mut f, "1");
    assert_eq!(f.state.rows.len(), 1);

    // Undo/redo the completion.
    feed(&mut f, "x");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Done
    );
    feed(&mut f, "u");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Active
    );
    feed(&mut f, "<c-r>");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Done
    );
}

#[test]
fn d_archives_and_space_D_unarchives() {
    let mut f = fx();
    let ids = seed(&mut f, &["one"]);
    feed(&mut f, "d");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Archived
    );
    // archived tasks live in the archived view; unarchive from there
    feed(&mut f, ":");
    type_prompt_text(&mut f, "archived");
    feed(&mut f, "<cr>");
    assert_eq!(f.state.rows.len(), 1);
    feed(&mut f, "<space> D");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Active
    );
}

#[test]
fn capital_D_delete_requires_confirmation_and_is_undoable() {
    let mut f = fx();
    let ids = seed(&mut f, &["precious"]);
    feed(&mut f, "D");
    assert!(f.state.prompt.is_some(), "D opens confirmation");
    feed(&mut f, "n"); // any non-y key cancels immediately
    assert!(f.state.prompt.is_none());
    assert!(f.state.store.get_task(&ids[0]).is_ok(), "n keeps the task");

    feed(&mut f, "D");
    feed(&mut f, "y"); // single keypress confirms
    assert!(f.state.store.get_task(&ids[0]).is_err());
    feed(&mut f, "u");
    assert!(
        f.state.store.get_task(&ids[0]).is_ok(),
        "delete is session-undoable"
    );
}

// ---- dates via leader ------------------------------------------------------------

#[test]
fn leader_defer_due_remind_prompts() {
    let mut f = fx();
    let ids = seed(&mut f, &["defer me", "date me"]);
    let today = f.state.today;

    // Inbox is newest-first: the cursor is on ids[1] ("date me").
    feed(&mut f, "<space> d");
    assert!(f.state.prompt.is_some());
    type_prompt_text(&mut f, "+3d");
    feed(&mut f, "<cr>");
    let t = f.state.store.get_task(&ids[1]).unwrap();
    assert_eq!(t.defer_until, Some(today.add_days(3)));
    assert!(t.inbox, "deferring alone does not organize");

    feed(&mut f, "<space> u"); // still on ids[1]
    type_prompt_text(&mut f, "tomorrow");
    feed(&mut f, "<cr>");
    let t = f.state.store.get_task(&ids[1]).unwrap();
    assert_eq!(t.due, Some(today.add_days(1)));
    assert!(!t.inbox, "setting due organizes out of inbox");
    // ids[1] left the inbox; the cursor drops to ids[0] ("defer me")

    feed(&mut f, "<space> r");
    type_prompt_text(&mut f, "not a date at all");
    feed(&mut f, "<cr>");
    assert!(f.state.prompt.is_some(), "bad date keeps the prompt open");
    assert!(f.state.prompt.as_ref().unwrap().error.is_some());
    feed(&mut f, "<esc>");
    assert!(f.state.prompt.is_none());
}

#[test]
fn shift_due_keys_and_repeat() {
    let mut f = fx();
    let ids = seed(&mut f, &["task"]);
    let today = f.state.today;

    feed(&mut f, ">"); // no due: sets today (day 0)
    assert_eq!(f.state.store.get_task(&ids[0]).unwrap().due, Some(today));
    // the task is due today: continue in the Today view
    feed(&mut f, "2");
    assert_eq!(f.state.rows.len(), 1);
    feed(&mut f, ">"); // +1 day: the task leaves Today (no longer due)
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().due,
        Some(today.add_days(1))
    );
    feed(&mut f, "3"); // continue in Upcoming
    feed(&mut f, ">");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().due,
        Some(today.add_days(2))
    );
    feed(&mut f, "<"); // -1
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().due,
        Some(today.add_days(1))
    );

    // . repeats the last op (the -1 shift)
    feed(&mut f, ".");
    assert_eq!(f.state.store.get_task(&ids[0]).unwrap().due, Some(today));
}

#[test]
fn priority_bump_and_repeat() {
    let mut f = fx();
    let ids = seed(&mut f, &["task"]);
    feed(&mut f, "+");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().priority,
        intent_core::Priority::Low
    );
    feed(&mut f, "+ +");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().priority,
        intent_core::Priority::High
    );
    feed(&mut f, "-");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().priority,
        intent_core::Priority::Medium
    );
    feed(&mut f, ".");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().priority,
        intent_core::Priority::Low
    );
}

// ---- editor -----------------------------------------------------------------------

#[test]
fn editor_open_type_write_through_and_close() {
    let mut f = fx();
    let ids = seed(&mut f, &["plain"]);
    feed(&mut f, "<cr>"); // open editor
    assert!(f.state.editor.is_some());
    assert_eq!(f.state.editor.as_ref().unwrap().task_id, ids[0]);

    feed(&mut f, "A"); // append to title (insert mode)
    type_prompt_text(&mut f, "!");
    feed(&mut f, "<esc>");
    // write-through already persisted
    assert_eq!(f.state.store.get_task(&ids[0]).unwrap().title, "plain!");
    // still in editor (single esc only left insert)
    assert!(f.state.editor.is_some());
    feed(&mut f, "<esc>"); // close editor
    assert!(f.state.editor.is_none());

    // SPEC §5: editor text changes are NOT on the task undo stack — u here
    // has nothing to undo, and the edit persists.
    feed(&mut f, "u");
    assert_eq!(f.state.store.get_task(&ids[0]).unwrap().title, "plain!");
    assert!(
        f.state.status.contains("nothing to undo"),
        "status: {}",
        f.state.status
    );
}

#[test]
fn editor_notes_field_and_tab() {
    let mut f = fx();
    let ids = seed(&mut f, &["t"]);
    feed(&mut f, "<cr>");
    feed(&mut f, "tab"); // title -> notes (normal mode)
    assert!(!f.state.editor.as_ref().unwrap().focus_title);
    feed(&mut f, "i"); // insert
    type_prompt_text(&mut f, "line one");
    feed(&mut f, "<esc>");
    feed(&mut f, "o"); // open a line below (normal mode)
    type_prompt_text(&mut f, "line two");
    feed(&mut f, "<esc>");
    let t = f.state.store.get_task(&ids[0]).unwrap();
    assert_eq!(t.notes, "line one\nline two");
    feed(&mut f, "tab");
    assert!(
        f.state.editor.as_ref().unwrap().focus_title,
        "tab wraps back to title"
    );
    feed(&mut f, "<esc>");
}

// ---- search -----------------------------------------------------------------------

#[test]
fn search_flow() {
    let mut f = fx();
    seed(&mut f, &["buy milk", "buy eggs", "sell car"]);
    feed(&mut f, "/");
    type_prompt_text(&mut f, "buy");
    feed(&mut f, "<cr>");
    assert_eq!(f.state.search_query.as_deref(), Some("buy"));
    assert_eq!(f.state.rows.len(), 2);
    feed(&mut f, "<esc>");
    assert!(f.state.search_query.is_none());
    assert_eq!(
        f.state.view,
        TaskView::Inbox,
        "esc returns to the previous view"
    );
}

// ---- commands ---------------------------------------------------------------------

#[test]
fn command_prompt_due_and_quit() {
    let mut f = fx();
    let ids = seed(&mut f, &["task"]);
    feed(&mut f, ":");
    type_prompt_text(&mut f, "due tomorrow");
    feed(&mut f, "<cr>");
    let t = f.state.store.get_task(&ids[0]).unwrap();
    assert_eq!(t.due, Some(f.state.today.add_days(1)));

    feed(&mut f, ":");
    type_prompt_text(&mut f, "nosuchcmd");
    feed(&mut f, "<cr>");
    assert!(
        f.state.prompt.is_some(),
        "unknown command keeps prompt open"
    );
    feed(&mut f, "<esc>");

    feed(&mut f, ":");
    type_prompt_text(&mut f, "q");
    feed(&mut f, "<cr>");
    assert!(f.state.quit_requested, ":q from the list quits");
}

#[test]
fn command_q_closes_editor_first() {
    let mut f = fx();
    seed(&mut f, &["t"]);
    feed(&mut f, "<cr>");
    assert!(f.state.editor.is_some());
    feed(&mut f, ":");
    type_prompt_text(&mut f, "q");
    feed(&mut f, "<cr>");
    assert!(f.state.editor.is_none());
    assert!(
        !f.state.quit_requested,
        ":q in editor only closes the editor"
    );
}

// ---- visual -----------------------------------------------------------------------

#[test]
fn visual_complete_multiple() {
    let mut f = fx();
    let ids = seed(&mut f, &["a", "b", "c"]);
    f.state.switch_view(TaskView::Inbox);
    feed(&mut f, "V");
    assert!(f.state.visual_anchor.is_some());
    feed(&mut f, "j");
    feed(&mut f, "x");
    assert!(f.state.visual_anchor.is_none());
    let done: usize = f
        .state
        .store
        .all_tasks()
        .unwrap()
        .iter()
        .filter(|t| t.id != ids[2] && t.status == intent_core::Status::Done)
        .count();
    assert_eq!(done, 2);
    feed(&mut f, "u");
    let any_done = f
        .state
        .store
        .all_tasks()
        .unwrap()
        .iter()
        .any(|t| t.status == intent_core::Status::Done);
    assert!(!any_done, "one undo restores both");
}

// ---- focus guarantees ---------------------------------------------------------------

#[test]
fn prompt_cancel_restores_focus_and_cursor() {
    let mut f = fx();
    seed(&mut f, &["a", "b", "c"]);
    f.state.switch_view(TaskView::Inbox);
    feed(&mut f, "j");
    let before = f.state.cursor;
    feed(&mut f, "<space> d");
    feed(&mut f, "<esc>");
    assert_eq!(
        f.state.cursor, before,
        "cursor unchanged after cancelled prompt"
    );
}

#[test]
fn external_changes_are_detected() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    // Simulate a CLI write through a second connection: mutate via a
    // separate store handle on the same database path.
    let path = std::env::temp_dir().join(format!("intent-gui-test-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let store = Store::open(&path).unwrap();
    let clip = Rc::new(MemClipboard::new());
    let mut state = GuiState::new(store, clip);
    ops::add(
        &state.store,
        NewTask {
            title: "gui made this".into(),
            ..Default::default()
        },
    )
    .unwrap();
    state.refresh();
    assert!(
        !state.check_external_changes(),
        "own writes are not external"
    );

    let other = Store::open(&path).unwrap();
    ops::add(
        &other,
        NewTask {
            title: "cli made this".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(state.check_external_changes());
    state.switch_view(intent_core::TaskView::Inbox);
    let titles: Vec<String> = state.rows.iter().map(|t| t.title.clone()).collect();
    assert!(
        titles.contains(&"cli made this".to_string()),
        "reload picks up new rows"
    );
    let _ = std::fs::remove_file(&path);
    let _ = &f;
}

// ---- misc ---------------------------------------------------------------------------

#[test]
fn mode_label_and_pending_display() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    assert_eq!(f.state.mode_label(), "NORMAL");
    f.state.handle_key(Key::new("space"));
    assert!(
        f.state.pending_display().contains("SPC"),
        "leader is visible"
    );
    f.state.handle_key(Key::new("esc"));
    assert_eq!(f.state.pending_display(), "");
}

#[test]
fn unbound_keys_cancel_gracefully() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    feed(&mut f, "z");
    assert!(!f.state.status.is_empty(), "z reports unbound");
    feed(&mut f, "<space> z");
    assert!(
        f.state.status.contains("unknown leader"),
        "bad leader key reported"
    );
}

#[test]
fn yank_writes_clipboard() {
    let mut f = fx();
    seed(&mut f, &["hello task"]);
    f.state.switch_view(TaskView::Inbox);
    feed(&mut f, "y");
    assert_eq!(f.clip.read().as_deref(), Some("hello task"));
    feed(&mut f, "Y");
    assert_eq!(f.clip.read().as_deref(), Some("hello task"));
}

// ---- spec-verification additions (SPEC §4.1) --------------------------------------

#[test]
fn ctrl_d_and_ctrl_u_jump_ten_rows() {
    let mut f = fx();
    seed(
        &mut f,
        &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"],
    );
    feed(&mut f, "<c-d>");
    assert_eq!(f.state.cursor, 10, "ctrl-d jumps 10 rows");
    feed(&mut f, "<c-d>");
    assert_eq!(f.state.cursor, 11, "clamped at the last row");
    feed(&mut f, "<c-u>");
    assert_eq!(f.state.cursor, 1, "ctrl-u jumps 10 rows back");
}

#[test]
fn x_on_archived_revives_to_active() {
    // SPEC §4.1: x semantics — archived → active.
    let mut f = fx();
    let ids = seed(&mut f, &["old"]);
    feed(&mut f, "d"); // archive
    feed(&mut f, ":");
    type_prompt_text(&mut f, "archived");
    feed(&mut f, "<cr>");
    assert_eq!(f.state.rows.len(), 1);
    feed(&mut f, "x");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().status,
        intent_core::Status::Active
    );
}

#[test]
fn visual_y_writes_titles_to_clipboard() {
    let mut f = fx();
    seed(&mut f, &["first", "second"]);
    feed(&mut f, "V j y");
    let text = f.clip.read().unwrap_or_default();
    assert_eq!(
        text, "second\nfirst",
        "visual y yanks newline-joined titles (inbox is newest-first)"
    );
}

#[test]
fn list_yank_seeds_editor_paste() {
    // SPEC §4.5 rule 5: list y/Y write the system clipboard AND the edit
    // buffer, so p inside a later editor pastes the list yank.
    let mut f = fx();
    seed(&mut f, &["yanked title"]);
    feed(&mut f, "y");
    feed(&mut f, "<cr>"); // open editor on the task
    feed(&mut f, "P"); // paste edit buffer before cursor (title, col 0)
    assert_eq!(
        f.state.editor.as_ref().unwrap().title.text(),
        "yanked titleyanked title"
    );
    feed(&mut f, "<esc><esc>");
}
