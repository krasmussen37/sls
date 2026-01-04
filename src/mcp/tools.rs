//! MCP Tool definitions for SLS
//!
//! Defines the tool parameters and result types for MCP integration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for sls_search tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query string to match against log messages
    #[schemars(description = "Search query to match against log messages")]
    pub query: String,

    /// Filter by log level (ERROR, WARN, INFO, DEBUG)
    #[schemars(description = "Filter by log level (ERROR, WARN, INFO, DEBUG)")]
    pub level: Option<String>,

    /// Filter by service name
    #[schemars(description = "Filter by service name")]
    pub service: Option<String>,

    /// Time range (e.g., "1h", "30m", "1d")
    #[schemars(description = "Time range filter (e.g., '1h', '30m', '1d')")]
    pub since: Option<String>,

    /// Maximum results to return (default: 50)
    #[schemars(description = "Maximum number of results to return (default: 50)")]
    pub limit: Option<usize>,
}

/// Result entry from search
#[derive(Debug, Clone, Serialize)]
pub struct SearchResultEntry {
    pub id: i64,
    pub timestamp: String,
    pub level: Option<String>,
    pub service: Option<String>,
    pub message: String,
}

/// Result from sls_search tool
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub success: bool,
    pub query: String,
    pub total_matches: usize,
    pub entries: Vec<SearchResultEntry>,
}

/// Parameters for sls_alert tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AlertParams {
    /// Time range (e.g., "1h", "30m", "1d")
    #[schemars(description = "Time range to check (e.g., '1h', '30m', '1d')")]
    pub since: Option<String>,

    /// Error count threshold for CRITICAL status (default: 1)
    #[schemars(description = "Error count threshold for CRITICAL status (default: 1)")]
    pub error_threshold: Option<usize>,

    /// Warning count threshold for WARNING status (default: 10)
    #[schemars(description = "Warning count threshold for WARNING status (default: 10)")]
    pub warning_threshold: Option<usize>,

    /// Filter by service name
    #[schemars(description = "Filter by service name")]
    pub service: Option<String>,
}

/// Alert status levels
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Ok,
    Warning,
    Critical,
}

/// Result from sls_alert tool
#[derive(Debug, Clone, Serialize)]
pub struct AlertResult {
    pub status: AlertStatus,
    pub error_count: usize,
    pub warning_count: usize,
    pub error_threshold: usize,
    pub warning_threshold: usize,
    pub time_range: String,
    pub message: String,
}

/// Parameters for sls_tail tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TailParams {
    /// Source type (journald, syslog, file)
    #[schemars(description = "Log source type: journald, syslog, or file")]
    pub source: Option<String>,

    /// Path to log file (for syslog or file sources)
    #[schemars(description = "Path to log file (for syslog or file sources)")]
    pub path: Option<String>,

    /// Systemd unit filter (for journald)
    #[schemars(description = "Systemd unit filter (for journald source)")]
    pub unit: Option<String>,

    /// Number of recent lines to return
    #[schemars(description = "Number of recent lines to return (default: 50)")]
    pub lines: Option<usize>,
}

/// Result from sls_tail tool
#[derive(Debug, Clone, Serialize)]
pub struct TailResult {
    pub success: bool,
    pub source: String,
    pub entries: Vec<TailEntry>,
}

/// Single entry from tail
#[derive(Debug, Clone, Serialize)]
pub struct TailEntry {
    pub timestamp: String,
    pub level: Option<String>,
    pub message: String,
}

/// Result from sls_capabilities tool
#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesResult {
    pub name: String,
    pub version: String,
    pub description: String,
    pub commands: Vec<CommandInfo>,
}

/// Command info for capabilities
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
}
