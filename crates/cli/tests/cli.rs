//! End-to-end CLI tests: run the real `intent` binary against a throwaway
//! database (via INTENT_DB). These cover the capture → organize → focus →
//! complete → retrieve workflow from SPEC §3.

use std::collections::HashMap;
use std::process::{Command, Output};

struct Env {
    db: std::path::PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let db = std::env::temp_dir().join(format!(
            "intent-cli-test-{name}-{}-{}",
            std::process::id(),
            nanos()
        ));
        let _ = std::fs::remove_file(&db);
        Env { db }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_intent"))
            .args(args)
            .env("INTENT_DB", &self.db)
            .output()
            .expect("failed to run intent binary")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "expected success for {args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn fails(&self, args: &[&str]) -> (i32, String) {
        let out = self.run(args);
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let stdout = self.ok(args);
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("invalid JSON from {args:?}: {e}\n{stdout}"))
    }

    fn add(&self, title: &str) -> String {
        let out = self.json(&["add", title, "--json"]);
        out["id"].as_str().unwrap().to_string()
    }
}

fn nanos() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        + SEQ.fetch_add(1, Ordering::Relaxed) as u128
}

#[test]
fn capture_organize_focus_complete_retrieve() {
    let e = Env::new("workflow");

    // Capture: zero metadata required.
    let id = e.add("Renew passport");
    assert_eq!(id.len(), 6);

    // It lands in the inbox.
    let inbox = e.json(&["list", "inbox", "--json"]);
    assert_eq!(inbox.as_array().unwrap().len(), 1);
    assert_eq!(inbox[0]["title"], "Renew passport");
    assert_eq!(inbox[0]["inbox"], true);
    assert_eq!(inbox[0]["status"], "active");

    // Organize with a due date; leaves the inbox, enters Today.
    e.ok(&["edit", &id, "--due", "today", "--project", "admin"]);
    let t = e.json(&["show", &id, "--json"]);
    assert_eq!(t["inbox"], false);
    assert_eq!(t["project"], "admin");
    assert!(t["due"].is_string());

    let today = e.json(&["list", "today", "--json"]);
    assert_eq!(today.as_array().unwrap().len(), 1);

    // Defer it away: Today hides it until the defer date.
    e.ok(&["defer", &id, "+30d"]);
    let today = e.json(&["list", "today", "--json"]);
    assert_eq!(today.as_array().unwrap().len(), 0, "deferred task leaves Today");
    let upcoming = e.json(&["list", "upcoming", "--json"]);
    assert_eq!(upcoming.as_array().unwrap().len(), 1);

    // Bring it back by clearing the defer.
    e.ok(&["defer", &id, "none"]);
    assert_eq!(e.json(&["list", "today", "--json"]).as_array().unwrap().len(), 1);

    // Reminders are distinct from deadlines (both surface in Today).
    let rid = e.add("re-read paper");
    e.ok(&["edit", &rid, "--remind", "tomorrow"]);
    assert_eq!(
        e.json(&["list", "today", "--json"]).as_array().unwrap().len(),
        1,
        "tomorrow's reminder is not today yet"
    );

    // Complete, then retrieve from Completed; reopen restores it.
    e.ok(&["done", &id]);
    assert_eq!(e.json(&["list", "today", "--json"]).as_array().unwrap().len(), 0);
    let done = e.json(&["list", "completed", "--json"]);
    assert_eq!(done.as_array().unwrap().len(), 1);
    assert!(done[0]["completed_at"].is_string());

    e.ok(&["reopen", &id]);
    assert_eq!(e.json(&["list", "completed", "--json"]).as_array().unwrap().len(), 0);

    // Search retrieves by content.
    let sid = e.add("buy ferrite rings");
    e.ok(&["edit", &sid, "--notes", "mcmaster part 123"]);
    let hits = e.json(&["search", "ferrite", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    let hits = e.json(&["search", "FERRITE rings", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(e.json(&["search", "zzz-nope", "--json"]).as_array().unwrap().len(), 0);
}

#[test]
fn archive_flow() {
    let e = Env::new("archive");
    let id = e.add("old thing");
    e.ok(&["archive", &id]);
    assert_eq!(e.json(&["list", "archived", "--json"]).as_array().unwrap().len(), 1);
    assert_eq!(e.json(&["list", "all", "--json"]).as_array().unwrap().len(), 0);
    e.ok(&["unarchive", &id]);
    assert_eq!(e.json(&["list", "archived", "--json"]).as_array().unwrap().len(), 0);
    assert_eq!(e.json(&["list", "inbox", "--json"]).as_array().unwrap().len(), 1);
}

#[test]
fn exit_codes_and_error_json() {
    let e = Env::new("errors");

    let (code, err) = e.fails(&["show", "zzzzzz", "--json"]);
    assert_eq!(code, 3);
    let v: serde_json::Value = serde_json::from_str(err.trim()).expect("error is JSON");
    assert_eq!(v["error"]["code"], "not_found");

    let (code, err) = e.fails(&["add", "x", "--due", "not a date", "--json"]);
    assert_eq!(code, 4);
    let v: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
    assert_eq!(v["error"]["code"], "bad_date");

    let (code, _) = e.fails(&["add", "x", "--priority", "urgent", "--json"]);
    assert_eq!(code, 4);

    let (code, _) = e.fails(&["list", "bogus-view"]);
    assert_eq!(code, 4, "unknown view is a value error");

    let (code, err) = e.fails(&["edit", "zzzzzz", "--title", "y", "--json"]);
    assert_eq!(code, 3);
    assert!(err.contains("not_found"));

    // No mutation flags → usage error, exit 2.
    let (code, _) = e.fails(&["edit", "zzzzzz"]);
    assert_eq!(code, 2);
}

#[test]
fn delete_requires_force_non_interactively() {
    let e = Env::new("delete");
    let id = e.add("doomed");
    // Non-TTY stdin: refuse without --force.
    let (code, err) = e.fails(&["delete", &id]);
    assert_eq!(code, 1);
    assert!(err.contains("--force"));

    e.ok(&["delete", &id, "--force"]);
    let (code, _) = e.fails(&["show", &id]);
    assert_eq!(code, 3);
}

#[test]
fn smart_dates_everywhere() {
    let e = Env::new("dates");
    let id = e.add("dated");
    e.ok(&["edit", &id, "--due", "2026-12-25"]);
    assert_eq!(e.json(&["show", &id, "--json"])["due"], "2026-12-25");

    e.ok(&["edit", &id, "--due", "tomorrow"]);
    let t = e.json(&["show", &id, "--json"]);
    let due = t["due"].as_str().unwrap().to_string();

    // tomorrow parsed relative to the real today.
    let jiff_today = jiff::Zoned::now().date();
    let expected = jiff_today.tomorrow().unwrap().to_string();
    assert_eq!(due, expected);

    e.ok(&["edit", &id, "--due", "none"]);
    assert!(e.json(&["show", &id, "--json"])["due"].is_null());
}

#[test]
fn add_with_metadata_leaves_inbox() {
    let e = Env::new("addmeta");
    e.ok(&["add", "direct to project", "--project", "core"]);
    assert_eq!(e.json(&["list", "inbox", "--json"]).as_array().unwrap().len(), 0);
    assert_eq!(e.json(&["list", "all", "--json"]).as_array().unwrap().len(), 1);

    e.ok(&["add", "plain"]);
    assert_eq!(e.json(&["list", "inbox", "--json"]).as_array().unwrap().len(), 1);

    // --no-inbox forces organized capture.
    e.ok(&["add", "skip inbox", "--no-inbox"]);
    assert_eq!(e.json(&["list", "inbox", "--json"]).as_array().unwrap().len(), 1);
}

#[test]
fn prioritize_and_counts() {
    let e = Env::new("prio");
    let id = e.add("important");
    e.ok(&["prioritize", &id, "high"]);
    assert_eq!(e.json(&["show", &id, "--json"])["priority"], "high");
    e.ok(&["prioritize", &id, "none"]);
    assert_eq!(e.json(&["show", &id, "--json"])["priority"], "none");

    let counts = e.json(&["counts", "--json"]);
    let map = counts.as_object().unwrap();
    assert_eq!(map["inbox"], 1);
    assert_eq!(map["today"], 0);
}

#[test]
fn tags_round_trip() {
    let e = Env::new("tags");
    let id = e.add("tagged");
    e.ok(&["edit", &id, "--tags", "home,chores"]);
    let t = e.json(&["show", &id, "--json"]);
    assert_eq!(t["tags"], serde_json::json!(["home", "chores"]));

    e.ok(&["edit", &id, "--tag-add", "later", "--tag-rm", "chores"]);
    let t = e.json(&["show", &id, "--json"]);
    assert_eq!(t["tags"], serde_json::json!(["home", "later"]));

    let hits = e.json(&["search", "#never", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 0);
    let hits = e.json(&["list", "--tag", "home", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 1);
}

#[test]
fn human_output_is_reasonable() {
    let e = Env::new("human");
    let id = e.add("A visible title");
    let list = e.ok(&["list", "all"]);
    assert!(list.contains("A visible title"), "list shows title: {list}");
    assert!(list.contains(&id), "list shows id: {list}");
    let shown = e.ok(&["show", &id]);
    assert!(shown.contains("title      A visible title"));

    let msg = e.ok(&["done", &id]);
    assert!(msg.contains("done") && msg.contains(&id));
}

#[test]
fn many_tasks_stay_fast() {
    let e = Env::new("bulk");
    for i in 0..300 {
        e.ok(&["add", &format!("task {i:03}")]);
    }
    let all = e.json(&["list", "all", "--json"]);
    assert_eq!(all.as_array().unwrap().len(), 300);
    let hits = e.json(&["search", "task 29", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 13); // 029, 129, 229, 290..299
}

#[test]
fn usage_error_is_json_with_json_flag() {
    let e = Env::new("usagejson");
    let out = e.run(&["edit", "zzzzzz", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(v["error"]["code"], "usage");
}

#[test]
fn version_and_help() {
    let e = Env::new("help");
    let out = e.run(&["--version"]);
    assert!(out.status.success());
    let help = e.ok(&["--help"]);
    for cmd in [
        "add", "list", "show", "edit", "done", "reopen", "defer", "prioritize", "archive",
        "unarchive", "organize", "delete", "search", "counts",
    ] {
        assert!(help.contains(cmd), "help mentions {cmd}");
    }
}

// Silence unused warnings for HashMap import if not needed elsewhere.
#[allow(dead_code)]
fn _typecheck(_: Option<HashMap<String, i32>>) {}
