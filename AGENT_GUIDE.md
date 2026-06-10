# Task CLI — Agent Guide

---

## Part 1: Orchestrator

An orchestrator manages the backlog. Its only actions are: create tasks, spawn worker sub-agents, monitor progress, recover stalls, and close out completed work. **It never acquires tasks, writes code, edits files, or does any implementation work itself.** Anything that needs doing becomes a task delegated to a worker.

### Prerequisites

`tasks` must be installed as a global CLI tool on `PATH`. Verify:

```
tasks --help
```

If not found, ask a human to install it. Do not use a local dev copy (`./target/release/tasks`, `cargo run`, etc.) — it may be stale or write to a different database.

Run `tasks migrate` once before anything else to initialize the database (safe to re-run). See README for installation details.

### Creating and delegating tasks

Write a task for every unit of work you want to delegate. The `--description` is the worker's primary briefing — make it complete:

```
tasks create --title "Add rate limiting to /api/login" \
            --description "Sliding-window counter in Redis. Max 10 req/IP/min. Return 429 + Retry-After. Tests required."
```

The command prints the assigned full ID (e.g. `0007-add-rate-limiting-to-api-login`). Pass that ID to the worker sub-agent. The worker sets its own `TASK_HOLDER` and calls `tasks acquire` itself — **the orchestrator must never acquire on a worker's behalf**.

### Parallelizing work

Create all tasks first, then launch workers concurrently. Each `acquire` is atomic — only one agent wins per task:

```
tasks create --title "Migrate users table"    --description "..."
tasks create --title "Migrate products table" --description "..."
tasks create --title "Migrate orders table"   --description "..."

# Launch three workers, each given one task ID
```

If two workers race for the same task, the loser gets an error and should notify the orchestrator so it can assign a different task explicitly.

### Decomposing large tasks

When a task is too large to delegate as a single unit, break it up before assigning anything. Create child tasks linked to the parent with `--parent`, then assign each child to its own worker:

```
tasks create --title "Write migration for users table"    --parent 0005-migrate-schema --description "..."
tasks create --title "Write migration for products table" --parent 0005-migrate-schema --description "..."
tasks create --title "Write migration for orders table"   --parent 0005-migrate-schema --description "..."
```

The parent task becomes a coordination marker. Acquire it yourself to signal it is active, then close it only once all children are done:

```
tasks acquire 0005-migrate-schema --holder orchestrator
# ... workers complete children ...
tasks list --parent 0005-migrate-schema --status open
tasks list --parent 0005-migrate-schema --status in_progress
# both empty → safe to close
tasks release 0005-migrate-schema
tasks update  0005-migrate-schema --status done
```

### Monitoring workers

```
tasks list --status in_progress --json
```

Use `--count` for a quick backlog-size check — it prints only the integer count of matching tasks:

```
tasks list --status open --count
```

The JSON output includes `locked_by` and `lock_expires` per task. Use `tasks show <id>` for full detail on a specific task.

### Recovering a stalled worker

Run `tasks gc` at the start of each orchestration loop to automatically recover all tasks with expired locks in one call:

```
tasks gc
```

This resets every stalled task to open without needing to identify each one individually. Use `tasks release --force` only when you need to recover a task whose lock has not yet expired (e.g. a worker that is known dead but still holds a live lock).

If a lock has expired or the worker is known dead, force-release and reassign:

```
tasks release 0007-add-rate-limiting-to-api-login --force
tasks acquire 0007-add-rate-limiting-to-api-login --holder replacement-worker
```

`--force` bypasses the holder check — no need to impersonate the stalled agent.

### Orchestrator mistakes to avoid

**Acquiring tasks meant for workers.** The lock ends up under the wrong identity. The worker can't renew or release it cleanly.

**Doing implementation work inline.** If you find yourself writing code, editing files, or running tests, stop — create a task and delegate it.

**Marking a parent done with open children.** Always check `tasks list --parent <id> --status open` and `--status in_progress` before closing a parent.

**Committing, pushing, or publishing changes.** Orchestrators must never run `git commit`, `git push`, release commands, or any operation that publishes work externally. Only humans decide when changes are committed and pushed.

