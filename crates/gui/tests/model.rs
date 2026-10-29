//! Headless tests of the GUI interaction model — the same key stream the
//! GPUI layer delivers (SPEC §4, §5).

use intent_core::model::{NewTask, TaskView};
use intent_core::{ops, Store};
use intent_gui::model::{GuiState, PromptKind};
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
    assert!(all[0].inbox);
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
fn d_archives_and_space_d_unarchives() {
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
fn capital_d_delete_requires_confirmation_and_is_undoable() {
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
fn visual_complete_exact_rows() {
    // Inbox is newest-first: rows are [c, b, a] for seed order a, b, c.
    let mut f = fx();
    let ids = seed(&mut f, &["a", "b", "c"]);
    feed(&mut f, "V j x"); // select rows 0-1 (c, b)
    assert_eq!(
        status_of(&f, &ids[2]),
        intent_core::Status::Done,
        "row 0 (c) completed"
    );
    assert_eq!(
        status_of(&f, &ids[1]),
        intent_core::Status::Done,
        "row 1 (b) completed"
    );
    assert_eq!(
        status_of(&f, &ids[0]),
        intent_core::Status::Active,
        "row 2 (a) untouched"
    );
    feed(&mut f, "u");
    for id in &ids {
        assert_eq!(
            status_of(&f, id),
            intent_core::Status::Active,
            "undo restores all"
        );
    }
}

fn status_of(f: &Fixture, id: &str) -> intent_core::Status {
    f.state.store.get_task(id).unwrap().status
}

/// Deletes the temp dir when dropped — including on test failure, so a
/// panic mid-test cannot orphan SQLite's -wal/-shm sidecars.
struct TempDirGuard(std::path::PathBuf);

impl TempDirGuard {
    fn new(name: &str) -> TempDirGuard {
        let dir = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDirGuard(dir)
    }

    fn path(&self) -> std::path::PathBuf {
        self.0.join("tasks.db")
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn visual_complete_upward_selection_exact_rows() {
    let mut f = fx();
    let ids = seed(&mut f, &["a", "b", "c"]);
    feed(&mut f, "j V k x"); // cursor row 1, select up to row 0
    assert_eq!(
        status_of(&f, &ids[2]),
        intent_core::Status::Done,
        "row 0 completed"
    );
    assert_eq!(
        status_of(&f, &ids[1]),
        intent_core::Status::Done,
        "row 1 completed"
    );
    assert_eq!(
        status_of(&f, &ids[0]),
        intent_core::Status::Active,
        "row 2 untouched"
    );
}

#[test]
fn visual_archive_exact_rows() {
    let mut f = fx();
    let ids = seed(&mut f, &["a", "b", "c"]);
    feed(&mut f, "j V k d"); // select rows 0-1
    assert_eq!(status_of(&f, &ids[2]), intent_core::Status::Archived);
    assert_eq!(status_of(&f, &ids[1]), intent_core::Status::Archived);
    assert_eq!(
        status_of(&f, &ids[0]),
        intent_core::Status::Active,
        "row 2 untouched"
    );
    feed(&mut f, "u");
    assert_eq!(
        status_of(&f, &ids[2]),
        intent_core::Status::Active,
        "undo restores"
    );
}

#[test]
fn visual_arrow_keys_move_correct_direction() {
    let mut f = fx();
    seed(&mut f, &["a", "b", "c"]);
    feed(&mut f, "V");
    assert_eq!(f.state.cursor, 0);
    feed(&mut f, "<down>");
    assert_eq!(f.state.cursor, 1, "down extends downward");
    feed(&mut f, "<up>");
    assert_eq!(f.state.cursor, 0, "up retracts upward");
}

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
    let dir = TempDirGuard::new("intent-gui-ext");
    let path = dir.path();
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
    drop(other);
    drop(state);
    drop(dir); // explicit; Drop also cleans up on panic
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

// ---- round-2 audit regression tests ----------------------------------------

#[test]
fn ngg_with_count_lands_and_leaves_no_pending() {
    let mut f = fx();
    seed(&mut f, &["a", "b", "c", "d", "e", "f"]);
    feed(&mut f, "5 g g");
    assert_eq!(f.state.cursor, 4, "5gg lands on row 5");
    assert_eq!(f.state.pending_display(), "", "no dangling pending");
}

#[test]
fn repeat_preserves_original_count() {
    // `.` after 5x must complete 5 MORE (not 1) — distinguishable only when
    // enough rows remain.
    let mut f = fx();
    let names: Vec<&str> = (0..12)
        .map(|i| match i {
            0 => "t00",
            1 => "t01",
            2 => "t02",
            3 => "t03",
            4 => "t04",
            5 => "t05",
            6 => "t06",
            7 => "t07",
            8 => "t08",
            9 => "t09",
            10 => "t10",
            _ => "t11",
        })
        .collect();
    seed(&mut f, &names);
    feed(&mut f, "5 x");
    let done = |st: &GuiState| {
        st.store
            .all_tasks()
            .unwrap()
            .iter()
            .filter(|t| t.status == intent_core::Status::Done)
            .count()
    };
    assert_eq!(done(&f.state), 5);
    feed(&mut f, ".");
    assert_eq!(
        done(&f.state),
        10,
        "bare `.` reuses the original count (5x)"
    );
}

#[test]
fn repeat_explicit_count_replaces_original() {
    // Counts 1-4 switch views (SPEC §4.1), so explicit repeat counts use 5+.
    let mut f = fx();
    let names: Vec<&str> = (0..20)
        .map(|i| match i {
            0 => "t00",
            1 => "t01",
            2 => "t02",
            3 => "t03",
            4 => "t04",
            5 => "t05",
            6 => "t06",
            7 => "t07",
            8 => "t08",
            9 => "t09",
            10 => "t10",
            11 => "t11",
            12 => "t12",
            13 => "t13",
            14 => "t14",
            15 => "t15",
            16 => "t16",
            17 => "t17",
            18 => "t18",
            _ => "t19",
        })
        .collect();
    seed(&mut f, &names);
    feed(&mut f, "5 x");
    let done = |st: &GuiState| {
        st.store
            .all_tasks()
            .unwrap()
            .iter()
            .filter(|t| t.status == intent_core::Status::Done)
            .count()
    };
    assert_eq!(done(&f.state), 5);
    feed(&mut f, "7 .");
    assert_eq!(
        done(&f.state),
        12,
        "7. replaces the original 5 (bare . would give 10)"
    );
}

#[test]
fn delete_confirmation_y_n_esc_and_stray_keys() {
    let mut f = fx();
    let ids = seed(&mut f, &["precious"]);
    // stray keys are ignored — prompt stays open
    feed(&mut f, "D");
    feed(&mut f, "z");
    assert!(f.state.prompt.is_some(), "stray key ignored");
    feed(&mut f, "n");
    assert!(f.state.prompt.is_none());
    assert!(f.state.store.get_task(&ids[0]).is_ok());

    feed(&mut f, "D");
    feed(&mut f, "<esc>");
    assert!(f.state.prompt.is_none());
    assert!(f.state.store.get_task(&ids[0]).is_ok());

    feed(&mut f, "D");
    feed(&mut f, "y");
    assert!(f.state.store.get_task(&ids[0]).is_err(), "y confirms");
}

#[test]
fn project_without_argument_is_a_usage_error() {
    let mut f = fx();
    let ids = seed(&mut f, &["t"]);
    feed(&mut f, ":");
    type_prompt_text(&mut f, "project");
    feed(&mut f, "<cr>");
    let t = f.state.store.get_task(&ids[0]).unwrap();
    assert_eq!(t.project, None, "no argument mutates nothing");
    assert!(
        f.state.status.contains("usage"),
        "status: {:?}",
        f.state.status
    );
}

#[test]
fn notes_survive_empty_title() {
    let mut f = fx();
    let ids = seed(&mut f, &["keep"]);
    feed(&mut f, "<cr>"); // editor
    feed(&mut f, "d$"); // empty the title (normal mode, single-line)
    feed(&mut f, "x");
    feed(&mut f, "tab"); // to notes
    feed(&mut f, "i");
    type_prompt_text(&mut f, "saved notes");
    feed(&mut f, "<esc>");
    let t = f.state.store.get_task(&ids[0]).unwrap();
    assert_eq!(t.notes, "saved notes", "notes persist despite empty title");
    assert_eq!(t.title, "keep", "old title kept");
}

#[test]
fn search_clear_restores_pre_search_cursor() {
    let mut f = fx();
    seed(&mut f, &["alpha thing", "beta thing", "gamma"]);
    feed(&mut f, "j"); // cursor row 1
    feed(&mut f, "/");
    type_prompt_text(&mut f, "thing");
    feed(&mut f, "<cr>");
    assert_eq!(f.state.cursor, 0, "results start at the top");
    feed(&mut f, "<esc>");
    assert!(f.state.search_query.is_none());
    assert_eq!(f.state.cursor, 1, "pre-search cursor restored");
}

#[test]
fn external_reload_keeps_cursor_task() {
    let dir = TempDirGuard::new("intent-gui-reload");
    let path = dir.path();
    let store = Store::open(&path).unwrap();
    let clip = Rc::new(MemClipboard::new());
    let mut state = GuiState::new(store, clip);
    let a = ops::add(
        &state.store,
        NewTask {
            title: "a".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .task
    .unwrap();
    let _b = ops::add(
        &state.store,
        NewTask {
            title: "b".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .task
    .unwrap();
    state.switch_view(TaskView::Inbox);
    // Inbox is newest-first: rows [b, a]. Cursor on "a" (row 1).
    state.handle_key(Key::new("j"));
    assert_eq!(state.cursor_task().unwrap().id, a.id);

    // External CLI write lands a new task ABOVE the cursor task.
    let other = Store::open(&path).unwrap();
    ops::add(
        &other,
        NewTask {
            title: "new".into(),
            ..Default::default()
        },
    )
    .unwrap();

    assert!(state.check_external_changes());
    assert_eq!(
        state.cursor_task().unwrap().id,
        a.id,
        "cursor follows the task id across the reload"
    );
    drop(other);
    drop(state);
    drop(dir);
}

#[test]
fn empty_date_input_is_an_error_not_a_clear() {
    let mut f = fx();
    let ids = seed(&mut f, &["t"]);
    f.state.switch_view(TaskView::Today);
    let today = f.state.today;
    ops::edit(
        &f.state.store,
        &ids[0],
        &intent_core::TaskPatch {
            due: intent_core::Patch::Set(today),
            ..Default::default()
        },
    )
    .unwrap();
    f.state.switch_view(TaskView::Today);
    feed(&mut f, "<space> u");
    feed(&mut f, "<cr>"); // empty input
    assert!(f.state.prompt.is_some(), "prompt stays open");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().due,
        Some(today),
        "empty input must not clear the date"
    );
    feed(&mut f, "n o n e");
    feed(&mut f, "<cr>");
    assert_eq!(
        f.state.store.get_task(&ids[0]).unwrap().due,
        None,
        "none clears"
    );
}

// ---- capture focus + SPC SPC (help keymap regressions) ---------------------

#[test]
fn space_space_opens_capture() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    feed(&mut f, "<space> <space>");
    assert!(
        matches!(
            f.state.prompt.as_ref().map(|p| p.kind),
            Some(PromptKind::Capture)
        ),
        "SPC SPC opens the capture prompt, status: {:?}",
        f.state.status
    );
    feed(&mut f, "<esc>");
    assert!(f.state.prompt.is_none());
    // Other leader sequences still work afterwards.
    feed(&mut f, "<space> c");
    assert!(matches!(
        f.state.prompt.as_ref().map(|p| p.kind),
        Some(PromptKind::Capture)
    ));
}

#[test]
fn capture_moves_cursor_to_new_task_when_visible() {
    // SPEC §4.1: the new task is visible in the current view (inbox is
    // newest-first) → the cursor moves onto it.
    let mut f = fx();
    seed(&mut f, &["a", "b", "c", "d", "e"]);
    feed(&mut f, "j j j");
    let before = f.state.cursor_task().unwrap().title.clone();
    feed(&mut f, "o");
    type_prompt_text(&mut f, "fresh capture");
    feed(&mut f, "<cr>");
    let cur = f.state.cursor_task().unwrap();
    assert_eq!(cur.title, "fresh capture");
    assert_ne!(cur.title, before, "cursor left the old task");
}

#[test]
fn capture_in_today_keeps_cursor_on_existing_task() {
    // SPEC §4.1: the new task is NOT visible in the current view (undated
    // captures are inbox-only) → the cursor stays where it was.
    let mut f = fx();
    let ids = seed(&mut f, &["a", "b", "c"]);
    let today = f.state.today;
    for id in &ids {
        ops::edit(
            &f.state.store,
            id,
            &intent_core::TaskPatch {
                due: intent_core::Patch::Set(today),
                ..Default::default()
            },
        )
        .unwrap();
    }
    f.state.switch_view(TaskView::Today);
    feed(&mut f, "j");
    let before = f.state.cursor_task().unwrap().id.clone();
    feed(&mut f, "o");
    type_prompt_text(&mut f, "undated capture");
    feed(&mut f, "<cr>");
    assert!(f.state.prompt.is_none());
    assert_eq!(
        f.state.cursor_task().unwrap().id,
        before,
        "cursor stays on the existing task"
    );
    assert_eq!(
        f.state.rows.len(),
        3,
        "the undated capture does not enter Today"
    );
}

// ---- sweep round regressions -------------------------------------------------

#[test]
fn search_then_view_switch_keeps_remembered_cursor() {
    // SPEC §2: unfocused view state (cursor per view) survives — a search
    // commit must not reset the view's remembered cursor.
    let mut f = fx();
    seed(&mut f, &["alpha thing", "plain", "beta thing", "gamma"]);
    feed(&mut f, "j j j"); // cursor row 3
    feed(&mut f, "/");
    type_prompt_text(&mut f, "thing");
    feed(&mut f, "<cr>");
    assert_eq!(f.state.rows.len(), 2);
    feed(&mut f, "2"); // Today (empty)
    feed(&mut f, "1"); // back to Inbox
    assert_eq!(f.state.cursor, 3, "remembered inbox cursor survives search");
}

#[test]
fn count_then_leader_is_not_a_leader_sequence() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    feed(&mut f, "5 <space> d");
    assert!(
        f.state.prompt.is_none(),
        "count + SPC must not fire the leader"
    );
    assert!(!f.state.status.is_empty(), "rejected with a message");
}

#[test]
fn editor_engine_pending_blocks_leader_and_fires_first() {
    let mut f = fx();
    seed(&mut f, &["abc"]);
    feed(&mut f, "<cr>"); // editor, title "abc"
    feed(&mut f, "d"); // engine operator pending
    feed(&mut f, "<space>"); // must NOT arm the GUI leader
    feed(&mut f, "<esc>"); // cancels the operator AND closes the editor (normal)
    feed(&mut f, "<cr>"); // reopen: the stale `d` must not have fired
    feed(&mut f, "w");
    assert_eq!(
        f.state.editor.as_ref().unwrap().title.text(),
        "abc",
        "the stale `d` must not complete as dw later"
    );
    feed(&mut f, "<esc>");
    feed(&mut f, "<esc>");
}

#[test]
fn editor_tab_with_pending_goes_through_pending() {
    let mut f = fx();
    seed(&mut f, &["abc"]);
    feed(&mut f, "<cr>");
    feed(&mut f, "<space>"); // leader armed
    feed(&mut f, "tab");
    assert!(
        f.state.editor.as_ref().unwrap().focus_title,
        "tab while the leader is pending must not switch fields"
    );
    assert!(!f.state.status.is_empty(), "leader rejection is visible");
    feed(&mut f, "<esc>");
}

#[test]
fn list_i_inserts_at_cursor_a_appends() {
    let mut f = fx();
    seed(&mut f, &["abc"]);
    feed(&mut f, "i");
    assert!(f.state.editor.is_some());
    type_prompt_text(&mut f, "X");
    feed(&mut f, "<esc>");
    feed(&mut f, "<esc>");
    assert_eq!(
        f.state.store.get_task(&f.state.rows[0].id).unwrap().title,
        "Xabc",
        "i inserts before the first char"
    );
    feed(&mut f, "a");
    type_prompt_text(&mut f, "Y");
    feed(&mut f, "<esc>");
    feed(&mut f, "<esc>");
    assert_eq!(
        f.state.store.get_task(&f.state.rows[0].id).unwrap().title,
        "XabcY",
        "a appends at the end"
    );
}

#[test]
fn visual_mode_leader_works_and_unbound_keys_report() {
    let mut f = fx();
    seed(&mut f, &["a", "b"]);
    feed(&mut f, "V <space> d");
    assert!(
        matches!(
            f.state.prompt.as_ref().map(|p| p.kind),
            Some(PromptKind::Defer)
        ),
        "leader works inside visual mode"
    );
    feed(&mut f, "<esc>");
    feed(&mut f, "V");
    feed(&mut f, "z");
    assert!(
        f.state.status.contains("unbound"),
        "unbound visual keys are never silent"
    );
}

#[test]
fn zero_and_empty_list_ops_report() {
    let mut f = fx();
    seed(&mut f, &["a"]);
    feed(&mut f, "0");
    assert!(!f.state.status.is_empty(), "0 reports instead of silence");
    // empty list: x and d report like D does
    f.state.switch_view(TaskView::Today); // empty
    feed(&mut f, "x");
    assert!(!f.state.status.is_empty(), "x on an empty list reports");
    feed(&mut f, "d");
    assert!(!f.state.status.is_empty(), "d on an empty list reports");
}

#[test]
fn editor_shift_tab_returns_to_title() {
    let mut f = fx();
    seed(&mut f, &["abc"]);
    feed(&mut f, "<cr>");
    feed(&mut f, "tab"); // title -> notes
    assert!(!f.state.editor.as_ref().unwrap().focus_title);
    feed(&mut f, "<stab>");
    assert!(
        f.state.editor.as_ref().unwrap().focus_title,
        "shift-tab goes back to the title"
    );
    feed(&mut f, "<esc>");
    feed(&mut f, "<esc>");
}
