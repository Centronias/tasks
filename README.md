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

## Commands

Run `tasks --help` for a full command listing, or `tasks <subcommand> --help` for flags and usage details for any individual command.

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
tasks close 0007-add-rate-limiting --summary "Added Redis sliding-window counter; returns 429 + Retry-After header"
```

To return a task to the queue without completing it:

```bash
tasks release 0007-add-rate-limiting
tasks update 0007-add-rate-limiting --status open
```

---

## Shell Completions

Generate a completion script with `tasks completions <shell>` (supports `bash`, `zsh`, `fish`, `powershell`, `elvish`).

**PowerShell (one-time setup):**

```powershell
tasks completions powershell | Out-File -Append $PROFILE
```

**Bash:**

```bash
tasks completions bash >> ~/.bashrc
source ~/.bashrc
```

Open a new terminal (or reload your profile) and tab-completion is active for all subcommands, flags, and `--status` values.

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
