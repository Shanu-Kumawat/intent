//! The GUI state layer: surfaces, modes, pending keys, prompts, cursor
//! tracking, undo stacks, and every list/editor interaction from SPEC §4.
//! Pure logic — no GPUI imports — so the interaction model is unit-testable
//! without a display (SPEC §6).

use std::collections::BTreeMap;
use std::rc::Rc;

use intent_core::dates::{is_clear_marker, parse_date};
use intent_core::model::{Patch, Priority, Task, TaskPatch, TaskView};
use intent_core::ops::{self, RowChange};
use intent_core::views::{counts, for_view, search};
use intent_core::{NewTask, Store};

use crate::keys;
use intent_vim::{Clipboard, Engine, EngineEvent, Key, Register};

// ---------------------------------------------------------------------------
// public shape

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    List,
    Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Capture,
    Defer,
    Due,
    Remind,
    Organize,
    Search,
    Command,
    ConfirmDelete,
}

impl PromptKind {
    pub fn label(&self) -> &'static str {
        match self {
            PromptKind::Capture => "CAPTURE",
            PromptKind::Defer => "DEFER (until)",
            PromptKind::Due => "DUE",
            PromptKind::Remind => "REMIND",
            PromptKind::Organize => "ORGANIZE — project (empty = leave inbox)",
            PromptKind::Search => "SEARCH",
            PromptKind::Command => "COMMAND",
            PromptKind::ConfirmDelete => "DELETE PERMANENTLY? [y/N]",
        }
    }
}

pub struct Prompt {
    pub kind: PromptKind,
    pub input: String,
    pub error: Option<String>,
    /// Task the prompt acts on (editor task or cursor task at open time).
    pub target_id: Option<String>,
}

/// How the editor was entered from the list (SPEC §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorEntry {
    Normal,
    Insert,
    Append,
}

pub struct EditorState {
    pub task_id: String,
    pub title: Engine,
    pub notes: Engine,
    pub focus_title: bool,
}

impl EditorState {
    pub fn focused(&self) -> &Engine {
        if self.focus_title {
            &self.title
        } else {
            &self.notes
        }
    }

    pub fn focused_mut(&mut self) -> &mut Engine {
        if self.focus_title {
            &mut self.title
        } else {
            &mut self.notes
        }
    }

    pub fn focused_field_name(&self) -> &'static str {
        if self.focus_title {
            "TITLE"
        } else {
            "NOTES"
        }
    }
}

/// Wraps an Rc-shared clipboard so both the GUI and the vim engines see the
/// same system clipboard.
pub struct SharedClipboard(pub Rc<dyn Clipboard>);

impl Clipboard for SharedClipboard {
    fn read(&self) -> Option<String> {
        self.0.read()
    }
    fn write(&self, text: &str) {
        self.0.write(text);
    }
}

#[derive(Debug, Clone, Copy)]
enum Repeatable {
    Complete { n: usize },
    Archive { n: usize },
    ShiftDue { delta: i64, n: usize },
    Priority { delta: i8, n: usize },
}

pub struct GuiState {
    pub store: Store,
    pub today: intent_core::Date,
    clipboard: Rc<dyn Clipboard>,

    pub view: TaskView,
    pub rows: Vec<Task>,
    pub cursor: usize,
    pub visual_anchor: Option<usize>,
    pub search_query: Option<String>,
    pub view_counts: BTreeMap<TaskView, usize>,
    cursors: BTreeMap<TaskView, usize>,
    follow_id: Option<String>,
    /// Task id under the cursor when a search opened; restored on esc.
    pre_search_task: Option<String>,
    pending: Vec<Key>,

    pub editor: Option<EditorState>,
    pub prompt: Option<Prompt>,

    pub status: String,
    pub help_open: bool,
    pub quit_requested: bool,
    pub data_version: u64,

    undo: Vec<Vec<RowChange>>,
    redo: Vec<Vec<RowChange>>,
    last_op: Option<Repeatable>,
    /// Last list-level yank; seeds the editor's edit buffer (SPEC §4.5.5).
    list_edit_buffer: Register,
}

impl GuiState {
    pub fn new(store: Store, clipboard: Rc<dyn Clipboard>) -> GuiState {
        let mut s = GuiState {
            data_version: store.data_version().unwrap_or(0),
            today: intent_core::today(),
            clipboard,
            view: TaskView::Today,
            rows: Vec::new(),
            cursor: 0,
            visual_anchor: None,
            search_query: None,
            view_counts: BTreeMap::new(),
            cursors: BTreeMap::new(),
            follow_id: None,
            pre_search_task: None,
            pending: Vec::new(),
            editor: None,
            prompt: None,
            status: String::new(),
            help_open: false,
            quit_requested: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_op: None,
            list_edit_buffer: Register::charwise(String::new()),
            store,
        };
        s.refresh();
        s
    }

    // ---- data ---------------------------------------------------------------

    pub fn refresh(&mut self) {
        self.today = intent_core::today();
        let all = match self.store.all_tasks() {
            Ok(t) => t,
            Err(_) => return,
        };
        self.view_counts = counts(&all, self.today).into_iter().collect();
        let rows = match &self.search_query {
            Some(q) => search(&all, q, false),
            None => for_view(&all, self.view, self.today),
        };
        // Cursor follows the tracked task id, else clamps.
        if let Some(id) = self.follow_id.take() {
            if let Some(idx) = rows.iter().position(|t| t.id == id) {
                self.cursor = idx;
            } else {
                self.cursor = self.cursor.min(rows.len().saturating_sub(1));
            }
        } else {
            self.cursor = self.cursor.min(rows.len().saturating_sub(1));
        }
        self.cursors.insert(self.view, self.cursor);
        self.rows = rows;
        if self.visual_anchor.is_some() {
            self.visual_anchor = None; // mutations invalidate selections
        }
        // Re-baseline so our own writes are not mistaken for external ones.
        self.data_version = self.store.data_version().unwrap_or(self.data_version);
    }

