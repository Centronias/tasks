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

TODO
