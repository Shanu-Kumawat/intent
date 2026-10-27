# Intent — Interaction Specification

This document is the authoritative specification for Intent's behavior. Where
implementation and this document disagree, this document wins and the
implementation is a bug. The goal is a small, coherent, predictable system:
Vim's grammar applied to commitment management, a first-class CLI, local-first
storage, no bloat.

Design principles, in order:

1. **Predictable** — same keys, same result, every time. No timeouts, no
   hidden state. Pending input is always visible.
2. **Recoverable** — every GUI mutation can be undone in-session. Destructive
   actions are deliberate.
3. **Keyboard-only** — everything is reachable without a mouse.
4. **Local-first** — one SQLite file, no accounts, no network.
5. **One brain** — GUI and CLI share `intent-core`. No duplicated logic.

---

## 1. Data model

A **task** is the only entity.

| Field         | Type                | Meaning                                              |
|---------------|---------------------|------------------------------------------------------|
| `id`          | string, 6 chars     | Stable identifier, alphabet `23456789abcdefghjkmnpqrstuvwxyz`, never reused |
| `title`       | string              | Single line, non-empty on creation                   |
| `notes`       | string              | Multi-line body, may be empty                        |
| `status`      | `active \| done \| archived` | Lifecycle status (below)                     |
| `inbox`       | bool                | Unprocessed capture (below)                          |
| `priority`    | `none \| low \| medium \| high` |                                                  |
| `project`     | string, optional    | Free-form group name, case-sensitive                 |
| `tags`        | [string]            | Free-form labels                                     |
| `due`         | date, optional      | **Deadline** — hard commitment                       |
| `remind`      | date, optional      | **Resurface** — "look at this again on", soft       |
| `defer_until` | date, optional      | Hide from Today until this date                     |
| `created_at`  | timestamp (UTC)     |                                                     |
| `updated_at`  | timestamp (UTC)     | Bumped by every mutation                             |
| `completed_at`| timestamp, optional | Set when status becomes `done`                       |

Dates are **local calendar dates** (`YYYY-MM-DD`). Timestamps are RFC 3339 UTC.

### 1.1 Lifecycle

```
            capture
              ─▶ active ◀──────────────┐
   organize ──│     │                  │
(inbox→kept) │  done │ reopen          │
              │   │  │                  │
              ▼   ▼  │                  │
            archived ─┘ (unarchive → active)
```

- `active` = something you intend to do. New tasks start here, in the inbox.
- `done` = completed. Keeps `completed_at`. Reversible (`reopen`).
- `archived` = out of mind, kept for the record. Reversible (`unarchive`).
- **Deferral is not a status.** It is the `defer_until` date; a deferred task
  is still `active`.

### 1.2 Inbox semantics

Capture requires zero metadata; captured tasks are `inbox = true` and appear
in the Inbox view. A task **leaves the inbox** when any of these happen:

- a non-empty `project` is assigned,
- `due` is set,
- `remind` is set,
- an explicit organize action runs (`intent organize`, `Space o`, `:organize`).

Priority, tags, notes, and title edits do **not** clear the inbox.

### 1.3 Views

`today` means the local calendar date at evaluation time.

| View        | Definition (in precedence order)                                                                 |
|-------------|--------------------------------------------------------------------------------------------------|
| `inbox`     | `status = active AND inbox`                                                                       |
| `today`     | `status = active AND (defer_until IS NULL OR defer_until ≤ today) AND (due ≤ today OR remind ≤ today)` |
| `upcoming`  | `status = active AND NOT inbox AND not in today` — future-dated and undated ("someday") tasks     |
| `completed` | `status = done`, newest `completed_at` first                                                      |
| `archived`  | `status = archived`, newest `updated_at` first                                                    |
| `all`       | `status != archived` (CLI default; nothing is ever invisible from the CLI)                        |

Sort orders:

- `inbox`: newest first (fresh captures on top, cursor lands on them)
- `today`: dated first by `due` ascending, then priority high→low, then oldest
- `upcoming`: dated ascending (by earliest of due/remind/defer), undated last,
  then priority, then oldest
