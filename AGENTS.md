# Task CLI

This project is a CLI for a human to manage tasks which are handed over to a worker, usually an LLM.

# Spec

```
tasks create  --title "..." [--description "..."] [--id slug-without-prefix]
tasks migrate
tasks list    [--status open|in_progress|done|cancelled] [--json]
tasks show    <id> [--json]
tasks update  <id> [--title "..."] [--description "..."] [--status STATUS]
tasks delete  <id> [--force]
tasks acquire <id> [--holder NAME] [--ttl SECONDS]
tasks release <id> [--holder NAME] [--force]
tasks renew   <id> [--holder NAME] [--ttl SECONDS]
```

# Usage Example

Tasks will usually be picked up by an agent with the following prompt:

```
/loop 1m "Familiarize yourself with @AGENT_GUIDE.md, which describes how to use tasks.
If no task is in progress by you, pick a task and complete it. You may CRUD more tasks as needed. Do all work
in subagent(s) to keep context clean. Your main loop should be orchestration only and keeping agents on track to
complete tasks; you are a manager. You may parallelize where appropriate with background subagents. Push through any
blockers to complete the tasks autonomously. Start by reading AGENTS.md"
```

# Implementation

## ID Format

IDs are `{NNNN}-{slug}` — a zero-padded 4-digit sequential number followed by a kebab-case slug derived from the title (e.g. `0003-fix-auth-bug`). Pass only the slug to `--id`; the numeric prefix is assigned automatically.

## Storage

SQLite database at `./tasks.db` in the current working directory. Run `tasks migrate` once to initialize; it is safe to re-run.

## Status Values

`open` → `in_progress` → `done` / `cancelled`. Acquiring a task sets it to `in_progress` automatically; all other transitions are explicit via `tasks update --status`.
