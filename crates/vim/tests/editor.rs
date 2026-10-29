//! Behavioral tests for the modal engine, driven through the public API
//! exactly as the GUI drives it (SPEC §4.2, §4.5, §5).

use intent_vim::{Clipboard as _, Engine, EngineEvent, Key, MemClipboard, Register};
use std::rc::Rc;

struct Ctx {
    clip: Rc<MemClipboard>,
}

fn engine(text: &str) -> (Engine, Ctx) {
    let clip = Rc::new(MemClipboard::new());
    let e = Engine::new(text, false, Box::new(ClipAdapter(clip.clone())));
    (e, Ctx { clip })
}

fn engine_title(text: &str) -> (Engine, Ctx) {
    let clip = Rc::new(MemClipboard::new());
    let e = Engine::new(text, true, Box::new(ClipAdapter(clip.clone())));
    (e, Ctx { clip })
}

#[derive(Clone)]
struct ClipAdapter(Rc<MemClipboard>);

impl intent_vim::Clipboard for ClipAdapter {
    fn read(&self) -> Option<String> {
        self.0.read()
    }
    fn write(&self, text: &str) {
        self.0.write(text);
    }
}

/// Parse a compact key string. `<esc>` `<cr>` `<bs>` `<tab>` `<space>` `<c-r>`
/// `<c-w>` `<c-u>` `<up>` etc. are named keys; any other multi-character
/// token expands to individual character keys (so "abc" types a, b, c).
/// Uppercase letters become shift+letter.
fn parse_keys(s: &str) -> Vec<Key> {
    let mut out = Vec::new();
    for t in s.split(' ').filter(|t| !t.is_empty()) {
        if let Some(inner) = t.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
            let lower = inner.to_ascii_lowercase();
            let key = match lower.as_str() {
                "esc" => Key::new("esc"),
                "cr" | "enter" => Key::new("enter"),
                "bs" | "backspace" => Key::new("backspace"),
                "tab" => Key::new("tab"),
                "space" => Key::new("space"),
                "up" => Key::new("up"),
                "down" => Key::new("down"),
                "left" => Key::new("left"),
                "right" => Key::new("right"),
                _ if lower.starts_with("c-") => Key::ctrl(&lower[2..]),
                _ => Key::new(inner),
            };
            out.push(key);
        } else {
            for c in t.chars() {
                if c.is_ascii_uppercase() {
                    out.push(Key::shifted(&c.to_ascii_lowercase().to_string()));
                } else {
                    out.push(Key::new(&c.to_string()));
                }
            }
        }
    }
    out
}

fn feed(e: &mut Engine, s: &str) {
    for k in parse_keys(s) {
        e.handle(k);
    }
}

fn at(e: &Engine) -> (usize, usize) {
    e.cursor()
}

// ---- motions -----------------------------------------------------------------

#[test]
fn basic_motions() {
    let (mut e, _) = engine("hello world\nsecond line");
    feed(&mut e, "$"); // last char of line 0
    assert_eq!(at(&e), (0, 10));
    feed(&mut e, "0 w"); // start of "world"
    assert_eq!(at(&e), (0, 6));
    feed(&mut e, "e"); // end of "world"
    assert_eq!(at(&e), (0, 10));
    feed(&mut e, "b");
    assert_eq!(at(&e), (0, 6));
    feed(&mut e, "0");
    assert_eq!(at(&e), (0, 0));
    feed(&mut e, "3 l");
    assert_eq!(at(&e), (0, 3));
    feed(&mut e, "h");
    assert_eq!(at(&e), (0, 2));
    feed(&mut e, "j");
    assert_eq!(at(&e), (1, 2));
    feed(&mut e, "k");
    assert_eq!(at(&e), (0, 2));
    feed(&mut e, "G"); // last line, first non-blank
    assert_eq!(at(&e), (1, 0));
    feed(&mut e, "2 G");
    assert_eq!(at(&e), (1, 0));
    feed(&mut e, "g g");
    assert_eq!(at(&e), (0, 0));
}

