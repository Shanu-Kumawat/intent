use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    div, px, uniform_list, App, Application, ClipboardItem, Context, FocusHandle, KeyDownEvent,
    MouseButton, MouseDownEvent, Render, ScrollStrategy, SharedString, Timer,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions,
};

use intent_core::{Store, TaskView};
use intent_gui::keys;
use intent_gui::model::{priority_mark, GuiState, PromptKind};
use intent_vim::{Clipboard, Engine, Mode};

const BLACK: gpui::Rgba = rgba_lit(0.0, 0.0, 0.0, 1.0);
const WHITE: gpui::Rgba = rgba_lit(1.0, 1.0, 1.0, 1.0);
const PAPER: gpui::Rgba = rgba_lit(0.957, 0.957, 0.941, 1.0);
const GREY: gpui::Rgba = rgba_lit(0.6, 0.6, 0.6, 1.0);
const fn rgba_lit(r: f32, g: f32, b: f32, a: f32) -> gpui::Rgba {
    gpui::Rgba { r, g, b, a }
}

const MONO: &str = "Liberation Mono";

/// Bridges the shared clipboard the state layer sees with GPUI's OS
/// clipboard: reads are snapshotted before each keypress, writes flushed
/// after.
#[derive(Default)]
struct SyncClipboard {
    inner: RefCell<SyncInner>,
}

#[derive(Default)]
struct SyncInner {
    read: Option<String>,
    write: Option<String>,
}

impl SyncClipboard {
    fn snapshot_read(&self, text: Option<String>) {
        self.inner.borrow_mut().read = text;
    }

    fn take_write(&self) -> Option<String> {
        self.inner.borrow_mut().write.take()
    }
}

impl Clipboard for SyncClipboard {
    fn read(&self) -> Option<String> {
        self.inner.borrow().read.clone()
    }

    fn write(&self, text: &str) {
        self.inner.borrow_mut().write = Some(text.to_string());
    }
}

fn scrollable(mut d: gpui::Div) -> gpui::Div {
    d.style().overflow.y = Some(gpui::Overflow::Scroll);
    d
}

struct Shell {
    state: GuiState,
    clip: Rc<SyncClipboard>,
    focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
}

impl Shell {
    fn dispatch_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        if ks.modifiers.alt || ks.modifiers.platform || ks.modifiers.function {
            return;
        }
        // Non-printing keys we don't model at all.
        if matches!(
            ks.key.as_str(),
            "capslock" | "numlock" | "scrolllock" | "shift" | "control" | "alt" | "super" | "meta"
        ) {
            return;
        }
        let key = keys::from_display(&ks.key, ks.modifiers.control, ks.modifiers.shift);

        self.clip
            .snapshot_read(cx.read_from_clipboard().and_then(|i| i.text()));
        self.state.handle_key(key);
        if let Some(text) = self.clip.take_write() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if self.state.quit_requested {
            cx.quit();
            return;
        }
        if self.state.editor.is_none() {
            self.list_scroll
                .scroll_to_item(self.state.cursor, ScrollStrategy::Center);
        }
        cx.notify();
    }
}

fn header(state: &GuiState) -> impl IntoElement {
    let search = state
        .search_query
        .as_ref()
        .map(|q| format!("  /{q}"))
        .unwrap_or_default();
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_4()
        .h(px(44.0))
        .border_b_2()
        .border_color(BLACK)
        .bg(BLACK)
        .text_color(WHITE)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(div().font_weight(gpui::FontWeight::BOLD).child("INTENT"))
                .child(div().text_color(gpui::rgba(0x888888ff)).child("///"))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(SharedString::from(if state.search_query.is_some() {
                            "SEARCH".to_string()
                        } else {
                            state.view.to_string().to_uppercase()
                        })),
                )
                .child(
                    div()
                        .text_color(gpui::rgba(0x888888ff))
                        .child(SharedString::from(search)),
                ),
        )
        .child(div().child(SharedString::from(state.today.to_string())))
}

