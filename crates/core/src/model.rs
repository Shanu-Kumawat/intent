use crate::dates::Date;
use crate::error::{bad_value, Result};
use jiff::Timestamp;

/// Lifecycle status (SPEC §1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Status {
    Active,
    Done,
    Archived,
}

impl Status {
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "active" => Status::Active,
            "done" | "completed" => Status::Done,
            "archived" => Status::Archived,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Done => "done",
            Status::Archived => "archived",
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Status {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Status {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Status::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown status {s:?}")))
    }
}

/// Priority (SPEC §1). Ordered low → high for comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Priority> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "none" | "-" | "0" => Priority::None,
            "low" | "l" | "1" => Priority::Low,
            "medium" | "med" | "m" | "2" => Priority::Medium,
            "high" | "hi" | "h" | "3" => Priority::High,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::None => "none",
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
        }
    }

    pub fn raise(self) -> Priority {
        match self {
            Priority::None => Priority::Low,
            Priority::Low => Priority::Medium,
            Priority::Medium | Priority::High => Priority::High,
        }
    }

    pub fn lower(self) -> Priority {
        match self {
            Priority::High => Priority::Medium,
            Priority::Medium => Priority::Low,
            Priority::None | Priority::Low => Priority::None,
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Priority {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Priority {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Priority::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown priority {s:?}")))
    }
}

/// Task views (SPEC §1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TaskView {
    Inbox,
    Today,
    Upcoming,
    Completed,
    Archived,
    All,
}

pub const ALL_VIEWS: [TaskView; 6] = [
    TaskView::Inbox,
    TaskView::Today,
    TaskView::Upcoming,
    TaskView::Completed,
    TaskView::Archived,
    TaskView::All,
];

impl TaskView {
    pub fn parse(s: &str) -> Option<TaskView> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "inbox" => TaskView::Inbox,
            "today" => TaskView::Today,
            "upcoming" => TaskView::Upcoming,
            "completed" | "done" => TaskView::Completed,
            "archived" | "archive" => TaskView::Archived,
            "all" => TaskView::All,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TaskView::Inbox => "inbox",
            TaskView::Today => "today",
            TaskView::Upcoming => "upcoming",
            TaskView::Completed => "completed",
            TaskView::Archived => "archived",
            TaskView::All => "all",
        }
    }
}

impl std::fmt::Display for TaskView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A task. Field set is the stable JSON contract (SPEC §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub status: Status,
    pub inbox: bool,
    pub priority: Priority,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub due: Option<Date>,
    pub remind: Option<Date>,
    pub defer_until: Option<Date>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

impl serde::Serialize for Task {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut o = s.serialize_struct("Task", 14)?;
        o.serialize_field("id", &self.id)?;
        o.serialize_field("title", &self.title)?;
        o.serialize_field("notes", &self.notes)?;
        o.serialize_field("status", &self.status)?;
        o.serialize_field("inbox", &self.inbox)?;
        o.serialize_field("priority", &self.priority)?;
        o.serialize_field("project", &self.project)?;
        o.serialize_field("tags", &self.tags)?;
        o.serialize_field("due", &self.due)?;
        o.serialize_field("remind", &self.remind)?;
        o.serialize_field("defer_until", &self.defer_until)?;
        o.serialize_field("created_at", &self.created_at)?;
        o.serialize_field("updated_at", &self.updated_at)?;
        o.serialize_field("completed_at", &self.completed_at)?;
        o.end()
    }
}

impl Task {
    pub fn is_due_or_reminding(&self, today: Date) -> bool {
        if self.status != Status::Active {
            return false;
        }
        if let Some(d) = self.defer_until {
            if d > today {
                return false;
            }
        }
        let due_hit = self.due.map(|d| d <= today).unwrap_or(false);
        let remind_hit = self.remind.map(|r| r <= today).unwrap_or(false);
        due_hit || remind_hit
    }
}

/// Field patch used by `edit` and the GUI. `Keep` = don't touch,
/// `Set(v)` = assign, `Clear` = remove (only valid on optional fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Patch<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

impl<T> Patch<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Patch::Keep)
    }
}

/// Every editable field, patch-style. Title/notes/priority cannot be cleared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskPatch {
    pub title: Patch<String>,
    pub notes: Patch<String>,
    pub priority: Patch<Priority>,
    pub project: Patch<String>,
    pub tags: Patch<Vec<String>>,
    pub due: Patch<Date>,
    pub remind: Patch<Date>,
    pub defer_until: Patch<Date>,
    /// Explicit organize: set inbox=false regardless of other fields.
    pub organize: bool,
}