- `completed` / `archived`: newest first

### 1.4 Date input (shared by CLI and GUI prompts)

Accepted everywhere a date is expected:

- `YYYY-MM-DD`
- `today` / `tod`, `tomorrow` / `tom` / `tmr`, `yesterday`
- `+N` / `+Nd` (N days), `+Nw`, `+Nm`, `+Ny`; `-N[d|w|m|y]` for the past
- weekday: `mon`…`sun` or full names — next occurrence, today included
- `none` or `-` where "clear the date" is allowed

Everything else is a parse error. No natural-language guessing beyond this
list — predictability over cleverness.

---

## 2. Storage and concurrency

- Single SQLite database, WAL mode, `busy_timeout = 5s`. Default path
  `$XDG_DATA_HOME/intent/tasks.db` (→ `~/.local/share/intent/tasks.db`),
  overridable with `INTENT_DB` (absolute path).
- Schema is versioned (`meta` table); migrations are forward-only and run on
  open. Failures abort the process with a clear message — never operate on a
  database of a newer schema version.
- **Every mutation is one transaction.** Readers never block writers
  (snapshot isolation under WAL).
- Concurrency model: **last writer wins per mutation**, where a mutation is
  one CLI invocation or one GUI action. There are no field-level merges and
  no cross-process locking beyond SQLite's. This is deliberate: mutations are
  small and explicit, so interleaving risk is low and behavior is explainable.
- The GUI detects external changes (CLI or another GUI) by polling
  `PRAGMA data_version` every 2 seconds, on window activation, and after every
  mutating action (pure navigation keys rely on the poll); on change it
  reloads views. Cursor position is tracked **by task
  id**: after reload the cursor stays on the same task if it still exists,
  otherwise the nearest row. Unfocused view state (cursor per view) survives
  reloads.
- The GUI editor writes through to the database on every change (local-first,
  crash-safe). There is no save command anywhere in Intent.

---

## 3. CLI contract

Binary: `intent`. Human output by default; `--json` on every read and write
command switches to machine-readable output. JSON goes to stdout, always parseable.
Errors go to stderr; with `--json`, stderr is a single JSON object:

```json
{"error": {"code": "not_found", "message": "no task k3x9qa"}}
```

Error codes: `not_found`, `bad_date`, `bad_value`, `usage`, `io`, `db`, `conflict`.

**Exit codes**

| Code | Meaning                              |
|------|--------------------------------------|
| 0    | success                              |
| 1    | runtime error (db, io)               |
| 2    | usage error (bad flags)              |
| 3    | task not found                       |
| 4    | invalid value (date, priority, …)    |

**Task JSON** (field set is stable; agents may rely on it):

```json
{
  "id": "k3x9qa",
  "title": "Renew passport",
  "notes": "",
  "status": "active",
  "inbox": true,
  "priority": "high",
  "project": null,
  "tags": ["admin"],
  "due": "2026-11-02",
  "remind": null,
  "defer_until": null,
  "created_at": "2026-10-26T09:00:00Z",
  "updated_at": "2026-10-26T09:00:00Z",
  "completed_at": null
}
```

**Commands** (`add` accepts a `--title` alternative to positional for agents;
date flags take the date syntax from §1.4 and `none`/`-` to clear):

```
intent add TITLE | --title TITLE
    [--notes N] [--due D] [--remind D] [--priority P] [--project P]
    [--tags a,b] [--json] [--no-inbox]
intent list [VIEW] [--project P] [--tag T] [--priority P] [--query Q] [--limit N] [--json]
    VIEW = inbox|today|upcoming|completed|archived|all   (default all)
intent show ID [--json]
intent edit ID [--title T] [--notes N] [--due D] [--remind D] [--defer-until D]
    [--priority P] [--project P|none] [--tags a,b] [--tag-add T] [--tag-rm T] [--json]
intent done ID [--json]          intent reopen ID [--json]
intent defer ID DATE|none [--json]        (sets defer_until)
intent prioritize ID none|low|medium|high [--json]
intent archive ID [--json]       intent unarchive ID [--json]
intent organize ID [--project P] [--json]
intent delete ID [--force] [--json]       (see §3.1)
intent search QUERY [--json] [--limit N]  (= list --query, all non-archived)
intent counts [--json]                    (view → count map)
intent completions bash|zsh|fish          (print a completion script; no db access)
intent help [COMMAND] | --help | --version
```

