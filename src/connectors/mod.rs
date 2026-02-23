//! Log source connectors for SLS
//!
//! All connectors emit LogEntry structs aligned with the OpenTelemetry Logs
//! Data Model (https://opentelemetry.io/docs/specs/otel/logs/data-model/).
//! This gives SLS a unified schema regardless of source format.
//!
//! Connectors:
//! - AgentStderrConnector: Claude Code JSONL session files
//! - CodexConnector: Codex CLI TUI log
//! - GeminiConnector: Gemini CLI session JSON files
//! - JournaldConnector: systemd journal
//! - SyslogConnector: /var/log/syslog format
//! - OpenClawConnector: OpenClaw gateway JSONL logs

pub mod agent_stderr;
pub mod codex;
pub mod gemini;
pub mod journald;
pub mod openclaw;
pub mod syslog;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// OTel-aligned severity levels.
/// Maps to OpenTelemetry SeverityNumber (0-24).
/// See: https://opentelemetry.io/docs/specs/otel/logs/data-model/#severity-fields
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Severity {
    Unspecified = 0,
    Trace = 1,
    Debug = 5,
    Info = 9,
    Warn = 13,
    Error = 17,
    Fatal = 21,
}

impl Severity {
    /// Convert from OTel severity number (0-24)
    pub fn from_number(n: u8) -> Self {
        match n {
            0 => Severity::Unspecified,
            1..=4 => Severity::Trace,
            5..=8 => Severity::Debug,
            9..=12 => Severity::Info,
            13..=16 => Severity::Warn,
            17..=20 => Severity::Error,
            21..=24 => Severity::Fatal,
            _ => Severity::Unspecified,
        }
    }

    /// Parse from common log level strings (case-insensitive)
    pub fn from_text(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "TRACE" => Severity::Trace,
            "DEBUG" => Severity::Debug,
            "INFO" | "INFORMATIONAL" | "NOTICE" => Severity::Info,
            "WARN" | "WARNING" => Severity::Warn,
            "ERROR" | "ERR" | "CRITICAL" | "CRIT" => Severity::Error,
            "FATAL" | "EMERG" | "EMERGENCY" | "ALERT" | "PANIC" => Severity::Fatal,
            _ => Severity::Unspecified,
        }
    }

    /// OTel severity number
    pub fn number(&self) -> u8 {
        *self as u8
    }

    /// OTel severity text (short name)
    pub fn text(&self) -> &'static str {
        match self {
            Severity::Unspecified => "UNSPECIFIED",
            Severity::Trace => "TRACE",
            Severity::Debug => "DEBUG",
            Severity::Info => "INFO",
            Severity::Warn => "WARN",
            Severity::Error => "ERROR",
            Severity::Fatal => "FATAL",
        }
    }

    /// Map journald/syslog priority (0-7) to OTel severity
    pub fn from_syslog_priority(priority: u8) -> Self {
        match priority {
            0 => Severity::Fatal,  // emerg
            1 => Severity::Fatal,  // alert
            2 => Severity::Error,  // crit
            3 => Severity::Error,  // err
            4 => Severity::Warn,   // warning
            5 => Severity::Info,   // notice
            6 => Severity::Info,   // info
            7 => Severity::Debug,  // debug
            _ => Severity::Unspecified,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text())
    }
}

/// A parsed log entry aligned with the OTel LogRecord data model.
///
/// Field mapping to OTel:
///   timestamp        → Timestamp
///   severity         → SeverityNumber + SeverityText
///   body             → Body (string form)
///   service_name     → Resource: service.name
///   service_version  → Resource: service.version
///   hostname         → Resource: host.name
///   attributes       → Attributes (JSON-encoded)
///   raw_line         → Attribute: log.record.original
///   fingerprint      → Attribute: log.record.uid
///   source_id        → SLS internal (maps to log_sources.id)
///   scope_name       → InstrumentationScope.name
#[derive(Debug, Clone)]
pub struct LogEntry {
    // --- SLS internal ---
    /// FK to log_sources table
    pub source_id: Option<i64>,

    // --- OTel top-level fields ---
    /// When the event occurred (OTel: Timestamp)
    pub timestamp: DateTime<Utc>,
    /// Normalized severity (OTel: SeverityNumber + SeverityText)
    pub severity: Severity,
    /// The log message body (OTel: Body)
    pub body: String,

    // --- OTel Resource fields (flattened) ---
    /// service.name — the service that produced this log
    pub service_name: Option<String>,
    /// service.version
    pub service_version: Option<String>,
    /// host.name
    pub hostname: Option<String>,

    // --- OTel Attributes (JSON-encoded for flexibility) ---
    /// Structured attributes as JSON (OTel: Attributes)
    pub attributes: Option<String>,

    // --- OTel InstrumentationScope ---
    /// Instrumentation scope name (e.g., "sls.connector.openclaw")
    pub scope_name: Option<String>,

