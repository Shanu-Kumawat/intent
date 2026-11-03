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
use intent_vim::{Clipboard, Engine};

// Dark monochrome theme. Two anchor tones (INK body / PAPER foreground)
// with an inverted selection — the same neo-brutalist structure as the light
// theme, roles kept explicit so text/bg contrast is never accidental.
/// Body background — near-black, clearly not pure #000 so chrome reads apart.
const BG: gpui::Rgba = rgba_lit(0.055, 0.055, 0.055, 1.0); // #0e0e0e
/// Chrome (header/status) — pure black, one step below the body.
const CHROME: gpui::Rgba = rgba_lit(0.0, 0.0, 0.0, 1.0);
/// Primary foreground — pure white, 19.3:1 on BG, 21:1 on CHROME.
const FG: gpui::Rgba = rgba_lit(1.0, 1.0, 1.0, 1.0);
/// Secondary text — #b0b0b0 clears WCAG AAA (≥7:1) on every surface it
/// appears on: 8.9:1 on BG, 8.0:1 on SURFACE, 7.3:1 on FIELD.
const MUTED: gpui::Rgba = rgba_lit(0.69, 0.69, 0.69, 1.0);
/// Structure lines — #303030 on #0e0e0e stays visible without shouting.
const HAIRLINE: gpui::Rgba = rgba_lit(0.188, 0.188, 0.188, 1.0);
/// Elevated surfaces (prompt bar, editor pane) — #1a1a1a.
const SURFACE: gpui::Rgba = rgba_lit(0.102, 0.102, 0.102, 1.0);
/// Focused editor field — #222222, one step above SURFACE.
const FIELD: gpui::Rgba = rgba_lit(0.133, 0.133, 0.133, 1.0);
/// Visual-selection plate: the inverse language at reduced intensity —
/// clearly a marked row, clearly softer than NORMAL's pure-white focus.
/// Dark text on it keeps AA+.
const VISUAL_PLATE: gpui::Rgba = rgba_lit(0.788, 0.788, 0.788, 1.0); // #c9c9c9
/// Danger accent — the app's single non-monochrome color, reserved for
/// the delete-armed row. White on it keeps AA+ contrast.
const DANGER: gpui::Rgba = rgba_lit(0.60, 0.11, 0.11, 1.0); // #991b1b
/// Body font size (px); every surface derives from these.
const TEXT: f32 = 16.0;
/// Secondary text (meta columns, subtitles, footer hints).
const SMALL: f32 = 13.0;
/// View-heading name size / line height.
const HEADING: f32 = 24.0;
const HEADING_LH: f32 = 26.0;
/// Spacing scale (4px base) — every padding, margin, and gap in the shell
/// is one of these, so the layout rhythm is tunable in one place.
const S1: f32 = 4.0; // hairline gaps
const S2: f32 = 8.0; // tight
const S3: f32 = 12.0; // default column gap
const S4: f32 = 16.0; // standard padding
const S6: f32 = 24.0; // section padding
/// Component dimensions.
const HEADER_H: f32 = 44.0;
const BAR_H: f32 = 36.0;
const ROW_H: f32 = 36.0;
const SIDEBAR_W: f32 = 196.0;
/// Sidebar keeps the pre-bump body size (user preference).
const SIDEBAR_TEXT: f32 = 15.0;
/// Editor line height; the caret bar derives from it.
const LINE_H: f32 = 26.0;
/// Task-row columns: priority mark column, mark/checkbox column, drawn box.
const PRI_W: f32 = 31.0;
const MARK_W: f32 = 22.0;
const CHECKBOX: f32 = 14.0;
/// Mono trailing-bearing compensation for the 3-glyph priority mark
/// (!!! paints ~21px but reserves ~29px of advance): keeps the gap after
/// it equal to the row's left padding. Coupled to TEXT and MONO.
const PRI_TRAILING: f32 = -9.0;
/// Insert-mode caret bar.
const CARET_W: f32 = 2.0;
const CARET_H: f32 = 20.0;
/// Horizontal indent of the view heading's first letter.
const LIST_INDENT: f32 = 28.0;
/// Status-bar fields.
const PENDING_MIN_W: f32 = 60.0;
const STATUS_MAX_W: f32 = 640.0;
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
    /// Keeps the window-activation reload subscription alive (SPEC §2).
    #[allow(dead_code)]
    activation_sub: Option<gpui::Subscription>,
}

