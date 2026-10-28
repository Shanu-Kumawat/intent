//! Pure caret/selection segment computation for editor rendering — split out
//! of the GPUI shell so caret visibility is unit-testable without a display.

use intent_vim::{Engine, Mode};

/// One styled run of an editor line. `Text(txt, inverse)` renders `txt` on
/// the inverse plate when `inverse`; `CaretBar` is the thin insert-mode caret
/// (a narrow vertical line the shell draws between characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditSeg {
    Text(String, bool),
    CaretBar,
}

/// Styled segments for one buffer line. The caret always breaks the text
/// runs so it stays visible: a block (inverse character) in NORMAL/VISUAL, a
/// thin vertical bar in INSERT — mid-line, at end-of-line, and on empty
/// lines alike.
pub fn line_segments(engine: &Engine, line_ix: usize, focused: bool) -> Vec<EditSeg> {
    let chars: Vec<char> = engine.buf.lines[line_ix].clone();
    let (cur_l, cur_c) = engine.cursor();
    let on_line = focused && cur_l == line_ix;
    let insert_caret = on_line && engine.mode == Mode::Insert;
    let block_caret = on_line && engine.mode != Mode::Insert;

    // Selection as a [start,end) char range on this line.
    let (s_start, s_end) = match engine.selection() {
        Some((a, b, lw)) if a.0 <= line_ix && b.0 >= line_ix => {
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
            (s, e)
        }
        _ => (usize::MAX, 0),
    };

    let mut segs: Vec<EditSeg> = Vec::new();
    let mut ix = 0usize;
    let mut bar_done = false;
    while ix < chars.len() {
        // The insert caret is a bar drawn *before* the character at the
        // cursor — the character itself must still render.
        if insert_caret && !bar_done && cur_c == ix {
            segs.push(EditSeg::CaretBar);
            bar_done = true;
        }
        let in_sel = s_start <= ix && ix < s_end;
        let inverse = in_sel || (block_caret && cur_c == ix);
        // Run until the next visual break: selection boundary or caret. The
        // caret must bound the run even when there is no selection, or a
        // plain run swallows it (the mid-line caret was invisible this way).
        // Inside a selection no caret break is needed — the run is inverse
        // either way.
        let mut end = if in_sel { s_end } else { s_start };
        if !in_sel {
            if block_caret && cur_c == ix {
                end = ix + 1;
            } else if cur_c > ix {
                end = end.min(cur_c);
            }
        }
        let len = end.min(chars.len()).saturating_sub(ix).max(1);
        segs.push(EditSeg::Text(chars[ix..ix + len].iter().collect(), inverse));
        ix += len;
    }
    if insert_caret && !bar_done {
        segs.push(EditSeg::CaretBar);
    } else if block_caret && cur_c >= chars.len() {
        // NORMAL on an empty line: block on a virtual space.
        segs.push(EditSeg::Text(" ".into(), true));
    } else if segs.is_empty() {
        segs.push(EditSeg::Text(" ".into(), false));
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use intent_vim::{Key, MemClipboard};

    fn engine(text: &str, single_line: bool) -> Engine {
        Engine::new(text, single_line, Box::new(MemClipboard::new()))
    }

    fn texts(segs: &[EditSeg]) -> Vec<String> {
        segs.iter()
            .map(|s| match s {
                EditSeg::Text(t, inv) => {
                    if *inv {
                        format!("[{t}]")
                    } else {
                        t.clone()
                    }
                }
                EditSeg::CaretBar => "|".to_string(),
            })
            .collect()
    }

    #[test]
    fn normal_caret_is_a_block_mid_line() {
        let e = engine("hello", true);
        // cursor (0,0) in normal mode: block on 'h', then the rest.
        assert_eq!(texts(&line_segments(&e, 0, true)), ["[h]", "ello"]);
    }

    #[test]
    fn normal_caret_survives_without_selection_anywhere() {
        // Regression: with no selection the plain run used to span the whole
        // line and swallow the caret character.
        let mut e = engine("hello", true);
        e.handle(Key::new("l"));
        e.handle(Key::new("l")); // cursor to (0,2)
        assert_eq!(e.cursor(), (0, 2));
        assert_eq!(texts(&line_segments(&e, 0, true)), ["he", "[l]", "lo"]);
    }

    #[test]
    fn insert_caret_is_a_thin_bar_mid_line() {
        let mut e = engine("hello", true);
        e.handle(Key::new("i"));
        e.handle(Key::new("x"));
        e.handle(Key::new("y")); // "xyhello", cursor (0,2)
        assert_eq!(e.cursor(), (0, 2));
        // The bar sits *before* the character at the cursor; that character
        // still renders (a regression once swallowed it).
        assert_eq!(texts(&line_segments(&e, 0, true)), ["xy", "|", "hello"]);
    }

    #[test]
    fn insert_bar_never_hides_the_char_under_it() {
        let mut e = engine("hello", true);
        e.handle(Key::new("i")); // cursor (0,0), insert
        assert_eq!(texts(&line_segments(&e, 0, true)), ["|", "hello"]);
        let mut e = engine("abc", true);
        e.handle(Key::new("l"));
        e.handle(Key::new("i")); // cursor (0,1), insert before 'b'
        assert_eq!(texts(&line_segments(&e, 0, true)), ["a", "|", "bc"]);
    }

    #[test]
    fn esc_from_insert_returns_to_a_visible_block() {
        let mut e = engine("hello", true);
        e.handle(Key::new("A"));
        e.handle(Key::new("!")); // "hello!", cursor (0,6) in insert
        e.handle(Key::new("esc")); // clamped to (0,5)
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.cursor(), (0, 5));
        assert_eq!(texts(&line_segments(&e, 0, true)), ["hello", "[!]"]);
    }

    #[test]
    fn insert_caret_at_end_of_line_and_on_empty_lines() {
        let mut e = engine("", true);
        e.handle(Key::new("i"));
        assert_eq!(texts(&line_segments(&e, 0, true)), ["|"]);

        let mut e = engine("abc", true);
        e.handle(Key::new("A")); // insert at EOL
        assert_eq!(texts(&line_segments(&e, 0, true)), ["abc", "|"]);
    }

    #[test]
    fn normal_caret_on_empty_line_is_a_virtual_block() {
        let e = engine("", true);
        assert_eq!(texts(&line_segments(&e, 0, true)), ["[ ]"]);
    }

    #[test]
    fn unfocused_field_has_no_caret() {
        let e = engine("hello", true);
        assert_eq!(texts(&line_segments(&e, 0, false)), ["hello"]);
    }

    #[test]
    fn visual_selection_rendered_in_inverse() {
        let mut e = engine("hello", true);
        e.handle(Key::shifted("v"));
        e.handle(Key::new("l"));
        // linewise selection covers the whole line.
        assert_eq!(texts(&line_segments(&e, 0, true)), ["[hello]"]);
    }
}
