//! P3-15 regression: iW/aW respect the big-word class.

use intent_vim::{Engine, Key, MemClipboard};

fn go(text: &str, keys: &str) -> String {
    let mut e = Engine::new(text, false, Box::new(MemClipboard::new()));
    for t in keys.split(' ').filter(|t| !t.is_empty()) {
        for c in t.chars() {
            e.handle(Key::new(&c.to_string()));
        }
    }
    e.text()
}

#[test]
fn big_word_text_objects() {
    // aW: "foo.bar" is ONE big word; trailing space goes with it.
    assert_eq!(go("foo.bar baz", "d a W"), "baz");
    assert_eq!(
        go("foo.bar baz", "l d a W"),
        "baz",
        "mid-word cursor still takes the whole big word"
    );
    // iW: the word only, surrounding whitespace survives.
    assert_eq!(go("foo.bar baz", "d i W"), " baz");
    // Contrast with small words: "foo" and "." are separate objects.
    assert_eq!(go("foo.bar", "d i w"), ".bar");
}
