# tasks

A CLI for managing work handed off to LLM agents. Tasks are stored in a local SQLite database and support an acquire/release locking model so multiple agents can pull from a shared backlog without stepping on each other.

---

## Installation

```
cargo install --path .
```

Initialize the database once in your project directory before using any other command:

```
tasks migrate
```

`migrate` is safe to re-run; it never alters existing data.

---

## Command Reference

### `migrate`

Initialize or update the database schema. Run once before anything else.

```
tasks migrate
```

---

### `create`

Create a new task. Prints the full assigned ID on success.

```
tasks create --title "Add rate limiting to /api/login" \
             --description "Sliding-window counter. Max 10 req/IP/min. Return 429 + Retry-After."
```

| Flag | Description |
|---|---|
| `--title` | Short summary of the work (also accepted as a positional argument) |
| `--description` | Longer context or acceptance criteria |
| `--id <slug>` | Custom slug for the ID (e.g. `fix-auth-bug`); number prefix is assigned automatically |
| `--parent <id>` | Full ID of the parent task; use when breaking a large task into subtasks |
| `--summary` | Closing summary; set this before releasing when finishing work |

---

### `list`

List tasks. By default shows only `open` and `in_progress` tasks.

```
tasks list
tasks list --status open
tasks list --parent 0005-migrate-schema --status open
tasks list --all --json
```

| Flag | Description |
|---|---|
| `--status` | Filter to one status: `open`, `in_progress`, `done`, `cancelled` |
| `--parent <id>` | Show only direct children of this parent task |
| `--all` | Include every status (overrides `--status`) |
| `--tree` | Display tasks as an indented hierarchy grouped by parent |
| `--json` | Machine-readable JSON array with full fields and live lock info |

---

### `search`

Full-text search across task titles and descriptions.

```
tasks search "rate limiting"
tasks search "auth" --status open
tasks search "schema" --parent 0005-migrate-schema
tasks search "jwt" --json
```

| Flag | Description |
|---|---|
| `--status` | Limit results to one status: `open`, `in_progress`, `done`, `cancelled` |
| `--parent <id>` | Limit results to children of this parent task |
| `--json` | Output results as a JSON array |

---

### `show`

Print all fields for a single task, including lock holder and expiry if locked.

```
tasks show 0003-fix-auth-bug
tasks show 0003-fix-auth-bug --json
```

---

### `log`

Show the event history for a task: status transitions, lock acquisitions, releases, and updates.

```
tasks log 0003-fix-auth-bug
tasks log 0003-fix-auth-bug --json
```

---

### `update`

Update one or more fields on a task. At least one flag is required.

```
tasks update 0003-fix-auth-bug --summary "Patched JWT expiry check; added regression test"
tasks update 0003-fix-auth-bug --status done
```

| Flag | Description |
|---|---|
| `--title` | Replace the title |
| `--description` | Replace the description |
| `--status` | Set status: `open`, `in_progress`, `done`, `cancelled` |
| `--summary` | Worker's closing record of decisions and outcomes |

---

### `delete`

Permanently delete a task. Refuses to delete locked or in-progress tasks without `--force`.

```
tasks delete 0003-fix-auth-bug
tasks delete 0003-fix-auth-bug --force
```

---

### `acquire`

Take an exclusive lock on a task and set its status to `in_progress`. Fails if another holder already holds the lock. Re-acquiring your own lock refreshes the expiry.

Requires a holder identity via `--holder` or the `TASK_HOLDER` environment variable.

```
export TASK_HOLDER=worker-1
tasks acquire 0003-fix-auth-bug
tasks acquire 0003-fix-auth-bug --ttl 7200
```

| Flag | Description |
|---|---|
| `--holder` | Identity of the acquiring agent (falls back to `$TASK_HOLDER`) |
| `--ttl` | Lock duration in seconds (default: 3600) |

---

### `next`

Atomically find and acquire the next open task. Prints the task as JSON. Exits 1 if no open tasks are available.

