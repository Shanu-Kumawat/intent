use intent_core::dates::{is_clear_marker, parse_date};
use intent_core::model::{NewTask, Patch, TaskPatch, TaskView};
use intent_core::views::ListFilter;
use intent_core::{ops, today, CoreError, Result, Store};

use crate::output;
use crate::{Cli, Cmd};

pub fn dispatch(cli: &Cli) -> Result<()> {
    let store = Store::open_default()?;
    match &cli.command {
        Cmd::Add {
            title,
            title_opt,
            notes,
            due,
            remind,
            priority,
            project,
            tags,
            no_inbox,
        } => {
            let title = title
                .as_deref()
                .or(title_opt.as_deref())
                .ok_or_else(|| usage("provide a TITLE argument or --title"))?;
            let spec = NewTask {
                title: title.to_string(),
                notes: notes.clone().unwrap_or_default(),
                priority: priority
                    .as_deref()
                    .map(intent_core::dates::parse_priority)
                    .transpose()?
                    .unwrap_or_default(),
                project: match project.as_deref() {
                    None => None,
                    Some(p) if is_clear_marker(p) => None,
                    Some(p) => Some(p.to_string()),
                },
                tags: tags
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect(),
                due: parse_opt_date(due.as_deref())?,
                remind: parse_opt_date(remind.as_deref())?,
                no_inbox: *no_inbox,
            };
            let out = ops::add(&store, spec)?;
            output::one_task(cli, out.task.as_ref().unwrap(), "captured");
        }
        Cmd::List {
            view,
            project,
            tag,
            priority,
            query,
            limit,
        } => {
            let view = parse_view(view.as_deref().unwrap_or("all"))?;
            let filter = ListFilter {
                project: project.clone(),
                tag: tag.clone(),
                priority: priority
                    .as_deref()
                    .map(intent_core::dates::parse_priority)
                    .transpose()?,
                query: query.clone(),
                limit: *limit,
            };
            let tasks = intent_core::views::load_view(&store, view, today())?;
            let tasks = filter.apply(tasks);
            output::task_list(cli, view, &tasks);
        }
        Cmd::Show { id } => {
            let task = store.get_task(id.trim())?;
            output::show_task(cli, &task);
        }
        Cmd::Edit {
            id,
            title,
            notes,
            due,
            remind,
            defer_until,
            priority,
            project,
            tags,
            tag_add,
            tag_rm,
        } => {
            let mut patch = TaskPatch::default();
            if let Some(t) = title {
                patch.title = Patch::Set(t.clone());
            }
            if let Some(n) = notes {
                patch.notes = Patch::Set(n.clone());
            }
            apply_date_flag(&mut patch.due, due.as_deref())?;
            apply_date_flag(&mut patch.remind, remind.as_deref())?;
            apply_date_flag(&mut patch.defer_until, defer_until.as_deref())?;
            if let Some(p) = priority {
                patch.priority = Patch::Set(intent_core::dates::parse_priority(p)?);
            }
            if let Some(p) = project {
                patch.project = if is_clear_marker(p) {
                    Patch::Clear
                } else {
                    Patch::Set(p.clone())
                };
            }
            let mut final_tags = tags.clone();
            if final_tags.is_none() && (tag_add.is_some() || tag_rm.is_some()) {
                final_tags = Some(store.get_task(id.trim())?.tags);
            }
            if let Some(t) = final_tags {
                let mut t: Vec<String> = t.into_iter().filter(|x| !x.is_empty()).collect();
                if let Some(add) = tag_add {
                    for a in add {
                        if !t.contains(a) {
                            t.push(a.clone());
                        }
                    }
                }
                if let Some(rm) = tag_rm {
                    t.retain(|x| !rm.contains(x));
                }
                patch.tags = Patch::Set(t);
            }
            if patch.is_empty() {
                return Err(usage("edit needs at least one field flag"));
            }
            let out = ops::edit(&store, id.trim(), &patch)?;
            output::one_task(cli, out.task.as_ref().unwrap(), "edited");
        }
        Cmd::Done { id } => {
            let out = ops::done(&store, id.trim())?;
            output::one_task(cli, out.task.as_ref().unwrap(), "done");
        }
        Cmd::Reopen { id } => {
            let out = ops::reopen(&store, id.trim())?;
            output::one_task(cli, out.task.as_ref().unwrap(), "reopened");
        }
        Cmd::Defer { id, date } => {
            let d = if is_clear_marker(date) {
                None
            } else {
                Some(parse_date(date, today())?)
            };
            let out = ops::defer(&store, id.trim(), d)?;
            output::one_task(cli, out.task.as_ref().unwrap(), "deferred");
        }
        Cmd::Prioritize { id, level } => {
            let p = intent_core::dates::parse_priority(level)?;
            let out = ops::prioritize(&store, id.trim(), p)?;
            output::one_task(cli, out.task.as_ref().unwrap(), "prioritized");
        }
        Cmd::Archive { id } => {
            let out = ops::archive(&store, id.trim())?;
            output::one_task(cli, out.task.as_ref().unwrap(), "archived");
        }
        Cmd::Unarchive { id } => {
            let out = ops::unarchive(&store, id.trim())?;
            output::one_task(cli, out.task.as_ref().unwrap(), "unarchived");
        }
        Cmd::Organize { id, project } => {
            let project = project.as_deref().and_then(|p| {
                if is_clear_marker(p) {
                    None
                } else {
                    Some(p.to_string())
                }
            });
            let out = ops::organize(&store, id.trim(), project.as_deref())?;
            output::one_task(cli, out.task.as_ref().unwrap(), "organized");
        }
        Cmd::Delete { id, force } => {
            let id = id.trim();
            if !*force {
                use std::io::IsTerminal;
                if std::io::stdin().is_terminal() {
                    // stderr: under --json, stdout stays pure JSON (SPEC §3).
                    use std::io::Write;
                    eprint!("permanently delete {id}? [y/N] ");
                    let _ = std::io::stderr().flush();
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).ok();
                    let ans = line.trim().to_ascii_lowercase();
                    if ans != "y" && ans != "yes" {
                        eprintln!("aborted");
                        return Ok(());
                    }
                } else {
                    return Err(CoreError::Io(std::io::Error::other(
                        "refusing to delete without --force in non-interactive mode",
                    )));
                }
            }
            ops::delete(&store, id)?.ok_or_else(|| CoreError::NotFound(id.to_string()))?;
            output::deleted(cli, id);
        }
        Cmd::Search { query, limit } => {
            let filter = ListFilter {
                limit: *limit,
                ..Default::default()
            };
            let tasks = intent_core::views::search(&store.all_tasks()?, query, false);
            let tasks = filter.apply(tasks);
            output::task_list(cli, TaskView::All, &tasks);
        }
        Cmd::Completions { .. } => unreachable!("handled before dispatch in main"),
        Cmd::Counts => {
            let counts = intent_core::views::counts(&store.all_tasks()?, today());
            output::counts(cli, &counts);
        }
    }
    Ok(())
}

fn usage(msg: &str) -> CoreError {
    CoreError::Usage(msg.to_string())
}

fn parse_view(s: &str) -> Result<TaskView> {
    TaskView::parse(s).ok_or_else(|| CoreError::BadValue(format!("unknown view {s:?}")))
}

fn parse_opt_date(s: Option<&str>) -> Result<Option<intent_core::Date>> {
    match s {
        None => Ok(None),
        Some(v) if is_clear_marker(v) => Ok(None),
        Some(v) => Ok(Some(parse_date(v, today())?)),
    }
}

fn apply_date_flag(slot: &mut Patch<intent_core::Date>, v: Option<&str>) -> Result<()> {
    if let Some(v) = v {
        *slot = if is_clear_marker(v) {
            Patch::Clear
        } else {
            Patch::Set(parse_date(v, today())?)
        };
    }
    Ok(())
}
