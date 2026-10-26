use crate::buffer::{char_class, Buffer, CharClass, Pos};
use crate::key::Key;
use crate::register::{Clipboard, Register};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
        }
    }
}

/// Signals the host may care about (field navigation in the task editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineEvent {
    None,
    /// j at the bottom boundary (or any j in a single-line buffer)
    NextField,
    /// k at the top boundary (or any k in a single-line buffer)
    PrevField,
    /// The buffer text changed (host may write through)
    TextChanged,
}

impl EngineEvent {
    fn merge(&mut self, other: EngineEvent) {
        if *self == EngineEvent::None {
            *self = other;
        } else if other == EngineEvent::TextChanged && *self != EngineEvent::TextChanged {
            // TextChanged wins over nothing-only; navigation events keep it.
            if *self == EngineEvent::None {
                *self = other;
            }
        } else if *self == EngineEvent::TextChanged {
            // keep
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Delete,
    Change,
    Yank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordStart { big: bool },
    WordBack { big: bool },
    WordEnd { big: bool },
    LineCol0,
    FirstNonBlank,
    LineEnd,
    /// gg/G. `line` is 1-based; None means "count" (G default last, gg first).
    GotoLine { line: Option<usize> },
    Find { fwd: bool, till: bool, ch: char },
    FindRepeat { reverse: bool },
}

impl Motion {
    fn affectedness(&self) -> Affected {
        match self {
            Motion::WordEnd { .. } | Motion::LineEnd | Motion::Find { .. } => Affected::Inclusive,
            Motion::Up | Motion::Down | Motion::GotoLine { .. } => Affected::Linewise,
            _ => Affected::Exclusive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Affected {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Motion(Motion),
    TextObj { inner: bool, obj: char },
    Lines,
}

#[derive(Debug, Clone, PartialEq)]
enum Cmd {
    Move { count: usize, motion: Motion },
    Op { count: usize, op: OpKind, target: Target },
    EnterInsert { kind: InsertKind },
    DeleteChars { count: usize, fwd: bool },
    ReplaceChar { count: usize, ch: char },
    ToggleCase { count: usize },
    Join { count: usize },
    Paste { count: usize, after: bool },
    Undo,
    Redo,
    Repeat { count: usize },
    ToggleVisual { linewise: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertKind {
    Before,     // i
    After,      // a
    LineStart,  // I
    LineEnd,    // A (also o/O in single-line buffers)
    OpenBelow,  // o
    OpenAbove,  // O
}

enum Parse {
    NeedMore,
    Invalid,
    Ready(Cmd),
}

struct Snapshot {
    lines: Vec<Vec<char>>,
    cursor: Pos,
    desired_col: usize,
}

/// The modal editing engine (SPEC §4.2). One engine = one text field.
pub struct Engine {
    pub buf: Buffer,
    pub mode: Mode,
    pub status: String,
    cursor: Pos,
    desired_col: usize,
    anchor: Pos,
    pending: Vec<Key>,
    edit_buffer: Register,
    clipboard: Box<dyn Clipboard>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group_open: bool,
    applying_undo: bool,
    last_change: Vec<Key>,
    /// Keys of the normal-mode sequence that started the current insert run.
    last_trigger: Vec<Key>,
    insert_keys: Vec<Key>,
    /// Where the current insert run began (per-line bound for ctrl-u/ctrl-w).
    insert_origin: Option<Pos>,
    find: Option<(char, bool, bool)>, // (char, fwd, till)
    replaying: usize,
    pub single_line: bool,
}

impl Engine {
    pub fn new(text: &str, single_line: bool, clipboard: Box<dyn Clipboard>) -> Engine {
        let buf = Buffer::from_text(text);
        let cursor = buf.clamp_normal((0, 0));
        Engine {
            mode: Mode::Normal,
            status: String::new(),
            desired_col: cursor.1,
            cursor,
            anchor: cursor,
            pending: Vec::new(),
            edit_buffer: Register::charwise(String::new()),
            clipboard,
            undo: Vec::new(),
            redo: Vec::new(),
            group_open: false,
            applying_undo: false,
            last_change: Vec::new(),
            last_trigger: Vec::new(),
            insert_keys: Vec::new(),
            insert_origin: None,
            find: None,
            replaying: 0,
            single_line,
            buf,
        }
    }

    // ---- public accessors ---------------------------------------------------

    pub fn text(&self) -> String {
        self.buf.to_text()
    }

    pub fn set_text(&mut self, text: &str) {
        self.buf = Buffer::from_text(text);
        self.cursor = self.buf.clamp_normal((0, 0));
        self.desired_col = self.cursor.1;
        self.pending.clear();
        self.undo.clear();
        self.redo.clear();
        self.mode = Mode::Normal;
        self.status.clear();
    }

    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    pub fn set_cursor(&mut self, pos: Pos) {
        self.cursor = self.buf.clamp_normal(pos);
        self.desired_col = self.cursor.1;
    }

    /// Active selection for rendering: ordered inclusive start / exclusive
    /// end, plus linewise flag.
    pub fn selection(&self) -> Option<(Pos, Pos, bool)> {
        match self.mode {
            Mode::Visual => {
                let (a, b) = self.buf.order(self.anchor, self.cursor);
                let end = self.buf.pos_after(b);
                if a >= end {
                    None
                } else {
                    Some((a, end, false))
                }
            }
            Mode::VisualLine => {
                let (a, b) = self.buf.order(self.anchor, self.cursor);
                Some(((a.0, 0), (b.0, self.buf.line_len(b.0)), true))
            }
            _ => None,
        }
    }

    /// Pending key sequence for the status line (SPEC §4 "no timeouts").
    pub fn pending_display(&self) -> String {
        self.pending
            .iter()
            .map(|k| k.display())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn mode_name(&self) -> &'static str {
        self.mode.name()
    }

    // ---- dispatch --------------------------------------------------------------

    pub fn handle(&mut self, key: Key) -> EngineEvent {
        match self.mode {
            Mode::Insert => self.handle_insert(key),
            Mode::Normal => self.handle_normal(key),
            Mode::Visual | Mode::VisualLine => self.handle_visual(key),
        }
    }

    // ---- insert ------------------------------------------------------------------

    fn handle_insert(&mut self, key: Key) -> EngineEvent {
        if key.is_esc() || (key.ctrl && key.key == "c") {
            self.exit_insert();
            return EngineEvent::None;
        }
        if self.replaying == 0 {
            self.insert_keys.push(key.clone());
        }
        if key.is_enter() {
            if self.single_line {
                self.status = "titles are single-line".into();
                return EngineEvent::None;
            }
            self.begin_change();
            let (l, c) = self.cursor;
            let rest: Vec<char> = self.buf.lines[l][c..].to_vec();
            self.buf.lines[l].truncate(c);
            self.buf.lines.insert(l + 1, rest);
            self.cursor = (l + 1, 0);
            self.desired_col = 0;
            return EngineEvent::TextChanged;
        }
        if key.is_backspace() {
            if self.cursor == (0, 0) {
                return EngineEvent::None;
            }
            self.begin_change();
            if self.cursor.1 == 0 {
                let (l, c) = self.cursor;
                let cur: Vec<char> = self.buf.lines.remove(l);
                self.buf.lines[l - 1].extend(cur);
                self.cursor = (l - 1, c);
            } else {
                let (l, c) = self.cursor;
                self.buf.lines[l].remove(c - 1);
                self.cursor = (l, c - 1);
            }
            self.desired_col = self.cursor.1;
            return EngineEvent::TextChanged;
        }
        if key.is_tab() {
            self.begin_change();
            for _ in 0..4 {
                self.insert_raw(' ');
            }
            return EngineEvent::TextChanged;
        }
        if key.ctrl && key.key == "w" {
            let mut start = self.ctrl_w_start();
            if let Some(origin) = self.insert_origin {
                if origin.0 == self.cursor.0 && start.1 < origin.1 {
                    start = (origin.0, origin.1);
                }
            }
            if start != self.cursor {
                self.begin_change();
                let (a, b) = self.buf.order(start, self.cursor);
                self.buf.remove_charwise(a, b);
                self.cursor = a;
                self.desired_col = a.1;
                return EngineEvent::TextChanged;
            }
            return EngineEvent::None;
        }
        if key.ctrl && key.key == "u" {
            // Delete back to where this insert run started on this line —
            // never into text that existed before insert mode (Vim rule).
            let target = match self.insert_origin {
                Some(o) if o.0 == self.cursor.0 => o.1,
                _ => 0,
            };
            if self.cursor.1 > target {
                self.begin_change();
                let (l, c) = self.cursor;
                self.buf.lines[l].drain(target..c);
                self.cursor = (l, target);
                self.desired_col = target;
                return EngineEvent::TextChanged;
            }
            return EngineEvent::None;
        }
        if let Some(c) = plain_char(&key) {
            self.begin_change();
            self.insert_raw(c);
            return EngineEvent::TextChanged;
        }
        self.status = format!("unhandled key {}", key.display());
        EngineEvent::None
    }

    fn insert_raw(&mut self, c: char) {
        self.buf.insert_char(self.cursor, c);
        self.cursor.1 += 1;
        self.desired_col = self.cursor.1;
    }

    fn ctrl_w_start(&self) -> Pos {
        let mut pos = self.cursor;
        loop {
            let prev = self.buf.pos_before(pos);
            if prev == pos {
                return pos;
            }
            match self.buf.char_at(prev) {
                Some(c) if c.is_whitespace() => pos = prev,
                _ => break,
            }
        }
        let class = match self.buf.char_at(self.buf.pos_before(pos)) {
            Some(c) => char_class(c, true),
            None => return pos,
        };
        let mut p = pos;
        loop {
            let prev = self.buf.pos_before(p);
            if prev == p {
                return p;
            }
            match self.buf.char_at(prev) {
                Some(c) if char_class(c, true) == class => p = prev,
                _ => return p,
            }
        }
    }

    fn enter_insert(&mut self, kind: InsertKind) {
        let (l, c) = self.cursor;
        match kind {
            InsertKind::Before => {}
            InsertKind::After => self.cursor = (l, (c + 1).min(self.buf.line_len(l))),
            InsertKind::LineStart => self.cursor = (l, self.buf.first_non_blank(l)),
            InsertKind::LineEnd => self.cursor = (l, self.buf.line_len(l)),
            InsertKind::OpenBelow | InsertKind::OpenAbove => {
                if self.single_line {
                    self.cursor = (l, self.buf.line_len(l));
                } else {
                    self.begin_change();
                    let at = if kind == InsertKind::OpenBelow { l + 1 } else { l };
                    self.buf.lines.insert(at, Vec::new());
                    self.cursor = (at, 0);
                }
            }
        }
        self.desired_col = self.cursor.1;
        self.mode = Mode::Insert;
        self.insert_origin = Some(self.cursor);
        self.insert_keys.clear();
    }

    fn exit_insert(&mut self) {
        if self.mode != Mode::Insert {
            return;
        }
        self.mode = Mode::Normal;
        self.cursor = self.buf.clamp_normal(self.cursor);
        self.desired_col = self.cursor.1;
        self.end_change();
        if self.replaying == 0 && !self.last_trigger.is_empty() {
            let mut seq = std::mem::take(&mut self.last_trigger);
            seq.extend(self.insert_keys.iter().cloned());
            seq.push(Key::new("esc"));
            self.last_change = seq;
        }
        self.last_trigger.clear();
        self.insert_keys.clear();
    }

    // ---- normal ---------------------------------------------------------------------

    fn handle_normal(&mut self, key: Key) -> EngineEvent {
        let mut ev = EngineEvent::None;
        if key.is_esc() {
            self.pending.clear();
            self.status.clear();
            return ev;
        }
        self.pending.push(key.clone());
        match parse(&self.pending) {
            Parse::NeedMore => {}
            Parse::Invalid => {
                self.pending.clear();
                self.status = "sequence cancelled".into();
            }
            Parse::Ready(cmd) => {
                let keys = std::mem::take(&mut self.pending);
                self.execute(cmd, &keys, &mut ev);
            }
        }
        ev
    }

    fn execute(&mut self, cmd: Cmd, keys: &[Key], ev: &mut EngineEvent) {
        let mut changed = false;
        match cmd {
            Cmd::Move { count, motion } => {
                ev.merge(self.do_motion(count, motion));
            }
            Cmd::ToggleVisual { linewise } => {
                if (self.mode == Mode::Visual && !linewise)
                    || (self.mode == Mode::VisualLine && linewise)
                {
                    self.mode = Mode::Normal;
                } else {
                    self.mode = if linewise { Mode::VisualLine } else { Mode::Visual };
                }
                self.anchor = self.cursor;
            }
            Cmd::EnterInsert { kind } => {
                if self.replaying == 0 {
                    self.last_trigger = keys.to_vec();
                }
                self.enter_insert(kind);
            }
            Cmd::Op { count, op, target } => {
                changed = self.do_op(count, op, target, keys);
            }
            Cmd::DeleteChars { count, fwd } => {
                let (l, c) = self.cursor;
                if fwd {
                    let end = (c + count).min(self.buf.line_len(l));
                    if end > c {
                        self.begin_change();
                        let text = self.buf.remove_charwise((l, c), (l, end));
                        self.edit_buffer = Register::charwise(text);
                        changed = true;
                        self.end_change();
                        self.record_change(keys);
                    }
                } else {
                    let start = c.saturating_sub(count);
                    if start < c {
                        self.begin_change();
                        let text = self.buf.remove_charwise((l, start), (l, c));
                        self.edit_buffer = Register::charwise(text);
                        self.cursor = (l, start);
                        changed = true;
                        self.end_change();
                        self.record_change(keys);
                    }
                }
                self.clamp_cursor();
            }
            Cmd::ReplaceChar { count, ch } => {
                let (l, c) = self.cursor;
                if c + count <= self.buf.line_len(l) {
                    self.begin_change();
                    for i in 0..count {
                        self.buf.lines[l][c + i] = ch;
                    }
                    self.cursor = (l, c + count - 1);
                    changed = true;
                    self.end_change();
                    self.record_change(keys);
                } else {
                    self.status = "not enough characters".into();
                }
            }
            Cmd::ToggleCase { count } => {
                let (l, c) = self.cursor;
                if c + count <= self.buf.line_len(l) {
                    self.begin_change();
                    for i in 0..count {
                        let ch = self.buf.lines[l][c + i];
                        self.buf.lines[l][c + i] = if ch.is_uppercase() {
                            ch.to_lowercase().next().unwrap_or(ch)
                        } else {
                            ch.to_uppercase().next().unwrap_or(ch)
                        };
                    }
                    // Vim moves the cursor past the last toggled char.
                    self.cursor = (l, (c + count).min(self.line_last_col_at(l)));
                    changed = true;
                    self.end_change();
                    self.record_change(keys);
                } else {
                    self.status = "not enough characters".into();
                }
            }
            Cmd::Join { count } => {
                for _ in 0..count.max(1) {
                    let l = self.cursor.0;
                    if l + 1 >= self.buf.line_count() {
                        break;
                    }
                    self.begin_change();
                    let mut next: Vec<char> = self.buf.lines.remove(l + 1);
                    while next.first().map(|c| c.is_whitespace()).unwrap_or(false) {
                        next.remove(0);
                    }
                    let cur = &mut self.buf.lines[l];
                    if !cur.is_empty()
                        && !cur.last().map(|c| c.is_whitespace()).unwrap_or(false)
                    {
                        cur.push(' ');
                    }
                    cur.extend(next);
                    changed = true;
                }
                if changed {
                    self.end_change();
                    self.record_change(keys);
                }
                self.clamp_cursor();
            }
            Cmd::Paste { count, after } => {
                let reg = self.edit_buffer.clone();
                if reg.text.is_empty() {
                    self.status = "edit buffer is empty".into();
                    return;
                }
                self.begin_change();
                let text = reg.text.repeat(count.max(1));
                if reg.linewise {
                    let at = if after { self.cursor.0 + 1 } else { self.cursor.0 };
                    self.buf.insert_lines(at, &text);
                    let line = at.min(self.buf.line_count() - 1);
                    self.cursor = (line, self.buf.first_non_blank(line));
                } else {
                    let (l, c) = self.cursor;
                    let at = if after && self.buf.line_len(l) > 0 { (l, c + 1) } else { (l, c) };
                    self.cursor = self.buf.insert_text(at, &text);
                }
                changed = true;
                self.end_change();
                self.record_change(keys);
                self.clamp_cursor();
            }
            Cmd::Undo => self.undo(),
            Cmd::Redo => self.redo(),
            Cmd::Repeat { count } => {
                let seq = self.last_change.clone();
                if seq.is_empty() {
                    self.status = "nothing to repeat".into();
                    return;
                }
                self.replaying += 1;
                for _ in 0..count.max(1) {
                    for k in &seq {
                        if self.handle(k.clone()) == EngineEvent::TextChanged {
                            changed = true;
                        }
                    }
                }
                self.replaying -= 1;
            }
        }
        if changed {
            ev.merge(EngineEvent::TextChanged);
        }
    }

    fn record_change(&mut self, keys: &[Key]) {
        if self.replaying == 0 {
            self.last_change = keys.to_vec();
        }
    }

    // ---- motions ----------------------------------------------------------------------

    fn do_motion(&mut self, count: usize, motion: Motion) -> EngineEvent {
        let mut ev = EngineEvent::None;
        match motion {
            Motion::Left => {
                self.cursor.1 = self.cursor.1.saturating_sub(count);
                self.sync_desired();
            }
            Motion::Right => {
                self.cursor.1 = (self.cursor.1 + count).min(self.line_last_col());
                self.sync_desired();
            }
            Motion::Up | Motion::Down => {
                if self.single_line {
                    ev = if motion == Motion::Down {
                        EngineEvent::NextField
                    } else {
                        EngineEvent::PrevField
                    };
                } else if motion == Motion::Down {
                    if self.cursor.0 == self.buf.line_count() - 1 {
                        ev = EngineEvent::NextField;
                    } else {
                        self.cursor.0 = (self.cursor.0 + count).min(self.buf.line_count() - 1);
                        self.apply_desired();
                    }
                } else if self.cursor.0 == 0 {
                    ev = EngineEvent::PrevField;
                } else {
                    self.cursor.0 = self.cursor.0.saturating_sub(count);
                    self.apply_desired();
                }
            }
            Motion::LineCol0 => {
                self.cursor.1 = 0;
                self.sync_desired();
            }
            Motion::FirstNonBlank => {
                self.cursor.1 = self.buf.first_non_blank(self.cursor.0);
                self.sync_desired();
            }
            Motion::LineEnd => {
                let line = (self.cursor.0 + count.saturating_sub(1))
                    .min(self.buf.line_count() - 1);
                self.cursor.0 = line;
                self.cursor.1 = self.line_last_col_at(line);
                self.sync_desired();
            }
            Motion::WordStart { big } => {
                for _ in 0..count.max(1) {
                    match word_fwd(&self.buf, self.cursor, big) {
                        Some(p) => self.cursor = p,
                        None => {
                            self.cursor = self.buf.last_pos();
                            break;
                        }
                    }
                }
                self.sync_desired();
            }
            Motion::WordBack { big } => {
                for _ in 0..count.max(1) {
                    match word_back(&self.buf, self.cursor, big) {
                        Some(p) => self.cursor = p,
                        None => {
                            self.cursor = (0, 0);
                            break;
                        }
                    }
                }
                self.sync_desired();
            }
            Motion::WordEnd { big } => {
                let mut p = self.cursor;
                let mut ok = true;
                for _ in 0..count.max(1) {
                    match word_end(&self.buf, p, big) {
                        Some(q) => p = q,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    self.cursor = p;
                } else {
                    self.status = "motion failed".into();
                }
                self.sync_desired();
            }
            Motion::GotoLine { line } => {
                // gg → count-th line (default first); G → count-th (default last).
                let target = match line {
                    Some(n) => n.saturating_sub(1),
                    None => self.buf.line_count() - 1,
                };
                self.cursor.0 = target.min(self.buf.line_count() - 1);
                self.cursor.1 = self.buf.first_non_blank(self.cursor.0);
                self.sync_desired();
            }
            Motion::Find { fwd, till, ch } => {
                self.find = Some((ch, fwd, till));
                self.apply_find(count.max(1), fwd, till, ch);
            }
            Motion::FindRepeat { reverse } => {
                if let Some((ch, fwd, till)) = self.find {
                    let fwd = if reverse { !fwd } else { fwd };
                    self.apply_find(count.max(1), fwd, till, ch);
                } else {
                    self.status = "no previous find".into();
                }
            }
        }
        ev
    }

    fn apply_find(&mut self, count: usize, fwd: bool, till: bool, ch: char) {
        let (l, c) = self.cursor;
        let line: Vec<char> = self.buf.lines[l].clone();
        let target = if fwd {
            let mut hits = line.iter().enumerate().filter(|(_, &x)| x == ch).map(|(i, _)| i).filter(|&i| i > c);
            let mut found = None;
            for _ in 0..count {
                match hits.next() {
                    Some(i) => found = Some(i),
                    None => break,
                }
            }
            found.map(|i| if till { i - 1 } else { i })
        } else {
            let mut hits: Vec<usize> =
                line.iter().enumerate().filter(|(_, &x)| x == ch).map(|(i, _)| i).filter(|&i| i < c).collect();
            hits.reverse();
            let mut found = None;
            for _ in 0..count {
                if hits.is_empty() {
                    break;
                }
                found = Some(hits.remove(0));
            }
            found.map(|i| if till { i + 1 } else { i })
        };
        match target {
            Some(col) => {
                self.cursor.1 = col;
                self.sync_desired();
            }
            None => self.status = format!("'{ch}' not found"),
        }
    }

    fn line_last_col(&self) -> usize {
        self.line_last_col_at(self.cursor.0)
    }

    fn line_last_col_at(&self, line: usize) -> usize {
        if self.buf.line_len(line) == 0 {
            0
        } else {
            self.buf.line_len(line) - 1
        }
    }

    fn sync_desired(&mut self) {
        self.desired_col = self.cursor.1;
    }

    fn apply_desired(&mut self) {
        let l = self.cursor.0;
        self.cursor.1 = if self.buf.line_len(l) == 0 {
            0
        } else {
            self.desired_col.min(self.buf.line_len(l) - 1)
        };
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.buf.clamp_normal(self.cursor);
        self.desired_col = self.desired_col.min(self.cursor.1.max(self.desired_col));
    }

    // ---- operators ---------------------------------------------------------------------

    fn do_op(&mut self, count: usize, op: OpKind, target: Target, keys: &[Key]) -> bool {
        let region = match target {
            Target::Lines => {
                Some(Region::lines(self.cursor.0, self.cursor.0 + count.max(1) - 1))
            }
            Target::TextObj { inner, obj } => self.text_object(inner, obj),
            Target::Motion(m) => {
                // Vim rule: cw behaves like ce when starting on a non-blank.
                let m = if let (OpKind::Change, Motion::WordStart { big: false }) = (op, m) {
                    match self.buf.char_at(self.cursor) {
                        Some(c) if !c.is_whitespace() => Motion::WordEnd { big: false },
                        _ => m,
                    }
                } else {
                    m
                };
                self.motion_region(count.max(1), m)
            }
        };
        let region = match region {
            Some(r) => r,
            None => {
                self.status = "motion failed".into();
                return false;
            }
        };

        if op == OpKind::Yank {
            let (text, linewise, cursor) = match region.kind {
                RegionKind::Char { start, end } => {
                    let (a, b) = self.buf.order(start, end);
                    let mut text = String::new();
                    extract_charwise(&self.buf, a, b, &mut text);
                    (text, false, self.buf.clamp_normal(a))
                }
                RegionKind::Line { from, to } => {
                    let mut text = String::new();
                    extract_lines(&self.buf, from, to, &mut text);
                    (text, true, (from.min(self.buf.line_count() - 1), 0))
                }
            };
            let reg = if linewise {
                Register::linewise(text)
            } else {
                Register::charwise(text)
            };
            // Yank writes the system clipboard too (SPEC §4.5).
            self.clipboard.write(&reg.text);
            self.edit_buffer = reg;
            self.cursor = cursor;
            if linewise {
                self.cursor.1 = self.buf.first_non_blank(self.cursor.0);
            }
            self.record_change(keys);
            return false;
        }

        self.begin_change();
        let (text, linewise) = match region.kind {
            RegionKind::Char { start, end } => {
                let text = self.buf.remove_charwise(start, end);
                self.cursor = self.buf.clamp_normal(start);
                (text, false)
            }
            RegionKind::Line { from, to } => {
                let text = self.buf.remove_lines(from, to);
                self.cursor = (from.min(self.buf.line_count() - 1), 0);
                (text, true)
            }
        };
        // Delete/change set the edit buffer only; clipboard untouched (SPEC §4.5).
        self.edit_buffer = if linewise {
            Register::linewise(text)
        } else {
            Register::charwise(text)
        };

        match op {
            OpKind::Delete => {
                self.end_change();
                self.record_change(keys);
                true
            }
            OpKind::Change => {
                if linewise {
                    let line = self.cursor.0.min(self.buf.line_count());
                    self.buf.lines.insert(line, Vec::new());
                    self.cursor = (line, 0);
                }
                if self.replaying == 0 {
                    self.last_trigger = keys.to_vec();
                }
                self.mode = Mode::Insert;
                self.insert_origin = Some(self.cursor);
                self.insert_keys.clear();
                self.group_open = true; // continue group through the insert run
                true
            }
            OpKind::Yank => unreachable!(),
        }
    }

    fn motion_region(&self, count: usize, motion: Motion) -> Option<Region> {
        let from = self.cursor;
        match motion.affectedness() {
            Affected::Linewise => {
                let dest_line = match motion {
                    Motion::Down => from.0 + count,
                    Motion::Up => from.0.saturating_sub(count),
                    Motion::GotoLine { line: Some(n) } => n.saturating_sub(1),
                    Motion::GotoLine { line: None } => self.buf.line_count() - 1,
                    _ => return None,
                };
                let dest = dest_line.min(self.buf.line_count() - 1);
                Some(Region::lines(from.0.min(dest), from.0.max(dest)))
            }
            _ => {
                let dest = probe_motion(self, count, motion)?;
                if motion_is_forward(self, motion, dest) {
                    let end = match motion.affectedness() {
                        Affected::Inclusive => self.buf.pos_after(dest),
                        _ => dest,
                    };
                    let (a, b) = self.buf.order(from, end);
                    Some(Region::char(a, b))
                } else {
                    // Backward: [target, cursor). The cursor char survives;
                    // inclusive backward motions include the target char,
                    // which this range already does.
                    let (a, b) = self.buf.order(dest, from);
                    Some(Region::char(a, b))
                }
            }
        }
    }

    // ---- text objects --------------------------------------------------------------------

    fn text_object(&self, inner: bool, obj: char) -> Option<Region> {
        match obj {
            'w' | 'W' => {
                let big = obj == 'W';
                let class = match self.buf.char_at(self.cursor) {
                    Some(c) => char_class(c, big),
                    None => return None,
                };
                let mut a = self.cursor;
                let mut b = self.cursor;
                if class == CharClass::Whitespace {
                    while let Some(p) = step_while(&self.buf, a, class, false) {
                        a = p;
                    }
                    while let Some(p) = step_while(&self.buf, b, class, true) {
                        b = p;
                    }
                    if inner {
                        Some(Region::char(a, self.buf.pos_after(b)))
                    } else {
                        // aw on whitespace swallows following whitespace+word
                        let mut e = self.buf.pos_after(b);
                        while matches!(self.buf.char_at(e), Some(c) if char_class(c, big) == CharClass::Whitespace)
                        {
                            e = self.buf.pos_after(e);
                        }
                        Some(Region::char(a, e))
                    }
                } else {
                    while let Some(p) = step_while(&self.buf, a, class, false) {
                        a = p;
                    }
                    while let Some(p) = step_while(&self.buf, b, class, true) {
                        b = p;
                    }
                    let end = self.buf.pos_after(b);
                    if inner {
                        Some(Region::char(a, end))
                    } else {
                        let mut e = end;
                        while matches!(self.buf.char_at(e), Some(c) if char_class(c, big) == CharClass::Whitespace)
                        {
                            e = self.buf.pos_after(e);
                        }
                        if e == end {
                            // no trailing whitespace: take leading instead
                            while let Some(p) =
                                step_while(&self.buf, a, CharClass::Whitespace, false)
                            {
                                a = p;
                            }
                        }
                        Some(Region::char(a, e))
                    }
                }
            }
            '"' | '\'' | '`' => {
                let q = obj;
                let (l, c) = self.cursor;
                let line = &self.buf.lines[l];
                let bound = if line.is_empty() { 0 } else { c.min(line.len() - 1) };
                let open = line[..=bound].iter().rposition(|&x| x == q)?;
                let close = line[(open + 1)..].iter().position(|&x| x == q)? + open + 1;
                if inner {
                    Some(Region::char((l, open + 1), (l, close)))
                } else {
                    Some(Region::char((l, open), (l, close + 1)))
                }
            }
            '(' | ')' | 'b' => {
                let open = self.scan_back(self.cursor, '(')?;
                let close = self.scan_fwd(self.cursor, ')')?;
                if inner {
                    Some(Region::char((open.0, open.1 + 1), close))
                } else {
                    Some(Region::char(open, (close.0, close.1 + 1)))
                }
            }
            _ => None,
        }
    }

    fn scan_back(&self, from: Pos, target: char) -> Option<Pos> {
        let mut p = from;
        if self.buf.char_at(p) == Some(target) {
            return Some(p);
        }
        loop {
            let prev = self.buf.pos_before(p);
            if prev == p {
                return None;
            }
            p = prev;
            if self.buf.char_at(p) == Some(target) {
                return Some(p);
            }
        }
    }

    fn scan_fwd(&self, from: Pos, target: char) -> Option<Pos> {
        let mut p = from;
        loop {
            let next = self.buf.pos_after(p);
            if next == p {
                return None;
            }
            p = next;
            if self.buf.char_at(p) == Some(target) {
                return Some(p);
            }
        }
    }

    // ---- clipboard --------------------------------------------------------------------------

    /// Seed the edit buffer (hosts share list-level yanks, SPEC §4.5 rule 5).
    pub fn set_edit_buffer(&mut self, reg: Register) {
        self.edit_buffer = reg;
    }

    /// `Space p` in the editor: paste the system clipboard at the cursor.
    pub fn paste_from_clipboard(&mut self) -> bool {
        match self.clipboard.read() {
            Some(text) if !text.is_empty() => {
                self.begin_change();
                self.cursor = self.buf.insert_text(self.cursor, &text);
                self.clamp_cursor();
                self.end_change();
                self.status.clear();
                true
            }
            _ => {
                self.status = "clipboard is empty".into();
                false
            }
        }
    }

    // ---- undo -------------------------------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.buf.lines.clone(),
            cursor: self.cursor,
            desired_col: self.desired_col,
        }
    }

    fn begin_change(&mut self) {
        if self.applying_undo {
            return;
        }
        if !self.group_open {
            self.undo.push(self.snapshot());
            self.redo.clear();
            self.group_open = true;
        }
    }

    fn end_change(&mut self) {
        self.group_open = false;
    }

    fn undo(&mut self) {
        self.end_change();
        match self.undo.pop() {
            Some(s) => {
                self.redo.push(self.snapshot());
                self.restore(s);
                self.status = "undo".into();
            }
            None => self.status = "already at oldest change".into(),
        }
    }

    fn redo(&mut self) {
        match self.redo.pop() {
            Some(s) => {
                self.undo.push(self.snapshot());
                self.restore(s);
                self.status = "redo".into();
            }
            None => self.status = "already at newest change".into(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.applying_undo = true;
        self.buf.lines = s.lines;
        self.cursor = s.cursor;
        self.desired_col = s.desired_col;
        self.mode = Mode::Normal;
        self.applying_undo = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ---- visual -------------------------------------------------------------------------------

    fn handle_visual(&mut self, key: Key) -> EngineEvent {
        let mut ev = EngineEvent::None;
        if key.is_esc() {
            self.mode = Mode::Normal;
            self.pending.clear();
            return ev;
        }
        self.pending.push(key.clone());
        match parse_visual(&self.pending) {
            VisualParse::NeedMore => {}
            VisualParse::Invalid => {
                self.pending.clear();
                self.status = "sequence cancelled".into();
            }
            VisualParse::Ready(cmd) => {
                let keys = std::mem::take(&mut self.pending);
                self.execute_visual(cmd, &keys, &mut ev);
            }
        }
        ev
    }

    fn execute_visual(&mut self, cmd: VisualCmd, keys: &[Key], ev: &mut EngineEvent) {
        let mut changed = false;
        match cmd {
            VisualCmd::Move { count, motion } => {
                ev.merge(self.do_motion(count, motion));
            }
            VisualCmd::TextObj { inner, obj } => {
                if let Some(r) = self.text_object(inner, obj) {
                    if let RegionKind::Char { start, end } = r.kind {
                        self.anchor = start;
                        self.cursor = self.buf.pos_before(end);
                        self.sync_desired();
                    }
                } else {
                    self.status = "no such object here".into();
                }
            }
            VisualCmd::Toggle { linewise } => {
                if (self.mode == Mode::Visual && !linewise)
                    || (self.mode == Mode::VisualLine && linewise)
                {
                    self.mode = Mode::Normal;
                } else {
                    self.mode = if linewise { Mode::VisualLine } else { Mode::Visual };
                }
            }
            VisualCmd::ToggleCase => {
                changed = self.toggle_case_selection();
                self.mode = Mode::Normal;
                if changed {
                    self.record_change(keys);
                }
            }
            VisualCmd::Op(op) => {
                changed = self.apply_visual_op(op, keys);
            }
            VisualCmd::ReplaceWithRegister => {
                let reg = self.edit_buffer.clone();
                if reg.text.is_empty() {
                    self.status = "edit buffer is empty".into();
                    return;
                }
                let region = self.visual_region();
                self.begin_change();
                let removed = self.remove_region(region);
                if reg.linewise {
                    self.buf.insert_lines(self.cursor.0, &reg.text);
                } else {
                    self.cursor = self.buf.insert_text(self.cursor, &reg.text);
                }
                self.edit_buffer = Register::charwise(removed.trim_end_matches('\n').to_string());
                self.clamp_cursor();
                self.mode = Mode::Normal;
                changed = true;
                self.end_change();
                self.record_change(keys);
            }
        }
        if changed {
            ev.merge(EngineEvent::TextChanged);
        }
    }

    fn remove_region(&mut self, region: Region) -> String {
        match region.kind {
            RegionKind::Char { start, end } => {
                let t = self.buf.remove_charwise(start, end);
                self.cursor = self.buf.clamp_normal(start);
                t
            }
            RegionKind::Line { from, to } => {
                let t = self.buf.remove_lines(from, to);
                self.cursor = (from.min(self.buf.line_count() - 1), 0);
                t
            }
        }
    }

    fn visual_region(&self) -> Region {
        match self.mode {
            Mode::VisualLine => {
                let (a, b) = self.buf.order(self.anchor, self.cursor);
                Region::lines(a.0, b.0)
            }
            _ => {
                let (a, b) = self.buf.order(self.anchor, self.cursor);
                Region::char(a, self.buf.pos_after(b))
            }
        }
    }

    fn apply_visual_op(&mut self, op: OpKind, keys: &[Key]) -> bool {
        let region = self.visual_region();
        let empty = match region.kind {
            RegionKind::Char { start, end } => {
                let (a, b) = self.buf.order(start, end);
                a >= b
            }
            RegionKind::Line { .. } => false,
        };
        if empty {
            self.status = "empty selection".into();
            self.mode = Mode::Normal;
            return false;
        }

        if op == OpKind::Yank {
            let (text, linewise, start) = match region.kind {
                RegionKind::Char { start, end } => {
                    let (a, b) = self.buf.order(start, end);
                    let mut t = String::new();
                    extract_charwise(&self.buf, a, b, &mut t);
                    (t, false, a)
                }
                RegionKind::Line { from, to } => {
                    let mut t = String::new();
                    extract_lines(&self.buf, from, to, &mut t);
                    (t, true, (from, 0))
                }
            };
            let reg = if linewise {
                Register::linewise(text)
            } else {
                Register::charwise(text)
            };
            self.clipboard.write(&reg.text);
            self.edit_buffer = reg;
            self.cursor = self.buf.clamp_normal(start);
            self.mode = Mode::Normal;
            self.record_change(keys);
            return false;
        }

        self.begin_change();
        let (text, linewise) = {
            let t = self.remove_region(region);
            let lw = matches!(region.kind, RegionKind::Line { .. });
            (t, lw)
        };
        self.edit_buffer = if linewise {
            Register::linewise(text)
        } else {
            Register::charwise(text.trim_end_matches('\n').to_string())
        };
        match op {
            OpKind::Delete => {
                self.mode = Mode::Normal;
                self.end_change();
                self.record_change(keys);
                true
            }
            OpKind::Change => {
                self.mode = Mode::Insert;
                self.insert_origin = Some(self.cursor);
                self.insert_keys.clear();
                if self.replaying == 0 {
                    self.last_trigger = keys.to_vec();
                }
                self.group_open = true;
                true
            }
            OpKind::Yank => unreachable!(),
        }
    }

    fn toggle_case_selection(&mut self) -> bool {
        if let Some((start, end, _)) = self.selection() {
            self.begin_change();
            let mut p = start;
            loop {
                if let Some(c) = self.buf.char_at(p) {
                    let t = if c.is_uppercase() {
                        c.to_lowercase().next().unwrap_or(c)
                    } else {
                        c.to_uppercase().next().unwrap_or(c)
                    };
                    self.buf.lines[p.0][p.1] = t;
                }
                if p >= self.buf.pos_before(end) {
                    break;
                }
                let next = self.buf.pos_after(p);
                if next == p || next >= end {
                    break;
                }
                p = next;
            }
            self.end_change();
            true
        } else {
            false
        }
    }
}

// ---- region ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum RegionKind {
    Char { start: Pos, end: Pos },
    Line { from: usize, to: usize },
}

#[derive(Debug, Clone, Copy)]
struct Region {
    kind: RegionKind,
}

impl Region {
    fn char(start: Pos, end: Pos) -> Region {
        Region { kind: RegionKind::Char { start, end } }
    }

    fn lines(from: usize, to: usize) -> Region {
        Region { kind: RegionKind::Line { from, to } }
    }
}

// ---- free motion helpers over a buffer ---------------------------------------------

fn motion_is_forward(eng: &Engine, motion: Motion, dest: Pos) -> bool {
    match motion {
        Motion::Left | Motion::WordBack { .. } | Motion::LineCol0 | Motion::FirstNonBlank => false,
        Motion::Find { fwd, .. } => fwd,
        _ => dest >= eng.cursor,
    }
}

fn probe_motion(eng: &Engine, count: usize, motion: Motion) -> Option<Pos> {
    let from = eng.cursor;
    match motion {
        Motion::Left => Some((from.0, from.1.saturating_sub(count))),
        Motion::Right => {
            let len = eng.buf.line_len(from.0);
            let max = if len == 0 { 0 } else { len - 1 };
            Some((from.0, (from.1 + count).min(max)))
        }
        Motion::Up => Some((from.0.saturating_sub(count), from.1)),
        Motion::Down => Some(((from.0 + count).min(eng.buf.line_count() - 1), from.1)),
        Motion::LineCol0 => Some((from.0, 0)),
        Motion::FirstNonBlank => Some((from.0, eng.buf.first_non_blank(from.0))),
        Motion::LineEnd => {
            let line = (from.0 + count.saturating_sub(1)).min(eng.buf.line_count() - 1);
            let col = if eng.buf.line_len(line) == 0 {
                0
            } else {
                eng.buf.line_len(line) - 1
            };
            Some((line, col))
        }
        Motion::WordStart { big } => {
            // For operators, w stops at end-of-line rather than crossing it
            // (Vim's dw never eats the newline); running out of buffer also
            // clamps to end-of-line so dw on the last word still deletes it.
            let mut p = from;
            for _ in 0..count {
                p = match word_fwd(&eng.buf, p, big) {
                    Some(q) => q,
                    None => return Some((from.0, eng.buf.line_len(from.0))),
                };
                if p.0 != from.0 {
                    let len = eng.buf.line_len(from.0);
                    return Some((from.0, len));
                }
            }
            Some(p)
        }
        Motion::WordBack { big } => {
            let mut p = from;
            for _ in 0..count {
                p = word_back(&eng.buf, p, big)?;
            }
            Some(p)
        }
        Motion::WordEnd { big } => {
            let mut p = from;
            for _ in 0..count {
                p = word_end(&eng.buf, p, big)?;
            }
            Some(p)
        }
        Motion::GotoLine { line } => {
            let target = match line {
                Some(n) => n.saturating_sub(1),
                None => eng.buf.line_count() - 1,
            };
            Some((target.min(eng.buf.line_count() - 1), 0))
        }
        Motion::Find { fwd, till, ch } => find_in_line(&eng.buf, from, count, fwd, till, ch),
        Motion::FindRepeat { .. } => None,
    }
}

fn word_fwd(buf: &Buffer, cursor: Pos, big: bool) -> Option<Pos> {
    let mut p = cursor;
    let class = buf.char_at(p).map(|c| char_class(c, big));
    if let Some(k) = class {
        if k != CharClass::Whitespace {
            loop {
                let next = buf.pos_after(p);
                if next == p {
                    return None;
                }
                match buf.char_at(next).map(|c| char_class(c, big)) {
                    Some(k2) if k2 == k => p = next,
                    _ => break,
                }
            }
        }
    }
    loop {
        if p.1 >= buf.line_len(p.0) {
            // At end of line: the newline is whitespace; an empty next line
            // is itself a word stop.
            if p.0 + 1 < buf.line_count() {
                let nl = p.0 + 1;
                if buf.line_len(nl) == 0 {
                    return Some((nl, 0));
                }
                p = (nl, 0);
                continue;
            }
            return None;
        }
        let next = buf.pos_after(p);
        match buf.char_at(next).map(|c| char_class(c, big)) {
            Some(CharClass::Whitespace) => p = next,
            Some(_) => return Some(next),
            None => p = next, // step onto the EOL position; handled above
        }
    }
}

fn word_back(buf: &Buffer, cursor: Pos, big: bool) -> Option<Pos> {
    let mut p = cursor;
    loop {
        let prev = buf.pos_before(p);
        if prev == p {
            return None;
        }
        match buf.char_at(prev).map(|c| char_class(c, big)) {
            Some(CharClass::Whitespace) => p = prev,
            _ => break,
        }
    }
    let class = match buf.char_at(p) {
        Some(c) => char_class(c, big),
        None => return Some(p),
    };
    loop {
        let prev = buf.pos_before(p);
        if prev == p {
            return Some(p);
        }
        match buf.char_at(prev).map(|c| char_class(c, big)) {
            Some(k) if k == class => p = prev,
            _ => return Some(p),
        }
    }
}

fn word_end(buf: &Buffer, from: Pos, big: bool) -> Option<Pos> {
    // If the cursor sits inside a word run (not on its last char), `e` stops
    // at the end of THAT run first (Vim rule; this is what makes `cw` work).
    if let Some(c) = buf.char_at(from) {
        let k = char_class(c, big);
        if k != CharClass::Whitespace {
            let mut p = from;
            let mut moved = false;
            loop {
                let n = buf.pos_after(p);
                if n == p {
                    return Some(p);
                }
                match buf.char_at(n).map(|c| char_class(c, big)) {
                    Some(k2) if k2 == k => {
                        p = n;
                        moved = true;
                    }
                    _ => break,
                }
            }
            if moved {
                return Some(p);
            }
            // Already on the run's last char: fall through to the next run.
        }
    }
    let mut p = from;
    loop {
        let next = buf.pos_after(p);
        if next == p {
            return None;
        }
        match buf.char_at(next).map(|c| char_class(c, big)) {
            Some(CharClass::Whitespace) => p = next,
            Some(_) => {
                let mut e = next;
                loop {
                    let n2 = buf.pos_after(e);
                    if n2 == e {
                        return Some(e);
                    }
                    match buf.char_at(n2).map(|c| char_class(c, big)) {
                        Some(k) if k != CharClass::Whitespace => e = n2,
                        _ => return Some(e),
                    }
                }
            }
            None => p = next,
        }
    }
}

fn find_in_line(buf: &Buffer, cursor: Pos, count: usize, fwd: bool, till: bool, ch: char) -> Option<Pos> {
    let (l, c) = cursor;
    let line = &buf.lines[l];
    if fwd {
        let mut hits = line.iter().enumerate().filter(|(_, &x)| x == ch).map(|(i, _)| i).filter(|&i| i > c);
        let mut found = None;
        for _ in 0..count.max(1) {
            match hits.next() {
                Some(i) => found = Some(i),
                None => break,
            }
        }
        found.map(|i| (l, if till && i > 0 { i - 1 } else { i }))
    } else {
        let mut hits: Vec<usize> = line.iter().enumerate().filter(|(_, &x)| x == ch).map(|(i, _)| i).filter(|&i| i < c).collect();
        hits.reverse();
        let mut found = None;
        for _ in 0..count.max(1) {
            if hits.is_empty() {
                break;
            }
            found = Some(hits.remove(0));
        }
        found.map(|i| (l, if till && i + 1 < line.len() { i + 1 } else { i }))
    }
}

fn step_while(buf: &Buffer, p: Pos, class: CharClass, fwd: bool) -> Option<Pos> {
    let q = if fwd { buf.pos_after(p) } else { buf.pos_before(p) };
    if q == p {
        return None;
    }
    match buf.char_at(q).map(|c| char_class(c, false)) {
        Some(k) if k == class => Some(q),
        _ => None,
    }
}

fn extract_charwise(buf: &Buffer, a: Pos, b: Pos, out: &mut String) {
    if a.0 == b.0 {
        out.extend(&buf.lines[a.0][a.1..b.1.max(a.1)]);
        return;
    }
    out.extend(&buf.lines[a.0][a.1..]);
    out.push('\n');
    for l in &buf.lines[a.0 + 1..b.0] {
        out.extend(l);
        out.push('\n');
    }
    if b.0 < buf.line_count() {
        out.extend(&buf.lines[b.0][..b.1.min(buf.lines[b.0].len())]);
    }
}

fn extract_lines(buf: &Buffer, from: usize, to: usize, out: &mut String) {
    for l in from..=to.min(buf.line_count() - 1) {
        out.extend(&buf.lines[l]);
        out.push('\n');
    }
}

fn plain_char(key: &Key) -> Option<char> {
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

// ---- parsers -------------------------------------------------------------------------

fn parse_count(keys: &[Key], i: &mut usize) -> Option<usize> {
    let mut n: Option<usize> = None;
    while *i < keys.len() {
        match keys[*i].chr() {
            Some(d @ '1'..='9') if n.is_none() => {
                n = Some(d as usize - '0' as usize);
                *i += 1;
            }
            Some(d @ '0'..='9') if n.is_some() => {
                n = Some(n.unwrap() * 10 + (d as usize - '0' as usize));
                *i += 1;
            }
            _ => break,
        }
    }
    n
}

fn op_kind(c: char) -> OpKind {
    match c {
        'd' => OpKind::Delete,
        'c' => OpKind::Change,
        _ => OpKind::Yank,
    }
}

fn parse(keys: &[Key]) -> Parse {
    let mut i = 0;
    let count1 = parse_count(keys, &mut i);
    if i == keys.len() {
        return Parse::NeedMore;
    }
    // Named keys: ctrl-r = redo; arrows = motions.
    if !keys[i].ctrl && matches!(keys[i].key.as_str(), "left" | "right" | "up" | "down") {
        if keys.len() > i + 1 {
            return Parse::Invalid;
        }
        let motion = match keys[i].key.as_str() {
            "left" => Motion::Left,
            "right" => Motion::Right,
            "up" => Motion::Up,
            _ => Motion::Down,
        };
        return Parse::Ready(Cmd::Move { count: count1.unwrap_or(1), motion });
    }
    if keys[i].ctrl {
        if keys[i].key == "r" && count1.is_none() && keys.len() == i + 1 {
            return Parse::Ready(Cmd::Redo);
        }
        return Parse::Invalid;
    }
    let c = match keys[i].chr() {
        Some(c) => c,
        None => return Parse::Invalid,
    };
    let count = count1.unwrap_or(1);

    if matches!(c, 'd' | 'c' | 'y') {
        let mut j = i + 1;
        let count2 = parse_count(keys, &mut j);
        if j == keys.len() {
            return Parse::NeedMore;
        }
        let total = count1.unwrap_or(1) * count2.unwrap_or(1);
        // Doubled operator: dd / cc / yy
        if keys[j].chr() == Some(c) {
            return if j + 1 == keys.len() {
                Parse::Ready(Cmd::Op { count: total, op: op_kind(c), target: Target::Lines })
            } else {
                Parse::Invalid
            };
        }
        // Text object: diw / caw / yi" ...
        if matches!(keys[j].chr(), Some('i' | 'a')) {
            if j + 1 == keys.len() {
                return Parse::NeedMore;
            }
            return match keys[j + 1].chr() {
                Some(obj) if "wW\"'`()b".contains(obj) => Parse::Ready(Cmd::Op {
                    count: total,
                    op: op_kind(c),
                    target: Target::TextObj { inner: keys[j].chr() == Some('i'), obj },
                }),
                _ => Parse::Invalid,
            };
        }
        // Motion target
        return match parse_motion_at(keys, j) {
            MotionParse::NeedMore => Parse::NeedMore,
            MotionParse::Invalid => Parse::Invalid,
            MotionParse::Ready(motion, end) => {
                if end != keys.len() {
                    Parse::Invalid
                } else {
                    Parse::Ready(Cmd::Op { count: total, op: op_kind(c), target: Target::Motion(motion) })
                }
            }
        };
    }

    // r<char> needs one more key
    if c == 'r' && i + 1 == keys.len() {
        return Parse::NeedMore;
    }

    // Simple single-key commands
    if i + 1 == keys.len() {
        if let Some(cmd) = simple_command(c, count) {
            return Parse::Ready(cmd);
        }
    }

    // r followed by its replacement char (possibly with a count prefix)
    if c == 'r' && keys.len() == i + 2 {
        if let Some(ch) = keys[i + 1].chr() {
            return Parse::Ready(Cmd::ReplaceChar { count, ch });
        }
    }

    match parse_motion_at(keys, i) {
        MotionParse::NeedMore => Parse::NeedMore,
        MotionParse::Invalid => Parse::Invalid,
        MotionParse::Ready(motion, end) => {
            if end != keys.len() {
                Parse::Invalid
            } else {
                // gg with a count goes to that line; G to count-th/last.
                let motion = match motion {
                    Motion::GotoLine { .. } if c == 'g' => {
                        Motion::GotoLine { line: count1.or(Some(1)) }
                    }
                    Motion::GotoLine { line: None } if c == 'G' => Motion::GotoLine { line: count1 },
                    m => m,
                };
                Parse::Ready(Cmd::Move { count, motion })
            }
        }
    }
}

fn simple_command(c: char, count: usize) -> Option<Cmd> {
    Some(match c {
        'i' => Cmd::EnterInsert { kind: InsertKind::Before },
        'a' => Cmd::EnterInsert { kind: InsertKind::After },
        'I' => Cmd::EnterInsert { kind: InsertKind::LineStart },
        'A' => Cmd::EnterInsert { kind: InsertKind::LineEnd },
        'o' => Cmd::EnterInsert { kind: InsertKind::OpenBelow },
        'O' => Cmd::EnterInsert { kind: InsertKind::OpenAbove },
        'x' => Cmd::DeleteChars { count, fwd: true },
        'X' => Cmd::DeleteChars { count, fwd: false },
        's' => Cmd::Op { count, op: OpKind::Change, target: Target::Motion(Motion::Right) },
        'S' => Cmd::Op { count, op: OpKind::Change, target: Target::Lines },
        'D' => Cmd::Op { count: 1, op: OpKind::Delete, target: Target::Motion(Motion::LineEnd) },
        'C' => Cmd::Op { count: 1, op: OpKind::Change, target: Target::Motion(Motion::LineEnd) },
        'Y' => Cmd::Op { count, op: OpKind::Yank, target: Target::Lines },
        'J' => Cmd::Join { count: count.max(1) },
        '~' => Cmd::ToggleCase { count },
        'p' => Cmd::Paste { count, after: true },
        'P' => Cmd::Paste { count, after: false },
        'u' => Cmd::Undo,
        'v' => Cmd::ToggleVisual { linewise: false },
        'V' => Cmd::ToggleVisual { linewise: true },
        '.' => Cmd::Repeat { count },
        _ => return None,
    })
}

enum MotionParse {
    NeedMore,
    Invalid,
    Ready(Motion, usize),
}

fn parse_motion_at(keys: &[Key], i: usize) -> MotionParse {
    if !keys[i].ctrl && matches!(keys[i].key.as_str(), "left" | "right" | "up" | "down") {
        let m = match keys[i].key.as_str() {
            "left" => Motion::Left,
            "right" => Motion::Right,
            "up" => Motion::Up,
            _ => Motion::Down,
        };
        return MotionParse::Ready(m, i + 1);
    }
    let c = match keys[i].chr() {
        Some(c) => c,
        None => return MotionParse::Invalid,
    };
    let single = |m: Motion| MotionParse::Ready(m, i + 1);
    match c {
        'h' => single(Motion::Left),
        'l' => single(Motion::Right),
        'j' => single(Motion::Down),
        'k' => single(Motion::Up),
        'w' => single(Motion::WordStart { big: false }),
        'W' => single(Motion::WordStart { big: true }),
        'b' => single(Motion::WordBack { big: false }),
        'B' => single(Motion::WordBack { big: true }),
        'e' => single(Motion::WordEnd { big: false }),
        'E' => single(Motion::WordEnd { big: true }),
        '0' => single(Motion::LineCol0),
        '^' => single(Motion::FirstNonBlank),
        '$' => single(Motion::LineEnd),
        'G' => single(Motion::GotoLine { line: None }),
        ';' => single(Motion::FindRepeat { reverse: false }),
        ',' => single(Motion::FindRepeat { reverse: true }),
        'g' => {
            if i + 1 == keys.len() {
                MotionParse::NeedMore
            } else if keys[i + 1].chr() == Some('g') && i + 2 == keys.len() {
                MotionParse::Ready(Motion::GotoLine { line: Some(1) }, i + 2)
            } else {
                MotionParse::Invalid
            }
        }
        'f' | 'F' | 't' | 'T' => {
            if i + 1 == keys.len() {
                MotionParse::NeedMore
            } else if let Some(ch) = keys[i + 1].chr() {
                let (fwd, till) = match c {
                    'f' => (true, false),
                    't' => (true, true),
                    'F' => (false, false),
                    _ => (false, true),
                };
                MotionParse::Ready(Motion::Find { fwd, till, ch }, i + 2)
            } else {
                MotionParse::NeedMore
            }
        }
        _ => MotionParse::Invalid,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum VisualCmd {
    Move { count: usize, motion: Motion },
    TextObj { inner: bool, obj: char },
    Op(OpKind),
    ReplaceWithRegister,
    Toggle { linewise: bool },
    ToggleCase,
}

fn parse_visual(keys: &[Key]) -> VisualParse {
    let mut i = 0;
    let count1 = parse_count(keys, &mut i);
    if i == keys.len() {
        return VisualParse::NeedMore;
    }
    let count = count1.unwrap_or(1);
    let c = match keys[i].chr() {
        Some(c) => c,
        None => {
            if !keys[i].ctrl && matches!(keys[i].key.as_str(), "left" | "right" | "up" | "down") {
                let m = match keys[i].key.as_str() {
                    "left" => Motion::Left,
                    "right" => Motion::Right,
                    "up" => Motion::Up,
                    _ => Motion::Down,
                };
                return VisualParse::Ready(VisualCmd::Move { count, motion: m });
            }
            return VisualParse::Invalid;
        }
    };
    match c {
        'd' | 'x' => VisualParse::Ready(VisualCmd::Op(OpKind::Delete)),
        'c' => VisualParse::Ready(VisualCmd::Op(OpKind::Change)),
        'y' => VisualParse::Ready(VisualCmd::Op(OpKind::Yank)),
        'p' | 'P' => VisualParse::Ready(VisualCmd::ReplaceWithRegister),
        '~' => VisualParse::Ready(VisualCmd::ToggleCase),
        'v' => VisualParse::Ready(VisualCmd::Toggle { linewise: false }),
        'V' => VisualParse::Ready(VisualCmd::Toggle { linewise: true }),
        'i' | 'a' => {
            if i + 1 == keys.len() {
                VisualParse::NeedMore
            } else {
                match keys[i + 1].chr() {
                    Some(obj) if "wW\"'`()b".contains(obj) => {
                        VisualParse::Ready(VisualCmd::TextObj { inner: c == 'i', obj })
                    }
                    _ => VisualParse::Invalid,
                }
            }
        }
        _ => match parse_motion_at(keys, i) {
            MotionParse::NeedMore => VisualParse::NeedMore,
            MotionParse::Invalid => VisualParse::Invalid,
            MotionParse::Ready(motion, end) => {
                if end == keys.len() {
                    VisualParse::Ready(VisualCmd::Move { count, motion })
                } else {
                    VisualParse::Invalid
                }
            }
        },
    }
}
enum VisualParse {
    NeedMore,
    Invalid,
    Ready(VisualCmd),
}

