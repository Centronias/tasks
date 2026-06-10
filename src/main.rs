#![warn(
    clippy::all,
    clippy::correctness,
    clippy::suspicious,
    clippy::style,
    clippy::complexity,
    clippy::perf,
    clippy::pedantic,
    clippy::nursery
)]
#![allow(
    // Pedantic/nursery opt-outs for stylistic preferences.
    clippy::missing_docs_in_private_items,
    clippy::missing_panics_doc,
    clippy::pattern_type_mismatch,
    clippy::wildcard_enum_match_arm,
    clippy::implicit_return,
    clippy::question_mark_used,
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::too_many_lines,
    clippy::enum_glob_use
)]

mod db;
mod models;

use clap::{CommandFactory, Parser, Subcommand};
use models::{Priority, SortBy, Status};

/// CLI for managing tasks handed off to LLM agents.
///
/// Tasks are stored in a `SQLite` database (`tasks.db`) in the current directory.
/// Run `task migrate` once to initialize the database before using other commands.
///
/// Task IDs have the form `NNNN-slug` (e.g. `0003-fix-auth-bug`). When creating
/// a task you supply only the slug portion via `--id`; the numeric prefix is
/// assigned automatically.
#[derive(Parser)]
#[command(name = "task", about = "Manage tasks for LLM agents", version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    /// Path to the tasks database file.
    /// Overrides the `TASKS_DB` environment variable and the ./tasks.db default.
    #[arg(long, global = true)]
    db: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize or update the database schema.
    ///
    /// Creates the `tasks` and `locks` tables if they do not already exist.
    /// Safe to run repeatedly; existing data is never altered.
    /// Must be run once before any other command will work.
    Migrate,

    /// Create a new task and print its assigned ID.
    ///
    /// The full ID is formed by combining an auto-incremented four-digit prefix
    /// with the slug: e.g. `0004-fix-auth-bug`. The new task starts in the
    /// `open` status.
    Create {
        /// Short human-readable summary of the work to be done.
        /// May be supplied as a bare positional argument or via `--title`.
        #[arg(index = 1)]
        title_pos: Option<String>,

        /// Short human-readable summary of the work to be done (flag form).
        #[arg(long)]
        title: Option<String>,

        /// Optional longer description with additional context or acceptance criteria.
        #[arg(long)]
        description: Option<String>,

        /// Slug portion of the ID (kebab-case, e.g. `fix-auth-bug`).
        /// Auto-derived from the title by lowercasing and replacing non-alphanumeric
        /// characters with hyphens when omitted.
        #[arg(long)]
        id: Option<String>,

        /// Full ID of the parent task, e.g. `0005-implement-auth`.
        /// Use when breaking a larger task into subtasks so that children
        /// can later be queried with `task list --parent <id>`.
        #[arg(long)]
        parent: Option<String>,

        /// Priority level for this task.
        /// Accepted values: `low`, `medium` (default), `high`, `critical`.
        #[arg(long, default_value = "medium")]
        priority: Priority,
    },

    /// List tasks, optionally filtered by status and/or parent.
    ///
    /// By default shows only incomplete tasks (`open` and `in_progress`).
    /// Pass `--all` to include every status, or `--status <s>` to narrow to
    /// one specific status.
    ///
    /// Outputs one task per line in the format:
    ///   `<id>  <status>  <title>  [locked]`
    /// Pass `--json` to get a machine-readable JSON array instead, which
    /// includes full field details and active lock information.
    List {
        /// Only show tasks with this status.
        /// Accepted values: `open`, `in_progress`, `done`, `cancelled`.
        #[arg(long)]
        status: Option<Status>,

        /// Only show direct children of this parent task ID.
        /// Use to check subtask progress without listing the entire backlog.
        #[arg(long)]
        parent: Option<String>,

        /// Show all tasks regardless of status (overrides `--status`).
        /// By default only `open` and `in_progress` tasks are shown.
        #[arg(long)]
        all: bool,

        /// Emit a JSON array instead of the default plain-text table.
        /// Each element includes all task fields plus `locked_by` and
        /// `lock_expires` when a live lock is present.
        #[arg(long)]
        json: bool,

        /// Render tasks as an indented tree showing parent-child relationships.
        /// Incompatible with --json.
        #[arg(long)]
        tree: bool,

        /// Sort order for results.
        /// Accepted values: `num` (default), `updated`, `created`, `status`.
        #[arg(long, default_value = "num")]
        sort: SortBy,

        /// Print only the count of matching tasks.
        #[arg(long)]
        count: bool,

        /// Filter by priority level.
        /// Accepted values: `low`, `medium`, `high`, `critical`.
        #[arg(long)]
        priority: Option<Priority>,
    },

    /// Show full details for a single task.
    ///
    /// Prints all fields of the task. If a live lock is held, the holder
    /// name and expiry time are also shown.
    Show {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Emit JSON instead of the default human-readable format.
        #[arg(long)]
        json: bool,
    },

    /// Update one or more fields of an existing task.
    ///
    /// At least one of `--title`, `--description`, `--status`, or `--summary`
    /// must be supplied. Omitted fields are left unchanged.
    Update {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Replace the task's title with this value.
        #[arg(long)]
        title: Option<String>,

        /// Replace the task's description with this value.
        #[arg(long)]
        description: Option<String>,

        /// Set the task status.
        /// Accepted values: `open`, `in_progress`, `done`, `cancelled`.
        #[arg(long)]
        status: Option<Status>,

        /// Worker's closing summary: decisions made, approach taken, caveats.
        /// Set this before releasing the lock to leave a record of outcomes.
        #[arg(long)]
        summary: Option<String>,

        /// Update the priority level.
        /// Accepted values: `low`, `medium`, `high`, `critical`.
        #[arg(long)]
        priority: Option<Priority>,
    },

    /// Delete a task permanently.
    ///
    /// Without `--force`, refuses to delete a task that is currently locked
    /// or has status `in_progress`. Use `--force` to override those guards.
    Delete {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Delete even if the task is locked or in progress.
        #[arg(long)]
        force: bool,
    },

    /// Acquire an exclusive lock on a task and set its status to `in_progress`.
    ///
    /// If the task is already locked by a different holder, the command fails
    /// with an error showing who holds the lock and when it expires. Re-acquiring
    /// a lock you already hold simply refreshes the expiry.
    ///
    /// The holder must be identified explicitly via `--holder` or the
    /// `TASK_HOLDER` environment variable. Anonymous acquisition is not allowed.
    Acquire {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Name identifying the agent or process taking ownership.
        /// Falls back to the `TASK_HOLDER` environment variable when omitted.
        /// One of `--holder` or `TASK_HOLDER` must be provided.
        #[arg(long)]
        holder: Option<String>,

        /// Seconds until the lock expires automatically.
        /// Expired locks are ignored and the task becomes acquirable again.
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
    },

    /// Release a lock held on a task.
    ///
    /// Does not change the task's status — call `task update --status done`
    /// (or another status) separately to close out the work.
    ///
    /// Without `--force`, fails if the lock is held by a different holder.
    Release {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Name of the holder releasing the lock.
        /// Falls back to the `TASK_HOLDER` environment variable when omitted.
        /// One of `--holder` or `TASK_HOLDER` must be provided.
        #[arg(long)]
        holder: Option<String>,

        /// Release the lock even if it belongs to a different holder.
        #[arg(long)]
        force: bool,
    },

    /// Extend the expiry of an existing lock.
    ///
    /// The new expiry is calculated from the current time, not from the
    /// existing expiry. Only the current holder may renew; there is no
    /// `--force` override.
    Renew {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Name of the holder renewing the lock. Must match the current holder.
        /// Falls back to the `TASK_HOLDER` environment variable when omitted.
        /// One of `--holder` or `TASK_HOLDER` must be provided.
        #[arg(long)]
        holder: Option<String>,

        /// New lock duration in seconds from now.
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
    },

    /// Report key health metrics about the task database.
    ///
    /// Shows a status breakdown, completion rate, likely-abandoned task count,
    /// stalled in-progress tasks (expired locks), and child-task usage.
    /// Use `--json` for machine-readable output.
    Stats {
        /// Emit a JSON object instead of the default human-readable summary.
        #[arg(long)]
        json: bool,
    },

    /// Show the event history for a task.
    ///
    /// Prints each recorded event in chronological order: timestamp, event type, and actor (if any).
    Log {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Emit JSON instead of the default human-readable format.
        #[arg(long)]
        json: bool,
    },

    /// Search tasks by text query across title, description, and summary.
    ///
    /// Returns tasks where title, description, or summary contains the query string
    /// (case-insensitive LIKE match). Supports the same --status, --parent, and --json
    /// flags as `list`.
    Search {
        /// Text to search for (matched against title, description, and summary).
        query: String,

        /// Only show tasks with this status.
        #[arg(long)]
        status: Option<Status>,

        /// Only show direct children of this parent task ID.
        #[arg(long)]
        parent: Option<String>,

        /// Emit a JSON array instead of the default plain-text table.
        #[arg(long)]
        json: bool,

        /// Sort order for results.
        /// Accepted values: `num` (default), `updated`, `created`, `status`.
        #[arg(long, default_value = "num")]
        sort: SortBy,

        /// Print only the count of matching tasks.
        #[arg(long)]
        count: bool,

        /// Filter by priority level.
        /// Accepted values: `low`, `medium`, `high`, `critical`.
        #[arg(long)]
        priority: Option<Priority>,
    },

    /// Complete a task in one step: optionally set summary, release the lock, and mark done.
    ///
    /// Convenience wrapper for the three commands workers typically run at the end of
    /// a task:
    ///   tasks update <id> --summary "..."
    ///   tasks release <id>
    ///   tasks update <id> --status done
    ///
    /// Example:
    ///   tasks close 0004-fix-auth-bug --summary "Fixed by patching the JWT middleware."
    Close {
        /// Full task ID, e.g. `0001-fix-login-bug`.
        id: String,

        /// Optional closing summary: decisions made, approach taken, caveats.
        #[arg(long)]
        summary: Option<String>,

        /// Name of the holder releasing the lock.
        /// Falls back to the `TASK_HOLDER` environment variable when omitted.
        /// One of `--holder` or `TASK_HOLDER` must be provided.
        #[arg(long)]
        holder: Option<String>,
    },

    /// Reap expired locks and reset stalled in-progress tasks to open.
    Gc {
        /// Preview what would be recovered without making changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Generate a shell completion script and print it to stdout.
    ///
    /// Source the output in your shell profile to enable tab-completion for all
    /// subcommands, flags, and `--status` values.
    ///
    /// PowerShell (one-time setup):
    ///   tasks completions powershell | Out-File -Append $PROFILE
    ///
    /// Bash:
    ///   tasks completions bash >> ~/.bashrc
    Completions {
        /// The shell to generate completions for (bash, zsh, fish, powershell, elvish).
        shell: clap_complete::Shell,
    },
}