Semantics notes:

- Mutating commands echo the resulting task (JSON or one-line summary).
- `delete` is the only irreversible command. Without `--force` on a TTY it
  asks `y/N`; without `--force` and not a TTY it refuses with error code `1`.
  In the GUI, `D` asks for confirmation and the deletion remains undoable for
  the session (§5).
- Setting `--due`/`--remind`/`--project` on `add` implies leaving the inbox
  (§1.2). `--no-inbox` forces it.
- `edit` with no mutation flags is a usage error (exit 2).

---

## 4. GUI model

One window. Three surfaces; exactly one receives keys at any time.

```
┌──────────────────────────────────────────────────────┐
│ INTENT ── TODAY ─────────────────────────── 2026-10-26│  header
├──────┬───────────────────────────────────────────────┤
│ 1 IN │  ▸ ! Renew passport          DUE 11-02  HIGH   │
│ 2 TO │    …                                            │  list
│ 3 UP │    …                                            │
│ 4 DN │                                                 │
├──────┴───────────────────────────────────────────────┤
│ :defer +3d_                                          │  prompt (when active)
├──────────────────────────────────────────────────────┤
│ -- NORMAL --  k3x9qa  3/12                            │  status line
└──────────────────────────────────────────────────────┘
```

Surfaces and their modes:

| Surface | Modes                                        |
|---------|----------------------------------------------|
| list    | NORMAL, VISUAL (line-wise over tasks)        |
| editor  | NORMAL, INSERT, VISUAL (char), VISUAL LINE   |
| prompt  | PROMPT (single-line, insert-like)            |

Precedence: prompt > editor > list. Opening a prompt suspends the surface
below it; closing the prompt (commit or cancel) returns focus to exactly the
surface, mode, and cursor it interrupted. There is never more than one prompt.

**Mode/pending display.** The status line always shows: current mode, any
pending key sequence (after `d` it shows `d`, after `2d` it shows `2d`),
the focused task id, and position. **There are no timeouts**: pending
operators, counts, and leader sequences wait indefinitely; `escape` cancels;
a key that cannot extend the sequence cancels it with a status message.
Pending state is always visible, so waiting is safe.

### 4.1 List — NORMAL

| Key                | Action                                                        |
|--------------------|---------------------------------------------------------------|
| `j` `k` ↑ ↓        | cursor down/up (count ok)                                      |
| `g g` / `G`        | top / bottom (`N g g` → row N)                                |
| `ctrl-d` / `ctrl-u`| jump 10 rows down/up                                           |
| `enter`            | open task in editor (normal mode)                              |
| `i` / `a`          | open task in editor, title, insert mode                        |
| `o`, `space space`, `space c` | capture (prompt). If the new task is visible in the current view the cursor moves to it; otherwise the cursor stays and the status line reports the new id. |
| `x`                | complete (active→done) / reopen (done→active) / revive (archived→active); count applies to N tasks from cursor |
| `d`                | archive (count ok)                                             |
| `space D`          | unarchive                                                     |
| `D`                | delete permanently — y/n confirmation, undoable in-session     |
| `>` / `<`          | shift due +1/−1 day; if no due, `>` sets due = today (count ok)|
| `+` `=` / `-`      | raise / lower priority (cycle none↔low↔medium↔high; count ok)  |
| `y` / `Y`          | yank title / title+notes to system clipboard                   |
| `V`                | VISUAL (line-wise): `j`/`k` extend; `x` complete all, `d` archive all, `y` yank titles (newline-joined), `esc` cancel |
| `/`                | search prompt; enter → list shows matches (title/notes/project/tags, all ANDed terms, case-insensitive); `esc` in list clears back to the previous view; `n`/`N` are reserved |
| `:`                | command prompt (§4.4)                                          |
| `?`                | help overlay (full keymap; any key closes)                    |
| `1`–`4`            | switch view inbox/today/upcoming/completed (only with no pending count). Counts therefore always start with `5`–`9`; `5gg` is the smallest count-gg. |
| `tab` / `shift-tab`| next / previous view                                           |
| `u` / `ctrl-r`     | undo / redo task operations (§5)                               |
| `.`                | repeat last task operation (`x d > < + -` are repeatable), count replaces the original count |
| `space d`          | defer prompt → sets `defer_until` (§1.4 syntax)                |
| `space u`          | due prompt → sets `due`                                        |
| `space r`          | remind prompt → sets `remind`                                 |
| `space o`          | organize prompt → sets `project` (empty answer = just leave inbox) |
| `esc`              | cancel pending sequence / visual / search results              |