fn sidebar(state: &GuiState) -> impl IntoElement {
    let items: Vec<(u8, TaskView)> = vec![
        (1, TaskView::Inbox),
        (2, TaskView::Today),
        (3, TaskView::Upcoming),
        (4, TaskView::Completed),
    ];
    let mut col = div()
        .flex()
        .flex_col()
        .w(px(196.0))
        .border_r_2()
        .border_color(BLACK);
    for (n, view) in items {
        let active = state.view == view;
        let count = state.view_counts.get(&view).copied().unwrap_or(0);
        let label = format!("{n} {}", view.to_string().to_uppercase());
        let mut row = div()
            .id(SharedString::from(format!("sb-{n}")))
            .flex()
            .justify_between()
            .px_4()
            .py_2();
        if active {
            row = row
                .bg(BLACK)
                .text_color(WHITE)
                .font_weight(gpui::FontWeight::BOLD);
        }
        col = col.child(row.child(label).child(format!("{count}")));
    }
    col = col.child(div().flex_1());
    col = col.child(
        div()
            .px_4()
            .py_2()
            .border_t_2()
            .border_color(BLACK)
            .text_color(GREY)
            .child("? keymap  :q quit"),
    );
    col
}

fn task_row(state: &GuiState, ix: usize) -> gpui::AnyElement {
    let Some(task) = state.rows.get(ix) else {
        return div().into_any_element();
    };
    let selected = ix == state.cursor;
    let in_visual = state
        .visual_anchor
        .map(|a| {
            let (lo, hi) = if a <= state.cursor {
                (a, state.cursor)
            } else {
                (state.cursor, a)
            };
            ix >= lo && ix <= hi
        })
        .unwrap_or(false);
    let inverse = selected || in_visual;

    let mark = match task.status {
        intent_core::Status::Done => "×".to_string(),
        intent_core::Status::Archived => "»".to_string(),
        _ if task.inbox => "□".to_string(),
        _ => "·".to_string(),
    };
    let title = if task.status == intent_core::Status::Done {
        format!("{} ", task.title)
    } else {
        task.title.clone()
    };

    let mut meta = Vec::new();
    if let Some(d) = task.due {
        let label = if d < state.today { "OVERDUE" } else { "DUE" };
        meta.push(format!("{label} {d}"));
    }
    if let Some(r) = task.remind {
        meta.push(format!("remind {r}"));
    }
    if let Some(d) = task.defer_until {
        meta.push(format!("defer {d}"));
    }
    if let Some(p) = &task.project {
        meta.push(format!("·{p}"));
    }
    for t in &task.tags {
        meta.push(format!("#{t}"));
    }
    let meta = meta.join("  ");

    let mut row = div()
        .id(SharedString::from(format!("task-{ix}")))
        .flex()
        .items_center()
        .gap_3()
        .h(px(34.0))
        .px_4()
        .border_b_1()
        .border_color(gpui::rgba(0xccccccff));
    if inverse {
        row = row.bg(BLACK).text_color(WHITE);
    }
    row.child(
        div()
            .w(px(28.0))
            .font_weight(gpui::FontWeight::BOLD)
            .when(task.priority != intent_core::Priority::None, |d| {
                d.font_weight(gpui::FontWeight::BLACK)
            })
            .child(priority_mark(task.priority)),
    )
    .child(div().w(px(20.0)).child(mark))
    .child(
        div()
            .flex_1()
            .font_weight(if task.status == intent_core::Status::Done {
                gpui::FontWeight::THIN
            } else {
                gpui::FontWeight::NORMAL
            })
            .when(task.status == intent_core::Status::Done, |d| {
                d.line_through()
            })
            .child(title),
    )
    .child(
        div()
            .text_size(px(12.0))
            .text_color(if inverse {
                gpui::rgba(0xbbbbbbff)
            } else {
                GREY
            })
            .child(meta),
    )
    .child(
        div()
            .text_size(px(12.0))
            .text_color(if inverse {
                gpui::rgba(0xbbbbbbff)
            } else {
                GREY
            })
            .child(task.id.clone()),
    )
    .into_any_element()
}

fn status_bar(state: &GuiState) -> impl IntoElement {
    let pos = if state.rows.is_empty() {
        "0/0".to_string()
    } else {
        format!("{}/{}", state.cursor + 1, state.rows.len())
    };
    let id = state
        .cursor_task()
        .map(|t| t.id.clone())
        .or_else(|| state.editor.as_ref().map(|e| e.task_id.clone()))
        .unwrap_or_default();
    div()
        .flex()
        .items_center()
        .gap_3()
        .h(px(36.0))
        .px_3()
        .border_t_2()
        .border_color(BLACK)
        .bg(WHITE)
        .child(
            div()
                .bg(BLACK)
                .text_color(WHITE)
                .px_2()
                .py_1()
                .font_weight(gpui::FontWeight::BOLD)
                .child(SharedString::from(state.mode_label())),
        )
        .child(
            div()
                .min_w(px(60.0))
                .child(SharedString::from(state.pending_display())),
        )
        .child(div().text_color(GREY).child(id))
        .child(div().text_color(GREY).child(pos))
        .child(div().flex_1())
        .child(
            div()
                .max_w(px(640.0))
                .overflow_x_hidden()
                .child(SharedString::from(state.status.clone())),
        )
}

