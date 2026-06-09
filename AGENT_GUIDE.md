# Task CLI — Agent Guide

This guide is written for LLM agents that use the `task` CLI to coordinate work, including delegating to sub-agents.

## Identity

Every agent must have a unique identity. Set it once at the start of your session:

```
export TASK_HOLDER="your-agent-id"
```

All lock operations (`acquire`, `release`, `renew`) use this identity. Pick something stable and unique — e.g. `orchestrator`, `subagent-auth`, `worker-1`. If two agents share the same identity they will share lock ownership, which is sometimes intentional (same logical worker restarting) and sometimes a bug.

## Setup

Run this once before anything else. It is safe to run repeatedly:

```
task migrate
```

## Task Sizing

Before acquiring a task, assess its scope. A well-sized task:

- Has a single clear output (a file, a fix, a passing test suite)
- Can be completed within one focused agent session without context pressure
- Does not depend on the outcome of work that hasn't happened yet

**If a task is too large, decompose it before starting.** Create child tasks first (see [Decomposing a Task into Children](#decomposing-a-task-into-children)), then acquire the parent as a coordination task whose only job is to create children, monitor their progress, and mark itself done when all children are done. Do not attempt large tasks monolithically — you will either blow context or produce incomplete work.

A useful heuristic: if writing the `--description` for a task requires more than three sentences, it probably needs to be split.

## Core Workflow

### Pick up a task

```
task list --status open
```

Pick one, then acquire it. Acquiring sets the status to `in_progress` and places a lock so other agents leave it alone:

```
task acquire 0003-fix-auth-bug
```

The default lock TTL is 3600 seconds. If your work will take longer, set a longer TTL:

```
task acquire 0003-fix-auth-bug --ttl 7200
```

### Renew a lock you already hold

If your work is taking longer than expected, renew before the lock expires. The new TTL starts from now:

```
task renew 0003-fix-auth-bug --ttl 3600
```

### Finish a task

Release the lock and update the status in any order. Both steps are required — `release` alone does not change status:

```
task release 0003-fix-auth-bug
task update  0003-fix-auth-bug --status done
```

If you want to leave a note about what was done, update the description before releasing:

```
task update 0003-fix-auth-bug --description "Fixed by patching the JWT expiry check in auth.rs"
task release 0003-fix-auth-bug
task update  0003-fix-auth-bug --status done
```

### Abandon a task (return it to the queue)

Release the lock and reset to open so another agent can pick it up:

```
task release 0003-fix-auth-bug
task update  0003-fix-auth-bug --status open
```

## Decomposing a Task into Children

Use child tasks when a task is too large to complete in one session, or when distinct parts of the work can proceed in parallel. Child tasks are linked to the parent with `--parent` so they can be queried and tracked as a group.

### Creating child tasks

Before acquiring the parent, create all the children you can identify:

```
task create --title "Write migration for users table" \
            --description "Add columns: display_name (TEXT), verified_at (TEXT nullable)." \
            --parent 0005-migrate-schema

task create --title "Write migration for products table" \
            --description "Add column: archived_at (TEXT nullable)." \
            --parent 0005-migrate-schema

task create --title "Write migration for orders table" \
            --description "Add FK orders.user_id REFERENCES users(id)." \
            --parent 0005-migrate-schema
```

Then acquire the parent as a coordination task:

```
task acquire 0005-migrate-schema
```

Your role as coordinator is to spawn sub-agents for each child, monitor progress, and close out the parent when all children are done.

### Querying children

To see the status of all subtasks for a parent:

```
task list --parent 0005-migrate-schema
task list --parent 0005-migrate-schema --status open
task list --parent 0005-migrate-schema --json
```

### Closing out the parent

Before marking the parent done, verify no children are still outstanding:

```
task list --parent 0005-migrate-schema --status open
task list --parent 0005-migrate-schema --status in_progress
```

If both return empty, release and close the parent:

```
task release 0005-migrate-schema
task update  0005-migrate-schema --status done
```

## Delegating to Sub-Agents

### Creating tasks for sub-agents

Create a task for each unit of work you want to delegate. Use `--description` to give the sub-agent full context — it is the primary briefing document:

```
task create --title "Add rate limiting to /api/login" \
            --description "Use a sliding-window counter in Redis. Max 10 attempts per IP per minute. Return 429 with Retry-After header. Tests required."
```

The command prints the full ID (e.g. `0007-add-rate-limiting-to-api-login`). Pass that ID to the sub-agent.

### Handing off to a sub-agent

Tell the sub-agent its identity, the task ID, and that it should acquire before starting:

```
TASK_HOLDER=subagent-ratelimit task acquire 0007-add-rate-limiting-to-api-login
```

Or instruct the sub-agent to set its own `TASK_HOLDER` and call `task acquire` itself.

### Monitoring sub-agent progress

```
task list --status in_progress --json
```

The JSON output includes `locked_by` and `lock_expires` so you can see which agent holds each task and when the lock expires.

```
task show 0007-add-rate-limiting-to-api-login
```

### Recovering a stalled sub-agent

If a sub-agent's lock has expired (visible in `lock_expires`) or the agent is known to be dead, force-release the lock and reassign:

```
task release 0007-add-rate-limiting-to-api-login --force
task acquire 0007-add-rate-limiting-to-api-login --holder replacement-agent
```

`--force` on release bypasses the holder check so you do not need to impersonate the stalled agent.

## Parallelizing Work

Create all tasks first, then have sub-agents acquire and work on them concurrently. Each `acquire` is atomic — only one agent will succeed per task:

```
task create --title "Migrate users table"    --description "..."  # prints 0008-migrate-users-table
task create --title "Migrate products table" --description "..."  # prints 0009-migrate-products-table
task create --title "Migrate orders table"   --description "..."  # prints 0010-migrate-orders-table

# Launch three sub-agents, each targeting one task ID
```

If two agents race to acquire the same task, the second one gets an error showing who holds the lock — it should back off and pick a different open task.

## Follow-up Tasks

A follow-up task is work discovered during the execution of another task — a bug noticed, a missing index, a related cleanup. It is **not** a child of the task that discovered it; it stands alone in the backlog for any agent to pick up later.

Create follow-up tasks immediately when you discover them, before you forget:

```
task create --title "Add index on users.email" \
            --description "Discovered during the users migration — full-table scans on login are now visible in EXPLAIN."
```

Do not add a `--parent` flag. The relationship here is temporal (discovered while doing X), not structural (required to complete X). Mixing the two makes `task list --parent` unreliable as a completion-gating tool.

The rule of thumb: if the parent task cannot be marked `done` without this work finishing, it is a child task. If the parent can finish regardless, it is a follow-up.

## Status Reference

| Status | Meaning |
|---|---|
| `open` | Available to be picked up |
| `in_progress` | Acquired by an agent (lock is live) |
| `done` | Completed |
| `cancelled` | Will not be done |

Set status explicitly with `task update --status <value>`. Acquiring a task sets it to `in_progress` automatically; no other transitions are enforced — use your judgment.

## Task ID Format

IDs look like `0003-fix-auth-bug`: a zero-padded 4-digit number followed by a kebab-case slug. Always use the full ID (number + slug) in commands. When creating with `--id`, supply only the slug — the number is assigned automatically.

## Common Mistakes

**Not releasing before marking done.** The lock and the status are independent. Always release the lock; otherwise the task shows as locked forever (until TTL expires).

**Sharing a `TASK_HOLDER` across unrelated agents.** Each logical worker should have a distinct identity. Sharing means either agent can accidentally steal or release the other's locks.

**Forgetting to renew long-running work.** Default TTL is 1 hour. If your sub-agent is doing slow work (large migrations, long builds), set a longer TTL at acquire time or renew periodically.

**Acquiring without checking.** Run `task list --status open` first. If nothing is open, there is nothing to do. Do not busy-wait — check once and stop or wait for new tasks to appear.

**Tagging follow-up tasks as children.** Only use `--parent` when the child must complete for the parent to be done. Discovered-but-not-blocking work should be created without a parent so it sits in the open backlog without polluting the parent's child list.