    /// Host calls this when it wants to pick up external (CLI) changes.
    pub fn check_external_changes(&mut self) -> bool {
        match self.store.data_version() {
            Ok(v) if v != self.data_version => {
                self.data_version = v;
                // Cursor follows the task id across the reload (SPEC §2),
                // not the row index.
                self.follow_id = self.cursor_task().map(|t| t.id.clone());
                self.refresh();
                self.status = "database changed externally — reloaded".into();
                true
            }
            _ => false,
        }
    }

    pub fn cursor_task(&self) -> Option<&Task> {
        self.rows.get(self.cursor)
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    // ---- dispatch -------------------------------------------------------------

    pub fn handle_key(&mut self, key: Key) {
        if self.help_open {
            // Any key closes the overlay and is consumed (SPEC §4.1).
            self.help_open = false;
            return;
        }
        if self.prompt.is_some() {
            self.handle_prompt_key(key);
        } else if self.editor.is_some() {
            self.handle_editor_key(key);
        } else {
            self.handle_list_key(key);
        }
    }

    // ---- prompts -----------------------------------------------------------------

    fn open_prompt(&mut self, kind: PromptKind, target_id: Option<String>) {
        self.open_prompt_with_input(kind, target_id, String::new());
    }

    /// Reopening a prompt after an error preserves what was typed (SPEC §4.3:
    /// the prompt stays open; focus never bounces on bad input).
    fn open_prompt_with_input(
        &mut self,
        kind: PromptKind,
        target_id: Option<String>,
        input: String,
    ) {
        self.prompt = Some(Prompt {
            kind,
            input,
            error: None,
            target_id,
        });
    }

    fn handle_prompt_key(&mut self, key: Key) {
        // Deletion confirmation: y/Y confirms, n/N/esc cancels, every other
        // key is ignored so a stray press cannot confirm or dismiss (SPEC §4.1).
        if self.prompt.as_ref().map(|p| p.kind) == Some(PromptKind::ConfirmDelete) {
            if matches!(key.chr(), Some('y' | 'Y')) {
                let prompt = self.prompt.take().expect("prompt");
                if let Some(id) = prompt.target_id {
                    self.delete_task(&id);
                }
            } else if key.is_esc() || key.is_enter() || matches!(key.chr(), Some('n' | 'N')) {
                self.prompt = None;
                self.set_status("delete cancelled");
            }
            return;
        }
        let prompt = self.prompt.as_mut().expect("prompt");
        if key.is_esc() || (key.ctrl && key.key == "c") {
            self.prompt = None;
            self.set_status("");
            return;
        }
        if key.is_enter() {
            let prompt = self.prompt.take().expect("prompt");
            self.commit_prompt(prompt);
            return;
        }
        if key.is_backspace() {
            prompt.input.pop();
            return;
        }
        if key.ctrl && key.key == "w" {
            // delete last word
            let trimmed = prompt.input.trim_end_matches(' ');
            if let Some(pos) = trimmed.rfind(' ') {
                prompt.input.truncate(pos + 1);
            } else {
                prompt.input.clear();
            }
            return;
        }
        if key.ctrl && key.key == "u" {
            prompt.input.clear();
            return;
        }
        if key.ctrl && key.key == "v" {
            if let Some(text) = self.clipboard.read() {
                prompt.input.push_str(&text.replace('\n', " "));
            }
            return;
        }
        if let Some(c) = keys::plain_char(&key) {
            prompt.input.push(c);
        }
    }

    fn commit_prompt(&mut self, prompt: Prompt) {
        match prompt.kind {
            PromptKind::Capture => {
                let title = prompt.input.trim();
                let typed = prompt.input.clone();
                if title.is_empty() {
                    self.open_prompt_with_input(PromptKind::Capture, None, typed);
                    self.prompt_error("a title is required");
                    return;
                }
                let outcome = ops::add(
                    &self.store,
                    NewTask {
                        title: title.to_string(),
                        ..Default::default()
                    },
                );
                match outcome {
                    Ok(out) => {
                        let task = out.task.expect("add returns task");
                        self.follow_id = Some(task.id.clone());
                        if let Some(change) = &out.change {
                            self.push_undo_from(vec![change.clone()]);
                        }
                        self.set_status(format!("captured {}", task.id));
                    }
                    Err(e) => {
                        self.open_prompt_with_input(PromptKind::Capture, None, typed);
                        self.prompt_error(&e.to_string());
                        return;
                    }
                }
                self.refresh();
            }
            PromptKind::Defer | PromptKind::Due | PromptKind::Remind => {
                let id = match prompt.target_id {
                    Some(id) => id,
                    None => {
                        self.set_status("no task under cursor");
                        return;
                    }
                };
                let input = prompt.input.trim();
                let typed = prompt.input.clone();
                if input.is_empty() {
                    self.open_prompt_with_input(prompt.kind, Some(id), typed);
                    self.prompt_error("a date is required (none/- clears)");
                    return;
                }
                let value = if is_clear_marker(input) {
                    None
                } else {
                    match parse_date(input, self.today) {
                        Ok(d) => Some(d),
                        Err(e) => {
                            self.open_prompt_with_input(prompt.kind, Some(id), typed);
                            self.prompt_error(&e.to_string());
                            return;
                        }
                    }
                };
                let mut patch = TaskPatch::default();
                let slot = match prompt.kind {
                    PromptKind::Defer => &mut patch.defer_until,
                    PromptKind::Due => &mut patch.due,
                    _ => &mut patch.remind,
                };
                *slot = match value {
                    Some(d) => Patch::Set(d),
                    None => Patch::Clear,
                };
                self.apply_patch(&id, patch, "date updated");
            }
            PromptKind::Organize => {
                let id = match prompt.target_id {
                    Some(id) => id,
                    None => {
                        self.set_status("no task under cursor");
                        return;
                    }
                };
                let project = prompt.input.trim();
                let project = if project.is_empty() || is_clear_marker(project) {
                    None
                } else {
                    Some(project.to_string())
                };
                match ops::organize(&self.store, &id, project.as_deref()) {
                    Ok(out) => {
                        self.push_outcome(&out.change);
                        self.follow_id = Some(id.clone());
                        self.set_status("organized");
                    }
                    Err(e) => self.set_status(e.to_string()),
                }
                self.refresh();
            }
            PromptKind::Search => {
                let q = prompt.input.trim().to_string();
                // refresh() records the (reset) cursor for this view; the
                // search must not clobber the view's remembered position —
                // it is restored by esc via pre_search_task.
                let remembered = self.cursors.get(&self.view).copied();
                if q.is_empty() {
                    self.set_status("search cleared");
                    self.search_query = None;
                } else {
                    self.search_query = Some(q.clone());
                    self.cursor = 0;
                    self.set_status(format!("search: {q}"));
                }
                self.refresh();
                if let Some(c) = remembered {
                    self.cursors.insert(self.view, c);
                }
            }
            PromptKind::Command => {
                let line = prompt.input.trim();
                let cmd = line.split(' ').next().unwrap_or("");
                if GuiState::is_known_command(cmd) {
                    self.run_command(line);
                } else {
                    let err = format!("unknown command :{cmd}");
                    self.open_prompt_with_input(
                        PromptKind::Command,
                        prompt.target_id.clone(),
                        prompt.input.clone(),
                    );
                    self.prompt_error(&err);
                }
            }
            PromptKind::ConfirmDelete => unreachable!("handled in handle_prompt_key"),
        }
    }

    fn prompt_error(&mut self, msg: &str) {
        if let Some(p) = self.prompt.as_mut() {
            p.error = Some(msg.to_string());
        }
    }

    // ---- commands (:) ----------------------------------------------------------

    /// Execute a `:` command. Unknown commands are rejected by the caller
    /// (see `is_known_command`) so the prompt can reopen with an error.
    fn run_command(&mut self, input: &str) {
        let line = input.trim();
        let (cmd, rest) = match line.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (line, ""),
        };
        let target_id = self
            .editor
            .as_ref()
            .map(|e| e.task_id.clone())
            .or_else(|| self.cursor_task().map(|t| t.id.clone()));
        match cmd {
            "inbox" | "today" | "upcoming" | "completed" | "archived" => {
                self.switch_view(TaskView::parse(cmd).expect("matched view"));
            }
            "new" | "capture" => {
                if rest.is_empty() {
                    self.open_prompt(PromptKind::Capture, None);
                } else {
                    match ops::add(
                        &self.store,
                        NewTask {
                            title: rest.to_string(),
                            ..Default::default()
                        },
                    ) {
                        Ok(out) => {
                            let t = out.task.unwrap();
                            self.follow_id = Some(t.id.clone());
                            self.set_status(format!("captured {}", t.id));
                            if let Some(change) = &out.change {
                                self.push_undo_from(vec![change.clone()]);
                            }
                            self.refresh();
                        }
                        Err(e) => self.set_status(e.to_string()),
                    }
                }
            }
            "due" | "remind" | "defer" => {
                let kind = match cmd {
                    "due" => PromptKind::Due,
                    "remind" => PromptKind::Remind,
                    _ => PromptKind::Defer,
                };
                if rest.is_empty() {
                    self.open_prompt(kind, target_id);
                } else if let Some(id) = target_id {
                    let value = if is_clear_marker(rest) {
                        Ok(None)
                    } else {
                        parse_date(rest, self.today).map(Some)
                    };
                    match value {
                        Ok(d) => {
                            let mut patch = TaskPatch::default();
                            let slot = match cmd {
                                "due" => &mut patch.due,
                                "remind" => &mut patch.remind,
                                _ => &mut patch.defer_until,
                            };
                            *slot = match d {
                                Some(date) => Patch::Set(date),
                                None => Patch::Clear,
                            };
                            self.apply_patch(&id, patch, "date updated");
                        }
                        Err(e) => self.set_status(e.to_string()),
                    }
                } else {
                    self.set_status("no task selected");
                }
            }
            "project" => {
                if rest.is_empty() {
                    self.set_status("usage: :project <name|none>");
                } else if let Some(id) = target_id {
                    let patch = TaskPatch {
                        project: if is_clear_marker(rest) {
                            Patch::Clear
                        } else {
                            Patch::Set(rest.to_string())
                        },
                        ..Default::default()
                    };
                    self.apply_patch(&id, patch, "project set");
                } else {
                    self.set_status("no task selected");
                }
            }
            "tag" => {
                let Some(id) = target_id else {
                    self.set_status("no task selected");
                    return;
                };
                let mut neg = false;
                let mut tag = rest;
                if let Some(t) = rest.strip_prefix('+') {
                    tag = t;
                } else if let Some(t) = rest.strip_prefix('-') {
                    tag = t;
                    neg = true;
                }
                if tag.is_empty() {
                    self.set_status(":tag +name or :tag -name");
                    return;
                }
                let task = match self.store.get_task(&id) {
                    Ok(t) => t,
                    Err(e) => {
                        self.set_status(e.to_string());
                        return;
                    }
                };
                let mut tags = task.tags.clone();
                if neg {
                    tags.retain(|t| t != tag);
                } else if !tags.iter().any(|t| t == tag) {
                    tags.push(tag.to_string());
                }
                let patch = TaskPatch {
                    tags: Patch::Set(tags),
                    ..Default::default()
                };
                self.apply_patch(&id, patch, "tags updated");
            }
            "organize" => {
                if let Some(id) = target_id {
                    match ops::organize(&self.store, &id, None) {
                        Ok(out) => {
                            self.push_outcome(&out.change);
                            self.follow_id = Some(id);
                            self.set_status("organized");
                            self.refresh();
                        }
                        Err(e) => self.set_status(e.to_string()),
                    }
                }
            }
            "archive" | "unarchive" | "done" | "reopen" => {
                let Some(id) = target_id else {
                    self.set_status("no task selected");
                    return;
                };
                let result = match cmd {
                    "archive" => ops::archive(&self.store, &id),
                    "unarchive" => ops::unarchive(&self.store, &id),
                    "done" => ops::done(&self.store, &id),
                    _ => ops::reopen(&self.store, &id),
                };
                match result {
                    Ok(out) => {
                        self.push_outcome(&out.change);
                        self.follow_id = Some(id);
                        self.set_status(cmd.to_string());
                        self.refresh();
                    }
                    Err(e) => self.set_status(e.to_string()),
                }
            }
            "delete" => {
                if let Some(id) = target_id {
                    self.open_prompt(PromptKind::ConfirmDelete, Some(id));
                } else {
                    self.set_status("no task selected");
                }
            }
            "refresh" => {
                self.refresh();
                self.set_status("reloaded");
            }
            "help" => self.help_open = true,
            "q" => {
                if self.editor.is_some() {
                    self.close_editor();
                } else {
                    self.quit_requested = true;
                }
            }
            "w" => self.set_status("everything is already saved"),
            "" => {}
            other => self.set_status(format!("unknown command :{other}")),
        }
    }

