//! The `intent completions` command (SPEC §3).

use std::process::Command;

fn gen(shell: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_intent"))
        .args(["completions", shell])
        .env("INTENT_DB", "/dev/null") // never touches the database
        .output()
        .expect("run intent")
}

#[test]
fn completions_for_supported_shells() {
    for (shell, marker) in [
        ("bash", "_intent()"),
        ("zsh", "#compdef intent"),
        ("fish", "intent"),
    ] {
        let out = gen(shell);
        assert!(out.status.success(), "completions {shell} failed");
        let script = String::from_utf8_lossy(&out.stdout);
        assert!(script.contains(marker), "{shell} script missing {marker:?}");
        assert!(
            script.contains("add") && script.contains("list"),
            "{shell} covers subcommands"
        );
    }
}

#[test]
fn completions_reject_unknown_shell() {
    let out = gen("notashell");
    assert_eq!(out.status.code(), Some(2), "usage error");
}

#[test]
fn completions_compose_with_global_flags() {
    // Regression: `intent --json completions bash` used to panic.
    let cases: [&[&str]; 2] = [&["--json", "completions", "bash"], &["completions", "zsh"]];
    for args in cases {
        let out = Command::new(env!("CARGO_BIN_EXE_intent"))
            .args(args)
            .env("INTENT_DB", "/dev/null")
            .output()
            .expect("run intent");
        assert!(out.status.success(), "completions with {args:?} failed");
        assert!(!out.stdout.is_empty());
    }
}
