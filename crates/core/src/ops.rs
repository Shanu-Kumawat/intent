use jiff::Timestamp;

use crate::error::Result;
use crate::id;
use crate::model::{NewTask, Patch, Priority, Status, Task, TaskPatch};
use crate::store::Store;

/// One task's before/after snapshots for a mutation. The GUI builds undo
/// entries from these; the CLI ignores them (SPEC §5).
#[derive(Debug, Clone)]
pub struct RowChange {
    pub id: String,
    pub before: Option<Task>,
    pub after: Option<Task>,
}

/// Result of an operation that changed something.
#[derive(Debug, Clone)]
pub struct OpOutcome {
    pub task: Option<Task>,
    pub change: Option<RowChange>,
}

impl OpOutcome {
    fn unchanged(task: Task) -> OpOutcome {
        OpOutcome { task: Some(task), change: None }
    }
    fn changed(before: Task, after: Task) -> OpOutcome {
        OpOutcome { task: Some(after.clone()), change: Some(RowChange { id: before.id.clone(), before: Some(before), after: Some(after) }) }
    }
}

fn now() -> Timestamp {
    Timestamp::now()
}

/// Capture a new task (SPEC §1.2). Zero metadata required.
pub fn add(store: &Store, spec: NewTask) -> Result<OpOutcome> {
    spec.validate()?;
    let t = now();
    let inbox = spec.inbox();
    let task = Task {
        id: id::new_id(),
        title: spec.title.trim().to_string(),
        notes: spec.notes,
        status: Status::Active,
        inbox,
        priority: spec.priority,
        project: spec.project,
        tags: spec.tags,
        due: spec.due,
        remind: spec.remind,
        defer_until: None,
        created_at: t,
        updated_at: t,
        completed_at: None,
    };
    store.insert_task(&task)?;
    Ok(OpOutcome {
        change: Some(RowChange { id: task.id.clone(), before: None, after: Some(task.clone()) }),
        task: Some(task),
    })
}

