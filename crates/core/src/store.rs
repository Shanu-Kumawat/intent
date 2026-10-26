use std::path::{Path, PathBuf};

use jiff::Timestamp;
use rusqlite::{params, Connection, OptionalExtension};

use crate::dates::Date;
use crate::error::{CoreError, Result};
use crate::model::{Priority, Status, Task};

const SCHEMA_VERSION: i64 = 1;

/// Opened SQLite database. One instance per process (GUI) or per invocation
/// (CLI); WAL mode lets them coexist (SPEC §2).
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if necessary) the database at `path`.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Default database path: `$INTENT_DB`, else `$XDG_DATA_HOME/intent/tasks.db`,
    /// else `~/.local/share/intent/tasks.db` (SPEC §2). An empty `INTENT_DB`
    /// is treated as unset — SQLite would otherwise open a throwaway temp
    /// database and the user's writes would silently vanish.
    pub fn default_path() -> PathBuf {
        if let Some(p) = std::env::var("INTENT_DB").ok().filter(|s| !s.is_empty()) {
            return PathBuf::from(p);
        }
        let base = std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local/share")))
            .expect("no home directory available for database path");
        base.join("intent").join("tasks.db")
    }

    pub fn open_default() -> Result<Store> {
        Store::open(&Store::default_path())
    }

    /// In-memory store for tests.
    pub fn open_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS tasks(
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 notes TEXT NOT NULL DEFAULT '',
                 status TEXT NOT NULL DEFAULT 'active',
                 inbox INTEGER NOT NULL DEFAULT 1,
                 priority TEXT NOT NULL DEFAULT 'none',
                 project TEXT,
                 tags TEXT NOT NULL DEFAULT '[]',
                 due TEXT,
                 remind TEXT,
                 defer_until TEXT,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 completed_at TEXT);
             CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);",
        )?;
        let version: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| {
                r.get(0)
            })
            .optional()?;
        match version {
            None => {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES('schema_version', ?1)",
                    params![SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) => {
                let v: i64 = v
                    .parse()
                    .map_err(|_| CoreError::Db(rusqlite::Error::QueryReturnedNoRows))?;
                if v > SCHEMA_VERSION {
                    return Err(crate::error::bad_value(format!(
                        "database schema version {v} is newer than this build supports ({SCHEMA_VERSION}); upgrade the app"
                    )));
                }
                if v < SCHEMA_VERSION {
                    // Forward-only migrations would run here; v1 is current.
                }
            }
        }
        Ok(())
    }

    /// Monotonic counter that changes when another connection commits.
    /// Used by the GUI to detect external writes (SPEC §2).
    pub fn data_version(&self) -> Result<u64> {
        let v: i64 = self.conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        Ok(v as u64)
    }

    pub fn insert_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tasks(id, title, notes, status, inbox, priority, project, tags,
                               due, remind, defer_until, created_at, updated_at, completed_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                task.id,
                task.title,
                task.notes,
                task.status.as_str(),
                task.inbox as i64,
                task.priority.as_str(),
                task.project,
                serde_json::to_string(&task.tags).expect("tags serialize"),
                task.due.map(|d| d.to_string()),
                task.remind.map(|d| d.to_string()),
                task.defer_until.map(|d| d.to_string()),
                task.created_at.to_string(),
                task.updated_at.to_string(),
                task.completed_at.map(|t| t.to_string()),
            ],
        )?;
        Ok(())
    }

    pub fn update_task(&self, task: &Task) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE tasks SET title=?2, notes=?3, status=?4, inbox=?5, priority=?6, project=?7,
                              tags=?8, due=?9, remind=?10, defer_until=?11, updated_at=?12,
                              completed_at=?13
             WHERE id=?1",
            params![
                task.id,
                task.title,
                task.notes,
                task.status.as_str(),
                task.inbox as i64,
                task.priority.as_str(),
                task.project,
                serde_json::to_string(&task.tags).expect("tags serialize"),
                task.due.map(|d| d.to_string()),
                task.remind.map(|d| d.to_string()),
                task.defer_until.map(|d| d.to_string()),
                task.updated_at.to_string(),
                task.completed_at.map(|t| t.to_string()),
            ],
        )?;
        if n == 0 {
            return Err(CoreError::NotFound(task.id.clone()));
        }
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Task> {
        self.conn
            .query_row("SELECT * FROM tasks WHERE id=?1", params![id], row_to_task)
            .optional()?
            .ok_or_else(|| CoreError::NotFound(id.to_string()))
    }

    pub fn delete_task(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute("DELETE FROM tasks WHERE id=?1", params![id])?;
        Ok(n > 0)
    }

    /// All tasks, any status. Views/search filter on top of this (SPEC §1.3).
    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare("SELECT * FROM tasks ORDER BY created_at, id")?;
        let rows = stmt.query_map([], row_to_task)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status: String = row.get("status")?;
    let priority: String = row.get("priority")?;
    let tags_json: String = row.get("tags")?;
    let due: Option<String> = row.get("due")?;
    let remind: Option<String> = row.get("remind")?;
    let defer_until: Option<String> = row.get("defer_until")?;
    let created_at: String = row.get("created_at")?;
    let updated_at: String = row.get("updated_at")?;
    let completed_at: Option<String> = row.get("completed_at")?;
    Ok(Task {
        id: row.get("id")?,
        title: row.get("title")?,
        notes: row.get("notes")?,
        status: Status::parse(&status).unwrap_or(Status::Active),
        inbox: row.get::<_, i64>("inbox")? != 0,
        priority: Priority::parse(&priority).unwrap_or(Priority::None),
        project: row.get("project")?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        due: due.and_then(|s| s.parse().ok()).map(Date),
        remind: remind.and_then(|s| s.parse().ok()).map(Date),
        defer_until: defer_until.and_then(|s| s.parse().ok()).map(Date),
        created_at: parse_ts(&created_at),
        updated_at: parse_ts(&updated_at),
        completed_at: completed_at.and_then(|s| s.parse::<Timestamp>().ok()),
    })
}

