//! Job sources that tie a job to an owner task.
//!
//! A lead queues the first job for a task with `--source task:TASK-NNN`;
//! `pulse job add` keeps one open job per source, so the lead gives every
//! further job of the same task a label: `task:TASK-NNN:hub-ui`. Everything
//! that asks "which jobs belong to this task" must accept both shapes.

pub const TASK_SOURCE_PREFIX: &str = "task:";

/// The task id a job source names, for `task:TASK-NNN` and `task:TASK-NNN:label`.
pub fn task_source_id(source: &str) -> Option<&str> {
    let rest = source.strip_prefix(TASK_SOURCE_PREFIX)?;
    let id = rest.split(':').next().unwrap_or_default();
    (!id.is_empty()).then_some(id)
}

/// The two SQL parameters that match every job source of one task when the
/// clause is `(source = ? OR source LIKE ? ESCAPE '\')`.
pub fn task_source_params(id: &str) -> (String, String) {
    let exact = format!("{TASK_SOURCE_PREFIX}{id}");
    let escaped = exact
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    (exact, format!("{escaped}:%"))
}

/// The clause [`task_source_params`] feeds, over `col`.
pub fn task_source_clause(col: &str) -> String {
    format!("({col} = ? OR {col} LIKE ? ESCAPE '\\')")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_names_its_task_with_or_without_a_label() {
        assert_eq!(task_source_id("task:TASK-022"), Some("TASK-022"));
        assert_eq!(task_source_id("task:TASK-022:hub-ui"), Some("TASK-022"));
        assert_eq!(task_source_id("task:"), None);
        assert_eq!(task_source_id("task::x"), None);
        assert_eq!(task_source_id("schedule:SCHED-001"), None);
    }

    #[test]
    fn the_clause_matches_both_shapes_and_nothing_else() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE j (source TEXT);
             INSERT INTO j VALUES ('task:TASK-022'), ('task:TASK-022:hub-ui'),
                ('task:TASK-0221'), ('task:TASK-02'), ('suggestion:TASK-022');",
        )
        .unwrap();
        let (exact, like) = task_source_params("TASK-022");
        let n: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM j WHERE {}",
                    task_source_clause("source")
                ),
                [&exact, &like],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }
}
