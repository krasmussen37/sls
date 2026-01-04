//! Log source connectors for SLS
//!
//! Each connector knows how to read from a specific log source type:
//! - AgentStderrConnector: Parse Claude/Codex/Gemini JSONL session files
//! - JournaldConnector: Read from systemd journal
//! - SyslogConnector: Parse /var/log/syslog format
//! - DockerConnector: Read Docker JSON logs
//! - JsonConnector: Generic JSON/JSONL files

pub mod agent_stderr;
pub mod journald;
pub mod syslog;
// Future connectors:
// pub mod docker;
// pub mod json;

use anyhow::Result;
use chrono::{DateTime, Utc};

/// A parsed log entry from any connector
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Source-specific identifier
    pub source_id: Option<i64>,
    /// When the log event occurred
    pub timestamp: DateTime<Utc>,
    /// Log level (ERROR, WARN, INFO, DEBUG, etc.)
    pub level: Option<String>,
    /// Service or application name
    pub service: Option<String>,
    /// Hostname where the log originated
    pub hostname: Option<String>,
    /// The actual log message
    pub message: String,
    /// Original raw line (for context)
    pub raw_line: Option<String>,
    /// Any structured data as JSON
    pub structured_data: Option<String>,
    /// Fingerprint for deduplication/pattern matching
    pub fingerprint: Option<String>,
}

/// Trait that all log connectors must implement
pub trait Connector: Send + Sync {
    /// Human-readable name for this connector type
    fn name(&self) -> &str;

    /// Get the source type identifier (e.g., "journald", "agent_stderr")
    fn source_type(&self) -> &str;

    /// Check if this connector can read from the given path/source
    fn can_read(&self, path: &str) -> bool;

    /// Read log entries from the source
    /// If `since` is provided, only read entries after that position
    fn read_entries(&self, since: Option<&str>) -> Result<Vec<LogEntry>>;

    /// Get the current position marker (for incremental reads)
    fn get_position(&self) -> Option<String>;
}

/// Registry of available connectors
pub struct ConnectorRegistry {
    connectors: Vec<Box<dyn Connector>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self {
            connectors: Vec::new(),
        }
    }

    pub fn register(&mut self, connector: Box<dyn Connector>) {
        self.connectors.push(connector);
    }

    pub fn find_for_source(&self, path: &str) -> Option<&dyn Connector> {
        self.connectors
            .iter()
            .find(|c| c.can_read(path))
            .map(|c| c.as_ref())
    }

    pub fn all(&self) -> &[Box<dyn Connector>] {
        &self.connectors
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
