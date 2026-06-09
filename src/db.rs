use std::str::FromStr;

use crate::models::{Lock, Status, Task};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

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
            updated_at  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS locks (
            task_id     TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
            holder      TEXT NOT NULL,
            acquired_at TEXT NOT NULL,
            expires_at  TEXT NOT NULL
        );",
    )
}

pub fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn create_task(
    conn: &Connection,
    slug: &str,
    title: &str,
    description: Option<&str>,
) -> rusqlite::Result<String> {
    let now = Utc::now().to_rfc3339();
    // Single statement: SELECT MAX(num)+1 and INSERT are atomic, eliminating TOCTOU.
    conn.execute(
        "INSERT INTO tasks (id, num, title, description, status, created_at, updated_at)
         SELECT printf('%04d-%s', n, ?1), n, ?2, ?3, 'open', ?4, ?4
         FROM (SELECT COALESCE(MAX(num), 0) + 1 AS n FROM tasks)",
        params![slug, title, description, now],
    )?;
    conn.query_row(
        "SELECT id FROM tasks WHERE rowid = ?1",
        [conn.last_insert_rowid()],
        |r| r.get(0),
    )
}

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    let status_str: String = row.get(4)?;
    let status = Status::from_str(&status_str)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into()))?;
    let created_at: DateTime<Utc> = row
        .get::<_, String>(5)?
        .parse()
        .map_err(|e: chrono::ParseError| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e)))?;
    let updated_at: DateTime<Utc> = row
        .get::<_, String>(6)?
        .parse()
        .map_err(|e: chrono::ParseError| rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e)))?;
    let lock_expires: Option<DateTime<Utc>> = row
        .get::<_, Option<String>>(8)?
        .and_then(|s| s.parse().ok());
    Ok(Task {
        id: row.get(0)?,
        num: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        status,
        created_at,
        updated_at,
        locked_by: row.get(7)?,
        lock_expires,
    })
}

pub fn list_tasks(
    conn: &Connection,
    status_filter: Option<&Status>,
) -> rusqlite::Result<Vec<Task>> {
    let now = Utc::now().to_rfc3339();
    let sql = "SELECT t.id, t.num, t.title, t.description, t.status,
                      t.created_at, t.updated_at,
                      CASE WHEN l.expires_at > ?1 THEN l.holder END,
                      CASE WHEN l.expires_at > ?1 THEN l.expires_at END
               FROM tasks t
               LEFT JOIN locks l ON l.task_id = t.id
               WHERE (?2 IS NULL OR t.status = ?2)
               ORDER BY t.num ASC";
    let status_val = status_filter.map(ToString::to_string);
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![now, status_val], row_to_task)?;
    rows.collect()
}

