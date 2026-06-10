use std::str::FromStr;

use crate::models::{Lock, Status, Task, TaskStats};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub fn open_db() -> rusqlite::Result<Connection> {
    let conn = Connection::open("tasks.db")?;
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
            id          TEXT PRIMARY KEY,
            num         INTEGER UNIQUE NOT NULL,
            title       TEXT NOT NULL,
            description TEXT,
            status      TEXT NOT NULL DEFAULT 'open',
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            parent_id   TEXT REFERENCES tasks(id)
        );
        CREATE TABLE IF NOT EXISTS locks (
            task_id     TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
            holder      TEXT NOT NULL,
            acquired_at TEXT NOT NULL,
            expires_at  TEXT NOT NULL
        );",
    )?;
    // Add parent_id to databases created before this column existed.
    let col_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'parent_id'",
        [],
        |r| r.get(0),
    )?;
    if !col_exists {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN parent_id TEXT REFERENCES tasks(id)")?;
    }
    // Add summary to databases created before this column existed.
    let summary_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'summary'",
        [],
        |r| r.get(0),
    )?;
    if !summary_exists {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN summary TEXT")?;
    }
    Ok(())
}

pub fn slugify(title: &str) -> String {
    let slug = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.len() <= 40 {
        return slug;
    }

    // The slug is ASCII-only (all chars are alphanumeric ASCII or '-'), so byte
    // indexing is safe and equivalent to char indexing.
    if slug.as_bytes()[40] == b'-' {
        // Cut lands exactly at a word boundary — keep all 40 chars.
        slug[..40].to_string()
    } else {
        // Cut lands mid-word — back up to the last hyphen.
        match slug[..40].rfind('-') {
            Some(pos) => slug[..pos].to_string(),
            // Single word longer than 40 chars: hard-truncate with no hyphen to
            // back up to.
            None => slug[..40].to_string(),
        }
    }
}

pub fn create_task(
    conn: &Connection,
    slug: &str,
    title: &str,
    description: Option<&str>,
    parent_id: Option<&str>,
) -> rusqlite::Result<String> {
    let now = Utc::now().to_rfc3339();
    // Single statement: SELECT MAX(num)+1 and INSERT are atomic, eliminating TOCTOU.
    conn.execute(
        "INSERT INTO tasks (id, num, title, description, status, created_at, updated_at, parent_id, summary)
         SELECT printf('%04d-%s', n, ?1), n, ?2, ?3, 'open', ?4, ?4, ?5, NULL
         FROM (SELECT COALESCE(MAX(num), 0) + 1 AS n FROM tasks)",
        params![slug, title, description, now, parent_id],
    )?;
    conn.query_row(
        "SELECT id FROM tasks WHERE rowid = ?1",
        [conn.last_insert_rowid()],
        |r| r.get(0),
    )
}

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    let status_str: String = row.get(4)?;
    let status = Status::from_str(&status_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into())
    })?;
    let created_at: DateTime<Utc> =
        row.get::<_, String>(5)?
            .parse()
            .map_err(|e: chrono::ParseError| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
    let updated_at: DateTime<Utc> =
        row.get::<_, String>(6)?
            .parse()
            .map_err(|e: chrono::ParseError| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
    let lock_expires: Option<DateTime<Utc>> = row
        .get::<_, Option<String>>(8)?
        .and_then(|s| s.parse().ok());
    Ok(Task {
        id: row.get(0)?,
        num: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        summary: row.get(10)?,
        status,
        created_at,
        updated_at,
        locked_by: row.get(7)?,
        lock_expires,
        parent_id: row.get(9)?,
    })
}

pub fn list_tasks(
    conn: &Connection,
    status_filter: Option<&Status>,
    parent_filter: Option<&str>,
    show_all: bool,
) -> rusqlite::Result<Vec<Task>> {
    let now = Utc::now().to_rfc3339();
    // When show_all is true  → no status restriction.
    // When status_filter is Some(s) → restrict to that exact status.
    // Otherwise (default)   → show only open and in_progress tasks.
    let sql = "SELECT t.id, t.num, t.title, t.description, t.status,
                      t.created_at, t.updated_at,
                      CASE WHEN l.expires_at > ?1 THEN l.holder END,
                      CASE WHEN l.expires_at > ?1 THEN l.expires_at END,
                      t.parent_id, t.summary
               FROM tasks t
               LEFT JOIN locks l ON l.task_id = t.id
               WHERE (
                   ?4 = 1
                   OR (?2 IS NOT NULL AND t.status = ?2)
                   OR (?2 IS NULL AND t.status IN ('open', 'in_progress'))
               )
                 AND (?3 IS NULL OR t.parent_id = ?3)
               ORDER BY t.num ASC";
    let status_val = status_filter.map(ToString::to_string);
    let show_all_int: i32 = i32::from(show_all);
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        params![now, status_val, parent_filter, show_all_int],
        row_to_task,
    )?;
    rows.collect()
}

