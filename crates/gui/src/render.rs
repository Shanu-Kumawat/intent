//! Pure caret/selection computation for editor rendering — split out of the
//! GPUI shell so caret behavior is unit-testable without a display.
//!
//! Rendering model (fixes column drift): each line is shaped as ONE text run
//! and inverse plates (selection, block caret) are per-run highlight ranges.
//! Splitting a line into separate text elements used to cost ~1px of column
//! drift after every split point, visible as uneven spacing in the cursor's
//! column across a multi-line buffer.

use intent_vim::{Engine, Mode};

/// What the shell should paint for one buffer line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRender {
    /// Full line text to shape as a single run. Includes one virtual space
    /// when NORMAL's block caret sits on an empty line.
    pub text: String,
    /// Inverse ranges (FG plate, CHROME text) as byte offsets into `text`:
    /// the visual selection plus the block caret, merged and ordered.
    pub inverse: Vec<(usize, usize)>,
    /// Insert-mode caret: byte offset to draw the thin bar before
    /// (`text.len()` = end of line). `None` outside insert mode.
    pub insert_bar: Option<usize>,
}

pub fn line_render(engine: &Engine, line_ix: usize, focused: bool) -> LineRender {
    let chars: Vec<char> = engine.buf.lines[line_ix].clone();
    let (cur_l, cur_c) = engine.cursor();
    let on_line = focused && cur_l == line_ix;
    let insert_caret = on_line && engine.mode == Mode::Insert;
    let block_caret = on_line && engine.mode != Mode::Insert;

    // Char index → byte offset table (plus the final text length).
    let mut byte_of: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    let mut off = 0usize;
    for c in &chars {
        byte_of.push(off);
        off += c.len_utf8();
    }
    byte_of.push(off);

    let text: String = chars.iter().collect();

    if chars.is_empty() {
        if block_caret {
            // NORMAL on an empty line: block on a virtual space.
            return LineRender {
                text: " ".into(),
                inverse: vec![(0, 1)],
                insert_bar: None,
            };
        }
        return LineRender {
            text,
            inverse: vec![],
            insert_bar: insert_caret.then_some(0),
        };
    }

    let mut inverse: Vec<(usize, usize)> = Vec::new();

    // Block caret covers the char under the cursor.
    if block_caret {
        let c = cur_c.min(chars.len() - 1);
        inverse.push((byte_of[c], byte_of[c] + chars[c].len_utf8()));
    }

    // Selection as a byte range on this line.
    if let Some((a, b, lw)) = engine.selection() {
        if a.0 <= line_ix && b.0 >= line_ix {
            let s_char = if lw || a.0 < line_ix {
                0
            } else {
                a.1.min(chars.len())
            };
            let e_char = if lw || b.0 > line_ix {
                chars.len()
            } else {
                b.1.min(chars.len())
            };
            if s_char < e_char {
                inverse.push((byte_of[s_char], byte_of[e_char]));
            }
        }
    }

    inverse.sort_by_key(|(a, b)| (*a, *b));
    // Merge touching/overlapping ranges so the shell gets disjoint plates.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in inverse {
        match merged.last_mut() {
            Some((_, eb)) if a <= *eb => *eb = (*eb).max(b),
            _ => merged.push((a, b)),
        }
    }

    LineRender {
        text,
        inverse: merged,
        insert_bar: insert_caret.then(|| byte_of[cur_c.min(chars.len())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use intent_vim::{Key, MemClipboard};

    fn engine(text: &str, single_line: bool) -> Engine {
        Engine::new(text, single_line, Box::new(MemClipboard::new()))
    }

    fn desc(lr: &LineRender) -> String {
        // Render the line with `[]` around inverse ranges and `|` at the bar.
        let mut out = String::new();
        let mut byte = 0usize;
        for (a, b) in &lr.inverse {
            out.push_str(&lr.text[byte..*a]);
            out.push('[');
            out.push_str(&lr.text[*a..*b]);
            out.push(']');
            byte = *b;
        }
        out.push_str(&lr.text[byte..]);
        if let Some(bar) = lr.insert_bar {
            out.insert(bar, '|');
        }
        out
    }

    #[test]
    fn normal_caret_is_a_block_mid_line() {
        let e = engine("hello", true);
        assert_eq!(desc(&line_render(&e, 0, true)), "[h]ello");
    }

    #[test]
    fn normal_caret_survives_without_selection_anywhere() {
        let mut e = engine("hello", true);
        e.handle(Key::new("l"));
        e.handle(Key::new("l")); // cursor (0,2)
        assert_eq!(e.cursor(), (0, 2));
        assert_eq!(desc(&line_render(&e, 0, true)), "he[l]lo");
    }

    #[test]
    fn insert_caret_is_a_bar_before_the_char() {
        let mut e = engine("hello", true);
        e.handle(Key::new("i"));
        e.handle(Key::new("x"));
        e.handle(Key::new("y")); // "xyhello", cursor (0,2)
        assert_eq!(e.cursor(), (0, 2));
        let lr = line_render(&e, 0, true);
        assert_eq!(
            desc(&lr),
            "xy|hello",
            "bar sits before the char at the cursor"
        );
        assert_eq!(lr.inverse, vec![], "insert mode has no inverse ranges");
    }

    #[test]
    fn esc_from_insert_returns_to_a_visible_block() {
        let mut e = engine("hello", true);
        e.handle(Key::new("A"));
        e.handle(Key::new("!"));
        e.handle(Key::new("esc"));
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.cursor(), (0, 5));
        assert_eq!(desc(&line_render(&e, 0, true)), "hello[!]");
    }

    #[test]
    fn insert_bar_at_end_of_line_and_on_empty_lines() {
        let mut e = engine("", true);
        e.handle(Key::new("i"));
        assert_eq!(desc(&line_render(&e, 0, true)), "|");

        let mut e = engine("abc", true);
        e.handle(Key::new("A")); // insert at EOL
        assert_eq!(desc(&line_render(&e, 0, true)), "abc|");
    }

    #[test]
    fn normal_caret_on_empty_line_is_a_virtual_block() {
        let e = engine("", true);
        let lr = line_render(&e, 0, true);
        assert_eq!(lr.text, " ");
        assert_eq!(desc(&lr), "[ ]");
    }

    #[test]
    fn unfocused_field_has_no_caret() {
        let e = engine("hello", true);
        let lr = line_render(&e, 0, false);
        assert_eq!(desc(&lr), "hello");
        assert_eq!(lr.insert_bar, None);
    }

    #[test]
    fn visual_selection_rendered_in_inverse() {
        let mut e = engine("hello", true);
        e.handle(Key::shifted("v"));
        e.handle(Key::new("l"));
        assert_eq!(desc(&line_render(&e, 0, true)), "[hello]");
    }

    #[test]
    fn bar_offset_is_a_byte_offset() {
        // A multi-byte char before the caret must not corrupt the offset.
        let mut e = engine("héllo", true);
        e.handle(Key::new("l"));
        e.handle(Key::new("i")); // insert before 'é'
        let lr = line_render(&e, 0, true);
        assert_eq!(lr.insert_bar, Some(1), "byte offset after 'h'");
        assert_eq!(&lr.text[..1], "h");
    }
}