#[test]
fn counts_on_motions() {
    let (mut e, _) = engine("aaaa\nbbbb\ncccc\ndddd");
    feed(&mut e, "2 j");
    assert_eq!(at(&e), (2, 0));
    feed(&mut e, "3 l");
    assert_eq!(at(&e), (2, 3));
    feed(&mut e, "3 G");
    assert_eq!(at(&e), (2, 0));
    feed(&mut e, "1 G");
    assert_eq!(at(&e), (0, 0));
}

#[test]
fn word_vs_big_word() {
    let (mut e, _) = engine("foo.bar baz");
    feed(&mut e, "w");
    assert_eq!(at(&e), (0, 3), "punctuation run is its own word");
    feed(&mut e, "w");
    assert_eq!(at(&e), (0, 4));
    feed(&mut e, "0 W");
    assert_eq!(at(&e), (0, 8), "W treats foo.bar as one word");
}

#[test]
fn find_motions_and_repeats() {
    let (mut e, _) = engine("aXbXcXd");
    feed(&mut e, "f X");
    assert_eq!(at(&e), (0, 1));
    feed(&mut e, ";");
    assert_eq!(at(&e), (0, 3));
    feed(&mut e, "2 ;");
    assert_eq!(at(&e), (0, 5));
    feed(&mut e, ",");
    assert_eq!(at(&e), (0, 3));
    feed(&mut e, "0 t X");
    assert_eq!(at(&e), (0, 0));
    feed(&mut e, "F X");
    assert_eq!(at(&e), (0, 0), "no X behind: stays put");
}

#[test]
fn dollar_and_empty_lines() {
    let (mut e, _) = engine("ab\n\nlonger line");
    feed(&mut e, "$");
    assert_eq!(at(&e), (0, 1));
    feed(&mut e, "j");
    assert_eq!(at(&e), (1, 0));
    feed(&mut e, "j $");
    assert_eq!(at(&e), (2, 10));
}

// ---- operators -----------------------------------------------------------------

#[test]
fn delete_word_and_line() {
    let (mut e, _) = engine("foo bar baz");
    feed(&mut e, "d w");
    assert_eq!(e.text(), "bar baz");
    feed(&mut e, "d d");
    assert_eq!(e.text(), "");
    feed(&mut e, "d d");
    assert_eq!(e.text(), "", "dd on the last empty line is a no-op");
}

#[test]
fn delete_to_ends() {
    let (mut e, _) = engine("hello world");
    feed(&mut e, "3 l d $");
    assert_eq!(e.text(), "hel", "d$ deletes through end of line");

    let (mut e, _) = engine("hello");
    feed(&mut e, "$ d 0");
    assert_eq!(e.text(), "o", "backward delete keeps the cursor char");

    let (mut e, _) = engine("hello world");
    feed(&mut e, "3 l D");
    assert_eq!(e.text(), "hel", "D deletes from cursor through EOL");
}

#[test]
fn delete_with_find() {
    let (mut e, _) = engine("foo(bar)baz");
    feed(&mut e, "d f )");
    assert_eq!(e.text(), "baz", "df deletes through the target");

    let (mut e, _) = engine("foo(bar)baz");
    feed(&mut e, "d t b");
    assert_eq!(e.text(), "bar)baz", "dt deletes up to before the target");
}

#[test]
fn delete_lines_with_jk() {
    let (mut e, _) = engine("one\ntwo\nthree\nfour");
    feed(&mut e, "d j");
    assert_eq!(e.text(), "three\nfour");
    feed(&mut e, "2 d d");
    assert_eq!(e.text(), "");
}

#[test]
fn dw_never_eats_newlines() {
    let (mut e, _) = engine("aa bb\ncc");
    feed(&mut e, "$ d w");
    assert_eq!(e.text(), "aa b\ncc");
    let (mut e, _) = engine("aa\ncc");
    feed(&mut e, "d w");
    assert_eq!(e.text(), "\ncc", "dw on the only word deletes to EOL");
}

#[test]
fn text_objects() {
    let (mut e, _) = engine("foo bar baz");
    feed(&mut e, "d i w");
    assert_eq!(e.text(), " bar baz");

    let (mut e, _) = engine("foo bar baz");
    feed(&mut e, "w d a w");
    assert_eq!(e.text(), "foo baz");

    let (mut e, _) = engine("say \"hello world\" now");
    feed(&mut e, "2 w d i \"");
    assert_eq!(e.text(), "say \"\" now");

    let (mut e, _) = engine("say \"hello\" now");
    feed(&mut e, "2 w d a \"");
    assert_eq!(e.text(), "say  now");

    let (mut e, _) = engine("fn(a b c) end");
    feed(&mut e, "f ( d i b");
    assert_eq!(e.text(), "fn() end");
}

