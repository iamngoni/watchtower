use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// ============================================================================
// Events
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Event {
    pub id: i64,
    pub event_type: String,
    pub summary: String,
    pub detail: Option<String>,
    pub session_id: Option<String>,
    pub task_id: Option<i64>,
    pub metadata: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEvent {
    pub event_type: String,
    pub summary: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<i64>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

// ============================================================================
// Tasks
// ============================================================================

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Urgent => write!(f, "urgent"),
        }
    }
}

impl std::str::FromStr for TaskPriority {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "normal" => Ok(Self::Normal),
            "high" => Ok(Self::High),
            "urgent" => Ok(Self::Urgent),
            _ => Err(format!("Invalid priority: {}", s)),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Backlog,
    Todo,
    InProgress,
    Blocked,
    InReview,
    Done,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backlog => write!(f, "backlog"),
            Self::Todo => write!(f, "todo"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Blocked => write!(f, "blocked"),
            Self::InReview => write!(f, "in_review"),
            Self::Done => write!(f, "done"),
        }
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "backlog" => Ok(Self::Backlog),
            "todo" => Ok(Self::Todo),
            "in_progress" => Ok(Self::InProgress),
            "blocked" => Ok(Self::Blocked),
            "in_review" => Ok(Self::InReview),
            "done" => Ok(Self::Done),
            _ => Err(format!("Invalid status: {}", s)),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Human,
    Agent,
    Unassigned,
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Human => write!(f, "human"),
            Self::Agent => write!(f, "agent"),
            Self::Unassigned => write!(f, "unassigned"),
        }
    }
}

impl std::str::FromStr for Actor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "agent" => Ok(Self::Agent),
            "unassigned" => Ok(Self::Unassigned),
            _ => Err(format!("Invalid actor: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub priority: String,
    pub status: String,
    pub labels: String,
    pub created_by: String,
    pub assigned_to: String,
    pub due_date: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTask {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default = "default_actor_human")]
    pub created_by: String,
    #[serde(default = "default_actor_unassigned")]
    pub assigned_to: String,
    #[serde(default)]
    pub due_date: Option<i64>,
}

fn default_priority() -> String { "normal".to_string() }
fn default_status() -> String { "backlog".to_string() }
fn default_actor_human() -> String { "human".to_string() }
fn default_actor_unassigned() -> String { "unassigned".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateTask {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub assigned_to: Option<String>,
    #[serde(default)]
    pub due_date: Option<i64>,
}

// ============================================================================
// Task Comments
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TaskComment {
    pub id: i64,
    pub task_id: i64,
    pub author: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateComment {
    pub content: String,
    #[serde(default = "default_actor_human")]
    pub author: String,
}

// ============================================================================
// Task History
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TaskHistory {
    pub id: i64,
    pub task_id: i64,
    pub from_status: Option<String>,
    pub to_status: String,
    pub changed_by: String,
    pub created_at: i64,
}

// ============================================================================
// Sessions
// ============================================================================

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    Conversation,
    SubAgent,
    Cron,
}

impl std::fmt::Display for SessionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conversation => write!(f, "conversation"),
            Self::SubAgent => write!(f, "sub_agent"),
            Self::Cron => write!(f, "cron"),
        }
    }
}

impl std::str::FromStr for SessionType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "conversation" => Ok(Self::Conversation),
            "sub_agent" => Ok(Self::SubAgent),
            "cron" => Ok(Self::Cron),
            _ => Err(format!("Invalid session type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Session {
    pub id: i64,
    pub session_key: String,
    pub title: Option<String>,
    pub session_type: String,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
    pub task_id: Option<i64>,
    pub parent_session_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSession {
    pub session_key: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_session_type")]
    pub session_type: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub task_id: Option<i64>,
    #[serde(default)]
    pub parent_session_id: Option<i64>,
}

fn default_session_type() -> String { "conversation".to_string() }

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateSession {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub cache_read_tokens: Option<i64>,
    #[serde(default)]
    pub cache_write_tokens: Option<i64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub ended_at: Option<i64>,
}

// ============================================================================
// Cron Jobs
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CronJob {
    pub id: i64,
    pub job_id: String,
    pub name: String,
    pub schedule: String,
    pub enabled: i64,
    pub last_status: Option<String>,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub consecutive_errors: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncCronJob {
    pub job_id: String,
    pub name: String,
    pub schedule: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub consecutive_errors: i64,
}

fn default_enabled() -> bool { true }

// ============================================================================
// Usage
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UsageDaily {
    pub id: i64,
    pub date: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportUsage {
    pub date: String,
    pub model: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_usd: f64,
    pub by_model: Vec<ModelUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

// ============================================================================
// SSE Message
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseMessage {
    pub event: String,
    pub data: serde_json::Value,
    /// Raw HTML string to send instead of JSON-serialized data (for HTMX sse-swap)
    #[serde(skip)]
    pub raw_html: Option<String>,
}