pub fn get_task(conn: &Connection, id: &str) -> rusqlite::Result<Option<Task>> {
    let now = Utc::now().to_rfc3339();
    let sql = "SELECT t.id, t.num, t.title, t.description, t.status,
                      t.created_at, t.updated_at,
                      CASE WHEN l.expires_at > ?1 THEN l.holder END,
                      CASE WHEN l.expires_at > ?1 THEN l.expires_at END,
                      t.parent_id, t.summary
               FROM tasks t
               LEFT JOIN locks l ON l.task_id = t.id
               WHERE t.id = ?2";
    conn.query_row(sql, params![now, id], row_to_task)
        .optional()
}

pub fn update_task(
    conn: &Connection,
    id: &str,
    title: Option<&str>,
    description: Option<&str>,
    status: Option<&Status>,
    summary: Option<&str>,
) -> rusqlite::Result<bool> {
    let now = Utc::now().to_rfc3339();
    let rows = conn.execute(
        "UPDATE tasks SET
            title       = COALESCE(?1, title),
            description = COALESCE(?2, description),
            status      = COALESCE(?3, status),
            summary     = COALESCE(?4, summary),
            updated_at  = ?5
         WHERE id = ?6",
        params![
            title,
            description,
            status.map(ToString::to_string),
            summary,
            now,
            id
        ],
    )?;
    Ok(rows > 0)
}

pub fn delete_task(conn: &Connection, id: &str, force: bool) -> anyhow::Result<bool> {
    if !force {
        let now = Utc::now().to_rfc3339();
        let locked: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM locks WHERE task_id = ?1 AND expires_at > ?2",
            params![id, now],
            |r| r.get(0),
        )?;
        if locked {
            anyhow::bail!("task {id} is locked; use --force to delete anyway");
        }
        let status: Option<String> = conn
            .query_row("SELECT status FROM tasks WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        if status.as_deref() == Some("in_progress") {
            anyhow::bail!("task {id} is in_progress; use --force to delete anyway");
        }
    }
    let rows = conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
    Ok(rows > 0)
}