/// Resolves the holder name from the CLI flag or the `TASK_HOLDER` env var.
/// Errors if neither is set, since anonymous task ownership is not permitted.
fn resolve_holder(flag: Option<String>) -> anyhow::Result<String> {
    flag.or_else(|| std::env::var("TASK_HOLDER").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "a holder name is required: pass --holder or set the TASK_HOLDER environment variable"
            )
        })
}

fn print_task_table(tasks: &[models::Task]) {
    use comfy_table::presets::UTF8_FULL_CONDENSED;
    use comfy_table::{Cell, ContentArrangement, Table};
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["ID", "STATUS", "PRIORITY", "TITLE", "LOCKED"]);
    for task in tasks {
        let lock_marker = if task.locked_by.is_some() {
            "\u{1f512}"
        } else {
            ""
        };
        table.add_row(vec![
            Cell::new(&task.id),
            Cell::new(&task.status),
            Cell::new(task.priority),
            Cell::new(&task.title),
            Cell::new(lock_marker),
        ]);
    }
    println!("{table}");
}

fn print_task_human(task: &models::Task) {
    println!("id:          {}", task.id);
    println!("title:       {}", task.title);
    println!("status:      {}", task.status);
    if let Some(p) = &task.parent_id {
        println!("parent:      {p}");
    }
    if let Some(d) = &task.description {
        println!("description: {d}");
    }
    if let Some(s) = &task.summary {
        println!("summary:     {s}");
    }
    println!(
        "created_at:  {}",
        task.created_at.format("%Y-%m-%dT%H:%M:%SZ")
    );
    println!(
        "updated_at:  {}",
        task.updated_at.format("%Y-%m-%dT%H:%M:%SZ")
    );
    if let Some(holder) = &task.locked_by {
        let expires = task
            .lock_expires
            .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();
        println!("locked_by:   {holder}  (until {expires})");
    }
}

