//! Database models for SLS — OTel-aligned schema

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

/// Stored log entry — OTel LogRecord aligned.
///
/// Field mapping:
///   severity_number  → OTel SeverityNumber (0-24)
///   severity_text    → OTel SeverityText
///   body             → OTel Body (was: message)
///   service_name     → OTel Resource: service.name (was: service)
///   service_version  → OTel Resource: service.version
///   hostname         → OTel Resource: host.name
///   attributes       → OTel Attributes JSON (was: structured_data)
///   scope_name       → OTel InstrumentationScope.name
///   raw_line         → OTel Attribute: log.record.original
///   fingerprint      → OTel Attribute: log.record.uid
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLogEntry {
    pub id: i64,
    pub source_id: Option<i64>,
    pub timestamp_utc: i64,
    // OTel severity fields
    pub severity_number: u8,
    pub severity_text: Option<String>,
    // OTel body
    pub body: String,
    // OTel resource fields (flattened)
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub hostname: Option<String>,
    // OTel attributes
    pub attributes: Option<String>,
    // OTel instrumentation scope
    pub scope_name: Option<String>,
    // Dedup + original
    pub raw_line: Option<String>,
    pub fingerprint: Option<String>,
    // SLS internal
    pub indexed_at: i64,
}

/// Agent tool output (enrichment table for agent-specific data)
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