`space` is **always** the leader, on every surface, in every mode (in the
editor it never acts as a motion). Unknown leader keys produce a status
message and clear the pending state — never a silent no-op.

### 4.2 Editor

Opened by `enter`/`i`/`a` from the list. Two stacked fields, **TITLE**
(single line) and **NOTES** (multi-line), each an independent text buffer.
The focused field is highlighted; `tab`/`shift-tab` and boundary `j`/`k`
move between fields (cursor column carried over, clamped). Line-oriented
operations never cross the field boundary.

Closing: `escape` in NORMAL (i.e. a second `escape` after leaving insert),
`:q`, or switching views. Focus returns to the list on the same task id.

Vim grammar implemented (this is the complete set — anything else is a
documented no-op with a status message, not a crash):

- **Motions:** `h l w b e W B E 0 ^ $ f F t T ; , g g G j k`
- **Operators:** `d c y` + motion, doubled (`dd cc yy D C`), `x X s S r ~ J`
- **Text objects:** `iw aw i" a" i' a' i( a( ib ab`
- **Counts:** before operators and motions (`3dd`, `d2w`, `2j`)
- **Insert entries:** `i a I A o O` (`o`/`O` only in NOTES; in TITLE they
  behave like `A`), `esc`/`ctrl-c` exit insert
- **Insert mode:** printable ASCII, `enter` (split line, NOTES only; in TITLE
  a no-op message), `backspace` (join to previous line in NOTES; start of
  TITLE no-op), `tab` = 4 spaces, `ctrl-w` delete word back, `ctrl-u` delete back to where this insert run
  started on the line (typed text is cleared; pre-existing text survives)
- **Paste:** `p P` (§4.5), **Visual:** char-wise `v`, line-wise `V`; motions
  extend; `d x c y ~` operate on selection; `esc` cancels
- **Undo/redo:** `u`, `ctrl-r` per field buffer (§5)
- **Repeat:** `.` repeats the last change (insert run, `x X r ~ J o O p P`,
  and any operator+motion/text-object)

Leader in editor normal mode: `space space`/`space c` still capture (returns
to the surface you were in), `space p` pastes the **system clipboard** at the
cursor (distinct from `p`, §4.5). `1`–`4` are counts here — view switching
keys belong to the list; this keeps Vim number semantics intact. `tab` moves
between fields, consistent with its "advance" role in the list.

### 4.3 Prompts

One-line, insert-like: printable ASCII, `backspace`, `ctrl-w`, `ctrl-u`,
`ctrl-v` (paste system clipboard), `enter` commits, `escape` cancels and
restores the interrupted surface exactly. Prompts show a title
(`CAPTURE:`, `DEFER:`, `DUE:`, `REMIND:`, `ORGANIZE project:`, `/`, `:`) and
a hint line where useful. Invalid values (e.g. bad date) keep the prompt
open with an error message — focus never bounces on bad input.

### 4.4 Command prompt (`:`)