fn print_node(
    task: &models::Task,
    children: &std::collections::HashMap<Option<&str>, Vec<&models::Task>>,
    prefix: &str,
    is_last: bool,
) {
    let connector = if is_last { "└─ " } else { "├─ " };
    let lock = if task.locked_by.is_some() {
        " 🔒"
    } else {
        ""
    };
    println!(
        "{prefix}{connector}[{}] {} {}{lock}",
        task.status, task.id, task.title
    );

    let child_prefix = format!("{}{}", prefix, if is_last { "   " } else { "│  " });
    if let Some(kids) = children.get(&Some(task.id.as_str())) {
        for (i, kid) in kids.iter().enumerate() {
            print_node(kid, children, &child_prefix, i == kids.len() - 1);
        }
    }
}

fn print_tree(tasks: &[models::Task], root_parent: Option<&str>) {
    use std::collections::HashMap;

    // Build parent → children index
    let mut children: HashMap<Option<&str>, Vec<&models::Task>> = HashMap::new();
    for task in tasks {
        children
            .entry(task.parent_id.as_deref())
            .or_default()
            .push(task);
    }

    // Get top-level tasks (those whose parent matches root_parent)
    let roots = children.get(&root_parent).cloned().unwrap_or_default();
    for (i, root) in roots.iter().enumerate() {
        print_node(root, &children, "", i == roots.len() - 1);
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Handle completions before opening the database — no DB required.
    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "tasks", &mut std::io::stdout());
        return Ok(());
    }

    let db_path = cli
        .db
        .or_else(|| std::env::var("TASKS_DB").ok())
        .unwrap_or_else(|| "tasks.db".to_string());
    let conn = db::open_db(&db_path)?;

    match cli.command {
        Command::Migrate => {
            db::migrate(&conn)?;
            println!("migrated");
        }

        Command::Create {
            title_pos,
            title,
            description,
            id,
            parent,
            priority,
        } => {
            let title = title.or(title_pos).ok_or_else(|| {
                anyhow::anyhow!(
                    "title is required: pass it as a positional argument or with --title"
                )
            })?;
            let slug = id.unwrap_or_else(|| db::slugify(&title));
            let full_id = db::create_task(
                &conn,
                &slug,
                &title,
                description.as_deref(),
                parent.as_deref(),
                priority,
            )?;
            println!("{full_id}");
        }

        Command::List {
            status,
            parent,
            all,
            json,
            tree,
            sort,
            count,
            priority,
        } => {
            if tree && json {
                anyhow::bail!("--tree and --json cannot be used together");
            }
            if tree {
                // Fetch all tasks (ignoring parent filter for tree — we show the whole tree)
                let tasks = db::list_tasks(&conn, status.as_ref(), None, true, SortBy::Num, None)?;
                print_tree(&tasks, parent.as_deref());
            } else {
                let tasks = db::list_tasks(
                    &conn,
                    status.as_ref(),
                    parent.as_deref(),
                    all,
                    sort,
                    priority.as_ref(),
                )?;
                if count {
                    println!("{}", tasks.len());
                    return Ok(());
                }
                if json {
                    println!("{}", serde_json::to_string_pretty(&tasks)?);
                } else {
                    print_task_table(&tasks);
                }
            }
        }

        Command::Show { id, json } => {
            let id = db::resolve_id(&conn, &id)?;
            match db::get_task(&conn, &id)? {
                None => anyhow::bail!("task not found: {id}"),
                Some(task) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&task)?);
                    } else {
                        print_task_human(&task);
                    }
                }
            }
        }

        Command::Update {
            id,
            title,
            description,
            status,
            summary,
            priority,
        } => {
            if title.is_none()
                && description.is_none()
                && status.is_none()
                && summary.is_none()
                && priority.is_none()
            {
                anyhow::bail!(
                    "at least one of --title, --description, --status, --summary, --priority is required"
                );
            }
            let id = db::resolve_id(&conn, &id)?;
            let found = db::update_task(
                &conn,
                &id,
                title.as_deref(),
                description.as_deref(),
                status.as_ref(),
                summary.as_deref(),
                priority,
            )?;
            if !found {
                anyhow::bail!("task not found: {id}");
            }
            println!("updated {id}");
        }

        Command::Delete { id, force } => {
            let id = db::resolve_id(&conn, &id)?;
            let found = db::delete_task(&conn, &id, force)?;
            if !found {
                anyhow::bail!("task not found: {id}");
            }
            println!("deleted {id}");
        }

        Command::Acquire { id, holder, ttl } => {
            let id = db::resolve_id(&conn, &id)?;
            let holder = resolve_holder(holder)?;
            db::acquire_task(&conn, &id, &holder, ttl)?;
            println!("acquired {id} by {holder}");
        }

        Command::Release { id, holder, force } => {
            let id = db::resolve_id(&conn, &id)?;
            let found = if force {
                db::release_task(&conn, &id, "", true)?
            } else {
                let holder = resolve_holder(holder)?;
                db::release_task(&conn, &id, &holder, false)?
            };
            if !found {
                anyhow::bail!("task not found or has no lock: {id}");
            }
            println!("released {id}");
        }

        Command::Renew { id, holder, ttl } => {
            let id = db::resolve_id(&conn, &id)?;
            let holder = resolve_holder(holder)?;
            db::renew_task(&conn, &id, &holder, ttl)?;
            println!("renewed {id} for {ttl}s");
        }

        Command::Log { id, json } => {
            let id = db::resolve_id(&conn, &id)?;
            let events = db::get_events(&conn, &id)?;
            if json {
                // serialize as array of {at, event, actor} objects
                let v: Vec<_> = events.iter().map(|(event, actor, at)| {
                    serde_json::json!({"at": at, "event": event, "actor": actor})
                }).collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                for (event, actor, at) in &events {
                    if let Some(a) = actor {
                        println!("{at}  {event}  ({a})");
                    } else {
                        println!("{at}  {event}");
                    }
                }
            }
        }

        Command::Search {
            query,
            status,
            parent,
            json,
            sort,
            count,
            priority,
        } => {
            let tasks = db::search_tasks(
                &conn,
                &query,
                status.as_ref(),
                parent.as_deref(),
                sort,
                priority.as_ref(),
            )?;
            if count {
                println!("{}", tasks.len());
                return Ok(());
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else {
                print_task_table(&tasks);
            }
        }

        Command::Close {
            id,
            summary,
            holder,
        } => {
            let id = db::resolve_id(&conn, &id)?;
            let holder = resolve_holder(holder)?;
            // Optionally set summary first.
            if let Some(s) = summary {
                let found = db::update_task(&conn, &id, None, None, None, Some(s.as_str()), None)?;
                if !found {
                    anyhow::bail!("task not found: {id}");
                }
            }
            // Release the lock (fails if held by a different holder).
            db::release_task(&conn, &id, &holder, false)?;
            // Mark done.
            let found = db::update_task(&conn, &id, None, None, Some(&Status::Done), None, None)?;
            if !found {
                anyhow::bail!("task not found: {id}");
            }
            println!("closed {id}");
        }

        Command::Gc { dry_run } => {
            let ids = db::gc_tasks(&conn, dry_run)?;
            if ids.is_empty() {
                println!("nothing to recover");
            } else {
                let id_list = ids.join(", ");
                if dry_run {
                    println!(
                        "dry-run: would recover {} stalled tasks: {id_list}",
                        ids.len()
                    );
                } else {
                    println!("recovered {} stalled tasks: {id_list}", ids.len());
                }
            }
        }

        // Handled before db::open_db() above; unreachable here.
        Command::Completions { .. } => unreachable!(),

        Command::Stats { json } => {
            let s = db::stats(&conn)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                println!("total:            {}", s.total);
                println!("  open:           {}", s.open);
                println!("  in_progress:    {}", s.in_progress);
                println!("  done:           {}", s.done);
                println!("  cancelled:      {}", s.cancelled);
                if let Some(pct) = s.completion_pct {
                    println!("completion:       {pct:.1}%");
                } else {
                    println!("completion:       n/a");
                }
                println!("likely_abandoned: {}", s.likely_abandoned);
                println!("stalled:          {}", s.stalled);
                println!("child_tasks:      {}", s.child_tasks);
                println!("parent_tasks:     {}", s.parent_tasks);
            }
        }
    }

    Ok(())
}