/// Resolve a partial (or full) task ID to an exact ID.
///
/// - If `partial` matches an existing ID exactly, return it as-is.
/// - Otherwise, look for all IDs containing `partial` as a substring
///   (`LIKE '%partial%'`).
///   - Exactly one match → return it.
///   - Zero matches → error "no task found matching '…'".
///   - Multiple matches → error "ambiguous ID '…' matches: …".
pub fn resolve_id(conn: &Connection, partial: &str) -> anyhow::Result<String> {
    // Fast path: exact match.
    let exact: Option<String> = conn
        .query_row("SELECT id FROM tasks WHERE id = ?1", [partial], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(id) = exact {
        return Ok(id);
    }

    // Substring match.
    let pattern = format!("%{partial}%");
    let mut stmt = conn.prepare("SELECT id FROM tasks WHERE id LIKE ?1 ORDER BY id")?;
    let matches: Vec<String> = stmt
        .query_map([&pattern], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    match matches.len() {
        0 => anyhow::bail!("no task found matching '{partial}'"),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => anyhow::bail!("ambiguous ID '{partial}' matches: {}", matches.join(", ")),
    }
}

pub fn acquire_task(conn: &Connection, id: &str, holder: &str, ttl: u64) -> anyhow::Result<()> {
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    let expires_str = (now + chrono::Duration::seconds(ttl.cast_signed())).to_rfc3339();

    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
        [id],
        |r| r.get(0),
    )?;
    if !exists {
        anyhow::bail!("task not found: {id}");
    }

    // WHERE NOT EXISTS makes the check-and-insert atomic: 0 rows inserted means
    // a different holder holds an active lock.
    // ?1=task_id  ?2=holder  ?3=acquired_at(now)  ?4=expires_at
    let rows = conn.execute(
        "INSERT OR REPLACE INTO locks (task_id, holder, acquired_at, expires_at)
         SELECT ?1, ?2, ?3, ?4
         WHERE NOT EXISTS (
             SELECT 1 FROM locks
             WHERE task_id = ?1 AND expires_at > ?3 AND holder != ?2
         )",
        params![id, holder, now_str, expires_str],
    )?;

    if rows == 0 {
        let lock: Lock = conn.query_row(
            "SELECT task_id, holder, acquired_at, expires_at
             FROM locks WHERE task_id = ?1 AND expires_at > ?2",
            params![id, now_str],
            |r| {
                Ok(Lock {
                    task_id: r.get(0)?,
                    holder: r.get(1)?,
                    acquired_at: r
                        .get::<_, String>(2)?
                        .parse()
                        .expect("acquired_at in locks table is not valid RFC3339"),
                    expires_at: r
                        .get::<_, String>(3)?
                        .parse()
                        .expect("expires_at in locks table is not valid RFC3339"),
                })
            },
        )?;
        anyhow::bail!(
            "task {id} is already held by '{}' until {}",
            lock.holder,
            lock.expires_at.format("%Y-%m-%dT%H:%M:%SZ")
        );
    }

    conn.execute(
        "UPDATE tasks SET status = 'in_progress', updated_at = ?1 WHERE id = ?2",
        params![now_str, id],
    )?;
    Ok(())
}

pub fn release_task(
    conn: &Connection,
    id: &str,
    holder: &str,
    force: bool,
) -> anyhow::Result<bool> {
    let now = Utc::now().to_rfc3339();
    if !force {
        let existing_holder: Option<String> = conn
            .query_row(
                "SELECT holder FROM locks WHERE task_id = ?1 AND expires_at > ?2",
                params![id, now],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(h) = existing_holder
            && h != holder
        {
            anyhow::bail!("task {id} is held by '{h}'; use --force to release anyway");
        }
    }
    let rows = conn.execute("DELETE FROM locks WHERE task_id = ?1", [id])?;
    Ok(rows > 0)
}

pub fn renew_task(conn: &Connection, id: &str, holder: &str, ttl: u64) -> anyhow::Result<()> {
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    let expires_str = (now + chrono::Duration::seconds(ttl.cast_signed())).to_rfc3339();

    // Single UPDATE with all conditions makes the holder check atomic with the update.
    let rows = conn.execute(
        "UPDATE locks SET expires_at = ?1
         WHERE task_id = ?2 AND holder = ?3 AND expires_at > ?4",
        params![expires_str, id, holder, now_str],
    )?;

    if rows == 0 {
        let existing_holder: Option<String> = conn
            .query_row(
                "SELECT holder FROM locks WHERE task_id = ?1 AND expires_at > ?2",
                params![id, now_str],
                |r| r.get(0),
            )
            .optional()?;
        match existing_holder {
            None => anyhow::bail!("task {id} has no active lock to renew"),
            Some(h) => anyhow::bail!("task {id} is held by '{h}', not '{holder}'"),
        }
    }

    Ok(())
}

pub fn stats(conn: &Connection) -> anyhow::Result<TaskStats> {
    let now = Utc::now().to_rfc3339();

    // Single pass for status counts, completion rate, abandonment proxy, and
    // child-task usage.  The abandonment heuristic flags open tasks whose
    // updated_at is more than 5 seconds after created_at — a sign the task was
    // returned to the queue after an agent touched it.
    let (
        total,
        open,
        in_progress,
        done,
        cancelled,
        completion_pct,
        likely_abandoned,
        child_tasks,
        parent_tasks,
    ) = conn.query_row(
        "SELECT
               COUNT(*)                                                        AS total,
               COUNT(*) FILTER (WHERE status = 'open')                        AS open_count,
               COUNT(*) FILTER (WHERE status = 'in_progress')                 AS in_progress_count,
               COUNT(*) FILTER (WHERE status = 'done')                        AS done_count,
               COUNT(*) FILTER (WHERE status = 'cancelled')                   AS cancelled_count,
               ROUND(
                 100.0 * COUNT(*) FILTER (WHERE status = 'done')
                 / NULLIF(COUNT(*) FILTER (WHERE status != 'cancelled'), 0),
                 1
               )                                                               AS completion_pct,
               COUNT(*) FILTER (
                 WHERE status = 'open'
                   AND (strftime('%s', updated_at) - strftime('%s', created_at)) > 5
               )                                                               AS likely_abandoned,
               COUNT(*) FILTER (WHERE parent_id IS NOT NULL)                  AS child_tasks,
               COUNT(DISTINCT parent_id) FILTER (WHERE parent_id IS NOT NULL) AS parent_tasks
             FROM tasks",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        },
    )?;

    // Stall count: in_progress tasks whose lock has expired or has no lock row.
    let stalled: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM tasks t
         LEFT JOIN locks l ON l.task_id = t.id
         WHERE t.status = 'in_progress'
           AND (l.expires_at IS NULL OR l.expires_at < ?1)",
        params![now],
        |r| r.get(0),
    )?;

    Ok(TaskStats {
        total,
        open,
        in_progress,
        done,
        cancelled,
        completion_pct,
        likely_abandoned,
        stalled,
        child_tasks,
        parent_tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_task, create_task, delete_task, get_task, list_tasks, migrate, release_task,
        renew_task, resolve_id, slugify, update_task,
    };
    use crate::models::Status;
    use chrono::Utc;
    use rstest::{fixture, rstest};
    use rusqlite::Connection;

    #[fixture]
    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        migrate(&c).unwrap();
        c
    }

    fn insert_expired_lock(conn: &Connection, task_id: &str, holder: &str) {
        conn.execute(
            "INSERT INTO locks (task_id, holder, acquired_at, expires_at) \
             VALUES (?1, ?2, '2000-01-01T00:00:00+00:00', '2000-01-01T00:00:00+00:00')",
            [task_id, holder],
        )
        .unwrap();
    }

    // ── slugify ───────────────────────────────────────────────────────────────

    #[rstest]
    #[case("Fix login bug", "fix-login-bug")]
    #[case("  hello  world  ", "hello-world")]
    #[case("multiple---hyphens", "multiple-hyphens")]
    #[case("already-a-slug", "already-a-slug")]
    #[case("123 task", "123-task")]
    #[case("!!!", "")]
    // truncation: mid-word cut backs up to last hyphen (result < 40 chars)
    #[case(
        "this is a very long title that exceeds the maximum allowed slug length",
        "this-is-a-very-long-title-that-exceeds"
    )]
    // truncation: cut lands exactly at a word boundary (char 40 is '-'), keep 40
    #[case(
        "aaaaaaaaaa bbbbbbbbbb cccccccccc ddddddd extra",
        "aaaaaaaaaa-bbbbbbbbbb-cccccccccc-ddddddd"
    )]
    // no truncation: slug is exactly 40 chars
    #[case(
        "aaaaaaaaaa bbbbbbbbbb cccccccccc ddddddd",
        "aaaaaaaaaa-bbbbbbbbbb-cccccccccc-ddddddd"
    )]
    // truncation: single word > 40 chars — hard cut at 40
    #[case(
        "supercalifragilisticexpialidocioussupercalifragilisticexpialidocious",
        "supercalifragilisticexpialidocioussuperc"
    )]
    fn test_slugify(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(slugify(input), expected);
    }

    // ── migrate ───────────────────────────────────────────────────────────────

    #[test]
    fn migrate_is_idempotent() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        migrate(&c).unwrap();
    }

    // ── create_task ───────────────────────────────────────────────────────────

    #[rstest]
    fn create_assigns_sequential_ids(conn: Connection) {
        let id1 = create_task(&conn, "task-a", "Task A", None, None).unwrap();
        let id2 = create_task(&conn, "task-b", "Task B", None, None).unwrap();
        assert!(id1.starts_with("0001-"));
        assert!(id2.starts_with("0002-"));
    }

    #[rstest]
    fn create_starts_open(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[rstest]
    fn create_stores_description(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", Some("some detail"), None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.description.as_deref(), Some("some detail"));
    }

    #[rstest]
    fn create_with_parent_links_child(conn: Connection) {
        let parent = create_task(&conn, "parent", "Parent Task", None, None).unwrap();
        let child = create_task(&conn, "child", "Child Task", None, Some(parent.as_str())).unwrap();
        let task = get_task(&conn, &child).unwrap().unwrap();
        assert_eq!(task.parent_id.as_deref(), Some(parent.as_str()));
    }

    #[rstest]
    fn create_with_nonexistent_parent_errors(conn: Connection) {
        let err = create_task(&conn, "orphan", "Orphan", None, Some("9999-ghost")).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("foreign key")
                || err.to_string().to_lowercase().contains("constraint")
        );
    }

    // ── get_task ──────────────────────────────────────────────────────────────

    #[rstest]
    fn get_missing_returns_none(conn: Connection) {
        assert!(get_task(&conn, "9999-no-such-task").unwrap().is_none());
    }

    #[rstest]
    fn get_found_returns_correct_fields(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", Some("desc"), None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.id, id);
        assert_eq!(task.title, "My Task");
        assert_eq!(task.description.as_deref(), Some("desc"));
        assert_eq!(task.status, Status::Open);
        assert!(task.parent_id.is_none());
    }

    // ── list_tasks ────────────────────────────────────────────────────────────

    #[rstest]
    fn list_empty(conn: Connection) {
        assert!(list_tasks(&conn, None, None, false).unwrap().is_empty());
    }

    #[rstest]
    fn list_returns_all_in_order(conn: Connection) {
        create_task(&conn, "a", "A", None, None).unwrap();
        create_task(&conn, "b", "B", None, None).unwrap();
        create_task(&conn, "c", "C", None, None).unwrap();
        let tasks = list_tasks(&conn, None, None, false).unwrap();
        assert_eq!(tasks.len(), 3);
        assert!(tasks[0].num < tasks[1].num);
        assert!(tasks[1].num < tasks[2].num);
    }

    #[rstest]
    #[case(Status::Open)]
    #[case(Status::InProgress)]
    #[case(Status::Done)]
    #[case(Status::Cancelled)]
    fn list_filters_by_status(conn: Connection, #[case] filter: Status) {
        let _id1 = create_task(&conn, "task-a", "Task A", None, None).unwrap();
        let id2 = create_task(&conn, "task-b", "Task B", None, None).unwrap();
        let id3 = create_task(&conn, "task-c", "Task C", None, None).unwrap();
        let id4 = create_task(&conn, "task-d", "Task D", None, None).unwrap();
        // id1 starts as Open; set the rest explicitly
        update_task(&conn, &id2, None, None, Some(&Status::InProgress), None).unwrap();
        update_task(&conn, &id3, None, None, Some(&Status::Done), None).unwrap();
        update_task(&conn, &id4, None, None, Some(&Status::Cancelled), None).unwrap();

        let tasks = list_tasks(&conn, Some(&filter), None, false).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, filter);
    }

    #[rstest]
    fn list_filters_by_parent(conn: Connection) {
        let parent = create_task(&conn, "parent", "Parent", None, None).unwrap();
        let other = create_task(&conn, "other", "Other", None, None).unwrap();
        let c1 = create_task(&conn, "child-1", "Child 1", None, Some(parent.as_str())).unwrap();
        let c2 = create_task(&conn, "child-2", "Child 2", None, Some(parent.as_str())).unwrap();
        create_task(&conn, "unrelated", "Unrelated", None, Some(other.as_str())).unwrap();

        let children = list_tasks(&conn, None, Some(parent.as_str()), false).unwrap();
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|t| t.id == c1));
        assert!(children.iter().any(|t| t.id == c2));
    }

    // ── update_task ───────────────────────────────────────────────────────────

    #[rstest]
    fn update_missing_returns_false(conn: Connection) {
        assert!(!update_task(&conn, "9999-no-such", Some("New"), None, None, None).unwrap());
    }

    #[rstest]
    fn update_title_only(conn: Connection) {
        let id = create_task(&conn, "my-task", "Old Title", Some("desc"), None).unwrap();
        update_task(&conn, &id, Some("New Title"), None, None, None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.title, "New Title");
        assert_eq!(task.description.as_deref(), Some("desc"));
        assert_eq!(task.status, Status::Open);
    }

    #[rstest]
    fn update_status_only(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        update_task(&conn, &id, None, None, Some(&Status::Done), None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.title, "My Task");
    }

    #[rstest]
    fn update_summary_only(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        update_task(
            &conn,
            &id,
            None,
            None,
            None,
            Some("Completed via approach X"),
        )
        .unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.summary.as_deref(), Some("Completed via approach X"));
        assert_eq!(task.title, "My Task");
        assert_eq!(task.status, Status::Open);
    }

    // ── delete_task ───────────────────────────────────────────────────────────

    #[rstest]
    fn delete_missing_returns_false(conn: Connection) {
        assert!(!delete_task(&conn, "9999-no-such", false).unwrap());
    }

    #[rstest]
    fn delete_open_task(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        delete_task(&conn, &id, false).unwrap();
        assert!(get_task(&conn, &id).unwrap().is_none());
    }

    #[rstest]
    fn delete_locked_without_force_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "holder", 3600).unwrap();
        let err = delete_task(&conn, &id, false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[rstest]
    fn delete_in_progress_without_force_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        update_task(&conn, &id, None, None, Some(&Status::InProgress), None).unwrap();
        let err = delete_task(&conn, &id, false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[rstest]
    fn delete_locked_with_force(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "holder", 3600).unwrap();
        assert!(delete_task(&conn, &id, true).unwrap());
        assert!(get_task(&conn, &id).unwrap().is_none());
    }

    // ── acquire_task ──────────────────────────────────────────────────────────

    #[rstest]
    fn acquire_sets_in_progress(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.locked_by.as_deref(), Some("agent1"));
    }

    #[rstest]
    fn acquire_same_holder_reacquires(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
    }

    #[rstest]
    fn acquire_different_holder_rejected(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let err = acquire_task(&conn, &id, "agent2", 3600).unwrap_err();
        assert!(err.to_string().contains("agent1"));
    }

    #[rstest]
    fn acquire_expired_lock_allows_new_holder(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        insert_expired_lock(&conn, &id, "old-holder");
        acquire_task(&conn, &id, "new-holder", 3600).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.locked_by.as_deref(), Some("new-holder"));
    }

    #[rstest]
    fn acquire_nonexistent_task_errors(conn: Connection) {
        let err = acquire_task(&conn, "9999-ghost", "agent1", 3600).unwrap_err();
        assert!(err.to_string().contains("not found"));
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM locks WHERE task_id = '9999-ghost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    // ── release_task ──────────────────────────────────────────────────────────

    #[rstest]
    fn release_removes_lock(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        release_task(&conn, &id, "agent1", false).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert!(task.locked_by.is_none());
    }

    #[rstest]
    fn release_no_lock_returns_false(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        assert!(!release_task(&conn, &id, "agent1", false).unwrap());
    }

    #[rstest]
    fn release_other_holder_rejected(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let err = release_task(&conn, &id, "agent2", false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[rstest]
    fn release_other_holder_with_force(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        release_task(&conn, &id, "agent2", true).unwrap();
        assert!(get_task(&conn, &id).unwrap().unwrap().locked_by.is_none());
    }

    // ── resolve_id ────────────────────────────────────────────────────────────

    #[rstest]
    fn resolve_id_exact_match(conn: Connection) {
        let id = create_task(&conn, "fix-login", "Fix Login", None, None).unwrap();
        assert_eq!(resolve_id(&conn, &id).unwrap(), id);
    }

    #[rstest]
    fn resolve_id_prefix_match(conn: Connection) {
        let id = create_task(&conn, "fix-login", "Fix Login", None, None).unwrap();
        // id looks like "0001-fix-login"; prefix "0001" should resolve it
        let resolved = resolve_id(&conn, "0001").unwrap();
        assert_eq!(resolved, id);
    }

    #[rstest]
    fn resolve_id_substring_match(conn: Connection) {
        let id = create_task(&conn, "fix-login", "Fix Login", None, None).unwrap();
        let resolved = resolve_id(&conn, "fix-login").unwrap();
        assert_eq!(resolved, id);
    }

    #[rstest]
    fn resolve_id_not_found_errors(conn: Connection) {
        let err = resolve_id(&conn, "no-such-task").unwrap_err();
        assert!(err.to_string().contains("no task found matching"));
        assert!(err.to_string().contains("no-such-task"));
    }

    #[rstest]
    fn resolve_id_ambiguous_errors(conn: Connection) {
        create_task(&conn, "fix-login", "Fix Login", None, None).unwrap();
        create_task(&conn, "fix-signup", "Fix Signup", None, None).unwrap();
        // "fix" appears in both IDs
        let err = resolve_id(&conn, "fix").unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        assert!(err.to_string().contains("fix"));
    }

    // ── renew_task ────────────────────────────────────────────────────────────

    #[rstest]
    fn renew_extends_expiry(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 1).unwrap();
        renew_task(&conn, &id, "agent1", 7200).unwrap();
        let raw: String = conn
            .query_row(
                "SELECT expires_at FROM locks WHERE task_id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        let expires_at = raw.parse::<chrono::DateTime<Utc>>().unwrap();
        assert!(expires_at > Utc::now());
    }

    #[rstest]
    fn renew_no_lock_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        let err = renew_task(&conn, &id, "agent1", 3600).unwrap_err();
        assert!(err.to_string().contains("no active lock"));
    }

    #[rstest]
    fn renew_wrong_holder_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None, None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let err = renew_task(&conn, &id, "agent2", 3600).unwrap_err();
        assert!(err.to_string().contains("agent1"));
    }
}
