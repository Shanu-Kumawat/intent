//! Regression tests for bugs found in the line-by-line review.

use intent_vim::{Engine, Key, MemClipboard};

fn engine(text: &str) -> Engine {
    Engine::new(text, false, Box::new(MemClipboard::new()))
}

fn feed(e: &mut Engine, s: &str) {
    for t in s.split(' ').filter(|t| !t.is_empty()) {
        for c in t.chars() {
            if c.is_ascii_uppercase() {
                e.handle(Key::shifted(&c.to_ascii_lowercase().to_string()));
            } else {
                e.handle(Key::new(&c.to_string()));
            }
        }
    }
}

#[test]
fn operator_gg_g_honor_counts() {
    // d3G: delete through line 3, keeping line 4.
    let mut e = engine("l1\nl2\nl3\nl4");
    feed(&mut e, "d 3 G");
    assert_eq!(e.text(), "l4");

    // d2gg: delete lines 1–2.
    let mut e = engine("l1\nl2\nl3\nl4");
    feed(&mut e, "d 2 g g");
    assert_eq!(e.text(), "l3\nl4");

    // 2dgg: a count before the operator counts too.
    let mut e = engine("l1\nl2\nl3\nl4");
    feed(&mut e, "2 d g g");
    assert_eq!(e.text(), "l3\nl4");

    // dG (no count) deletes through the last line — everything from the
    // cursor line down (Vim leaves one empty line, so text is "").
    let mut e = engine("l1\nl2\nl3");
    feed(&mut e, "d G");
    assert_eq!(e.text(), "");

    // y2gg yanks lines 1–2 (operator, not just delete).
    let mut e = engine("l1\nl2\nl3");
    feed(&mut e, "y 2 g g");
    assert_eq!(e.text(), "l1\nl2\nl3");
}

#[test]
fn absurd_counts_do_not_panic() {
    let mut e = engine("abcdef");
    feed(&mut e, "99999999999999999999 l");
    assert_eq!(e.cursor(), (0, 5), "count saturates; cursor clamps to EOL");

    let mut e = engine("one\ntwo");
    feed(&mut e, "99999999999999999999 d d");
    assert_eq!(e.text(), "");

    let mut e = engine("one\ntwo");
    feed(&mut e, "99999 j");
    assert_eq!(e.cursor(), (1, 0), "movement clamps at the last line");
}