impl TaskPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_keep()
            && self.notes.is_keep()
            && self.priority.is_keep()
            && self.project.is_keep()
            && self.tags.is_keep()
            && self.due.is_keep()
            && self.remind.is_keep()
            && self.defer_until.is_keep()
            && !self.organize
    }

    /// Per SPEC §1.2, these assignments take a task out of the inbox.
    fn leaves_inbox(&self) -> bool {
        matches!(self.project, Patch::Set(_))
            || matches!(self.due, Patch::Set(_))
            || matches!(self.remind, Patch::Set(_))
            || self.organize
    }

    /// Apply the patch in place. Returns true if anything changed.
    pub fn apply(&self, task: &mut Task) -> bool {
        let before = task.clone();
        if let Patch::Set(t) = &self.title {
            task.title = t.clone()
        }
        match &self.notes {
            Patch::Set(n) => task.notes = n.clone(),
            Patch::Clear => task.notes = String::new(),
            Patch::Keep => {}
        }
        if let Patch::Set(p) = self.priority {
            task.priority = p
        }
        match &self.project {
            Patch::Set(p) if p.is_empty() => task.project = None,
            Patch::Set(p) => task.project = Some(p.clone()),
            Patch::Clear => task.project = None,
            Patch::Keep => {}
        }
        if let Patch::Set(t) = &self.tags {
            task.tags = t.clone()
        }
        match self.due {
            Patch::Set(d) => task.due = Some(d),
            Patch::Clear => task.due = None,
            Patch::Keep => {}
        }
        match self.remind {
            Patch::Set(d) => task.remind = Some(d),
            Patch::Clear => task.remind = None,
            Patch::Keep => {}
        }
        match self.defer_until {
            Patch::Set(d) => task.defer_until = Some(d),
            Patch::Clear => task.defer_until = None,
            Patch::Keep => {}
        }
        if self.leaves_inbox() {
            task.inbox = false;
        }
        before != *task
    }
}

/// Options for creating a task.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub title: String,
    pub notes: String,
    pub priority: Priority,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub due: Option<Date>,
    pub remind: Option<Date>,
    /// Force inbox=false at creation (CLI `--no-inbox`).
    pub no_inbox: bool,
}

impl NewTask {
    pub fn inbox(&self) -> bool {
        // Consistent with SPEC §1.2: assigning project/due/remind at capture
        // time implies the task is already organized.
        !self.no_inbox && self.project.is_none() && self.due.is_none() && self.remind.is_none()
    }

    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            return Err(bad_value("title must not be empty"));
        }
        if self.title.contains(['\n', '\r']) {
            return Err(bad_value("title must be a single line"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parse() {
        assert_eq!(Status::parse("active"), Some(Status::Active));
        assert_eq!(Status::parse("DONE"), Some(Status::Done));
        assert_eq!(Status::parse("completed"), Some(Status::Done));
        assert_eq!(Status::parse("nope"), None);
    }

    #[test]
    fn priority_parse_and_cycle() {
        assert_eq!(Priority::parse("h"), Some(Priority::High));
        assert_eq!(Priority::parse("none"), Some(Priority::None));
        assert_eq!(Priority::parse("-"), Some(Priority::None));
        assert_eq!(Priority::parse("x"), None);
        assert!(Priority::None < Priority::High);
        assert_eq!(Priority::None.raise(), Priority::Low);
        assert_eq!(Priority::High.raise(), Priority::High);
        assert_eq!(Priority::None.lower(), Priority::None);
    }

    #[test]
    fn patch_inbox_rules() {
        let mut t = Task {
            id: "aaa".into(),
            title: "t".into(),
            notes: String::new(),
            status: Status::Active,
            inbox: true,
            priority: Priority::None,
            project: None,
            tags: vec![],
            due: None,
            remind: None,
            defer_until: None,
            created_at: jiff::Timestamp::now(),
            updated_at: jiff::Timestamp::now(),
            completed_at: None,
        };

        let p = TaskPatch {
            priority: Patch::Set(Priority::High),
            ..Default::default()
        };
        p.apply(&mut t);
        assert!(t.inbox, "priority alone must not clear inbox");

        let p = TaskPatch {
            due: Patch::Set(crate::dates::Date(jiff::civil::date(2026, 11, 2))),
            ..Default::default()
        };
        p.apply(&mut t);
        assert!(!t.inbox, "setting due clears inbox");
        assert_eq!(t.due.map(|d| d.to_string()), Some("2026-11-02".into()));

        let p = TaskPatch {
            project: Patch::Set(String::new()),
            ..Default::default()
        };
        p.apply(&mut t);
        assert_eq!(t.project, None, "empty project sets None");
    }

    #[test]
    fn new_task_inbox_rules() {
        let n = NewTask {
            title: "x".into(),
            ..Default::default()
        };
        assert!(n.inbox());
        let n = NewTask {
            title: "x".into(),
            project: Some("work".into()),
            ..Default::default()
        };
        assert!(!n.inbox());
        let n = NewTask {
            title: "x".into(),
            no_inbox: true,
            ..Default::default()
        };
        assert!(!n.inbox());
    }
}
