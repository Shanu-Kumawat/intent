use crate::dates::Date;
use crate::error::Result;
use crate::model::{Status, Task, TaskView};
use crate::store::Store;

/// Compute one of the named views (SPEC §1.3). Pure: takes all tasks,
/// returns the filtered and ordered subset.
pub fn for_view(tasks: &[Task], view: TaskView, today: Date) -> Vec<Task> {
    let mut out: Vec<Task> = tasks
        .iter()
        .filter(|t| match view {
            TaskView::Inbox => t.status == Status::Active && t.inbox,
            TaskView::Today => t.is_due_or_reminding(today),
            TaskView::Upcoming => {
                t.status == Status::Active && !t.inbox && !t.is_due_or_reminding(today)
            }
            TaskView::Completed => t.status == Status::Done,
            TaskView::Archived => t.status == Status::Archived,
            TaskView::All => t.status != Status::Archived,
        })
        .cloned()
        .collect();
    match view {
        TaskView::Inbox => out.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.id.cmp(&b.id))
        }),
        TaskView::Today => out.sort_by(|a, b| {
            due_key(a)
                .cmp(&due_key(b))
                .then_with(|| b.priority.cmp(&a.priority))
                .then_with(|| a.created_at.cmp(&b.created_at))
        }),
        TaskView::Upcoming => out.sort_by(|a, b| {
            upcoming_key(a, today)
                .cmp(&upcoming_key(b, today))
                .then_with(|| b.priority.cmp(&a.priority))
                .then_with(|| a.created_at.cmp(&b.created_at))
        }),
        TaskView::Completed => out.sort_by(|a, b| {
            b.completed_at
                .cmp(&a.completed_at)
                .then_with(|| a.id.cmp(&b.id))
        }),
        TaskView::Archived | TaskView::All => out.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        }),
    }
    out
}

fn due_key(t: &Task) -> (u8, Date) {
    // Dated first (0), undated last (1); date ascending.
    match t.due {
        Some(d) => (0, d),
        None => (1, Date(jiff::civil::date(9999, 12, 31))),
    }
}

fn upcoming_key(t: &Task, _today: Date) -> (u8, Date, u8) {
    // Earliest of due/remind/defers ascending; undated ("someday") last.
    let earliest = [t.due, t.remind, t.defer_until].into_iter().flatten().min();
    match earliest {
        Some(d) => (0, d, 0),
        None => (1, Date(jiff::civil::date(9999, 12, 31)), 0),
    }
}

/// Case-insensitive AND-terms search over title, notes, project, and tags
/// (SPEC §4.1 `/`). Archived excluded unless asked.
pub fn search(tasks: &[Task], query: &str, include_archived: bool) -> Vec<Task> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Task> = tasks
        .iter()
        .filter(|t| include_archived || t.status != Status::Archived)
        .filter(|t| {
            let hay = format!(
                "{}\n{}\n{}\n{}",
                t.title,
                t.notes,
                t.project.as_deref().unwrap_or(""),
                t.tags.join(" ")
            )
            .to_lowercase();
            terms.iter().all(|term| hay.contains(term))
        })
        .cloned()
        .collect();
    // Deterministic order — same as the `all` view (newest activity first);
    // searching must not inherit all_tasks' insertion order.
    hits.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    hits
}

/// Filters available on `list` beyond the view itself.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub project: Option<String>,
    pub tag: Option<String>,
    pub priority: Option<crate::model::Priority>,
    pub query: Option<String>,
    pub limit: Option<usize>,
}

impl ListFilter {
    pub fn apply(&self, tasks: Vec<Task>) -> Vec<Task> {
        let mut out = tasks;
        if let Some(p) = &self.project {
            out.retain(|t| t.project.as_deref() == Some(p.as_str()));
        }
        if let Some(tag) = &self.tag {
            out.retain(|t| t.tags.iter().any(|x| x == tag));
        }
        if let Some(pri) = self.priority {
            out.retain(|t| t.priority == pri);
        }
        if let Some(q) = &self.query {
            out = search(&out, q, true);
        }
        if let Some(n) = self.limit {
            out.truncate(n);
        }
        out
    }
}

/// Count per view, for the sidebar and `intent counts`.
pub fn counts(tasks: &[Task], today: Date) -> Vec<(TaskView, usize)> {
    crate::model::ALL_VIEWS
        .iter()
        .map(|v| (*v, for_view(tasks, *v, today).len()))
        .collect()
}

