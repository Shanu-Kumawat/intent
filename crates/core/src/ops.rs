use jiff::Timestamp;
use rusqlite::Connection;

use crate::error::{CoreError, Result};
use crate::id;
use crate::model::{NewTask, Patch, Priority, Status, Task, TaskPatch};
use crate::store::{delete_task_conn, get_task_conn, insert_task_conn, update_task_conn, Store};

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
        OpOutcome {
            task: Some(task),
            change: None,
        }
    }
    fn changed(before: Task, after: Task) -> OpOutcome {
        OpOutcome {
            task: Some(after.clone()),
            change: Some(RowChange {
                id: before.id.clone(),
                before: Some(before),
                after: Some(after),
            }),
        }
    }
}

fn now() -> Timestamp {
    Timestamp::now()
}

/// Insert with id-collision retry: 6-char ids are random, so a collision is
/// astronomically unlikely but the spec names the `conflict` error (SPEC §3).
fn insert_new_task(conn: &Connection, task: &Task) -> Result<Task> {
    let mut task = task.clone();
    for attempt in 0..5 {
        match insert_task_conn(conn, &task) {
            Ok(()) => return Ok(task),
            Err(CoreError::Db(rusqlite::Error::SqliteFailure(err, _)))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                if attempt == 4 {
                    return Err(CoreError::Conflict(format!(
                        "could not allocate a unique task id after {} attempts",
                        attempt + 1
                    )));
                }
                task.id = id::new_id();
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("loop returns on every branch")
}

/// Capture a new task (SPEC §1.2). Zero metadata required.
pub fn add(store: &Store, spec: NewTask) -> Result<OpOutcome> {
    spec.validate()?;
    let t = now();
    let inbox = spec.inbox();
    let project = spec.project.filter(|p| !p.is_empty());
    let task = Task {
        id: id::new_id(),
        title: spec.title.trim().to_string(),
        notes: spec.notes,
        status: Status::Active,
        inbox,
        priority: spec.priority,
        project,
        tags: spec.tags,
        due: spec.due,
        remind: spec.remind,
        defer_until: None,
        created_at: t,
        updated_at: t,
        completed_at: None,
    };
    let task = insert_new_task(store.conn(), &task)?;
    Ok(OpOutcome {
        change: Some(RowChange {
            id: task.id.clone(),
            before: None,
            after: Some(task.clone()),
        }),
        task: Some(task),
    })
}

/// Apply an arbitrary patch to a task.
pub fn edit(store: &Store, task_id: &str, patch: &TaskPatch) -> Result<OpOutcome> {
    edit_conn(store.conn(), task_id, patch)
}

fn edit_conn(conn: &Connection, task_id: &str, patch: &TaskPatch) -> Result<OpOutcome> {
    // Title constraints match creation (SPEC §1): single line, non-empty.
    let mut patch = patch.clone();
    if let Patch::Set(t) = &patch.title {
        let trimmed = t.trim().to_string();
        if trimmed.is_empty() {
            return Err(crate::error::bad_value("title must not be empty"));
        }
        if trimmed.contains('\n') || trimmed.contains('\r') {
            return Err(crate::error::bad_value("title must be a single line"));
        }
        patch.title = Patch::Set(trimmed);
    }
    let before = get_task_conn(conn, task_id)?;
    let mut after = before.clone();
    if !patch.apply(&mut after) {
        return Ok(OpOutcome::unchanged(after));
    }
    after.updated_at = now();
    update_task_conn(conn, &after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn done(store: &Store, task_id: &str) -> Result<OpOutcome> {
    done_conn(store.conn(), task_id)
}

fn done_conn(conn: &Connection, task_id: &str) -> Result<OpOutcome> {
    let before = get_task_conn(conn, task_id)?;
    if before.status == Status::Done {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Done;
    let t = now();
    after.completed_at = Some(t);
    after.updated_at = t;
    update_task_conn(conn, &after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn reopen(store: &Store, task_id: &str) -> Result<OpOutcome> {
    reopen_conn(store.conn(), task_id)
}

fn reopen_conn(conn: &Connection, task_id: &str) -> Result<OpOutcome> {
    let before = get_task_conn(conn, task_id)?;
    if before.status == Status::Active {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Active;
    after.completed_at = None;
    after.updated_at = now();
    update_task_conn(conn, &after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn archive(store: &Store, task_id: &str) -> Result<OpOutcome> {
    archive_conn(store.conn(), task_id)
}

fn archive_conn(conn: &Connection, task_id: &str) -> Result<OpOutcome> {
    let before = get_task_conn(conn, task_id)?;
    if before.status == Status::Archived {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Archived;
    after.updated_at = now();
    update_task_conn(conn, &after)?;
    Ok(OpOutcome::changed(before, after))
}

pub fn unarchive(store: &Store, task_id: &str) -> Result<OpOutcome> {
    unarchive_conn(store.conn(), task_id)
}

fn unarchive_conn(conn: &Connection, task_id: &str) -> Result<OpOutcome> {
    let before = get_task_conn(conn, task_id)?;
    if before.status == Status::Active {
        return Ok(OpOutcome::unchanged(before));
    }
    let mut after = before.clone();
    after.status = Status::Active;
    // An active task never carries completed_at (SPEC §1.1): done → archived
    // → active must come back clean, like reopen does.
    after.completed_at = None;
    after.updated_at = now();
    update_task_conn(conn, &after)?;
    Ok(OpOutcome::changed(before, after))
}

/// Take a task out of the inbox, optionally assigning a project.
pub fn organize(store: &Store, task_id: &str, project: Option<&str>) -> Result<OpOutcome> {
    let mut patch = TaskPatch {
        organize: true,
        ..Default::default()
    };
    if let Some(p) = project {
        patch.project = Patch::Set(p.to_string());
    }
    edit(store, task_id, &patch)
}

pub fn defer(store: &Store, task_id: &str, date: Option<crate::dates::Date>) -> Result<OpOutcome> {
    let patch = TaskPatch {
        defer_until: match date {
            Some(d) => Patch::Set(d),
            None => Patch::Clear,
        },
        ..Default::default()
    };
    edit(store, task_id, &patch)
}

pub fn prioritize(store: &Store, task_id: &str, priority: Priority) -> Result<OpOutcome> {
    let patch = TaskPatch {
        priority: Patch::Set(priority),
        ..Default::default()
    };
    edit(store, task_id, &patch)
}

/// Permanently delete.
pub fn delete(store: &Store, task_id: &str) -> Result<Option<OpOutcome>> {
    delete_conn(store.conn(), task_id)
}

fn delete_conn(conn: &Connection, task_id: &str) -> Result<Option<OpOutcome>> {
    let before = get_task_conn(conn, task_id)?;
    let deleted = delete_task_conn(conn, task_id)?;
    if !deleted {
        // Vanished between the read and the delete (another process).
        return Ok(None);
    }
    Ok(Some(OpOutcome {
        task: None,
        change: Some(RowChange {
            id: before.id.clone(),
            before: Some(before),
            after: None,
        }),
    }))
}

/// x semantics (SPEC §4.1): active→done, done→active, archived→active.
fn toggle_complete_conn(conn: &Connection, task_id: &str) -> Result<Option<RowChange>> {
    let task = get_task_conn(conn, task_id)?;
    let out = match task.status {
        Status::Active => done_conn(conn, task_id)?,
        Status::Done => reopen_conn(conn, task_id)?,
        _ => unarchive_conn(conn, task_id)?,
    };
    Ok(out.change)
}

/// Complete/reopen/revive several tasks in ONE transaction (SPEC §2): a
/// failure mid-batch leaves every task untouched.
pub fn complete_many(store: &Store, ids: &[String]) -> Result<Vec<RowChange>> {
    let tx = store.transaction()?;
    let mut changes = Vec::new();
    for id in ids {
        if let Some(change) = toggle_complete_conn(&tx, id)? {
            changes.push(change);
        }
    }
    tx.commit()?;
    Ok(changes)
}

/// Archive several tasks in one transaction.
pub fn archive_many(store: &Store, ids: &[String]) -> Result<Vec<RowChange>> {
    let tx = store.transaction()?;
    let mut changes = Vec::new();
    for id in ids {
        changes.extend(archive_conn(&tx, id)?.change);
    }
    tx.commit()?;
    Ok(changes)
}

/// Apply one recorded change in reverse (undo) or forward again (redo).
/// Only a genuine `NotFound` triggers an insert — a locked or corrupt
/// database must not be mistaken for a missing task (SPEC §2/§5).
pub fn apply_change(store: &Store, change: &RowChange, forward: bool) -> Result<()> {
    apply_change_conn(store.conn(), change, forward)
}

fn apply_change_conn(conn: &Connection, change: &RowChange, forward: bool) -> Result<()> {
    let to = if forward {
        &change.after
    } else {
        &change.before
    };
    match to {
        Some(task) => match get_task_conn(conn, &task.id) {
            Ok(_) => update_task_conn(conn, task),
            Err(CoreError::NotFound(_)) => insert_task_conn(conn, task),
            Err(e) => Err(e),
        },
        None => {
            delete_task_conn(conn, &change.id)?;
            Ok(())
        }
    }
}

/// Apply a whole undo/redo entry inside one transaction: all rows or none.
pub fn apply_changes(store: &Store, changes: &[RowChange], forward: bool) -> Result<()> {
    let tx = store.transaction()?;
    for change in changes {
        apply_change_conn(&tx, change, forward)?;
    }
    tx.commit()?;
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
        add(
            store,
            NewTask {
                title: title.into(),
                ..Default::default()
            },
        )
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
        assert!(add(
            &s,
            NewTask {
                title: "  ".into(),
                ..Default::default()
            }
        )
        .is_err());
        assert!(add(
            &s,
            NewTask {
                title: "two\nlines".into(),
                ..Default::default()
            }
        )
        .is_err());
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
    fn unarchive_clears_completed_at() {
        // done → archived → active must not carry completed_at (P1-2).
        let s = store();
        let t = mk(&s, "x");
        let t = done(&s, &t.id).unwrap().task.unwrap();
        let t = archive(&s, &t.id).unwrap().task.unwrap();
        let t = unarchive(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Active);
        assert!(t.completed_at.is_none(), "active task with completed_at");
    }

    #[test]
    fn archive_round_trip() {
        let s = store();
        let t = mk(&s, "x");
        let t = archive(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Archived);
        let t = unarchive(&s, &t.id).unwrap().task.unwrap();
        assert_eq!(t.status, Status::Active);
    }

    #[test]
    fn defer_and_undo() {
        let s = store();
        let t = mk(&s, "x");
        let out = defer(&s, &t.id, Some(Date(jiff::civil::date(2026, 12, 1)))).unwrap();
        let after = out.task.unwrap();
        assert_eq!(
            after.defer_until.map(|d| d.to_string()),
            Some("2026-12-01".into())
        );
        assert!(after.inbox, "deferring alone does not organize");

        let change = out.change.unwrap();
        apply_change(&s, &change, false).unwrap();
        let reverted = s.get_task(&t.id).unwrap();
        assert_eq!(reverted.defer_until, None);
        apply_change(&s, &change, true).unwrap();
        let redone = s.get_task(&t.id).unwrap();
        assert_eq!(
            redone.defer_until.map(|d| d.to_string()),
            Some("2026-12-01".into())
        );
    }

    #[test]
    fn delete_undo_resurrects_same_id() {
        let s = store();
        let t = mk(&s, "keep me");
        let created = t.created_at;
        let out = delete(&s, &t.id).unwrap().unwrap();
        assert!(s.get_task(&t.id).is_err());
        apply_change(&s, out.change.as_ref().unwrap(), false).unwrap();
        let back = s.get_task(&t.id).unwrap();
        assert_eq!(back.id, t.id);
        assert_eq!(back.created_at, created);
        assert_eq!(back.title, "keep me");
    }

    #[test]
    fn edit_noop_keeps_updated_at() {
        let s = store();
        let t = mk(&s, "x");
        let patch = TaskPatch {
            title: Patch::Set("x".into()),
            ..Default::default()
        };
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

    #[test]
    fn complete_many_is_atomic() {
        let s = store();
        let a = mk(&s, "a");
        let b = mk(&s, "b");
        let changes = complete_many(&s, &[a.id.clone(), b.id.clone()]).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(s.get_task(&a.id).unwrap().status, Status::Done);
        assert_eq!(s.get_task(&b.id).unwrap().status, Status::Done);
        // undo of a batch restores both in one step
        apply_changes(&s, &changes, false).unwrap();
        assert_eq!(s.get_task(&a.id).unwrap().status, Status::Active);
        assert_eq!(s.get_task(&b.id).unwrap().status, Status::Active);
    }

    #[test]
    fn complete_many_rolls_back_on_error() {
        let s = store();
        let a = mk(&s, "a");
        let missing = "zzzzzz".to_string();
        let err = complete_many(&s, &[a.id.clone(), missing]).unwrap_err();
        assert_eq!(err.code(), "not_found");
        // The successful first toggle rolled back with the failed batch.
        assert_eq!(s.get_task(&a.id).unwrap().status, Status::Active);
    }

    #[test]
    fn apply_change_propagates_db_errors() {
        let s = store();
        let t = mk(&s, "x");
        s.conn()
            .execute_batch(
                "CREATE TRIGGER block_update BEFORE UPDATE ON tasks
                 BEGIN SELECT RAISE(ABORT, 'updates blocked'); END;",
            )
            .unwrap();
        let _change = RowChange {
            id: t.id.clone(),
            before: Some(s.get_task(&t.id).unwrap()),
            after: None, // undo would delete; redo would update — force update path
        };
        // forward on after=None deletes; use a change that updates instead:
        let mut after = t.clone();
        after.title = "changed".into();
        let change = RowChange {
            id: t.id.clone(),
            before: Some(t.clone()),
            after: Some(after),
        };
        let err = apply_change(&s, &change, true).unwrap_err();
        assert_eq!(
            err.code(),
            "db",
            "a blocked update must surface as a db error"
        );
        // and must NOT have fallen back to an insert
        assert_eq!(s.get_task(&t.id).unwrap().title, "x");
    }

    #[test]
    fn add_retries_id_collision() {
        // Force one collision, then success.
        let s = store();
        let t = mk(&s, "x");
        // Insert a task with a KNOWN id to make the collision deterministic
        // is not possible with random ids; instead verify the conflict path
        // directly by pre-inserting and calling the internal helper.
        let mut spec_task = t.clone();
        spec_task.id = t.id.clone(); // guaranteed constraint violation
        let result = insert_new_task_with_forced_ids(&s, &[t.id.clone(), t.id.clone()]);
        assert!(result.is_ok(), "retry with a fresh id succeeds");
        let _ = spec_task;
    }

    fn insert_new_task_with_forced_ids(s: &Store, _ids: &[String]) -> Result<Task> {
        // Simplified: the public retry path is exercised by insertion of a
        // duplicate via the raw helper.
        let mut task = mk(s, "probe");
        // Force the first attempt to collide, then let the retry generate.
        insert_new_task(s.conn(), &task)?;
        task.id = crate::id::new_id();
        Ok(task)
    }
}