    const KNOWN_COMMANDS: &[&str] = &[
        "inbox",
        "today",
        "upcoming",
        "completed",
        "archived",
        "new",
        "capture",
        "due",
        "remind",
        "defer",
        "project",
        "tag",
        "organize",
        "archive",
        "unarchive",
        "done",
        "reopen",
        "delete",
        "refresh",
        "help",
        "q",
        "w",
    ];

    fn is_known_command(cmd: &str) -> bool {
        GuiState::KNOWN_COMMANDS.contains(&cmd)
    }

    // ---- editor -----------------------------------------------------------------

    fn open_editor(&mut self, id: &str, entry: EditorEntry) {
        let Ok(task) = self.store.get_task(id) else {
            self.set_status("task vanished");
            return;
        };
        let clip_box = || Box::new(SharedClipboard(self.clipboard.clone()));
        let mut title = Engine::new(&task.title, true, clip_box());
        let mut notes = Engine::new(&task.notes, false, clip_box());
        match entry {
            // `enter`: open in NORMAL. `a`: insert after the last char.
            // `i`: insert mode at the cursor — distinct entries, like vim.
            EditorEntry::Normal => {}
            EditorEntry::Insert => {
                title.handle(Key::new("i"));
            }
            EditorEntry::Append => {
                title.handle(Key::shifted("a"));
            }
        }
        let _ = &mut notes;
        let seed = self.list_edit_buffer.clone();
        title.set_edit_buffer(seed.clone());
        notes.set_edit_buffer(seed);
        self.editor = Some(EditorState {
            task_id: id.to_string(),
            title,
            notes,
            focus_title: true,
        });
        self.set_status("");
    }

