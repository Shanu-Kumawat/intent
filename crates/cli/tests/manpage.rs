//! Man-page drift guard: every CLI subcommand must be documented in the
//! COMMANDS section of docs/intent.1.

use std::path::PathBuf;

#[test]
fn man_page_covers_every_subcommand() {
    let man = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/intent.1");
    let Ok(roff) = std::fs::read_to_string(&man) else {
        // Standalone checkout without docs/ — not this test's concern.
        return;
    };
    let commands = roff.split(".SH COMMANDS").nth(1).unwrap_or("");
    assert!(
        !commands.is_empty(),
        "docs/intent.1 has no COMMANDS section"
    );
    for cmd in [
        "add",
        "list",
        "show",
        "edit",
        "done",
        "reopen",
        "defer",
        "prioritize",
        "archive",
        "unarchive",
        "organize",
        "delete",
        "search",
        "counts",
        "completions",
    ] {
        assert!(
            commands.contains(&format!("intent {}", cmd)),
            "docs/intent.1 COMMANDS section does not document {cmd:?}"
        );
    }
}