/// Apply an arbitrary patch to a task.
pub fn edit(store: &Store, task_id: &str, patch: &TaskPatch) -> Result<OpOutcome> {
    // Title constraints match creation (SPEC §1): single line, non-empty.
    let mut patch = patch.clone();
    if let Patch::Set(t) = &patch.title {
        let trimmed = t.trim().to_string();
        if trimmed.is_empty() {
            return Err(crate::error::bad_value("title must not be empty"));
        }
        if trimmed.contains('\n') {
            return Err(crate::error::bad_value("title must be a single line"));
        }
        patch.title = Patch::Set(trimmed);
    }
    let before = store.get_task(task_id)?;
    let mut after = before.clone();
    if !patch.apply(&mut after) {
        return Ok(OpOutcome::unchanged(after));
    }
    after.updated_at = now();
    store.update_task(&after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn done(store: &Store, task_id: &str) -> Result<OpOutcome> {
    let before = store.get_task(task_id)?;
    if before.status == Status::Done {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Done;
    let t = now();
    after.completed_at = Some(t);
    after.updated_at = t;
    store.update_task(&after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn reopen(store: &Store, task_id: &str) -> Result<OpOutcome> {
    let before = store.get_task(task_id)?;
    if before.status == Status::Active {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Active;
    after.completed_at = None;
    after.updated_at = now();
    store.update_task(&after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn archive(store: &Store, task_id: &str) -> Result<OpOutcome> {
    let before = store.get_task(task_id)?;
    if before.status == Status::Archived {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Archived;
    after.updated_at = now();
    store.update_task(&after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn unarchive(store: &Store, task_id: &str) -> Result<OpOutcome> {
    let before = store.get_task(task_id)?;
    if before.status == Status::Active {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Active;
    after.updated_at = now();
    store.update_task(&after)?;
    Ok(OpOutcome::changed(before, after))
}

/// Take a task out of the inbox, optionally assigning a project.
pub fn organize(store: &Store, task_id: &str, project: Option<&str>) -> Result<OpOutcome> {
    let mut patch = TaskPatch::default();
    patch.organize = true;
    if let Some(p) = project {
        patch.project = crate::model::Patch::Set(p.to_string());
    }
    edit(store, task_id, &patch)
}

pub fn defer(store: &Store, task_id: &str, date: Option<crate::dates::Date>) -> Result<OpOutcome> {
    let mut patch = TaskPatch::default();
    patch.defer_until = match date {
        Some(d) => crate::model::Patch::Set(d),
        None => crate::model::Patch::Clear,
    };
    edit(store, task_id, &patch)
}

pub fn prioritize(store: &Store, task_id: &str, priority: Priority) -> Result<OpOutcome> {
    let mut patch = TaskPatch::default();
    patch.priority = crate::model::Patch::Set(priority);
    edit(store, task_id, &patch)
}

/// Permanently delete. Returns false if the id did not exist.
pub fn delete(store: &Store, task_id: &str) -> Result<Option<OpOutcome>> {
    let before = match store.get_task(task_id) {
        Ok(t) => t,
        Err(e) => return Err(e),
    };
    store.delete_task(task_id)?;
    Ok(Some(OpOutcome {
        task: None,
        change: Some(RowChange { id: before.id.clone(), before: Some(before), after: None }),
    }))
}

/// Apply a recorded change in reverse (undo) or forward again (redo).
/// `None` snapshots mean create/delete respectively (SPEC §5).
pub fn apply_change(store: &Store, change: &RowChange, forward: bool) -> Result<()> {
    let (from, to) = if forward {
        (&change.before, &change.after)
    } else {
        (&change.after, &change.before)
    };
    let _ = from;
    match to {
        Some(task) => {
            if store.get_task(&task.id).is_ok() {
                store.update_task(task)?;
            } else {
                store.insert_task(task)?;
            }
        }
        None => {
            store.delete_task(&change.id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dates::Date;
    use crate::model::Patch;

    fn store() -> Store {
        Store::open_memory().unwrap()
    }

    fn mk(store: &Store, title: &str) -> Task {
        add(store, NewTask { title: title.into(), ..Default::default() })
            .unwrap()
            .task
            .unwrap()
    }

    #[test]
    fn capture_is_inbox_active() {
        let s = store();
        let t = mk(&s, "idea");
        assert!(t.inbox);
        assert_eq!(t.status, Status::Active);
        assert_eq!(t.title, "idea");
    }

    #[test]
    fn capture_validates_title() {
        let s = store();
        assert!(add(&s, NewTask { title: "  ".into(), ..Default::default() }).is_err());
        assert!(add(&s, NewTask { title: "two\nlines".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn done_sets_completed_at_reopen_clears() {
        let s = store();
        let t = mk(&s, "x");
        let t = done(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Done);
        assert!(t.completed_at.is_some());
        let again = done(&s, &t.id).unwrap();
        assert!(again.change.is_none(), "done on done task is a no-op");
        let t = reopen(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Active);
        assert!(t.completed_at.is_none());
    }

    #[test]
    fn archive_round_trip() {
        let s = store();
        let t = mk(&s, "x");
        let t = archive(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Archived);
        assert!(t.completed_at.is_none());
        let t = unarchive(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Active);
    }

    #[test]
    fn defer_and_undo() {
        let s = store();
        let t = mk(&s, "x");
        let out = defer(&s, &t.id, Some(Date(jiff::civil::date(2026, 12, 1)))).unwrap();
        let after = out.task.unwrap();
        assert_eq!(after.defer_until.map(|d| d.to_string()), Some("2026-12-01".into()));
        assert!(after.inbox, "deferring alone does not organize");

        let change = out.change.unwrap();
        apply_change(&s, &change, false).unwrap();
        let reverted = s.get_task(&t.id).unwrap();
        assert_eq!(reverted.defer_until, None);
        apply_change(&s, &change, true).unwrap();
        let redone = s.get_task(&t.id).unwrap();
        assert_eq!(redone.defer_until.map(|d| d.to_string()), Some("2026-12-01".into()));
    }

    #[test]
    fn delete_undo_resurrects_same_id() {
        let s = store();
        let t = mk(&s, "keep me");
        let created = t.created_at;
        let out = delete(&s, &t.id).unwrap().unwrap();
        assert!(s.get_task(&t.id).is_err());
        apply_change(&s, &out.change.as_ref().unwrap(), false).unwrap();
        let back = s.get_task(&t.id).unwrap();
        assert_eq!(back.id, t.id);
        assert_eq!(back.created_at, created);
        assert_eq!(back.title, "keep me");
    }

    #[test]
    fn edit_noop_keeps_updated_at() {
        let s = store();
        let t = mk(&s, "x");
        let patch = TaskPatch { title: Patch::Set("x".into()), ..Default::default() };
        let out = edit(&s, &t.id, &patch).unwrap();
        assert!(out.change.is_none());
        assert_eq!(out.task.unwrap().updated_at, t.updated_at);
    }

    #[test]
    fn organize_clears_inbox() {
        let s = store();
        let t = mk(&s, "x");
        let t = organize(&s, &t.id, Some("work")).unwrap().task.unwrap();
        assert!(!t.inbox);
        assert_eq!(t.project.as_deref(), Some("work"));
    }
}