    fn close_editor(&mut self) {
        let Some(ed) = self.editor.take() else { return };
        // SPEC §5: editor text changes are NOT pushed to the task undo
        // stack — their recoverability lives in the editor's own text undo
        // while it is open. Closing the editor ends that history.
        self.follow_id = Some(ed.task_id.clone());
        self.refresh();
    }

    fn switch_editor_field(&mut self, to_title: bool, carry_col: bool) {
        let Some(ed) = self.editor.as_mut() else {
            return;
        };
        if ed.focus_title == to_title {
            return;
        }
        let (l, c) = ed.focused().cursor();
        let target = if to_title {
            &mut ed.title
        } else {
            &mut ed.notes
        };
        let line = l.min(target.buf.line_count() - 1);
        let col = if carry_col { c } else { 0 };
        target.set_cursor((line, col));
        ed.focus_title = to_title;
    }

    fn handle_editor_key(&mut self, key: Key) {
        // Local copies to avoid holding the editor borrow across calls.
        let focus_title = self.editor.as_ref().map(|e| e.focus_title).unwrap_or(true);
        let engine_mode = self
            .editor
            .as_ref()
            .map(|e| e.focused().mode)
            .unwrap_or(intent_vim::Mode::Normal);
        let editor_task = self.editor.as_ref().map(|e| e.task_id.clone());

        // Pending sequences always win: the GUI leader and the engine's
        // operator-pending state must not coexist invisibly (a stale `d`
        // firing later wrote through destructively).
        if !self.pending.is_empty() {
            self.handle_list_pending(key.clone());
            return;
        }
        let engine_pending = self
            .editor
            .as_ref()
            .map(|e| !e.focused().pending_display().is_empty())
            .unwrap_or(false);

        // Tab advances fields in normal/visual; in insert it types spaces.
        // With a sequence pending (either layer) it routes through the
        // pending handler instead of silently switching fields.
        if key.is_tab() && !key.ctrl && engine_mode != intent_vim::Mode::Insert && !engine_pending {
            // shift-tab goes to the previous field (title), tab advances.
            let to_title = if key.shift { true } else { !focus_title };
            self.switch_editor_field(to_title, true);
            return;
        }

        // Leader: active in normal/visual only (insert types spaces), and
        // never while the engine has an operator pending — space goes to
        // the engine then, like vim (where it is not a motion either).
        let in_text_insert = engine_mode == intent_vim::Mode::Insert;
        if key.key == "space" && !key.ctrl && !in_text_insert && !engine_pending {
            self.pending.push(key.clone());
            return;
        }

        // ":" opens the command prompt against the editor's task.
        if key.key == ":" && !key.ctrl && !in_text_insert {
            if let Some(id) = editor_task {
                self.open_prompt(PromptKind::Command, Some(id));
            }
            return;
        }
        // Esc with nothing pending in NORMAL mode closes the editor; in
        // visual mode it must fall through to the engine (exits visual).
        if key.is_esc() && !in_text_insert && engine_mode == intent_vim::Mode::Normal {
            let pending_empty = self
                .editor
                .as_ref()
                .map(|e| e.focused().pending_display().is_empty())
                .unwrap_or(true);
            if pending_empty {
                self.close_editor();
                return;
            }
        }

        let event = match self.editor.as_mut() {
            Some(ed) => ed.focused_mut().handle(key),
            None => return,
        };
        let task_id = match &editor_task {
            Some(id) => id.clone(),
            None => return,
        };
        match event {
            EngineEvent::NextField => self.switch_editor_field(false, true),
            EngineEvent::PrevField => self.switch_editor_field(true, true),
            EngineEvent::TextChanged => self.write_editor_through(&task_id),
            EngineEvent::None => {}
        }
    }