#[test]
fn change_operations() {
    let (mut e, _) = engine("foo bar");
    feed(&mut e, "c w X <esc>");
    assert_eq!(e.text(), "X bar");

    // cw from inside a word changes to the end of that word
    let (mut e, _) = engine("quux mid");
    feed(&mut e, "l c w ab <esc>");
    assert_eq!(e.text(), "qab mid");

    let (mut e, _) = engine("one\ntwo");
    feed(&mut e, "c c new <esc>");
    assert_eq!(e.text(), "new\ntwo");

    let (mut e, _) = engine("hello");
    feed(&mut e, "C yo <esc>");
    assert_eq!(e.text(), "yo");

    let (mut e, _) = engine("abc");
    feed(&mut e, "s Z <esc>");
    assert_eq!(e.text(), "Zbc");

    let (mut e, _) = engine("a\nb");
    feed(&mut e, "S Q <esc>");
    assert_eq!(e.text(), "Q\nb");
}

#[test]
fn insert_variants() {
    let (mut e, _) = engine("hello");
    feed(&mut e, "l l i X <esc>");
    assert_eq!(e.text(), "heXllo");
    let (mut e, _) = engine("hello");
    feed(&mut e, "l l a X <esc>");
    assert_eq!(e.text(), "helXlo");
    let (mut e, _) = engine("  hi");
    feed(&mut e, "I X <esc>");
    assert_eq!(e.text(), "  Xhi", "I inserts before first non-blank");
    let (mut e, _) = engine("hi");
    feed(&mut e, "A !");
    assert_eq!(e.text(), "hi!");
    let (mut e, _) = engine("a\nb");
    feed(&mut e, "o new <esc>");
    assert_eq!(e.text(), "a\nnew\nb");
    feed(&mut e, "O top <esc>");
    assert_eq!(e.text(), "a\ntop\nnew\nb");
}

#[test]
fn insert_mode_editing() {
    let (mut e, _) = engine("one two");
    feed(&mut e, "A three");
    assert_eq!(e.text(), "one twothree");
    feed(&mut e, "<esc>");

    // ctrl-w deletes the word back (spaces must be explicit in the feed)
    let (mut e, _) = engine("");
    feed(&mut e, "i alpha <space> beta <space> <c-w> <esc>");
    assert_eq!(e.text(), "alpha ");

    // ctrl-u stops at the insert origin (pre-existing text survives)
    let (mut e, _) = engine("keep");
    feed(&mut e, "A junk junk <c-u> <esc>");
    assert_eq!(e.text(), "keep");

    let (mut e, _) = engine("");
    feed(&mut e, "i typed <c-u> <esc>");
    assert_eq!(e.text(), "");

    // backspace joins lines in insert mode
    let (mut e, _) = engine("ab\ncd");
    feed(&mut e, "j i <bs> <esc>");
    assert_eq!(e.text(), "abcd");
}

#[test]
fn enter_splits_lines_only_in_notes() {
    let (mut e, _) = engine("ab cd");
    feed(&mut e, "f c a <cr> <esc>");
    assert_eq!(e.text(), "ab c\nd");

    let (mut e, _) = engine_title("ab");
    let events: Vec<EngineEvent> = parse_keys("A <cr> x <esc>")
        .into_iter()
        .map(|k| e.handle(k))
        .collect();
    assert_eq!(e.text(), "abx", "title ignores enter");
    assert!(events.contains(&EngineEvent::TextChanged));
}

// ---- clipboard semantics (SPEC §4.5) -----------------------------------------

#[test]
fn yank_writes_clipboard_delete_does_not() {
    let (mut e, ctx) = engine("foo bar");
    feed(&mut e, "y y");
    assert_eq!(ctx.clip.read().as_deref(), Some("foo bar\n"));

    feed(&mut e, "d d");
    assert_eq!(
        ctx.clip.read().as_deref(),
        Some("foo bar\n"),
        "delete never clobbers the system clipboard"
    );
    feed(&mut e, "P");
    assert_eq!(
        e.text(),
        "foo bar\n",
        "dd P restores; Vim leaves the empty line below"
    );
}