Commands act on the cursor task unless stated. Executing an impossible
command (no cursor task, empty list) is an error message, not a crash.

```
:inbox :today :upcoming :completed :archived   switch views
:new [title]        capture (empty title → capture prompt)
:due <date|none>    :remind <date|none>    :defer <date|none>
:project <name|none>  :tag +tag  :tag -tag
:organize           leave inbox (no project change)
:archive :unarchive :done :reopen
:delete             same as D (y/n confirm)
:refresh            reload from database
:q                  close editor / quit app from list
:w                  no-op confirmation — Intent always saves (local-first)
:help               help overlay
```

Unknown command → error message, prompt stays open.

### 4.5 Clipboard

Two slots, deliberately simple:

- **edit buffer** — internal, holds the most recent yank/delete/change result
- **system clipboard** — the OS clipboard, via one abstraction (GPUI's, and a
  stub for tests)

Rules:

1. `yank` writes the text to **both** the edit buffer and the system clipboard.
2. `delete`/`change` write **only** the edit buffer. They never touch the
   system clipboard — destroying text must never clobber what you yanked for
   elsewhere.
3. `p`/`P` paste from the **edit buffer** (so `dd` `p` works, Vim-style).
4. `space p` (editor) and `ctrl-v` (prompts) paste from the **system
   clipboard** — the only operations that read it.
5. List `y`/`Y` write the system clipboard (and the edit buffer).
6. The clipboard is **not** undo state and is never restored by undo.

No named registers. Ever.

---

## 5. Undo guarantees

Two independent domains:

1. **Text editing** (editor fields). Standard linear undo/redo per field
   buffer. Undo units: one insert run (all typing until leaving insert mode
   or a non-insert operation) and each delete/change operation. Stacks are
   cleared when the editor closes.
2. **Task operations** (everything mutating tasks from the list: complete,
   archive, delete, priority, dates, organize, capture, and prompt/command
   actions). A session-wide stack; `u`/`ctrl-r` in the list surface operate
   on it. Each entry records full before/after row snapshots for every
   affected task. Undo of a capture deletes the task; undo of a delete
   resurrects it with the same id and `created_at`.

Rules:

- A new mutation after undos clears the redo stack (linear, like Vim).
- Editor field changes are task mutations too, but they are **not** pushed to
  the task stack; their recoverability lives in the editor's own undo while
  the editor is open. This boundary is stated, not accidental.
- `u` never crosses domains: in the editor it edits text, in the list it
  reverts task operations. The status line always says which surface has focus.
- After any undo/redo, views refresh and the cursor follows the task id
  (§2). If the task no longer exists, the cursor drops to the nearest row.
- The CLI has no session undo; it offers explicit inverses (`reopen`,
  `unarchive`, `defer none`, `edit`). This is the documented recovery path
  for CLI/agent mutations.

---

## 6. Architecture

```
crates/core   intent-core   model, schema/migrations, store, ops, views,
                             dates, search   (pure Rust, fully tested)
crates/vim    intent-vim    modal text engine: buffers, motions, operators,
                             text objects, counts, undo, repeat, registers
                             (pure Rust, no UI dependency, fully tested)
crates/cli    intent        clap CLI over intent-core
crates/gui    intent-gui    GPUI shell + a pure "gui model" layer:
                             surface/mode/pending-key state machine, cursor,
                             undo stacks, prompt state — unit-tested without
                             GPUI; the GPUI layer only renders the model and
                             feeds it key events
```

The GUI model consumes an abstract `Key` (key + modifiers) and produces state
+ effects; nothing in `core`, `vim`, or the gui model imports GPUI. This is
what makes the interaction spec testable without a display.

---

## 7. Non-goals

Explicitly out of scope, by design: named registers, macros, visual block,
regular-expression search, sub-second remapping timeouts, cloud sync,
accounts, AI features, background daemons, notifications outside the app,
recurring tasks (until demanded), collaboration. If a future feature cannot
be added without breaking this spec's guarantees, it needs a spec change
first.