/// Load all tasks and compute a view.
pub fn load_view(store: &Store, view: TaskView, today: Date) -> Result<Vec<Task>> {
    Ok(for_view(&store.all_tasks()?, view, today))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewTask, Priority};
    use crate::ops;
    use jiff::civil::date;

    fn today() -> Date {
        Date(date(2026, 10, 26))
    }

    fn setup() -> Vec<Task> {
        let s = Store::open_memory().unwrap();
        let seed = |n: NewTask| ops::add(&s, n).unwrap().task.unwrap();

        let inbox_task = seed(NewTask {
            title: "fresh capture".into(),
            ..Default::default()
        });
        assert!(inbox_task.inbox);

        let overdue = seed(NewTask {
            title: "overdue".into(),
            due: Some(Date(date(2026, 10, 20))),
            ..Default::default()
        });
        let _ = overdue;

        let upcoming = seed(NewTask {
            title: "future".into(),
            due: Some(Date(date(2026, 11, 15))),
            ..Default::default()
        });
        let _ = upcoming;

        let remind_today = seed(NewTask {
            title: "resurface".into(),
            remind: Some(Date(date(2026, 10, 26))),
            ..Default::default()
        });
        let _ = remind_today;

        let someday = seed(NewTask {
            title: "someday".into(),
            no_inbox: true,
            ..Default::default()
        });
        let _ = someday;

        let mut deferred = seed(NewTask {
            title: "deferred overdue".into(),
            due: Some(Date(date(2026, 10, 20))),
            ..Default::default()
        });
        deferred.defer_until = Some(Date(date(2026, 11, 1)));
        // write through store to keep single source
        let _ = ops::defer(&s, &deferred.id, Some(Date(date(2026, 11, 1))));

        let done_task = seed(NewTask {
            title: "finished".into(),
            ..Default::default()
        });
        let _ = ops::done(&s, &done_task.id);

        let archived_task = seed(NewTask {
            title: "gone".into(),
            ..Default::default()
        });
        let _ = ops::archive(&s, &archived_task.id);

        let high = seed(NewTask {
            title: "high undated".into(),
            no_inbox: true,
            ..Default::default()
        });
        let p = crate::model::TaskPatch {
            priority: crate::model::Patch::Set(Priority::High),
            ..Default::default()
        };
        let _ = ops::edit(&s, &high.id, &p);

        s.all_tasks().unwrap()
    }

    #[test]
    fn view_classification() {
        let tasks = setup();
        let t = today();

        let inbox = for_view(&tasks, TaskView::Inbox, t);
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].title, "fresh capture");

        let today_v = for_view(&tasks, TaskView::Today, t);
        let titles: Vec<&str> = today_v.iter().map(|x| x.title.as_str()).collect();
        assert!(
            titles.contains(&"overdue"),
            "overdue due date pulls into today: {titles:?}"
        );
        assert!(
            titles.contains(&"resurface"),
            "reminder hit pulls into today: {titles:?}"
        );
        assert!(
            !titles.contains(&"deferred overdue"),
            "deferred task hidden until defer_until"
        );
        assert!(!titles.contains(&"future"));

        let upcoming = for_view(&tasks, TaskView::Upcoming, t);
        let titles: Vec<&str> = upcoming.iter().map(|x| x.title.as_str()).collect();
        assert!(titles.contains(&"future"));
        assert!(titles.contains(&"deferred overdue"));
        assert!(titles.contains(&"someday"));
        assert!(titles.contains(&"high undated"));
        assert!(!titles.contains(&"fresh capture"));

        let completed = for_view(&tasks, TaskView::Completed, t);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].title, "finished");

        let archived = for_view(&tasks, TaskView::Archived, t);
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].title, "gone");

        let all = for_view(&tasks, TaskView::All, t);
        assert_eq!(all.len(), tasks.len() - 1, "all excludes only archived");
    }

    #[test]
    fn today_sort_dated_first_then_priority() {
        let tasks = setup();
        let t = today();
        let today_v = for_view(&tasks, TaskView::Today, t);
        let titles: Vec<&str> = today_v.iter().map(|x| x.title.as_str()).collect();
        // 'overdue' has a due date; 'resurface' is dated by remind only.
        // Both are dated; overdue (10-20) sorts before resurface (10-26).
        assert_eq!(titles, vec!["overdue", "resurface"]);
    }

    #[test]
    fn search_terms_and_case() {
        let tasks = setup();
        // "overdue" matches both the plain overdue task and "deferred overdue".
        let hits = search(&tasks, "OVERDUE", false);
        assert_eq!(hits.len(), 2);
        let hits = search(&tasks, "deferred overdue", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "deferred overdue");
        let hits = search(&tasks, "deferred nothing", false);
        assert!(hits.is_empty());
        let hits = search(&tasks, "gone", false);
        assert!(hits.is_empty(), "archived excluded by default");
        let hits = search(&tasks, "gone", true);
        assert_eq!(hits.len(), 1);
        assert!(
            search(&tasks, "   ", false).is_empty(),
            "blank query matches nothing"
        );
    }

    #[test]
    fn filter_chain() {
        let tasks = setup();
        let base = for_view(&tasks, TaskView::All, today());
        let f = ListFilter {
            priority: Some(Priority::High),
            ..Default::default()
        }
        .apply(base);
        assert!(f.iter().all(|t| t.priority == Priority::High));
        assert!(f.iter().any(|t| t.title == "high undated"));
        let f = ListFilter {
            limit: Some(2),
            ..Default::default()
        }
        .apply(for_view(&tasks, TaskView::All, today()));
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn counts_shape() {
        let tasks = setup();
        let c = counts(&tasks, today());
        let map: std::collections::HashMap<_, _> = c.into_iter().collect();
        assert_eq!(map[&TaskView::Inbox], 1);
        assert_eq!(map[&TaskView::Today], 2);
        assert!(map[&TaskView::Upcoming] >= 4);
    }
}