#[test]
fn paste_after_and_before() {
    let (mut e, _) = engine("hello world");
    feed(&mut e, "y w");
    feed(&mut e, "$ p");
    assert_eq!(e.text(), "hello worldhello ");

    let (mut e, _) = engine("a\nb");
    feed(&mut e, "y y p");
    assert_eq!(e.text(), "a\na\nb");
    feed(&mut e, "P");
    assert_eq!(e.text(), "a\na\na\nb");
}

#[test]
fn p_uses_edit_buffer_not_clipboard() {
    let (mut e, ctx) = engine("ab cd");
    feed(&mut e, "y i w"); // yanks "ab", also writes clipboard
    ctx.clip.write("from os");
    feed(&mut e, "$ p");
    assert_eq!(e.text(), "ab cdab", "p pastes the edit buffer");

    let (mut e, ctx) = engine("dest");
    ctx.clip.write("from os");
    feed(&mut e, "$");
    assert!(e.paste_from_clipboard());
    assert_eq!(e.text(), "desfrom ost", "explicit clipboard paste");
}

// ---- undo/redo (SPEC §5) --------------------------------------------------------

#[test]
fn undo_insert_run_is_one_unit() {
    let (mut e, _) = engine("");
    feed(&mut e, "i abc def <esc>");
    assert_eq!(e.text(), "abcdef");
    feed(&mut e, "u");
    assert_eq!(e.text(), "");
    feed(&mut e, "<c-r>");
    assert_eq!(e.text(), "abcdef");
}

#[test]
fn undo_change_operator_includes_typed_text() {
    let (mut e, _) = engine("foo bar");
    feed(&mut e, "c w xy <esc>");
    assert_eq!(e.text(), "xy bar");
    feed(&mut e, "u");
    assert_eq!(e.text(), "foo bar");
    feed(&mut e, "<c-r>");
    assert_eq!(e.text(), "xy bar");
}

#[test]
fn undo_delete_and_multi_line() {
    let (mut e, _) = engine("one\ntwo\nthree");
    feed(&mut e, "d j");
    assert_eq!(e.text(), "three");
    feed(&mut e, "u");
    assert_eq!(e.text(), "one\ntwo\nthree");
    feed(&mut e, "<c-r>");
    assert_eq!(e.text(), "three");
}

#[test]
fn undo_stack_is_linear() {
    let (mut e, _) = engine("");
    feed(&mut e, "i a <esc> a b <esc> a c <esc>");
    assert_eq!(e.text(), "abc");
    feed(&mut e, "u u");
    assert_eq!(e.text(), "a");
    feed(&mut e, "a X <esc>");
    assert_eq!(e.text(), "aX");
    feed(&mut e, "<c-r>");
    assert_eq!(e.text(), "aX", "new change clears the redo stack");
}

// ---- repeat (.) -------------------------------------------------------------------

#[test]
fn repeat_simple_and_operator() {
    let (mut e, _) = engine("abcdef");
    feed(&mut e, "x");
    assert_eq!(e.text(), "bcdef");
    feed(&mut e, ".");
    assert_eq!(e.text(), "cdef");
    feed(&mut e, "3 .");
    assert_eq!(e.text(), "f");

    let (mut e, _) = engine("one two three");
    feed(&mut e, "d w");
    assert_eq!(e.text(), "two three");
    feed(&mut e, ".");
    assert_eq!(e.text(), "three");
}

#[test]
fn repeat_insert_run() {
    let (mut e, _) = engine("");
    feed(&mut e, "a foo <esc>");
    assert_eq!(e.text(), "foo");
    feed(&mut e, ".");
    assert_eq!(e.text(), "foofoo");
    feed(&mut e, "2 .");
    assert_eq!(e.text(), "foofoofoofoo");
}

// ---- visual -----------------------------------------------------------------------

#[test]
fn visual_charwise_delete_yank() {
    let (mut e, _) = engine("hello world");
    feed(&mut e, "v 3 l d");
    assert_eq!(e.text(), "o world");

    let (mut e, ctx) = engine("hello world");
    feed(&mut e, "v 3 l y");
    assert_eq!(e.text(), "hello world");
    assert_eq!(
        ctx.clip.read().as_deref(),
        Some("hell"),
        "visual yank writes clipboard"
    );
}

