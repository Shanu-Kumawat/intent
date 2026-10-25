use clap::Parser;

mod output;
mod run;

#[derive(Parser, Debug)]
#[command(
    name = "intent",
    version,
    about = "Intent — keyboard-first, local-first task management",
    long_about = "Intent — keyboard-first, local-first task management.\n\
                  One SQLite database (see INTENT_DB). Dates accept YYYY-MM-DD, today/tod,\n\
                  tomorrow/tom/tmr, yesterday, ±N[d|w|m|y], or a weekday name (mon..sun).\n\
                  Pass --json on any command for machine-readable output; errors then go to\n\
                  stderr as {\"error\":{\"code\":..,\"message\":..}}. Exit codes: 0 ok, 1 runtime,\n\
                  2 usage, 3 task not found, 4 invalid value."
)]
struct Cli {
    /// Emit machine-readable JSON on stdout (errors as JSON on stderr).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Capture a new task (lands in the inbox unless organized by --project/--due/--remind)
    Add {
        /// Task title, single line (or use --title)
        title: Option<String>,
        #[arg(long = "title", help = "Task title (alternative to positional)")]
        title_opt: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, help = "Due date (deadline)")]
        due: Option<String>,
        #[arg(long, help = "Resurface date (soft reminder)")]
        remind: Option<String>,
        #[arg(long, help = "none|low|medium|high (aliases -/l/m/h)")]
        priority: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_delimiter = ',', help = "Comma-separated tags")]
        tags: Option<Vec<String>>,
        /// Create outside the inbox even with no organizing metadata
        #[arg(long)]
        no_inbox: bool,
    },
    /// List tasks in a view: inbox, today, upcoming, completed, archived, all (default)
    List {
        view: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long, help = "Filter by priority")]
        priority: Option<String>,
        #[arg(long, help = "Full-text terms over title/notes/project/tags")]
        query: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show one task in full
    Show { id: String },
    /// Edit task fields; at least one flag required
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, help = "Set due (a date, or none/- to clear)")]
        due: Option<String>,
        #[arg(long, help = "Set remind (a date, or none/- to clear)")]
        remind: Option<String>,
        #[arg(long, help = "Set defer-until (a date, or none/- to clear)")]
        defer_until: Option<String>,
        #[arg(long)]
        priority: Option<String>,
        #[arg(long, help = "Project name, or none/- to clear")]
        project: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        #[arg(long)]
        tag_add: Option<Vec<String>>,
        #[arg(long)]
        tag_rm: Option<Vec<String>>,
    },
    /// Complete a task
    Done { id: String },
    /// Reopen a completed task
    Reopen { id: String },
    /// Defer: hide from Today until DATE (or `none` to clear)
    Defer { id: String, date: String },
    /// Set priority: none|low|medium|high
    Prioritize { id: String, level: String },
    /// Move a task out of sight (reversible)
    Archive { id: String },
    /// Bring an archived task back to active
    Unarchive { id: String },
    /// Take a task out of the inbox, optionally assigning a project
    Organize {
        id: String,
        #[arg(long, help = "Project name (or none/- to clear)")]
        project: Option<String>,
    },
    /// Permanently delete a task (asks y/N on a TTY; requires --force otherwise)
    Delete {
        id: String,
        #[arg(long)]
        force: bool,
    },
    /// Search all non-archived tasks (ANDed case-insensitive terms)
    Search {
        query: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Per-view task counts
    Counts,
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(err) => {
            // Usage errors honor --json too (SPEC §3): check raw args.
            if std::env::args().any(|a| a == "--json") {
                let payload =
                    serde_json::json!({"error": {"code": "usage", "message": err.to_string()}});
                eprintln!("{payload}");
                std::process::exit(2);
            }
            let _ = err.print();
            std::process::exit(err.exit_code());
        }
    };
    let code = match run::dispatch(&cli) {
        Ok(()) => 0,
        Err(e) => {
            if cli.json {
                let payload =
                    serde_json::json!({"error": {"code": e.code(), "message": e.to_string()}});
                eprintln!("{payload}");
            } else {
                eprintln!("error: {e}");
            }
            e.exit_code()
        }
    };
    std::process::exit(code);
}