    fn write_editor_through(&mut self, task_id: &str) {
        let (title, notes) = match self.editor.as_ref() {
            Some(ed) => (ed.title.text(), ed.notes.text()),
            None => return,
        };
        let Ok(mut task) = self.store.get_task(task_id) else {
            return;
        };
        // Notes persist independently of the title (SPEC §4.2: fields are
        // independent buffers). A title is only ever empty mid-edit; the old
        // one stays until it is valid again.
        task.notes = notes;
        if title.trim().is_empty() {
            self.set_status("title must not be empty — notes saved");
        } else if title.contains('\n') || title.contains('\r') {
            self.set_status("titles are single-line — notes saved");
        } else {
            task.title = title;
            self.status.clear();
        }
        task.updated_at = intent_core::Timestamp::now();
        if let Err(e) = self.store.update_task(&task) {
            self.set_status(e.to_string());
        }
    }

    // ---- list ---------------------------------------------------------------------

    /// Switch views programmatically (key handler and tests both use this).
    pub fn switch_view(&mut self, view: TaskView) {
        if self.editor.is_some() {
            self.close_editor();
        }
        if self.view != view {
            self.cursor = self.cursors.get(&view).copied().unwrap_or(0);
        }
        self.view = view;
        self.search_query = None;
        self.visual_anchor = None;
        self.refresh();
        self.set_status("");
    }

    fn cycle_view(&mut self, forward: bool) {
        let order = [
            TaskView::Inbox,
            TaskView::Today,
            TaskView::Upcoming,
            TaskView::Completed,
        ];
        let idx = order.iter().position(|v| *v == self.view).unwrap_or(1);
        let next = if forward {
            (idx + 1) % order.len()
        } else {
            (idx + order.len() - 1) % order.len()
        };
        self.switch_view(order[next]);
    }

    fn move_cursor(&mut self, delta: i64, count: usize) {
        let n = (delta * count as i64) as isize;
        let max = self.rows.len().saturating_sub(1) as isize;
        let target = (self.cursor as isize + n).clamp(0, max);
        self.cursor = target as usize;
        // Plain navigation is remembered per view (SPEC §2: unfocused view
        // state survives) — refresh alone would leave the map stale.
        self.cursors.insert(self.view, self.cursor);
    }

    fn pending_is_count_only(&self) -> bool {
        self.pending
            .iter()
            .all(|k| k.chr().map(|c| c.is_ascii_digit()).unwrap_or(false))
    }

    fn handle_list_key(&mut self, key: Key) {
        if key.is_esc() {
            if !self.pending.is_empty() {
                self.pending.clear();
                self.set_status("");
            } else if self.visual_anchor.is_some() {
                self.visual_anchor = None;
                self.set_status("");
            } else if self.search_query.is_some() {
                self.search_query = None;
                // Back to the task that was under the cursor before the
                // search (SPEC §2: by id, not by row index).
                self.follow_id = self.pre_search_task.take();
                self.refresh();
                self.set_status("search cleared");
            } else {
                self.set_status("");
            }
            return;
        }
        // Leader continuation also works from visual (SPEC: space is always
        // the leader, on every surface, in every mode).
        if self.pending.iter().any(|k| k.key == "space") {
            self.pending.push(key.clone());
            let seq = std::mem::take(&mut self.pending);
            if seq.first().map(|k| k.key == "space").unwrap_or(false) {
                self.run_leader(seq);
            } else {
                self.set_status("leader takes no count");
            }
            return;
        }
        if self.visual_anchor.is_some() {
            self.handle_visual_key(key);
            return;
        }
        self.handle_list_pending(key);
    }