    // --- Deduplication ---
    /// Original raw line (OTel attribute: log.record.original)
    pub raw_line: Option<String>,
    /// Dedup fingerprint (OTel attribute: log.record.uid)
    pub fingerprint: Option<String>,
}

impl LogEntry {
    /// Convenience: create a minimal entry
    pub fn new(timestamp: DateTime<Utc>, severity: Severity, body: impl Into<String>) -> Self {
        Self {
            source_id: None,
            timestamp,
            severity,
            body: body.into(),
            service_name: None,
            service_version: None,
            hostname: None,
            attributes: None,
            scope_name: None,
            raw_line: None,
            fingerprint: None,
        }
    }

    // ---- Legacy compatibility helpers ----
    // These allow existing connectors to migrate incrementally.

    /// Build a LogEntry from old-style fields (level string, service, message).
    /// Used by connectors during migration from the pre-OTel schema.
    pub fn from_legacy(
        timestamp: DateTime<Utc>,
        level: Option<&str>,
        service: Option<String>,
        hostname: Option<String>,
        message: String,
        raw_line: Option<String>,
        structured_data: Option<String>,
        fingerprint: Option<String>,
    ) -> Self {
        let severity = level.map(Severity::from_text).unwrap_or(Severity::Unspecified);
        Self {
            source_id: None,
            timestamp,
            severity,
            body: message,
            service_name: service,
            service_version: None,
            hostname,
            attributes: structured_data,
            scope_name: None,
            raw_line,
            fingerprint,
        }
    }
}

/// Trait that all log connectors must implement
pub trait Connector: Send + Sync {
    /// Human-readable name for this connector type
    fn name(&self) -> &str;

    /// Source type identifier (e.g., "journald", "openclaw")
    fn source_type(&self) -> &str;

    /// Check if this connector can read from the given path/source
    fn can_read(&self, path: &str) -> bool;

    /// Read log entries from the source.
    /// If `since` is provided, only read entries after that position.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_from_text() {
        assert_eq!(Severity::from_text("ERROR"), Severity::Error);
        assert_eq!(Severity::from_text("error"), Severity::Error);
        assert_eq!(Severity::from_text("WARN"), Severity::Warn);
        assert_eq!(Severity::from_text("WARNING"), Severity::Warn);
        assert_eq!(Severity::from_text("INFO"), Severity::Info);
        assert_eq!(Severity::from_text("NOTICE"), Severity::Info);
        assert_eq!(Severity::from_text("DEBUG"), Severity::Debug);
        assert_eq!(Severity::from_text("FATAL"), Severity::Fatal);
        assert_eq!(Severity::from_text("EMERG"), Severity::Fatal);
        assert_eq!(Severity::from_text("unknown"), Severity::Unspecified);
    }

    #[test]
    fn test_severity_from_number() {
        assert_eq!(Severity::from_number(0), Severity::Unspecified);
        assert_eq!(Severity::from_number(1), Severity::Trace);
        assert_eq!(Severity::from_number(5), Severity::Debug);
        assert_eq!(Severity::from_number(9), Severity::Info);
        assert_eq!(Severity::from_number(13), Severity::Warn);
        assert_eq!(Severity::from_number(17), Severity::Error);
        assert_eq!(Severity::from_number(21), Severity::Fatal);
        assert_eq!(Severity::from_number(24), Severity::Fatal);
    }

    #[test]
    fn test_severity_from_syslog_priority() {
        assert_eq!(Severity::from_syslog_priority(0), Severity::Fatal);
        assert_eq!(Severity::from_syslog_priority(3), Severity::Error);
        assert_eq!(Severity::from_syslog_priority(4), Severity::Warn);
        assert_eq!(Severity::from_syslog_priority(6), Severity::Info);
        assert_eq!(Severity::from_syslog_priority(7), Severity::Debug);
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Fatal > Severity::Error);
        assert!(Severity::Error > Severity::Warn);
        assert!(Severity::Warn > Severity::Info);
        assert!(Severity::Info > Severity::Debug);
    }

    #[test]
    fn test_severity_roundtrip() {
        for sev in [Severity::Trace, Severity::Debug, Severity::Info, Severity::Warn, Severity::Error, Severity::Fatal] {
            assert_eq!(Severity::from_number(sev.number()), sev);
            assert_eq!(Severity::from_text(sev.text()), sev);
        }
    }

    #[test]
    fn test_log_entry_from_legacy() {
        let ts = Utc::now();
        let entry = LogEntry::from_legacy(
            ts,
            Some("ERROR"),
            Some("myapp".to_string()),
            Some("host1".to_string()),
            "something failed".to_string(),
            None,
            None,
            None,
        );
        assert_eq!(entry.severity, Severity::Error);
        assert_eq!(entry.body, "something failed");
        assert_eq!(entry.service_name, Some("myapp".to_string()));
    }
}
