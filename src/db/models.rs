//! Database models for SLS

use serde::{Deserialize, Serialize};

/// Log source configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSource {
    pub id: i64,
    pub source_type: String,
    pub source_path: Option<String>,
    pub last_position: Option<String>,
    pub active: bool,
    pub discovered: bool,
    pub confidence: Option<i32>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Stored log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLogEntry {
    pub id: i64,
    pub source_id: Option<i64>,
    pub timestamp_utc: i64,
    pub level: Option<String>,
    pub service: Option<String>,
    pub hostname: Option<String>,
    pub message: String,
    pub raw_line: Option<String>,
    pub structured_data: Option<String>,
    pub fingerprint: Option<String>,
    pub indexed_at: i64,
}

/// Agent tool output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolOutput {
    pub id: i64,
    pub log_entry_id: Option<i64>,
    pub agent_type: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub exit_code: Option<i32>,
    pub stderr: Option<String>,
    pub workspace: Option<String>,
    pub created_at: i64,
}