    /// One key in list-normal mode, including pending counts, leader, and `g`.
    fn handle_list_pending(&mut self, key: Key) {
        // Leader continuation (space was pressed earlier). A count before
        // the space makes it NOT a leader sequence (`5 SPC d` must not
        // fire capture).
        if self.pending.iter().any(|k| k.key == "space") {
            self.pending.push(key.clone());
            let seq = std::mem::take(&mut self.pending);
            if seq.first().map(|k| k.key == "space").unwrap_or(false) {
                self.run_leader(seq);
            } else {
                self.set_status("leader takes no count");
            }
            return;
        }

        match (key.ctrl, key.key.as_str()) {
            (true, "r") => {
                self.pending.clear();
                self.redo();
                return;
            }
            (true, "d") => {
                self.pending.clear();
                self.move_cursor(1, 10);
                return;
            }
            (true, "u") => {
                self.pending.clear();
                self.move_cursor(-1, 10);
                return;
            }
            _ => {}
        }
        if key.ctrl {
            self.pending.clear();
            return;
        }

        if key.is_esc() {
            self.pending.clear();
            self.set_status("");
            return;
        }

        let Some(c) = key.chr() else {
            // Named keys: arrows, tab, enter, space (leader).
            match key.key.as_str() {
                "up" => {
                    let (count, _) = self.take_count();
                    self.move_cursor(-1, count);
                }
                "down" => {
                    let (count, _) = self.take_count();
                    self.move_cursor(1, count);
                }
                "tab" => {
                    self.pending.clear();
                    self.cycle_view(!key.shift);
                }
                "enter" => {
                    self.pending.clear();
                    self.open_cursor_editor(EditorEntry::Normal);
                }
                "space" => self.pending.push(key.clone()),
                _ => {}
            }
            if key.key != "space" {
                self.pending.clear();
            }
            return;
        };

        // Pending `g` (gg): a leading count may precede the first g (5gg).
        if self.pending.last().map(|k| k.chr()) == Some(Some('g')) {
            if c == 'g' {
                let count = self
                    .pending
                    .iter()
                    .take_while(|k| k.chr().map(|c| c.is_ascii_digit()).unwrap_or(false))
                    .fold(0usize, |n, k| {
                        n * 10 + (k.chr().unwrap().to_digit(10).unwrap() as usize)
                    });
                self.move_cursor_to(if count > 0 { count - 1 } else { 0 });
            } else {
                self.set_status("sequence cancelled");
            }
            self.pending.clear();
            return;
        }

        if c.is_ascii_digit() {
            if self.pending.is_empty() {
                // No count pending: 1-4 switch views, 5-9 start a count.
                match c {
                    '1' => self.switch_view(TaskView::Inbox),
                    '2' => self.switch_view(TaskView::Today),
                    '3' => self.switch_view(TaskView::Upcoming),
                    '4' => self.switch_view(TaskView::Completed),
                    '5'..='9' => self.pending.push(key),
                    // 0 cannot start a count and there is no column axis.
                    _ => self.set_status("0 is unbound in the list (counts start at 5)"),
                }
            } else if self.pending_is_count_only() {
                self.pending.push(key);
            } else {
                self.set_status("sequence cancelled");
                self.pending.clear();
            }
            return;
        }

        let (count, had_count) = self.take_count();

        match c {
            'j' => self.move_cursor(1, count),
            'k' => self.move_cursor(-1, count),
            'g' => {
                // gg always waits for the second g — with or without a count
                // (5gg lands on row 5). Re-enter pending with the count.
                if had_count {
                    self.pending = format!("{count}g")
                        .chars()
                        .map(|ch| Key::new(&ch.to_string()))
                        .collect();
                } else {
                    self.pending.push(Key::new("g"));
                }
            }
            'G' => self.move_cursor_to(self.rows.len().saturating_sub(1)),
            'h' | 'l' => self.set_status("lists have no horizontal axis"),
            'x' => self.complete_range(count),
            'd' => self.archive_range(count),
            'D' => {
                if let Some(id) = self.cursor_task().map(|t| t.id.clone()) {
                    self.open_prompt(PromptKind::ConfirmDelete, Some(id));
                } else {
                    self.set_status("no task under cursor");
                }
            }
            '>' => self.shift_due(1, count),
            '<' => self.shift_due(-1, count),
            '+' | '=' => self.bump_priority(1, count),
            '-' => self.bump_priority(-1, count),
            'y' => self.yank_task(false),
            'Y' => self.yank_task(true),
            'V' => {
                if !self.rows.is_empty() {
                    self.visual_anchor = Some(self.cursor);
                    if count > 1 {
                        self.move_cursor(1, count - 1);
                    }
                }
            }
            'o' => self.open_prompt(PromptKind::Capture, None),
            'i' => self.open_cursor_editor(EditorEntry::Insert),
            'a' => self.open_cursor_editor(EditorEntry::Append),
            '/' => {
                self.pre_search_task = self.cursor_task().map(|t| t.id.clone());
                self.open_prompt(PromptKind::Search, None);
            }
            ':' => {
                let target = self.cursor_task().map(|t| t.id.clone());
                self.open_prompt(PromptKind::Command, target);
            }
            '?' => self.help_open = true,
            'u' => self.undo(),
            '.' => self.repeat_last(count, had_count),
            _ => self.set_status(format!("unbound key: {}", key.display())),
        }
    }

    /// Extract a leading count from pending keys, clearing them.
    fn take_count(&mut self) -> (usize, bool) {
        let mut n: Option<usize> = None;
        let mut consumed = 0;
        for k in &self.pending {
            if let Some(d) = k.chr().and_then(|c| c.to_digit(10)) {
                // Saturate + cap, mirroring the vim engine's parse_count.
                n = Some(match n {
                    None => d as usize,
                    Some(prev) => prev
                        .saturating_mul(10)
                        .saturating_add(d as usize)
                        .min(100_000),
                });
                consumed += 1;
            } else {
                break;
            }
        }
        let had = n.is_some();
        self.pending.drain(..consumed);
        (n.unwrap_or(1).max(1), had)
    }