fn prompt_bar(state: &GuiState) -> gpui::AnyElement {
    let Some(p) = &state.prompt else {
        return div().h_0().into_any_element();
    };
    let mut input = p.input.clone();
    if p.kind != PromptKind::Command && !p.input.is_empty() {
        input = p.input.clone();
    }
    let caret = if p.kind == PromptKind::ConfirmDelete {
        ""
    } else {
        "▮"
    };
    let err = p.error.clone().unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .border_t_2()
        .border_color(BLACK)
        .bg(PAPER)
        .px_3()
        .py_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(p.kind.label()),
                )
                .child(div().child(format!("{input}{caret}"))),
        )
        .when(!err.is_empty(), |d| {
            d.child(div().bg(BLACK).text_color(WHITE).px_2().mt_1().child(err))
        })
        .into_any_element()
}

/// One editor line rendered as styled segments (inverse for selection/caret).
fn engine_line(engine: &Engine, line_ix: usize, focused: bool) -> impl IntoElement {
    let chars: Vec<char> = engine.buf.lines[line_ix].clone();
    let cursor = engine.cursor();
    let insert_mode = engine.mode == Mode::Insert;
    let sel = engine.selection();

    // Character ranges [start,end) of inverse text on this line.
    let (s_start, s_end, linewise) = match sel {
        Some((a, b, lw)) if a.0 <= line_ix && b.0 >= line_ix && (lw || b.0 > line_ix || true) => {
            let s = if lw {
                0
            } else if a.0 == line_ix {
                a.1
            } else {
                0
            };
            let e = if lw {
                chars.len()
            } else if b.0 == line_ix {
                b.1.min(chars.len())
            } else {
                chars.len()
            };
            (s, e, true)
        }
        _ => (usize::MAX, 0, false),
    };
    let _ = linewise;

    let caret_at = if focused && cursor.0 == line_ix {
        Some(cursor.1)
    } else {
        None
    };

    let mut segments: Vec<(String, bool)> = Vec::new();
    let mut ix = 0usize;
    while ix < chars.len() {
        let in_sel = s_start <= ix && ix < s_end;
        let is_caret = caret_at == Some(ix);
        let inverse = in_sel || (is_caret && !insert_mode);
        let len = if inverse {
            1
        } else {
            (s_start
                .max(ix + 1)
                .min(if s_end > ix { s_end } else { chars.len() })
                - ix)
                .max(1)
        };
        let len = len.min(chars.len() - ix);
        segments.push((chars[ix..ix + len].iter().collect(), inverse));
        ix += len;
    }
    if caret_at == Some(chars.len()) {
        // caret at end of line: block on a virtual space (insert mode or EOL)
        segments.push((" ".into(), focused));
    } else if segments.is_empty() && caret_at.is_some() {
        segments.push((" ".into(), focused));
    } else if segments.is_empty() {
        segments.push((" ".into(), false));
    }

    let mut line = div().flex().h(px(24.0)).items_center();
    for (text, inverse) in segments {
        let seg = div().child(text);
        if inverse {
            line = line.child(seg.bg(BLACK).text_color(WHITE));
        } else {
            line = line.child(seg);
        }
    }
    line
}