impl Shell {
    fn dispatch_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let view_before = (self.state.cursor, self.state.rows.len());
        let ks = &event.keystroke;
        // alt+1-4 switch views (alias of the bare 1-4, same habit as
        // workspaces/tabs elsewhere).
        if ks.modifiers.alt && !ks.modifiers.control && !ks.modifiers.shift {
            let view = match ks.key.as_str() {
                "1" => Some(intent_core::TaskView::Inbox),
                "2" => Some(intent_core::TaskView::Today),
                "3" => Some(intent_core::TaskView::Upcoming),
                "4" => Some(intent_core::TaskView::Completed),
                _ => None,
            };
            if let Some(v) = view {
                self.clip.snapshot_read(None);
                self.state.switch_view(v);
                if self.state.editor.is_none() {
                    self.list_scroll
                        .scroll_to_item(self.state.cursor, ScrollStrategy::Center);
                }
                cx.notify();
                return;
            }
        }
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
        // Recenter only when the key moved the cursor or changed the rows;
        // the user's manual scroll position survives no-op keys.
        if self.state.editor.is_none() && (self.state.cursor, self.state.rows.len()) != view_before
        {
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
        .px(px(S4))
        .h(px(HEADER_H))
        .border_b_2()
        .border_color(FG)
        .bg(CHROME)
        .text_color(FG)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(div().font_weight(gpui::FontWeight::BOLD).child("INTENT"))
                .child(div().text_color(MUTED).child("///"))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(SharedString::from(if state.search_query.is_some() {
                            "SEARCH".to_string()
                        } else {
                            state.view.to_string().to_uppercase()
                        })),
                )
                .child(div().text_color(MUTED).child(SharedString::from(search))),
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
        .w(px(SIDEBAR_W))
        .border_r_2()
        .border_color(FG);
    for (n, view) in items {
        // While a search is active no view claims focus: the search spans
        // all non-archived tasks, and pretending you're in TODAY while
        // Inbox rows show would contradict the sidebar's count.
        let active = state.view == view && !state.search_active();
        let count = state.view_counts.get(&view).copied().unwrap_or(0);
        let label = format!("{n} {}", view.to_string().to_uppercase());
        let mut row = div()
            .id(SharedString::from(format!("sb-{n}")))
            .flex()
            .justify_between()
            .px(px(S4))
            .py(px(S2))
            .text_size(px(SIDEBAR_TEXT));
        if active {
            row = row
                .bg(FG)
                .text_color(CHROME)
                .font_weight(gpui::FontWeight::BOLD);
        }
        col = col.child(row.child(label).child(format!("{count}")));
    }
    col = col.child(div().flex_1());
    col = col.child(
        div()
            .px(px(S4))
            .py(px(S2))
            .border_t_2()
            .border_color(HAIRLINE)
            .text_color(MUTED)
            .text_size(px(SIDEBAR_TEXT))
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
    // Vim-style search: non-matching rows fade so matches stand out.
    let dimmed = state.search_active() && !state.search_match(ix);

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
        .relative()
        .flex()
        .items_center()
        .gap(px(S3))
        .w_full()
        .h(px(ROW_H))
        .pl(px(S3))
        .pr(px(S4))
        .border_b_1()
        .border_color(HAIRLINE);
    // The delete-armed row turns red — the app's single non-monochrome
    // accent, reserved for imminent destruction.
    let armed = state.delete_armed.as_deref() == Some(task.id.as_str());
    let ink = if in_visual || selected { CHROME } else { FG };
    // Plates are full-bleed; every content column starts at LIST_INDENT,
    // aligned under the heading's first letter.
    let plate = if armed {
        Some(DANGER)
    } else if in_visual {
        Some(VISUAL_PLATE)
    } else if selected {
        Some(FG)
    } else {
        None
    };
    if let Some(color) = plate {
        row = row.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .right_0()
                .bg(color),
        );
    }
    if armed {
        row = row.text_color(FG);
    } else if in_visual || selected {
        row = row.text_color(CHROME);
    }
    if in_visual && selected {
        // Cursor row inside the selection: thin dark edge on the plate's
        // left, so extending with j/k stays oriented.
        row = row.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(2.0))
                .bg(CHROME),
        );
    }
    row.child(
        div()
            .w(px(PRI_W))
            .font_weight(gpui::FontWeight::BOLD)
            .when(task.priority != intent_core::Priority::None, |d| {
                d.font_weight(gpui::FontWeight::BLACK)
            })
            .child(priority_mark(task.priority)),
    )
    .child(
        // The pending-inbox checkbox is drawn (not the □ glyph) so its
        // size is ours to tune; the other marks stay glyphs. The negative
        // margin cancels the priority text's trailing advance so the gap
        // after !!! matches the row's left padding.
        div()
            .w(px(MARK_W))
            .ml(px(PRI_TRAILING))
            .flex()
            .items_center()
            .child(
                if task.inbox && task.status == intent_core::Status::Active {
                    div()
                        .w(px(CHECKBOX))
                        .h(px(CHECKBOX))
                        .border_2()
                        .border_color(ink)
                        .flex_none()
                } else {
                    div().child(mark)
                },
            ),
    )
    .child({
        // While searching, matched substrings get the inverse plate right
        // in the title (neovim-style hl-search). Rows already on the
        // inverse plate skip it — white on white is invisible.
        let terms = state.search_terms();
        let ranges = if terms.is_empty() || inverse {
            vec![]
        } else {
            match_ranges(&title, &terms)
        };
        if ranges.is_empty() {
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .font_weight(if task.status == intent_core::Status::Done {
                    gpui::FontWeight::THIN
                } else {
                    gpui::FontWeight::NORMAL
                })
                .when(task.status == intent_core::Status::Done, |d| {
                    d.line_through()
                })
                // Dimming only applies off-plate: muted-on-white would be
                // unreadable on the focused row.
                .when(dimmed && !inverse, |d| d.text_color(MUTED))
                .child(title)
        } else {
            let base = mono_style(
                if dimmed && !inverse { MUTED } else { FG },
                if task.status == intent_core::Status::Done {
                    gpui::FontWeight::THIN
                } else {
                    gpui::FontWeight::NORMAL
                },
                task.status == intent_core::Status::Done,
            );
            let highlights: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = ranges
                .iter()
                .map(|(a, b)| {
                    (
                        *a..*b,
                        gpui::HighlightStyle {
                            color: Some(CHROME.into()),
                            background_color: Some(FG.into()),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .child(gpui::StyledText::new(title).with_default_highlights(&base, highlights))
        }
    })
    .child(
        div()
            .text_size(px(SMALL))
            .text_color(if armed {
                rgba_lit(0.93, 0.80, 0.80, 1.0) // soft red-tinted white: 5.6:1
            } else if inverse {
                rgba_lit(0.29, 0.29, 0.29, 1.0) // on the light plates: 5.4:1+
            } else {
                MUTED
            })
            .child(meta),
    )
    .child(
        div()
            .text_size(px(SMALL))
            .text_color(if armed {
                rgba_lit(0.93, 0.80, 0.80, 1.0)
            } else if inverse {
                rgba_lit(0.29, 0.29, 0.29, 1.0)
            } else {
                MUTED
            })
            .child(task.id.clone()),
    )
    .into_any_element()
}

/// Heading above the list: the view's name and its task count (match
/// count while searching — the search spans all non-archived tasks).
fn view_heading(state: &GuiState) -> impl IntoElement {
    let (name, sub) = if state.search_active() {
        let n = state.search_match_count();
        (
            "Search".to_string(),
            format!(
                "{} match{} — esc to exit",
                n,
                if n == 1 { "" } else { "es" }
            ),
        )
    } else {
        let name = {
            let raw = state.view.to_string();
            let mut cs = raw.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => raw,
            }
        };
        let n = state.rows.len();
        (name, format!("{} task{}", n, if n == 1 { "" } else { "s" }))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(S1))
        .pl(px(LIST_INDENT))
        .pr(px(S4))
        .pt(px(S3))
        .pb(px(S3))
        .child(
            div()
                .text_size(px(HEADING))
                .line_height(px(HEADING_LH))
                .font_weight(gpui::FontWeight::BOLD)
                .child(name),
        )
        .child(
            div()
                .text_size(px(SMALL))
                .line_height(px(S4))
                .text_color(MUTED)
                .child(sub),
        )
}

fn status_bar(state: &GuiState) -> impl IntoElement {
    let pos = if state.rows.is_empty() {
        "0/0".to_string()
    } else {
        format!("{}/{}", state.cursor + 1, state.rows.len())
    };
    let id = state
        .editor_task_id()
        .map(str::to_string)
        .or_else(|| state.cursor_task().map(|t| t.id.clone()))
        .unwrap_or_default();
    // In the editor the position is the buffer line/column, not the list row.
    let pos = state
        .editor_position()
        .map(|(l, c)| format!("{l}:{c}"))
        .unwrap_or(pos);
    div()
        .flex()
        .items_center()
        .gap(px(S3))
        .h(px(BAR_H))
        .px(px(S3))
        .border_t_2()
        .border_color(FG)
        .bg(CHROME)
        .child(
            div()
                .bg(FG)
                .text_color(CHROME)
                .px_2()
                .py_1()
                .font_weight(gpui::FontWeight::BOLD)
                .child(SharedString::from(state.mode_label())),
        )
        .child(
            div()
                .min_w(px(PENDING_MIN_W))
                .child(SharedString::from(state.pending_display())),
        )
        .child(div().text_color(MUTED).child(id))
        .child(div().text_color(MUTED).child(pos))
        .child(div().flex_1())
        .child(
            div()
                .max_w(px(STATUS_MAX_W))
                .overflow_x_hidden()
                .child(SharedString::from(state.status.clone())),
        )
}

fn prompt_bar(state: &GuiState) -> gpui::AnyElement {
    let Some(p) = &state.prompt else {
        return div().h_0().into_any_element();
    };
    let input = p.input.clone();
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
        .border_color(FG)
        .bg(SURFACE)
        .px(px(S3))
        .py(px(S2))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(S2))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(p.kind.label()),
                )
                .child(div().child(format!("{input}{caret}"))),
        )
        .when(!err.is_empty(), |d| {
            d.child(div().bg(FG).text_color(CHROME).px_2().mt_1().child(err))
        })
        .into_any_element()
}

