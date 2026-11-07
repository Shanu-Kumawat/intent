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
            // Snoozed tasks leave the triage list; the inbox flag itself is
            // untouched (SPEC §1.2 — defer never organizes).
            TaskView::Inbox => t.status == Status::Active && t.inbox && !t.is_snoozed(today),
            TaskView::Today => t.in_today(today),
            TaskView::Upcoming => {
                t.status == Status::Active
                    && (t.is_snoozed(today) || (!t.inbox && !t.in_today(today)))
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

/// Case-insensitive AND-terms search over id, title, notes, project, and
/// tags (SPEC §4.1 `/` — the id line makes `/abc123` find a task directly).
/// Archived excluded unless asked.
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
                "{}\n{}\n{}\n{}\n{}",
                t.id,
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
            // Filter in place: a --query on `list VIEW` keeps the view's
            // own §1.3 sort order instead of search's newest-activity order.
            let terms: Vec<String> = q.split_whitespace().map(|t| t.to_lowercase()).collect();
            if terms.is_empty() {
                out.clear();
            } else {
                out.retain(|t| {
                    let hay = format!(
                        "{}\n{}\n{}\n{}\n{}",
                        t.id,
                        t.title,
                        t.notes,
                        t.project.as_deref().unwrap_or(""),
                        t.tags.join(" ")
                    )
                    .to_lowercase();
                    terms.iter().all(|term| hay.contains(term))
                });
            }
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

fn enumerate_by(tasks: &[Task], keys: impl Fn(&Task) -> Vec<String>) -> Vec<(String, usize)> {
    // Archived tasks are out of mind (§1.1) — they don't organize anything.
    let mut tally: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in tasks.iter().filter(|t| t.status != Status::Archived) {
        for name in keys(t) {
            *tally.entry(name).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = tally.into_iter().collect();
    // Busiest first, then natural name order — stable and scannable.
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural_cmp(&a.0, &b.0)));
    out
}

/// Natural name order: embedded digit runs compare as numbers
/// (`proj2` before `proj10`), everything else byte-wise. Leading zeros
/// break ties deterministically (`02` before `2`).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut ra, mut rb) = (a, b);
    loop {
        match (ra.is_empty(), rb.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        let (ca, na) = next_chunk(ra);
        let (cb, nb) = next_chunk(rb);
        let ord = if ca.starts_with(|c: char| c.is_ascii_digit()) {
            let za = ca.trim_start_matches('0');
            let zb = cb.trim_start_matches('0');
            za.len()
                .cmp(&zb.len())
                .then_with(|| za.cmp(zb))
                .then_with(|| ca.len().cmp(&cb.len()))
        } else {
            ca.cmp(cb)
        };
        if ord != Ordering::Equal {
            return ord;
        }
        ra = na;
        rb = nb;
    }
}

/// Split off the leading run of digits (or non-digits) from `s`.
fn next_chunk(s: &str) -> (&str, &str) {
    let digits = s.starts_with(|c: char| c.is_ascii_digit());
    let end = s
        .find(|c: char| c.is_ascii_digit() != digits)
        .unwrap_or(s.len());
    s.split_at(end)
}

/// Distinct projects over non-archived tasks, busiest first (sidebar, CLI).
pub fn projects(tasks: &[Task]) -> Vec<(String, usize)> {
    enumerate_by(tasks, |t| t.project.clone().into_iter().collect())
}

/// Distinct tags over non-archived tasks, busiest first (sidebar, CLI).
pub fn tags(tasks: &[Task]) -> Vec<(String, usize)> {
    enumerate_by(tasks, |t| t.tags.clone())
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
    fn snoozed_inbox_task_leaves_triage_and_parks_in_upcoming() {
        let s = Store::open_memory().unwrap();
        let t = ops::add(
            &s,
            NewTask {
                title: "later".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .task
        .unwrap();
        assert!(t.inbox);
        let _ = ops::defer(&s, &t.id, Some(Date(date(2026, 11, 1))));
        let tasks = s.all_tasks().unwrap();
        let today = today();

        let inbox = for_view(&tasks, TaskView::Inbox, today);
        assert!(
            !inbox.iter().any(|x| x.id == t.id),
            "snoozed task hides from the Inbox view"
        );
        let upcoming = for_view(&tasks, TaskView::Upcoming, today);
        assert!(
            upcoming.iter().any(|x| x.id == t.id),
            "snoozed task parks in Upcoming"
        );
        // The data flag is untouched — defer never organizes (§1.2).
        assert!(tasks.iter().find(|x| x.id == t.id).unwrap().inbox);
    }

    #[test]
    fn pure_deferred_task_resurfaces_in_today() {
        let s = Store::open_memory().unwrap();
        let t = ops::add(
            &s,
            NewTask {
                title: "snoozed plain".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .task
        .unwrap();
        let _ = ops::defer(&s, &t.id, Some(Date(date(2026, 11, 1))));
        let tasks = s.all_tasks().unwrap();

        let before = for_view(&tasks, TaskView::Today, today());
        assert!(!before.iter().any(|x| x.id == t.id));
        assert!(for_view(&tasks, TaskView::Inbox, today())
            .iter()
            .all(|x| x.id != t.id));

        let on_date = for_view(&tasks, TaskView::Today, Date(date(2026, 11, 1)));
        assert!(
            on_date.iter().any(|x| x.id == t.id),
            "pure-deferred task lands in Today on the defer date"
        );
        let after = for_view(&tasks, TaskView::Today, Date(date(2026, 11, 9)));
        assert!(after.iter().any(|x| x.id == t.id), "and stays there");
    }

    #[test]
    fn dated_deferred_task_returns_to_today_after_snooze() {
        let s = Store::open_memory().unwrap();
        let t = ops::add(
            &s,
            NewTask {
                title: "overdue but snoozed".into(),
                due: Some(Date(date(2026, 10, 20))),
                ..Default::default()
            },
        )
        .unwrap()
        .task
        .unwrap();
        let _ = ops::defer(&s, &t.id, Some(Date(date(2026, 11, 1))));
        let tasks = s.all_tasks().unwrap();

        assert!(!for_view(&tasks, TaskView::Today, today())
            .iter()
            .any(|x| x.id == t.id));
        assert!(for_view(&tasks, TaskView::Today, Date(date(2026, 11, 1)))
            .iter()
            .any(|x| x.id == t.id));
    }

    #[test]
    fn re_defer_hides_resurfaced_task_again() {
        let s = Store::open_memory().unwrap();
        let t = ops::add(
            &s,
            NewTask {
                title: "not yet".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .task
        .unwrap();
        let _ = ops::defer(&s, &t.id, Some(Date(date(2026, 11, 1))));
        let _ = ops::defer(&s, &t.id, Some(Date(date(2026, 12, 1))));
        let tasks = s.all_tasks().unwrap();
        assert!(!for_view(&tasks, TaskView::Today, Date(date(2026, 11, 1)))
            .iter()
            .any(|x| x.id == t.id));
        assert!(for_view(&tasks, TaskView::Today, Date(date(2026, 12, 1)))
            .iter()
            .any(|x| x.id == t.id));
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
    fn projects_and_tags_enumerate_non_archived_busiest_first() {
        let s = Store::open_memory().unwrap();
        let seed = |n: NewTask| ops::add(&s, n).unwrap().task.unwrap();
        // 2 in "work", 1 in "home"; "trip" tagged twice, "errand" once.
        for (title, project, tgs) in [
            ("a", Some("work"), vec!["trip"]),
            ("b", Some("work"), vec!["trip"]),
            ("c", Some("home"), vec!["errand"]),
        ] {
            let project: Option<String> = project.map(String::from);
            let tgs: Vec<String> = tgs.into_iter().map(String::from).collect();
            let t = seed(NewTask {
                title: title.into(),
                ..Default::default()
            });
            let _ = ops::edit(
                &s,
                &t.id,
                &crate::model::TaskPatch {
                    project: project.map(crate::model::Patch::Set).unwrap_or_default(),
                    tags: crate::model::Patch::Set(tgs),
                    ..Default::default()
                },
            );
        }
        let gone = seed(NewTask {
            title: "gone".into(),
            ..Default::default()
        });
        let _ = ops::edit(
            &s,
            &gone.id,
            &crate::model::TaskPatch {
                project: crate::model::Patch::Set("work".into()),
                tags: crate::model::Patch::Set(vec!["trip".into()]),
                ..Default::default()
            },
        );
        let _ = ops::archive(&s, &gone.id);
        let tasks = s.all_tasks().unwrap();

        assert_eq!(
            projects(&tasks),
            vec![("work".into(), 2), ("home".into(), 1)]
        );
        assert_eq!(
            tags(&tasks),
            vec![("trip".into(), 2), ("errand".into(), 1)],
            "archived tasks don't tally"
        );
    }

    #[test]
    fn search_finds_tasks_by_id() {
        let tasks = setup();
        let target = &tasks[0];
        let hits = search(&tasks, &target.id, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, target.id);
    }

    #[test]
    fn natural_name_order_digits_run_numerically() {
        use std::cmp::Ordering;
        assert_eq!(natural_cmp("proj2", "proj10"), Ordering::Less);
        assert_eq!(natural_cmp("proj10", "proj2"), Ordering::Greater);
        assert_eq!(natural_cmp("proj1", "proj1"), Ordering::Equal);
        assert_eq!(
            natural_cmp("2", "02"),
            Ordering::Less,
            "equal numbers tie-break by length (fewer chars first)"
        );
        assert_eq!(natural_cmp("a2b", "a2c"), Ordering::Less);
        assert_eq!(natural_cmp("сеть", "日本語"), Ordering::Less);
        let names = ["proj10", "proj1", "proj2"];
        let mut v = names.to_vec();
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["proj1", "proj2", "proj10"]);
    }

    #[test]
    fn enumeration_sorts_names_naturally() {
        let s = Store::open_memory().unwrap();
        for title in ["p10", "p2", "p1"] {
            let t = ops::add(
                &s,
                NewTask {
                    title: title.into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .task
            .unwrap();
            let _ = ops::edit(
                &s,
                &t.id,
                &crate::model::TaskPatch {
                    project: crate::model::Patch::Set(format!("proj{title}")),
                    ..Default::default()
                },
            );
        }
        let names: Vec<String> = projects(&s.all_tasks().unwrap())
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            names,
            vec!["projp1", "projp2", "projp10"],
            "proj10 must not precede proj2"
        );
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