```
tasks next
tasks next --parent 0005-migrate-schema
tasks next --ttl 7200
tasks next --holder worker-1
```

| Flag | Description |
|---|---|
| `--parent <id>` | Restrict to children of this parent task |
| `--ttl` | Lock duration in seconds (default: 3600) |
| `--holder` | Identity of the acquiring agent (falls back to `$TASK_HOLDER`) |

---

### `release`

Release a lock. Does not change task status — update status separately.

```
tasks release 0003-fix-auth-bug
tasks release 0003-fix-auth-bug --force   # override a stale lock held by another agent
```

| Flag | Description |
|---|---|
| `--holder` | Identity releasing the lock (falls back to `$TASK_HOLDER`) |
| `--force` | Release even if the lock belongs to a different holder |

---

### `renew`

Extend the expiry of a lock you already hold. The new TTL starts from now.

```
tasks renew 0003-fix-auth-bug --ttl 3600
```

---

### `close`

Complete a task in one step: sets the summary, releases the lock, and marks the task `done`.

```
tasks close 0003-fix-auth-bug --summary "Patched JWT expiry check in auth.rs; added regression test"
tasks close 0003-fix-auth-bug --summary "..." --holder worker-1
```

| Flag | Description |
|---|---|
| `--summary` | Closing record of decisions and outcomes |
| `--holder` | Identity releasing the lock (falls back to `$TASK_HOLDER`) |

---

### `gc`

Reap expired locks and reset stalled `in_progress` tasks back to `open`. Use `--dry-run` to preview what would change without making any modifications.

```
tasks gc
tasks gc --dry-run
```

| Flag | Description |
|---|---|
| `--dry-run` | Preview affected tasks without making any changes |

---

### `stats`

Show health metrics: status breakdown, completion rate, stalled tasks, expired locks.

```
tasks stats
tasks stats --json
```

---

### `completions`

Generate a shell completion script and print it to stdout. Supports `bash`, `zsh`, `fish`, `powershell`, and `elvish`.

**PowerShell (one-time setup):**

```powershell
tasks completions powershell | Out-File -Append $PROFILE
```

Open a new terminal (or `. $PROFILE`) and tab-completion is active for all subcommands, flags, and `--status` values.

**Bash:**

```bash
tasks completions bash >> ~/.bashrc
source ~/.bashrc
```

Note: task IDs are dynamic and not completed by the static script. All subcommand names, flag names, and `--status` values (`open`, `in_progress`, `done`, `cancelled`) are completed automatically.

---

## Typical Worker Workflow

```bash
# 1. Set identity for the session
export TASK_HOLDER=worker-1

# 2. Initialize the database (if not already done)
tasks migrate

# 3. Pick up a task
tasks list --status open
tasks acquire 0007-add-rate-limiting

# 4. Do the work ...
#    Renew if the work runs long:
tasks renew 0007-add-rate-limiting --ttl 3600

# 5. Record what you did, release the lock, and mark done
tasks update 0007-add-rate-limiting --summary "Added Redis sliding-window counter; returns 429 + Retry-After header"
tasks release 0007-add-rate-limiting
tasks update 0007-add-rate-limiting --status done
```

To return a task to the queue without completing it:

```bash
tasks release 0007-add-rate-limiting
tasks update 0007-add-rate-limiting --status open
```

---

## Task ID Format

IDs look like `0003-fix-auth-bug`: a zero-padded 4-digit sequential number followed by a kebab-case slug derived from the title. Always use the full ID in commands. When supplying `--id` at creation time, provide only the slug portion.

## Status Values

| Status | Meaning |
|---|---|
| `open` | Available to be picked up |
| `in_progress` | Acquired by an agent (lock is live) |
| `done` | Completed |
| `cancelled` | Will not be done |

`acquire` transitions a task to `in_progress` automatically. All other transitions are explicit via `tasks update --status`.