---

## Part 2: Worker

A worker receives a task ID from an orchestrator, acquires it, does the work, and closes out the task. It does not manage the backlog or spawn other workers (unless it discovers a task needs decomposing — see [Decomposing a task](#decomposing-a-task)).

### Prerequisites

`tasks` must be a global CLI on `PATH`. Do not use a local dev copy. Run `tasks migrate` if the database has not been initialized yet.

### Identity

Set a unique identity once at the start of your session:

```powershell
$env:TASK_HOLDER = "worker-1"   # or any stable unique name
```

All lock operations (`acquire`, `release`, `renew`) use this identity. If two agents share the same identity they share lock ownership — intentional only when the same logical worker restarts.

### Picking up a task

Workers receive a specific task ID from the orchestrator — they do not browse the backlog or choose what to work on. Acquire exactly the ID you were given:

```
tasks acquire 0003-fix-auth-bug
```

If you want to confirm the task details before acquiring, inspect it directly:

```
tasks show 0003-fix-auth-bug
tasks acquire 0003-fix-auth-bug
```

### Renewing a lock

If your work is running long, renew before the lock expires (new TTL starts from now):

```
tasks renew 0003-fix-auth-bug --ttl 3600
```

### Finishing a task

Use `tasks close` to complete a task in one step. Pass `--summary` with your closing record — decisions made, approach taken, caveats, anything useful for someone reading the task later. Keep it to 1–3 sentences. It is separate from `--description`, which is the upfront brief written by whoever created the task.

```
tasks close 0003-fix-auth-bug --summary "Patched JWT expiry check in auth.rs; added regression test in auth_test.rs"
```

`close` does three things internally: sets the summary, releases the lock, and marks the task `done`. If you need finer control or want to understand what is happening under the hood, you can run the steps individually:

```
tasks update 0003-fix-auth-bug --summary "Patched JWT expiry check in auth.rs; added regression test in auth_test.rs"
tasks release 0003-fix-auth-bug
tasks update  0003-fix-auth-bug --status done
```

### Abandoning a task (returning it to the queue)

```
tasks release 0003-fix-auth-bug
tasks update  0003-fix-auth-bug --status open
```

### Decomposing a task

If the task you acquired turns out to be too large, decompose it before doing any work. A well-sized task:

- Has a single clear output
- Can finish within one focused session without context pressure
- Does not depend on work that hasn't happened yet

A useful heuristic: if explaining the task's acceptance criteria takes more than three sentences, split it.

To decompose, create child tasks under the current task's ID, then notify the orchestrator that it needs to coordinate them (or handle coordination yourself if the orchestrator is not monitoring):

```
tasks create --title "Sub-task A" --description "..." --parent 0003-fix-auth-bug
tasks create --title "Sub-task B" --description "..." --parent 0003-fix-auth-bug
```

Do not attempt large tasks monolithically — you will blow context or produce incomplete work.

### Follow-up tasks

If you notice unrelated work while executing your task (a bug, a missing index, a cleanup), create a standalone task for it immediately — **without** `--parent`:

```
tasks create --title "Add index on users.email" \
            --description "Discovered during migration — full-table scans on login now visible in EXPLAIN."
```

The distinction: if the parent task cannot be marked `done` without this work, it is a child. If the parent can finish regardless, it is a follow-up that stands alone in the backlog.

### Worker mistakes to avoid

**Not releasing before marking done.** The lock and the status are independent. Always release; otherwise the task shows locked forever until TTL expires.

**Forgetting to renew long-running work.** Default TTL is 1 hour. Set a longer TTL at acquire time or renew periodically for slow work.

**Tagging follow-up tasks as children.** Mixing temporal and structural relationships makes `tasks list --parent` unreliable as a completion gate.

**Committing, pushing, or publishing changes.** Workers must never run `git commit`, `git push`, release commands, or any operation that publishes work externally. Make the code changes, close the task, and leave the commit/push decision to the human.

---

See README for the full command reference, status values, task ID format, and installation instructions.
