use intent_core::model::{Task, TaskView};

use crate::Cli;

/// Emit the result of a single-task mutation.
pub fn one_task(cli: &Cli, task: &Task, verb: &str) {
    if cli.json {
        println!("{}", serde_json::to_string(task).unwrap());
        return;
    }
    println!("{verb} {} {}", task.id, task.title);
}

pub fn deleted(cli: &Cli, id: &str) {
    if cli.json {
        println!(
            "{}",
            serde_json::json!({"ok": true, "deleted": true, "id": id})
        );
        return;
    }
    println!("deleted {id}");
}

pub fn task_list(cli: &Cli, view: TaskView, tasks: &[Task]) {
    if cli.json {
        println!("{}", serde_json::to_string(tasks).unwrap());
        return;
    }
    if tasks.is_empty() {
        println!("no tasks in {view}");
        return;
    }
    for t in tasks {
        println!("{}", row(t));
    }
}

pub fn show_task(cli: &Cli, task: &Task) {
    if cli.json {
        println!("{}", serde_json::to_string(task).unwrap());
        return;
    }
    println!("id         {}", task.id);
    println!("title      {}", task.title);
    if !task.notes.is_empty() {
        for (i, line) in task.notes.lines().enumerate() {
            if i == 0 {
                println!("notes      {line}");
            } else {
                println!("           {line}");
            }
        }
    }
    println!("status     {}", task.status);
    println!("inbox      {}", if task.inbox { "yes" } else { "no" });
    println!("priority   {}", task.priority);
    if let Some(p) = &task.project {
        println!("project    {p}");
    }
    if !task.tags.is_empty() {
        println!("tags       {}", task.tags.join(", "));
    }
    for (label, d) in [
        ("due", task.due),
        ("remind", task.remind),
        ("defer", task.defer_until),
    ] {
        if let Some(d) = d {
            println!("{label:<10} {d}");
        }
    }
    println!("created    {}", task.created_at);
    println!("updated    {}", task.updated_at);
    if let Some(c) = task.completed_at {
        println!("completed  {c}");
    }
}

pub fn counts(cli: &Cli, counts: &[(TaskView, usize)]) {
    if cli.json {
        let map = serde_json::Map::from_iter(
            counts
                .iter()
                .map(|(v, n)| (v.as_str().to_string(), serde_json::json!(n))),
        );
        println!("{}", serde_json::Value::Object(map));
        return;
    }
    let line: Vec<String> = counts
        .iter()
        .map(|(v, n)| format!("{} {}", v.as_str(), n))
        .collect();
    println!("{}", line.join("  "));
}

fn row(t: &Task) -> String {
    let pri = match t.priority {
        intent_core::Priority::High => "!",
        intent_core::Priority::Medium => "+",
        intent_core::Priority::Low => "-",
        _ => " ",
    };
    let status = match t.status {
        intent_core::Status::Done => " [done]",
        intent_core::Status::Archived => " [archived]",
        _ => "",
    };
    let mut meta = Vec::new();
    if t.inbox {
        meta.push("inbox".into());
    }
    if let Some(d) = t.due {
        meta.push(format!("due {d}"));
    }
    if let Some(r) = t.remind {
        meta.push(format!("remind {r}"));
    }
    if let Some(d) = t.defer_until {
        meta.push(format!("defer {d}"));
    }
    if let Some(p) = &t.project {
        meta.push(format!("·{p}"));
    }
    for tag in &t.tags {
        meta.push(format!("#{tag}"));
    }
    let meta = if meta.is_empty() {
        String::new()
    } else {
        format!("  ({})", meta.join(" "))
    };
    format!("{} {} {}{}{}", t.id, pri, t.title, status, meta)
}