#[test]
fn visual_text_object() {
    let (mut e, _) = engine("hello world");
    feed(&mut e, "v i w d");
    assert_eq!(e.text(), " world");
}

#[test]
fn visual_linewise() {
    let (mut e, _) = engine("a\nb\nc");
    feed(&mut e, "V j d");
    assert_eq!(e.text(), "c");
    feed(&mut e, "u");
    assert_eq!(e.text(), "a\nb\nc");
    feed(&mut e, "g g V j y");
    feed(&mut e, "j p");
    assert_eq!(e.text(), "a\nb\na\nb\nc");
}

#[test]
fn visual_case_toggle_and_paste() {
    let (mut e, _) = engine("abc");
    feed(&mut e, "v i w ~");
    assert_eq!(e.text(), "ABC");

    let (mut e, _) = engine("xx repl yy");
    feed(&mut e, "y i w");
    feed(&mut e, "l l l v i w p");
    assert_eq!(e.text(), "xx xx yy");
}

// ---- pending sequence display and cancellation -----------------------------------

#[test]
fn pending_sequences_are_visible_and_cancelable() {
    let (mut e, _) = engine("abc");
    e.handle(Key::new("d"));
    assert_eq!(e.pending_display(), "d");
    e.handle(Key::new("2"));
    assert_eq!(e.pending_display(), "d2");
    e.handle(Key::new("w"));
    assert_eq!(e.pending_display(), "");
    assert_eq!(e.text(), "");

    let (mut e, _) = engine("abc");
    e.handle(Key::new("d"));
    e.handle(Key::new("z"));
    assert_eq!(e.pending_display(), "", "invalid continuation cancels");
    assert_eq!(e.status, "sequence cancelled");

    let (mut e, _) = engine("abc");
    e.handle(Key::new("2"));
    e.handle(Key::new("esc"));
    assert_eq!(e.pending_display(), "");
    assert_eq!(e.text(), "abc");
}

#[test]
fn no_timeout_semantics() {
    let (mut e, _) = engine("one two");
    e.handle(Key::new("d"));
    e.handle(Key::new("2"));
    e.handle(Key::new("0"));
    assert_eq!(
        e.pending_display(),
        "d20",
        "counts accumulate with no timeout"
    );
}

// ---- single-line (title) -----------------------------------------------------------

#[test]
fn title_field_events() {
    let (mut e, _) = engine_title("hi");
    assert_eq!(e.handle(Key::new("j")), EngineEvent::NextField);
    assert_eq!(e.handle(Key::new("k")), EngineEvent::PrevField);
}

#[test]
fn notes_field_boundary_events() {
    let (mut e, _) = engine("only");
    assert_eq!(e.handle(Key::new("j")), EngineEvent::NextField);
    assert_eq!(e.handle(Key::new("k")), EngineEvent::PrevField);
    assert_eq!(e.handle(Key::new("l")), EngineEvent::None);
}

// ---- assorted correctness ------------------------------------------------------------

#[test]
fn join_lines() {
    let (mut e, _) = engine("one\ntwo\nthree");
    feed(&mut e, "J");
    assert_eq!(e.text(), "one two\nthree");
    // [count]J joins [count] lines: 2J equals plain J here (Vim semantics).
    feed(&mut e, "u");
    feed(&mut e, "2 J");
    assert_eq!(e.text(), "one two\nthree");
    feed(&mut e, "u");
    feed(&mut e, "3 J");
    assert_eq!(e.text(), "one two three");
}

// ---- round-2 audit regression tests -------------------------------------------

#[test]
fn visual_linewise_change_types_onto_blank_line() {
    let (mut e, _) = engine("a\nb\nc");
    feed(&mut e, "V j c X <esc>");
    assert_eq!(
        e.text(),
        "X\nc",
        "following line must survive linewise change"
    );
}

#[test]
fn cc_single_line_needs_no_extra_blank() {
    let (mut e, _) = engine("abc");
    feed(&mut e, "c c Q <esc>");
    assert_eq!(e.text(), "Q", "cc on the only line leaves exactly one line");
}