fn parse_ts(s: &str) -> Timestamp {
    s.parse().unwrap_or_else(|_| Timestamp::now())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().filter(|s| !s.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewTask;

    fn task(id: &str, title: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            notes: "n".into(),
            status: Status::Active,
            inbox: true,
            priority: Priority::Low,
            project: Some("p".into()),
            tags: vec!["a".into()],
            due: Some(Date(jiff::civil::date(2026, 10, 1))),
            remind: None,
            defer_until: None,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
            completed_at: None,
        }
    }

    #[test]
    fn round_trip() {
        let store = Store::open_memory().unwrap();
        let t = task("abc123", "hello");
        store.insert_task(&t).unwrap();
        let got = store.get_task("abc123").unwrap();
        assert_eq!(got.title, "hello");
        assert_eq!(got.project.as_deref(), Some("p"));
        assert_eq!(got.tags, vec!["a".to_string()]);
        assert_eq!(got.due.map(|d| d.to_string()), Some("2026-10-01".into()));
        assert_eq!(got.priority, Priority::Low);
        assert_eq!(got.status, Status::Active);

        let mut t2 = got.clone();
        t2.title = "changed".into();
        store.update_task(&t2).unwrap();
        assert_eq!(store.get_task("abc123").unwrap().title, "changed");

        assert!(store.delete_task("abc123").unwrap());
        assert!(!store.delete_task("abc123").unwrap());
        assert!(store.get_task("abc123").is_err());
    }

    #[test]
    fn missing_task_is_not_found() {
        let store = Store::open_memory().unwrap();
        let err = store.get_task("zzzzzz").unwrap_err();
        assert_eq!(err.code(), "not_found");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn persistence_across_reopen() {
        let dir = tempfile_dir();
        let path = dir.join("t.db");
        {
            let store = Store::open(&path).unwrap();
            store.insert_task(&task("keep01", "persist me")).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.all_tasks().unwrap().len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_path_env_override() {
        // Safety: test manipulates the process env; restore afterwards.
        let saved = std::env::var("INTENT_DB").ok();
        std::env::set_var("INTENT_DB", "/tmp/intent-test-override.db");
        assert_eq!(Store::default_path(), PathBuf::from("/tmp/intent-test-override.db"));
        // Empty INTENT_DB is treated as unset (SQLite temp-db hazard, SPEC §2).
        std::env::set_var("INTENT_DB", "");
        let p = Store::default_path();
        assert!(p.file_name().map(|f| f == "tasks.db").unwrap_or(false));
        match saved {
            Some(v) => std::env::set_var("INTENT_DB", v),
            None => std::env::remove_var("INTENT_DB"),
        }
    }

    fn tempfile_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "intent-core-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let d = std::env::temp_dir().join(unique);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn schema_version_refuses_future() {
        let dir = tempfile_dir();
        let path = dir.join("future.db");
        {
            let store = Store::open(&path).unwrap();
            store
                .conn
                .execute("UPDATE meta SET value='99' WHERE key='schema_version'", [])
                .unwrap();
        }
        let err = match Store::open(&path) {
            Err(e) => e,
            Ok(_) => panic!("future schema version should be refused"),
        };
        assert_eq!(err.code(), "bad_value");
        let _ = std::fs::remove_file(&path);
    }
}
