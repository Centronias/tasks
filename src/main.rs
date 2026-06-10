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

use clap::{Parser, Subcommand};
use models::Status;
use std::str::FromStr;

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

        /// Optional closing summary (decisions, approach, caveats).
        #[arg(long)]
        summary: Option<String>,
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
        #[arg(long, value_parser = parse_status)]
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
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,

        /// Worker's closing summary: decisions made, approach taken, caveats.
        /// Set this before releasing the lock to leave a record of outcomes.
        #[arg(long)]
        summary: Option<String>,
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
}

fn parse_status(s: &str) -> Result<Status, String> {
    Status::from_str(s)
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

fn print_task_human(task: &models::Task) {
    println!("id:          {}", task.id);
    if let Some(p) = &task.parent_id {
        println!("parent:      {p}");
    }
    println!("title:       {}", task.title);
    if let Some(d) = &task.description {
        println!("description: {d}");
    }
    if let Some(s) = &task.summary {
        println!("summary:     {s}");
    }
    println!("status:      {}", task.status);
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let conn = db::open_db()?;

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
            summary,
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
            )?;
            if let Some(s) = summary {
                db::update_task(&conn, &full_id, None, None, None, Some(s.as_str()))?;
            }
            println!("{full_id}");
        }

        Command::List {
            status,
            parent,
            all,
            json,
        } => {
            let tasks = db::list_tasks(&conn, status.as_ref(), parent.as_deref(), all)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else {
                use comfy_table::presets::UTF8_FULL_CONDENSED;
                use comfy_table::{Cell, ContentArrangement, Table};
                let mut table = Table::new();
                table
                    .load_preset(UTF8_FULL_CONDENSED)
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(vec!["ID", "STATUS", "TITLE", "LOCKED"]);
                for task in &tasks {
                    let lock_marker = if task.locked_by.is_some() {
                        "\u{1f512}"
                    } else {
                        ""
                    };
                    table.add_row(vec![
                        Cell::new(&task.id),
                        Cell::new(&task.status),
                        Cell::new(&task.title),
                        Cell::new(lock_marker),
                    ]);
                }
                println!("{table}");
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
        } => {
            if title.is_none() && description.is_none() && status.is_none() && summary.is_none() {
                anyhow::bail!(
                    "at least one of --title, --description, --status, --summary is required"
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