#[test]
fn cc_last_line_replaces_last_line() {
    let (mut e, _) = engine("a\nb\nc");
    feed(&mut e, "j j c c Q <esc>");
    assert_eq!(e.text(), "a\nb\nQ", "blank replacement goes at the end");
}

#[test]
fn d_capital_honors_count() {
    let (mut e, _) = engine("aa\nbb\ncc");
    feed(&mut e, "2 D");
    assert_eq!(e.text(), "cc", "2D deletes through the second line's end");
}

#[test]
fn yank_does_not_replace_last_change() {
    let (mut e, _) = engine("abcdef");
    feed(&mut e, "x"); // delete 'a'
    feed(&mut e, "y l"); // yank 'b' — must NOT become the repeat target
    feed(&mut e, ".");
    assert_eq!(e.text(), "cdef", "`.` repeats the deletion, not the yank");
}

#[test]
fn paste_into_title_flattens_newlines() {
    let clip = std::rc::Rc::new(MemClipboard::new());
    clip.write("a\nb");
    let mut e = Engine::new("t", true, Box::new(ClipAdapter(clip)));
    assert!(e.paste_from_clipboard());
    assert_eq!(e.text(), "a bt", "title paste flattens to one line");
    assert!(!e.text().contains('\n'));
}

#[test]
fn replace_char() {
    let (mut e, _) = engine("aaaa");
    feed(&mut e, "r b");
    assert_eq!(e.text(), "baaa");
    feed(&mut e, "2 r c");
    assert_eq!(e.text(), "ccaa");
}

#[test]
fn case_toggle_motion() {
    let (mut e, _) = engine("abcd");
    feed(&mut e, "~");
    assert_eq!(e.text(), "Abcd");
    feed(&mut e, "3 ~");
    assert_eq!(e.text(), "ABCD");
}

#[test]
fn registers_carry_line_structure() {
    let (mut e, _) = engine("a\nb\nc");
    feed(&mut e, "y y j p");
    assert_eq!(e.text(), "a\nb\na\nc");
}

#[test]
fn cursor_desired_column_survives_vertical_moves() {
    let (mut e, _) = engine("longer\nab");
    feed(&mut e, "j $"); // (1,1)
    feed(&mut e, "k");
    assert_eq!(at(&e), (0, 1), "desired column carries");
    feed(&mut e, "$");
    assert_eq!(at(&e), (0, 5));
}

#[test]
fn zero_is_motion_not_count() {
    let (mut e, _) = engine("abcdef");
    feed(&mut e, "$ 0");
    assert_eq!(at(&e), (0, 0));
}

#[test]
fn mode_names_render() {
    let (mut e, _) = engine("x");
    assert_eq!(e.mode_name(), "NORMAL");
    feed(&mut e, "i");
    assert_eq!(e.mode_name(), "INSERT");
    feed(&mut e, "<esc> v");
    assert_eq!(e.mode_name(), "VISUAL");
    feed(&mut e, "<esc> V");
    assert_eq!(e.mode_name(), "VISUAL LINE");
}

#[test]
fn internal_register_struct_shape() {
    let r = Register::linewise("x");
    assert!(r.linewise && r.text == "x\n");
    let r = Register::charwise("x");
    assert!(!r.linewise && r.text == "x");
}

// ---- spec-verification additions (SPEC §4.2, §4.5) -------------------------------

#[test]
fn single_quote_text_objects() {
    // i' a' are in the §4.2 text-object set.
    let (mut e, _) = engine("it's fine' end");
    feed(&mut e, "f ' d a '");
    assert_eq!(e.text(), "it end");

    let (mut e, _) = engine("it's fine' end");
    feed(&mut e, "f ' d i '");
    assert_eq!(e.text(), "it'' end", "both quotes survive an inner delete");
}

#[test]
fn backtick_text_objects() {
    let (mut e, _) = engine("cmd `ls -la` done");
    feed(&mut e, "f ` d i `");
    assert_eq!(e.text(), "cmd `` done");
}

#[test]
fn undo_does_not_restore_registers() {
    // SPEC §4.5 rule 6: the clipboard is never restored by undo.
    let (mut e, _) = engine("abc def");
    feed(&mut e, "d w u P");
    assert_eq!(
        e.text(),
        "abc abc def",
        "undo restored text; edit buffer kept the deletion"
    );
}

