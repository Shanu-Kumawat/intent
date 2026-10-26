//! Regression tests for bugs found in the line-by-line review.

use std::process::Command;

struct Env {
    db: std::path::PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let db = std::env::temp_dir().join(format!(
            "intent-review-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&db);
        Env { db }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_intent"))
            .args(args)
            .env("INTENT_DB", &self.db)
            .output()
            .expect("failed to run intent binary")
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.run(args);
        assert!(out.status.success(), "expected success for {args:?}");
        serde_json::from_slice(&out.stdout).expect("valid JSON")
    }

    fn add(&self, title: &str) -> String {
        self.json(&["add", title, "--json"])["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[test]
fn edit_rejects_empty_and_multiline_titles() {
    let e = Env::new("titlecheck");
    let id = e.add("original");

    let out = e.run(&["edit", &id, "--title", "   ", "--json"]);
    assert_eq!(out.status.code(), Some(4));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["error"]["code"], "bad_value");

    let out = e.run(&["edit", &id, "--title", "a\nb", "--json"]);
    assert_eq!(out.status.code(), Some(4));

    // The task is untouched; surrounding whitespace is trimmed, not rejected.
    let t = e.json(&["show", &id, "--json"]);
    assert_eq!(t["title"], "original");
    let t = e.json(&["edit", &id, "--title", "  padded  ", "--json"]);
    assert_eq!(t["title"], "padded");
}

#[test]
fn extreme_relative_dates_do_not_panic() {
    let e = Env::new("satdates");
    let out = e.run(&["add", "far future", "--due", "+999999y", "--json"]);
    assert_eq!(out.status.code(), Some(0), "saturates instead of panicking");
    let t: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(t["due"], "9999-12-31");

    let out = e.run(&["add", "span overflow weeks", "--due", "+1152921504606846976w", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let t: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(t["due"], "9999-12-31");
}