fn editor_pane(state: &GuiState) -> gpui::AnyElement {
    let Some(ed) = &state.editor else {
        return div().into_any_element();
    };
    let field = |label: &str, engine: &Engine, focused: bool| {
        let line_count = engine.buf.line_count();
        let mut col = div()
            .flex()
            .flex_col()
            .p_2()
            .m_2()
            .border_2()
            .border_color(if focused {
                BLACK
            } else {
                gpui::rgba(0x999999ff)
            });
        if focused {
            col = col.bg(WHITE);
        }
        let mut inner = div().flex().flex_col();
        for li in 0..line_count {
            inner = inner.child(engine_line(engine, li, focused));
        }
        col.child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .text_size(px(12.0))
                .text_color(if focused { BLACK } else { GREY })
                .child(format!("{label}  {}", engine.mode_name())),
        )
        .child(inner)
    };

    div()
        .flex()
        .flex_col()
        .flex_1()
        .map(scrollable)
        .bg(PAPER)
        .child(field("TITLE", &ed.title, ed.focus_title))
        .child(field("NOTES", &ed.notes, !ed.focus_title))
        .child(
            div()
                .px_4()
                .py_1()
                .text_color(GREY)
                .text_size(px(12.0))
                .child("tab fields · esc esc back to list · :q close · u/ctrl-r undo text"),
        )
        .into_any_element()
}

fn help_overlay() -> gpui::AnyElement {
    let keys: &[&str] = &[
        "j k g g G        move        1-4  switch view",
        "o / SPC SPC      capture     tab  cycle view",
        "enter / i        edit task   /    search",
        "x                complete    d    archive",
        "SPC d/u/r        defer/due/remind prompt",
        "SPC o            organize    > <  shift due ±1",
        "+ -              priority    .    repeat",
        "y Y              yank        V    visual mode",
        "D                delete      u ctrl-r undo/redo",
        ":                commands    ?    this help",
    ];
    div()
        .absolute()
        .size_full()
        .top_0()
        .bg(gpui::rgba(0xffffffee))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .bg(WHITE)
                .border_2()
                .border_color(BLACK)
                .p_6()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BLACK)
                        .child("INTENT — KEYMAP"),
                )
                .children(keys.iter().map(|k| div().child(*k)))
                .child(div().text_color(GREY).child("any key closes")),
        )
        .into_any_element()
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.state;
        let rows_len = state.rows.len();
        let list_scroll = self.list_scroll.clone();

        let list = uniform_list(
            "tasks",
            rows_len,
            cx.processor(move |this: &mut Shell, range: Range<usize>, _window, _cx| {
                range
                    .map(|ix| task_row(&this.state, ix))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(list_scroll.clone());

        let empty = div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(GREY)
            .child("nothing here — o to capture, ? for keys");

        let main = if state.editor.is_some() {
            editor_pane(state).into_any_element()
        } else if rows_len == 0 {
            empty.into_any_element()
        } else {
            list.into_any_element()
        };

        div()
            .id("root")
            .track_focus(&self.focus)
            .key_context("intent")
            .on_key_down(cx.listener(Self::dispatch_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e: &MouseDownEvent, _window, cx| {
                    // Clicking anywhere returns key focus to the app surface.
                    let _ = this;
                    cx.notify();
                }),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(WHITE)
            .text_color(BLACK)
            .font_family(MONO)
            .text_size(px(15.0))
            .child(header(state))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(sidebar(state))
                    .child(div().flex().flex_col().flex_1().min_w_0().child(main)),
            )
            .child(prompt_bar(state))
            .child(status_bar(state))
            .when(state.help_open, |d| d.child(help_overlay()))
    }
}

fn main() {
    let store = match Store::open_default() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("intent-gui: cannot open database: {e}");
            std::process::exit(1);
        }
    };

    Application::new().run(|cx: &mut App| {
        let clip = Rc::new(SyncClipboard::default());
        let state = GuiState::new(store, clip.clone());

        let bounds = gpui::Bounds::centered(None, gpui::size(px(1180.0), px(760.0)), cx);
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Matched by the desktop entry's StartupWMClass; also becomes
                // the X11 WM_CLASS (gpui routes it to both).
                app_id: Some("intent-gui".into()),
                ..Default::default()
            },
            |window, cx| {
                let entity = cx.new(|cx| Shell {
                    state,
                    clip,
                    focus: cx.focus_handle(),
                    list_scroll: UniformListScrollHandle::new(),
                });

                // Poll for external (CLI/agent) database changes every 2s.
                let poll = entity.clone();
                window
                    .spawn(cx, async move |cx| loop {
                        Timer::after(Duration::from_secs(2)).await;
                        let _ = cx.update(|_window, cx| {
                            poll.update(cx, |shell: &mut Shell, cx| {
                                if shell.state.check_external_changes() {
                                    cx.notify();
                                }
                            });
                        });
                    })
                    .detach();
                entity
            },
        );
        let _ = window;
        cx.activate(true);
    });
}
