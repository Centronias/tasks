use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    InProgress,
    Done,
    Cancelled,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Done => write!(f, "done"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FromStr for Status {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "in_progress" => Ok(Self::InProgress),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown status: {other}")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub num: i64,
    pub title: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: Status,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_expires: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::Status;
    use rstest::rstest;

    #[rstest]
    #[case(Status::Open, "open")]
    #[case(Status::InProgress, "in_progress")]
    #[case(Status::Done, "done")]
    #[case(Status::Cancelled, "cancelled")]
    fn status_display_roundtrip(#[case] status: Status, #[case] s: &str) {
        assert_eq!(status.to_string(), s);
        assert_eq!(s.parse::<Status>().unwrap(), status);
    }

    #[test]
    fn status_from_str_invalid() {
        assert!("pending".parse::<Status>().is_err());
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Lock {
    pub task_id: String,
    pub holder: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Aggregate metrics about the task database, returned by `tasks stats`.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskStats {
    /// Total number of tasks (all statuses).
    pub total: i64,
    /// Tasks currently open.
    pub open: i64,
    /// Tasks currently `in_progress`.
    pub in_progress: i64,
    /// Tasks marked done.
    pub done: i64,
    /// Tasks marked cancelled.
    pub cancelled: i64,
    /// Percentage of non-cancelled tasks that reached `done`. Null when there
    /// are no non-cancelled tasks yet.
    pub completion_pct: Option<f64>,
    /// Open tasks whose `updated_at` is more than 5 seconds after `created_at`,
    /// used as a proxy for tasks that were returned to the queue after an
    /// agent started work on them.
    pub likely_abandoned: i64,
    /// In-progress tasks whose lock has expired (or has no lock record at all) —
    /// a proxy for stalled or dead agents.
    pub stalled: i64,
    /// Tasks that have a `parent_id` set (i.e. are children of another task).
    pub child_tasks: i64,
    /// Distinct parent tasks that have at least one child.
    pub parent_tasks: i64,
}