/// Base text style for a shaped text run in the shell's mono face.
fn mono_style(color: gpui::Rgba, weight: gpui::FontWeight, strikethrough: bool) -> gpui::TextStyle {
    gpui::TextStyle {
        color: color.into(),
        font_family: MONO.into(),
        font_size: gpui::AbsoluteLength::Pixels(px(TEXT)),
        font_weight: weight,
        strikethrough: strikethrough.then(Default::default),
        ..Default::default()
    }
}

/// Byte ranges of `terms` inside `title`, case-insensitively (for search
/// highlighting). Skips titles where lowercasing changes the char count
/// (byte offsets would no longer line up).
fn match_ranges(title: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let lower = title.to_lowercase();
    let orig_chars: Vec<char> = title.chars().collect();
    if lower.chars().count() != orig_chars.len() {
        return vec![];
    }
    let hay: Vec<char> = lower.chars().collect();
    let mut byte_of: Vec<usize> = Vec::with_capacity(hay.len() + 1);
    let mut off = 0usize;
    for c in &orig_chars {
        byte_of.push(off);
        off += c.len_utf8();
    }
    byte_of.push(off);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let t: Vec<char> = term.chars().collect();
        if t.is_empty() {
            continue;
        }
        for i in 0..hay.len().saturating_sub(t.len() - 1) {
            if hay[i..i + t.len()] == t[..] {
                ranges.push((byte_of[i], byte_of[i + t.len()]));
            }
        }
    }
    ranges.sort_by_key(|(a, b)| (*a, *b));
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in ranges {
        match merged.last_mut() {
            Some((_, eb)) if a <= *eb => *eb = (*eb).max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
}

/// One editor line: shaped as a SINGLE text run, with inverse plates
/// (selection, block caret) as per-run highlight ranges and the insert-mode
/// bar positioned by measuring the shaped prefix. Text elements must never
/// be split at the caret — each split point costs ~1px of column drift,
/// which shows up as uneven spacing down the cursor's column.
fn engine_line(
    engine: &Engine,
    line_ix: usize,
    focused: bool,
    ts: &std::sync::Arc<gpui::WindowTextSystem>,
) -> impl IntoElement {
    let lr = intent_gui::render::line_render(engine, line_ix, focused);
    let mut style = mono_style(FG, gpui::FontWeight::NORMAL, false);
    style.line_height = gpui::DefiniteLength::Absolute(gpui::AbsoluteLength::Pixels(px(LINE_H)));
    let highlights: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = lr
        .inverse
        .iter()
        .map(|(a, b)| {
            (
                *a..*b,
                gpui::HighlightStyle {
                    color: Some(CHROME.into()),
                    background_color: Some(FG.into()),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut line = div().relative().flex_none();
    line = line
        .child(gpui::StyledText::new(lr.text.clone()).with_default_highlights(&style, highlights));
    if let Some(byte) = lr.insert_bar {
        // The bar overlays the left edge of the caret column. Its x is the
        // shaped width of the prefix — same font, same shaping, so it lands
        // exactly on the column boundary (nudged 1px to straddle it).
        let prefix = &lr.text[..byte];
        let run = style.to_run(prefix.len());
        let width = ts
            .layout_line(prefix, px(TEXT), std::slice::from_ref(&run), None)
            .width;
        line = line.child(
            div()
                .absolute()
                .left(width - px(1.0))
                .top(px((LINE_H - CARET_H) / 2.0))
                .w(px(CARET_W))
                .h(px(CARET_H))
                .bg(FG),
        );
    }
    line
}

fn editor_pane(state: &GuiState, ts: &std::sync::Arc<gpui::WindowTextSystem>) -> gpui::AnyElement {
    let Some(ed) = &state.editor else {
        return div().into_any_element();
    };
    let field = |label: &str, engine: &Engine, focused: bool, ts: &_| {
        let line_count = engine.buf.line_count();
        let mut col = div()
            .flex()
            .flex_col()
            .p(px(S2))
            .m(px(S2))
            .border_2()
            .border_color(if focused { FG } else { HAIRLINE });
        if focused {
            col = col.bg(FIELD);
        }
        let mut inner = div().flex().flex_col();
        for li in 0..line_count {
            inner = inner.child(engine_line(engine, li, focused, ts));
        }
        col.child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .text_size(px(SMALL))
                .text_color(if focused { FG } else { MUTED })
                .child(format!("{label}  {}", engine.mode_name())),
        )
        .child(inner)
    };

    div()
        .flex()
        .flex_col()
        .flex_1()
        .map(scrollable)
        .bg(SURFACE)
        .child(field("TITLE", &ed.title, ed.focus_title, ts))
        .child(field("NOTES", &ed.notes, !ed.focus_title, ts))
        .child(
            div()
                .px(px(S4))
                .py(px(S1))
                .text_color(MUTED)
                .text_size(px(SMALL))
                .child(
                    "tab/ctrl-j/k fields · esc esc back to list · :q close · u/ctrl-r undo text",
                ),
        )
        .into_any_element()
}

fn help_overlay() -> gpui::AnyElement {
    let keys: &[&str] = &[
        "j k ↑ ↓ g g G   move        1-4 alt+1-4  view",
        "ctrl-d/ctrl-u    jump ±10    tab  cycle view",
        "o SPC SPC c      capture     ?    keymap",
        "enter / i / a    edit task   /    search (live)",
        "n N              cycle matches",
        "x                complete    d    archive",
        "SPC d/u/r        defer/due/remind prompt",
        "SPC o            organize    > <  shift due ±1",
        "SPC D            unarchive   SPC p  paste system clipboard",
        "+ - =            priority    .    repeat",
        "y Y              yank        V    visual mode",
        "D D              delete (2nd confirms)   u ctrl-r undo/redo",
        ":                commands    esc  cancel",
    ];
    div()
        .absolute()
        .size_full()
        .top_0()
        .bg(rgba_lit(0.0, 0.0, 0.0, 0.88))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .bg(SURFACE)
                .border_2()
                .border_color(FG)
                .p(px(S6))
                .flex()
                .flex_col()
                .gap(px(S2))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BLACK)
                        .child("INTENT — KEYMAP"),
                )
                .children(keys.iter().map(|k| div().child(*k)))
                .child(div().text_color(MUTED).child("any key closes")),
        )
        .into_any_element()
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Self-heal focus loss (e.g. after window activation): Intent routes
        // every surface's keys through this one root handle.
        if !self.focus.is_focused(window) {
            self.focus.focus(window);
        }
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
        .flex_1()
        .min_h_0()
        .track_scroll(list_scroll.clone());

        let empty = div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(MUTED)
            .child("nothing here — o to capture, ? for keys");

        let main = if state.editor.is_some() {
            editor_pane(state, window.text_system()).into_any_element()
        } else {
            // Heading above the list (and the empty state); the editor
            // takes the whole column.
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(view_heading(state))
                .child(if rows_len == 0 {
                    empty.into_any_element()
                } else {
                    list.into_any_element()
                })
                .into_any_element()
        };

        div()
            .id("root")
            .track_focus(&self.focus)
            .key_context("intent")
            .on_key_down(cx.listener(Self::dispatch_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e: &MouseDownEvent, window, cx| {
                    // Clicking anywhere returns key focus to the app surface.
                    window.focus(&this.focus);
                    cx.notify();
                }),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(FG)
            .font_family(MONO)
            .text_size(px(TEXT))
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
                    activation_sub: None,
                });
                // Key dispatch requires a focused handle; without this the
                // window is keyboard-dead until clicked. Also subscribes the
                // SPEC §2 activation reload — observe_window_activation is a
                // Context method, and on_focus_in would never re-fire (the
                // root handle stays focused across alt-tab).
                entity.update(cx, |shell, cx| {
                    shell.focus.focus(window);
                    shell.activation_sub =
                        Some(cx.observe_window_activation(window, |shell, window, cx| {
                            if window.is_window_active() && shell.state.check_external_changes() {
                                cx.notify();
                            }
                        }));
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
        // Closing the window through the WM must end the process: gpui does
        // not quit on the last window close by default, and the poll task
        // would otherwise spin headless forever, holding the store.
        // Normal exits (:q) already quit; this catches the WM path. The
        // subscription lives for the app's lifetime, which is exactly what
        // we want — drop it.
        let _ = cx.on_window_closed(|cx| cx.quit());
        cx.activate(true);
    });
}