#[test]
fn yank_then_paste_survives_intervening_undo() {
    // x populates the edit buffer, yy overwrites it with the yank, and the
    // undo of x must not touch the register (SPEC §4.5 rule 6).
    let (mut e, _) = engine("abc\ndef");
    feed(&mut e, "x y y u p");
    assert_eq!(
        e.text(),
        "abc\nbc\ndef",
        "yanked register survived the undo"
    );
}

// ---- insert-mode arrow keys (editor-grade insert) ---------------------------

#[test]
fn insert_arrows_move_within_the_line() {
    let (mut e, _c) = engine("hello");
    feed(&mut e, "i <left> <left>"); // cursor (0,0), still insert
    assert_eq!(e.cursor(), (0, 0));
    feed(&mut e, "<right> <right>");
    assert_eq!(e.cursor(), (0, 2), "right moves within the line");
    feed(&mut e, "X"); // inserts before 'l' of "llo"
    feed(&mut e, "<esc>");
    assert_eq!(e.text(), "heXllo");
    assert_eq!(e.cursor(), (0, 2), "esc backs onto the inserted char (vim)");
}

#[test]
fn insert_right_stops_at_end_of_line_and_can_insert_there() {
    let (mut e, _c) = engine("ab");
    feed(&mut e, "i <right> <right> <right> <right>");
    assert_eq!(e.cursor(), (0, 2), "right clamps at EOL (insert position)");
    feed(&mut e, "c <esc>");
    assert_eq!(e.text(), "abc");
}

#[test]
fn insert_up_down_move_lines_with_desired_column() {
    let (mut e, _c) = engine("alpha\nbeta\ngamma");
    // start insert at end of "alpha" (col 5), then move down twice.
    feed(&mut e, "A <down> <down>");
    assert_eq!(
        e.cursor(),
        (2, 5),
        "desired column kept, clamped to 'gamma'"
    );
    feed(&mut e, "! <esc>");
    assert_eq!(
        e.text(),
        "alpha\nbeta\ngamma!",
        "typed at the clamped column"
    );
    // up/down from mid-text keep the desired column too.
    feed(&mut e, "gg"); // (0,0) normal
    feed(&mut e, "i <down> <down>");
    assert_eq!(e.cursor(), (2, 0));
    feed(&mut e, "<up> <esc>");
    assert_eq!(e.cursor(), (1, 0));
}

// ---- sweep round: vim fidelity fixes (each verified against real vim) -------

#[test]
fn w_crosses_newline_onto_first_char() {
    let (mut e, _) = engine("foo\nbar");
    feed(&mut e, "w");
    assert_eq!(
        e.cursor(),
        (1, 0),
        "w lands on the first char of the next line"
    );
    let (mut e, _) = engine("one two\nthree four");
    feed(&mut e, "2 w");
    assert_eq!(e.cursor(), (1, 0), "2w crosses onto 'three'");
}

#[test]
fn insert_backspace_join_puts_cursor_on_join_point() {
    let (mut e, _) = engine("ab\ncd");
    feed(&mut e, "j i <bs> X <esc>");
    assert_eq!(e.text(), "abXcd");
    assert_eq!(e.cursor(), (0, 2), "cursor on the typed char at the join");
}

#[test]
fn s_deletes_through_end_of_line() {
    let (mut e, _) = engine("ab");
    feed(&mut e, "l s z <esc>");
    assert_eq!(e.text(), "az", "s always deletes the char under the cursor");
    let (mut e, _) = engine("abc");
    feed(&mut e, "l 2 s Z <esc>");
    assert_eq!(e.text(), "aZ", "2s deletes both remaining chars");
}

#[test]
fn insert_ctrl_w_deletes_preexisting_word_back() {
    let (mut e, _) = engine("hello world");
    feed(&mut e, "A <c-w> <esc>");
    assert_eq!(
        e.text(),
        "hello ",
        "ctrl-w deletes the word before the cursor"
    );
}