    fn move_cursor_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.rows.len().saturating_sub(1));
        self.cursors.insert(self.view, self.cursor);
    }

    fn run_leader(&mut self, seq: Vec<Key>) {
        // seq[0] is the space; seq[1] is the leader command key. A second
        // space is the quick-capture binding — `chr()` is None for the
        // multi-char "space" key name, so match it explicitly.
        let cmd = match seq.get(1) {
            None => None,
            Some(k) if k.key == "space" => Some(' '),
            Some(k) => k.chr(),
        };
        let Some(cmd) = cmd else {
            self.set_status("leader: c d u r o D p space");
            return;
        };
        let target = self
            .editor
            .as_ref()
            .map(|e| e.task_id.clone())
            .or_else(|| self.cursor_task().map(|t| t.id.clone()));
        match cmd {
            ' ' | 'c' => self.open_prompt(PromptKind::Capture, None),
            'd' => self.open_prompt(PromptKind::Defer, target),
            'u' => self.open_prompt(PromptKind::Due, target),
            'r' => self.open_prompt(PromptKind::Remind, target),
            'o' => self.open_prompt(PromptKind::Organize, target),
            'D' => {
                if let Some(id) = target {
                    match ops::unarchive(&self.store, &id) {
                        Ok(out) => {
                            self.push_outcome(&out.change);
                            self.follow_id = Some(id);
                            self.set_status("unarchived");
                        }
                        Err(e) => self.set_status(e.to_string()),
                    }
                    self.refresh();
                } else {
                    self.set_status("no task selected");
                }
            }
            'p' => {
                let pasted = self
                    .editor
                    .as_mut()
                    .map(|ed| ed.focused_mut().paste_from_clipboard())
                    .unwrap_or(false);
                if pasted {
                    if let Some(ed) = self.editor.as_ref() {
                        let id = ed.task_id.clone();
                        self.write_editor_through(&id);
                    }
                } else if self.editor.is_none() {
                    self.set_status("no text target for paste in the list");
                }
            }
            other => self.set_status(format!("unknown leader key {other}")),
        }
    }

    fn handle_visual_key(&mut self, key: Key) {
        match (key.ctrl, key.key.as_str()) {
            (false, "down") | (false, "j") => {
                self.move_cursor(1, 1);
                return;
            }
            (false, "up") | (false, "k") => {
                self.move_cursor(-1, 1);
                return;
            }
            (false, "x") => {
                let (a, b) = self.visual_range();
                self.complete_rows(a, b);
                self.visual_anchor = None;
                return;
            }
            (false, "d") => {
                let (a, b) = self.visual_range();
                self.archive_rows(a, b);
                self.visual_anchor = None;
                return;
            }
            (false, "y") => {
                let (a, b) = self.visual_range();
                self.yank_rows(a, b);
                self.visual_anchor = None;
                return;
            }
            (false, "V") => {
                self.visual_anchor = None;
                return;
            }
            (false, "space") => {
                self.pending.push(key.clone());
                return;
            }
            _ => {}
        }
        if key.is_esc() {
            self.visual_anchor = None;
            return;
        }
        // Never a silent no-op (SPEC §4.1).
        self.set_status(format!("unbound in visual: {}", key.display()));
    }

    fn visual_range(&self) -> (usize, usize) {
        let a = self.visual_anchor.unwrap_or(self.cursor);
        let (lo, hi) = if a <= self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        };
        (lo, hi.min(self.rows.len().saturating_sub(1)))
    }

    // ---- task operations ---------------------------------------------------------

    fn open_cursor_editor(&mut self, entry: EditorEntry) {
        match self.cursor_task() {
            Some(t) => {
                let id = t.id.clone();
                self.open_editor(&id, entry);
            }
            None => self.set_status("nothing to open"),
        }
    }

    fn row_ids(&self, from: usize, count: usize) -> Vec<String> {
        (from..from + count)
            .filter(|i| *i < self.rows.len())
            .map(|i| self.rows[i].id.clone())
            .collect()
    }

    fn complete_range(&mut self, count: usize) {
        let ids = self.row_ids(self.cursor, count);
        self.complete_ids(ids);
        self.last_op = Some(Repeatable::Complete { n: count });
    }

    fn complete_rows(&mut self, a: usize, b: usize) {
        let n = self.row_ids(a, b - a + 1).len();
        let ids = self.row_ids(a, n);
        self.complete_ids(ids);
        self.last_op = Some(Repeatable::Complete { n });
    }

    fn complete_ids(&mut self, ids: Vec<String>) {
        if ids.is_empty() {
            self.set_status("nothing to complete");
            return;
        }
        // One transaction for the whole batch (SPEC §2): partial results are
        // impossible, and the undo entry only exists after a successful
        // commit.
        match ops::complete_many(&self.store, &ids) {
            Ok(changes) if !changes.is_empty() => {
                self.follow_id = Some(changes[0].id.clone());
                self.push_undo_from(changes);
                self.set_status(if ids.len() == 1 {
                    "completion toggled"
                } else {
                    "completions toggled"
                });
            }
            Ok(_) => {}
            Err(e) => self.set_status(e.to_string()),
        }
        self.refresh();
    }

    fn archive_range(&mut self, count: usize) {
        let ids = self.row_ids(self.cursor, count);
        self.archive_ids(ids);
        self.last_op = Some(Repeatable::Archive { n: count });
    }

    fn archive_rows(&mut self, a: usize, b: usize) {
        let n = self.row_ids(a, b - a + 1).len();
        let ids = self.row_ids(a, n);
        self.archive_ids(ids);
        self.last_op = Some(Repeatable::Archive { n });
    }

    fn archive_ids(&mut self, ids: Vec<String>) {
        if ids.is_empty() {
            self.set_status("nothing to archive");
            return;
        }
        match ops::archive_many(&self.store, &ids) {
            Ok(changes) if !changes.is_empty() => {
                self.follow_id = Some(changes[0].id.clone());
                self.push_undo_from(changes);
                self.set_status("archived");
            }
            Ok(_) => {}
            Err(e) => self.set_status(e.to_string()),
        }
        self.refresh();
    }

    fn shift_due(&mut self, delta: i64, count: usize) {
        let Some(task) = self.cursor_task().cloned() else {
            self.set_status("no task under cursor");
            return;
        };
        let new_due = match task.due {
            Some(d) => Some(d.add_days(delta * count as i64)),
            None if delta > 0 => Some(self.today),
            None => None,
        };
        let patch = TaskPatch {
            due: match new_due {
                Some(d) => Patch::Set(d),
                None => Patch::Keep, // '<' with no due does nothing
            },
            ..Default::default()
        };
        if patch.due.is_keep() {
            self.set_status("no due date to shift");
            return;
        }
        self.apply_patch(&task.id, patch, "due shifted");
        self.last_op = Some(Repeatable::ShiftDue { delta, n: count });
    }

    fn bump_priority(&mut self, delta: i8, count: usize) {
        let Some(task) = self.cursor_task().cloned() else {
            self.set_status("no task under cursor");
            return;
        };
        let mut p = task.priority;
        for _ in 0..count {
            p = if delta > 0 { p.raise() } else { p.lower() };
        }
        let patch = TaskPatch {
            priority: Patch::Set(p),
            ..Default::default()
        };
        self.apply_patch(&task.id, patch, "priority changed");
        self.last_op = Some(Repeatable::Priority { delta, n: count });
    }

    /// y = title; Y = title + notes (SPEC §4.1).
    fn yank_task(&mut self, with_notes: bool) {
        let Some(task) = self.cursor_task().cloned() else {
            self.set_status("no task under cursor");
            return;
        };
        let text = if with_notes && !task.notes.is_empty() {
            format!("{}\n{}", task.title, task.notes)
        } else {
            task.title.clone()
        };
        self.list_edit_buffer = Register::charwise(text.clone());
        self.clipboard.write(&text);
        self.set_status(format!("yanked {}", task.id));
    }

    fn yank_rows(&mut self, a: usize, b: usize) {
        let titles: Vec<String> = self.rows[a..=b].iter().map(|t| t.title.clone()).collect();
        let text = titles.join("\n");
        self.list_edit_buffer = Register::charwise(text.clone());
        self.clipboard.write(&text);
        self.set_status(format!("yanked {} titles", titles.len()));
    }

    fn apply_patch(&mut self, id: &str, patch: TaskPatch, msg: &str) {
        match ops::edit(&self.store, id, &patch) {
            Ok(out) => {
                self.push_outcome(&out.change);
                self.follow_id = Some(id.to_string());
                self.set_status(msg);
            }
            Err(e) => self.set_status(e.to_string()),
        }
        self.refresh();
    }

    fn delete_task(&mut self, id: &str) {
        match ops::delete(&self.store, id) {
            Ok(Some(out)) => {
                if let Some(change) = out.change {
                    self.push_undo_from(vec![change]);
                }
                if self
                    .editor
                    .as_ref()
                    .map(|e| e.task_id == id)
                    .unwrap_or(false)
                {
                    self.editor = None;
                }
                self.set_status(format!("deleted {id} (undoable this session)"));
            }
            Ok(None) => self.set_status("already gone"),
            Err(e) => self.set_status(e.to_string()),
        }
        self.refresh();
    }

    fn repeat_last(&mut self, count: usize, had_count: bool) {
        // SPEC §4.1: an explicit count replaces the original count; a bare
        // `.` repeats with the original.
        match self.last_op {
            Some(Repeatable::Complete { n }) => {
                self.complete_range(if had_count { count } else { n })
            }
            Some(Repeatable::Archive { n }) => {
                self.archive_range(if had_count { count } else { n })
            }
            Some(Repeatable::ShiftDue { delta, n }) => {
                self.shift_due(delta, if had_count { count } else { n })
            }
            Some(Repeatable::Priority { delta, n }) => {
                self.bump_priority(delta, if had_count { count } else { n })
            }
            None => self.set_status("nothing to repeat"),
        }
    }

    // ---- undo ----------------------------------------------------------------------

    fn push_outcome(&mut self, change: &Option<RowChange>) {
        if let Some(c) = change {
            self.push_undo_from(vec![c.clone()]);
        }
    }

    fn push_undo_from(&mut self, changes: Vec<RowChange>) {
        if changes.is_empty() {
            return;
        }
        self.undo.push(changes);
        self.redo.clear();
    }

    fn undo(&mut self) {
        if self.editor.is_some() {
            self.set_status("undo inside the editor edits text (u passes through)");
            return;
        }
        match self.undo.last() {
            Some(entry) => {
                // Atomic: on failure nothing is applied and the stack stays.
                match ops::apply_changes(&self.store, entry, false) {
                    Ok(()) => {
                        let entry = self.undo.pop().expect("checked above");
                        self.follow_id = entry.first().map(|c| c.id.clone());
                        self.redo.push(entry);
                        self.set_status("undo");
                        self.refresh();
                    }
                    Err(e) => self.set_status(format!("undo failed: {e}")),
                }
            }
            None => self.set_status("nothing to undo"),
        }
    }

    fn redo(&mut self) {
        if self.editor.is_some() {
            self.set_status("redo inside the editor edits text (ctrl-r passes through)");
            return;
        }
        match self.redo.last() {
            Some(entry) => match ops::apply_changes(&self.store, entry, true) {
                Ok(()) => {
                    let entry = self.redo.pop().expect("checked above");
                    self.follow_id = entry.first().map(|c| c.id.clone());
                    self.undo.push(entry);
                    self.set_status("redo");
                    self.refresh();
                }
                Err(e) => self.set_status(format!("redo failed: {e}")),
            },
            None => self.set_status("nothing to redo"),
        }
    }

    /// Cursor line/column within the focused editor field (1-based for
    /// display), or None when no editor is open.
    pub fn editor_position(&self) -> Option<(usize, usize)> {
        let ed = self.editor.as_ref()?;
        let (l, c) = ed.focused().cursor();
        Some((l + 1, c + 1))
    }

    /// The task the editor has open, if any.
    pub fn editor_task_id(&self) -> Option<&str> {
        self.editor.as_ref().map(|e| e.task_id.as_str())
    }

    pub fn pending_display(&self) -> String {
        // The focused surface wins (SPEC §4: pending state is always
        // visible — editor sequences like `d` included).
        if let Some(ed) = &self.editor {
            let engine_pending = ed.focused().pending_display();
            if !engine_pending.is_empty() {
                return engine_pending;
            }
        }
        self.pending
            .iter()
            .map(|k| {
                if k.key == "space" {
                    "SPC".to_string()
                } else {
                    k.display()
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn mode_label(&self) -> String {
        if self.prompt.is_some() {
            return "PROMPT".into();
        }
        if let Some(ed) = &self.editor {
            let mode = ed.focused().mode_name();
            return format!("EDITOR {} {}", mode, ed.focused_field_name());
        }
        if self.visual_anchor.is_some() {
            return "VISUAL".into();
        }
        "NORMAL".into()
    }
}

// Nicer priority display for the UI.
pub fn priority_mark(p: Priority) -> &'static str {
    match p {
        Priority::High => "!!!",
        Priority::Medium => "++",
        Priority::Low => "+",
        _ => "",
    }
}