pub fn get_task(conn: &Connection, id: &str) -> rusqlite::Result<Option<Task>> {
    let now = Utc::now().to_rfc3339();
    let sql = "SELECT t.id, t.num, t.title, t.description, t.status,
                      t.created_at, t.updated_at,
                      CASE WHEN l.expires_at > ?1 THEN l.holder END,
                      CASE WHEN l.expires_at > ?1 THEN l.expires_at END
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
) -> rusqlite::Result<bool> {
    let now = Utc::now().to_rfc3339();
    let rows = conn.execute(
        "UPDATE tasks SET
            title       = COALESCE(?1, title),
            description = COALESCE(?2, description),
            status      = COALESCE(?3, status),
            updated_at  = ?4
         WHERE id = ?5",
        params![title, description, status.map(ToString::to_string), now, id],
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

pub fn acquire_task(
    conn: &Connection,
    id: &str,
    holder: &str,
    ttl: u64,
) -> anyhow::Result<()> {
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

pub fn release_task(conn: &Connection, id: &str, holder: &str, force: bool) -> anyhow::Result<bool> {
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

#[cfg(test)]
mod tests {
    use super::{
        acquire_task, create_task, delete_task, get_task, list_tasks, migrate, release_task,
        renew_task, slugify, update_task,
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
    #[case("Fix login bug",    "fix-login-bug")]
    #[case("  hello  world  ", "hello-world")]
    #[case("multiple---hyphens", "multiple-hyphens")]
    #[case("already-a-slug",   "already-a-slug")]
    #[case("123 task",         "123-task")]
    #[case("!!!",              "")]
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
        let id1 = create_task(&conn, "task-a", "Task A", None).unwrap();
        let id2 = create_task(&conn, "task-b", "Task B", None).unwrap();
        assert!(id1.starts_with("0001-"));
        assert!(id2.starts_with("0002-"));
    }

    #[rstest]
    fn create_starts_open(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.status, Status::Open);
    }

    #[rstest]
    fn create_stores_description(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", Some("some detail")).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.description.as_deref(), Some("some detail"));
    }

    // ── get_task ──────────────────────────────────────────────────────────────

    #[rstest]
    fn get_missing_returns_none(conn: Connection) {
        assert!(get_task(&conn, "9999-no-such-task").unwrap().is_none());
    }

    #[rstest]
    fn get_found_returns_correct_fields(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", Some("desc")).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.id, id);
        assert_eq!(task.title, "My Task");
        assert_eq!(task.description.as_deref(), Some("desc"));
        assert_eq!(task.status, Status::Open);
    }

    // ── list_tasks ────────────────────────────────────────────────────────────

    #[rstest]
    fn list_empty(conn: Connection) {
        assert!(list_tasks(&conn, None).unwrap().is_empty());
    }

    #[rstest]
    fn list_returns_all_in_order(conn: Connection) {
        create_task(&conn, "a", "A", None).unwrap();
        create_task(&conn, "b", "B", None).unwrap();
        create_task(&conn, "c", "C", None).unwrap();
        let tasks = list_tasks(&conn, None).unwrap();
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
        let _id1 = create_task(&conn, "task-a", "Task A", None).unwrap();
        let id2 = create_task(&conn, "task-b", "Task B", None).unwrap();
        let id3 = create_task(&conn, "task-c", "Task C", None).unwrap();
        let id4 = create_task(&conn, "task-d", "Task D", None).unwrap();
        // id1 starts as Open; set the rest explicitly
        update_task(&conn, &id2, None, None, Some(&Status::InProgress)).unwrap();
        update_task(&conn, &id3, None, None, Some(&Status::Done)).unwrap();
        update_task(&conn, &id4, None, None, Some(&Status::Cancelled)).unwrap();

        let tasks = list_tasks(&conn, Some(&filter)).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, filter);
    }

    // ── update_task ───────────────────────────────────────────────────────────

    #[rstest]
    fn update_missing_returns_false(conn: Connection) {
        assert!(!update_task(&conn, "9999-no-such", Some("New"), None, None).unwrap());
    }

    #[rstest]
    fn update_title_only(conn: Connection) {
        let id = create_task(&conn, "my-task", "Old Title", Some("desc")).unwrap();
        update_task(&conn, &id, Some("New Title"), None, None).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.title, "New Title");
        assert_eq!(task.description.as_deref(), Some("desc"));
        assert_eq!(task.status, Status::Open);
    }

    #[rstest]
    fn update_status_only(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        update_task(&conn, &id, None, None, Some(&Status::Done)).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.title, "My Task");
    }

    // ── delete_task ───────────────────────────────────────────────────────────

    #[rstest]
    fn delete_missing_returns_false(conn: Connection) {
        assert!(!delete_task(&conn, "9999-no-such", false).unwrap());
    }

    #[rstest]
    fn delete_open_task(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        delete_task(&conn, &id, false).unwrap();
        assert!(get_task(&conn, &id).unwrap().is_none());
    }

    #[rstest]
    fn delete_locked_without_force_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "holder", 3600).unwrap();
        let err = delete_task(&conn, &id, false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[rstest]
    fn delete_in_progress_without_force_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        update_task(&conn, &id, None, None, Some(&Status::InProgress)).unwrap();
        let err = delete_task(&conn, &id, false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[rstest]
    fn delete_locked_with_force(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "holder", 3600).unwrap();
        assert!(delete_task(&conn, &id, true).unwrap());
        assert!(get_task(&conn, &id).unwrap().is_none());
    }

    // ── acquire_task ──────────────────────────────────────────────────────────

    #[rstest]
    fn acquire_sets_in_progress(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert_eq!(task.status, Status::InProgress);
        assert_eq!(task.locked_by.as_deref(), Some("agent1"));
    }

    #[rstest]
    fn acquire_same_holder_reacquires(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
    }

    #[rstest]
    fn acquire_different_holder_rejected(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let err = acquire_task(&conn, &id, "agent2", 3600).unwrap_err();
        assert!(err.to_string().contains("agent1"));
    }

    #[rstest]
    fn acquire_expired_lock_allows_new_holder(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
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
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        release_task(&conn, &id, "agent1", false).unwrap();
        let task = get_task(&conn, &id).unwrap().unwrap();
        assert!(task.locked_by.is_none());
    }

    #[rstest]
    fn release_no_lock_returns_false(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        assert!(!release_task(&conn, &id, "agent1", false).unwrap());
    }

    #[rstest]
    fn release_other_holder_rejected(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let err = release_task(&conn, &id, "agent2", false).unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    #[rstest]
    fn release_other_holder_with_force(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        release_task(&conn, &id, "agent2", true).unwrap();
        assert!(get_task(&conn, &id).unwrap().unwrap().locked_by.is_none());
    }

    // ── renew_task ────────────────────────────────────────────────────────────

    #[rstest]
    fn renew_extends_expiry(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
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
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        let err = renew_task(&conn, &id, "agent1", 3600).unwrap_err();
        assert!(err.to_string().contains("no active lock"));
    }

    #[rstest]
    fn renew_wrong_holder_errors(conn: Connection) {
        let id = create_task(&conn, "my-task", "My Task", None).unwrap();
        acquire_task(&conn, &id, "agent1", 3600).unwrap();
        let err = renew_task(&conn, &id, "agent2", 3600).unwrap_err();
        assert!(err.to_string().contains("agent1"));
    }
}