#[test]
fn d_capital_edge_cases() {
    // D on an empty line is a no-op.
    let (mut e, _) = engine("aa\n\ncc");
    feed(&mut e, "j D");
    assert_eq!(e.text(), "aa\n\ncc", "D on an empty line does nothing");
    // D at col 0 empties the line, keeps it.
    let (mut e, _) = engine("aa\nbb");
    feed(&mut e, "D");
    assert_eq!(
        e.text(),
        "\nbb",
        "D at col 0 leaves the line empty, not removed"
    );
    // 2D spans lines: the prefix joins the line after.
    let (mut e, _) = engine("aa\nbb\ncc");
    feed(&mut e, "2 D");
    assert_eq!(e.text(), "cc");
}

#[test]
fn dj_on_last_line_aborts() {
    let (mut e, _) = engine("a\nb");
    feed(&mut e, "j d j");
    assert_eq!(e.text(), "a\nb", "dj at the boundary deletes nothing");
    let (mut e, _) = engine("a\nb");
    feed(&mut e, "d k");
    assert_eq!(e.text(), "a\nb", "dk at the top deletes nothing");
}

#[test]
fn find_repeat_works_as_operator_target() {
    let (mut e, _) = engine("axbxc");
    feed(&mut e, "d f x d ;");
    assert_eq!(
        e.text(),
        "c",
        "dfx deletes 'ax', d; deletes through the next x"
    );
}

#[test]
fn join_puts_cursor_on_the_space() {
    let (mut e, _) = engine("foo\nbar");
    feed(&mut e, "J");
    assert_eq!(e.text(), "foo bar");
    assert_eq!(e.cursor(), (0, 3), "cursor rests on the inserted space");
}

#[test]
fn tilde_clamps_to_end_of_line() {
    let (mut e, _) = engine("ab");
    feed(&mut e, "1 0 ~");
    assert_eq!(e.text(), "AB", "~ clamps to EOL instead of erroring");
}

#[test]
fn insert_ctrl_u_never_crosses_the_origin_line() {
    let (mut e, _) = engine("abcd\nefgh");
    feed(&mut e, "j i x <up> <c-u> <esc>");
    assert_eq!(
        e.text(),
        "abcd\nxefgh",
        "ctrl-u on another line than the origin is a no-op"
    );
}

#[test]
fn zero_is_a_motion_then_more_commands_follow() {
    let (mut e, _) = engine("hello\nworld");
    feed(&mut e, "$ 0 j");
    assert_eq!(e.cursor(), (1, 0), "0 executes, then j runs");
}

#[test]
fn linewise_delete_cursor_on_first_non_blank() {
    let (mut e, _) = engine("a\n  b");
    feed(&mut e, "d d");
    assert_eq!(e.cursor(), (0, 2), "dd lands on the 'b', not col 0");
}

#[test]
fn count_u_undoes_count_changes() {
    let (mut e, _) = engine("abcd");
    feed(&mut e, "x x x x");
    feed(&mut e, "3 u");
    assert_eq!(e.text(), "bcd", "3u undoes three of the four deletions");
}

#[test]
fn quote_object_works_from_closing_delimiter() {
    let (mut e, _) = engine("\"hi\"");
    feed(&mut e, "l l d i \"");
    assert_eq!(
        e.text(),
        "\"\"",
        "di\" from the closing quote deletes the inside"
    );
    let (mut e, _) = engine("(hi)");
    feed(&mut e, "l l d i (");
    assert_eq!(
        e.text(),
        "()",
        "di( from the closing paren deletes the inside"
    );
}

#[test]
fn repeat_explicit_count_replaces_recorded_count() {
    let (mut e, _) = engine("aaaaaaa");
    feed(&mut e, "3 x");
    assert_eq!(e.text(), "aaaa");
    feed(&mut e, "2 .");
    assert_eq!(e.text(), "aa", "2. replaces the original 3");
    feed(&mut e, ".");
    assert_eq!(e.text(), "", "bare . reuses the original 3");
}

#[test]
fn repeat_paste_reuses_recorded_text() {
    let (mut e, _c) = engine("foo bar baz");
    feed(&mut e, "y i w w w p"); // yank "foo", paste after "baz"
    assert_eq!(e.text(), "foo bar bfooaz");
    feed(&mut e, "y i w"); // register now holds "bfooaz"
    feed(&mut e, "$ .");
    assert_eq!(
        e.text(),
        "foo bar bfooazfoo",
        ". re-pastes the recorded 'foo', not the new register"
    );
}
