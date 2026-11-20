use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Short hex hash identifier for a task.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TaskId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for TaskId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Execution state of a task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Todo,
    InProgress,
    Done,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        };
        f.write_str(label)
    }
}

impl FromStr for Status {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "todo" => Ok(Self::Todo),
            "in_progress" | "in-progress" | "doing" | "wip" => Ok(Self::InProgress),
            "done" | "completed" => Ok(Self::Done),
            other => Err(format!(
                "invalid status '{other}': expected todo, in_progress, or done"
            )),
        }
    }
}

/// Task priority ordered from lowest (`Low`) to highest (`Critical`).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        };
        f.write_str(label)
    }
}

impl FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" | "l" | "p3" => Ok(Self::Low),
            "medium" | "med" | "m" | "p2" => Ok(Self::Medium),
            "high" | "h" | "p1" => Ok(Self::High),
            "critical" | "crit" | "c" | "p0" => Ok(Self::Critical),
            other => Err(format!(
                "invalid priority '{other}': expected low, medium, high, or critical"
            )),
        }
    }
}

/// Core task record stored one JSON file per task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub status: Status,
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub depends_on: BTreeSet<TaskId>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub related: BTreeSet<TaskId>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Task {
    /// Creates a new `Todo` task with a generated short hex ID and current timestamps.
    pub fn new(title: impl Into<String>, priority: Priority) -> Self {
        let title = title.into();
        let now = now_epoch_secs();
        Self {
            id: generate_id(&title),
            title,
            description: String::new(),
            status: Status::Todo,
            priority,
            depends_on: BTreeSet::new(),
            related: BTreeSet::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Returns current Unix timestamp in seconds.
pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generates a 6-character lowercase hex hash using 64-bit FNV-1a over time, counter, PID, and title.
pub fn generate_id(title: &str) -> TaskId {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());

    let mut hash = FNV_OFFSET;
    for byte in nanos
        .to_le_bytes()
        .into_iter()
        .chain(seq.to_le_bytes())
        .chain(pid.to_le_bytes())
        .chain(title.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    let folded = (hash ^ (hash >> 24) ^ (hash >> 48)) & 0x00ff_ffff;
    TaskId(format!("{folded:06x}"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn json_roundtrip_preserves_fields() {
        let mut task = Task::new("Write graph engine", Priority::High);
        task.description = "Add cycle detection and layering".into();
        task.status = Status::InProgress;
        task.depends_on.insert(TaskId::from("a1b2c3"));
        task.related.insert(TaskId::from("d4e5f6"));

        let json = serde_json::to_string_pretty(&task).expect("serialize task");
        let parsed: Task = serde_json::from_str(&json).expect("deserialize task");

        assert_eq!(parsed, task);
    }

    #[test]
    fn generate_id_produces_unique_six_char_hex_hashes() {
        let mut ids = HashSet::new();
        for i in 0..200 {
            let id = generate_id(&format!("task-{i}"));
            assert_eq!(id.as_str().len(), 6);
            assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
            assert!(ids.insert(id), "duplicate id generated at iteration {i}");
        }
    }

    #[test]
    fn priority_orders_from_low_to_critical() {
        assert!(Priority::Low < Priority::Medium);
        assert!(Priority::Medium < Priority::High);
        assert!(Priority::High < Priority::Critical);
    }

    #[test]
    fn parses_status_and_priority_aliases() {
        assert_eq!("in-progress".parse::<Status>(), Ok(Status::InProgress));
        assert_eq!("wip".parse::<Status>(), Ok(Status::InProgress));
        assert_eq!("p0".parse::<Priority>(), Ok(Priority::Critical));
        assert_eq!("med".parse::<Priority>(), Ok(Priority::Medium));
        assert!("unknown".parse::<Priority>().is_err());
    }
}
