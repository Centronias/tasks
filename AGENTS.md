# Task CLI

This project is a CLI for a human to manage tasks which are handed over to a worker, usually an LLM.

# Spec

```
./task create  --title "..." [--description "..."] [--id slug-without-prefix]
./task migrate
./task list    [--status open|in_progress|done|cancelled] [--json]
./task show    <id> [--json]
./task update  <id> [--title "..."] [--description "..."] [--status STATUS]
./task delete  <id> [--force]
./task acquire <id> [--holder NAME] [--ttl SECONDS]
./task release <id> [--holder NAME] [--force]
./task renew   <id> [--holder NAME] [--ttl SECONDS]
```

# Usage Example

Tasks will usually be picked up by an agent with the following prompt:

```
/loop 1m "If no task is in progress by you, pick a task and complete it. You may CRUD more tasks as needed. Do all work
in subagent(s) to keep context clean. Your main loop should be orchestration only and keeping agents on track to
complete tasks; you are a manager. You may parallelize where appropriate with background subagents. Push through any
blockers to complete the tasks autonomously. Start by reading AGENTS.md"
```

# Implementation

## ID Format

IDs are `{NNNN}-{slug}` — a zero-padded 4-digit sequential number followed by a hyphen and a kebab-case slug.

- `--id` accepts the slug portion only (e.g., `fix-auth-bug`)
- If `--id` is omitted, the slug is derived from the title (lowercase, spaces→hyphens, non-alphanumeric stripped)
- The numeric prefix is the next available integer, formatted `%04d`
- Example full ID: `0003-fix-auth-bug`

## Storage

SQLite database at `./tasks.db` (current working directory). The `migrate` command creates/updates the schema idempotently.

### Schema

```sql
CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,   -- full id: "0003-fix-auth-bug"
    num         INTEGER UNIQUE NOT NULL,
    title       TEXT NOT NULL,
    description TEXT,
    status      TEXT NOT NULL DEFAULT 'open',
    created_at  TEXT NOT NULL,      -- ISO-8601 UTC
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS locks (
    task_id     TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    holder      TEXT NOT NULL,
    acquired_at TEXT NOT NULL,
    expires_at  TEXT NOT NULL       -- ISO-8601 UTC; expired locks are ignored
);
```

## Crates

```toml
clap       = { version = "4", features = ["derive"] }
rusqlite   = { version = "0.31", features = ["bundled"] }
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
chrono     = { version = "0.4", features = ["serde"] }
```

## File Structure

```
src/
  main.rs     — clap CLI structs + main() dispatch
  db.rs       — open_db(), migrate(), all query helpers
  models.rs   — Task, Lock, Status (enum with Display/FromStr)
```

## Command Behavior

### `migrate`
Run `CREATE TABLE IF NOT EXISTS` for both tables. Safe to run repeatedly.

### `create`
1. Slugify title (or use `--id` slug directly).
2. `SELECT MAX(num) FROM tasks` → next num = max + 1 (or 1 if empty).
3. Full id = `format!("{:04}-{}", num, slug)`.
4. Insert with `status = 'open'`, timestamps = now.
5. Print the full id on success.

### `list`
`SELECT * FROM tasks [WHERE status = ?] ORDER BY num ASC`

For each task, LEFT JOIN locks and check `expires_at > now` to annotate locked tasks.

Output: table (human) or JSON array (`--json`). Include `locked_by` and `lock_expires` fields in JSON when a live lock exists.

### `show <id>`
Fetch task + lock info. Error if not found. Human or JSON output.

### `update <id>`
Accept any combination of `--title`, `--description`, `--status`. At least one required. Update `updated_at`. Error if no fields given.

### `delete <id>`
Without `--force`: refuse if task has a live lock or status is `in_progress`.
With `--force`: delete unconditionally (cascade removes lock row).

### `acquire <id>`
1. Check for live lock (`expires_at > now`). If found and holder ≠ caller → error.
2. Insert/replace lock row with `holder`, `acquired_at = now`, `expires_at = now + ttl`.
3. Set task `status = 'in_progress'`, update `updated_at`.
4. Default holder: `TASK_HOLDER` env var, else `"anonymous"`.
5. Default TTL: 3600 seconds.

### `release <id>`
1. Without `--force`: error if live lock exists and holder ≠ caller.
2. Delete lock row. Task status is left unchanged (agent calls `update --status done` separately).

### `renew <id>`
Extend `expires_at = now + ttl` for an existing lock. Error if no live lock or holder ≠ caller.

## Status Values

Stored as lowercase strings: `"open"`, `"in_progress"`, `"done"`, `"cancelled"`.
