//! JournaldConnector - Read from systemd journal
//!
//! This connector reads logs from journald using the journalctl command
//! with JSON output format.
//!
//! ## Journald JSON Format
//!
//! Key fields from journalctl --output=json:
//! - `MESSAGE`: The log message
//! - `PRIORITY`: Syslog priority (0=emerg, 3=err, 4=warning, 6=info, 7=debug)
//! - `_HOSTNAME`: Hostname
//! - `SYSLOG_IDENTIFIER`: Service/application identifier
//! - `_SYSTEMD_UNIT`: Systemd unit name
//! - `__REALTIME_TIMESTAMP`: Timestamp in microseconds since epoch
//! - `__CURSOR`: Cursor for resuming reads

use std::process::Command;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use super::{Connector, LogEntry};

/// Connector for systemd journald
pub struct JournaldConnector {
    /// Current cursor position
    cursor: Option<String>,
    /// Optional unit filter (e.g., "nginx.service")
    unit_filter: Option<String>,
    /// Maximum entries to read per call
    max_entries: usize,
}

impl JournaldConnector {
    pub fn new() -> Self {
        Self {
            cursor: None,
            unit_filter: None,
            max_entries: 1000,
        }
    }

    #[allow(dead_code)]
    pub fn with_unit(unit: String) -> Self {
        Self {
            cursor: None,
            unit_filter: Some(unit),
            max_entries: 1000,
        }
    }

    /// Build the journalctl command with appropriate arguments
    fn build_command(&self, since: Option<&str>) -> Command {
        let mut cmd = Command::new("journalctl");
        cmd.arg("--output=json");
        cmd.arg("--no-pager");
        cmd.arg(format!("--lines={}", self.max_entries));

        // Add unit filter if specified
        if let Some(ref unit) = self.unit_filter {
            cmd.arg(format!("--unit={}", unit));
        }

        // Resume from cursor or use time-based filter
        if let Some(cursor) = since.or(self.cursor.as_deref()) {
            if cursor.starts_with("s=") {
                // It's a cursor
                cmd.arg(format!("--after-cursor={}", cursor));
            } else {
                // It's a time specification
                cmd.arg(format!("--since={}", cursor));
            }
        }

        cmd
    }

    /// Parse a single JSON line from journalctl output
    pub(crate) fn parse_entry(&self, line: &str) -> Option<LogEntry> {
        let entry: JournaldEntry = serde_json::from_str(line).ok()?;

        // Parse timestamp (microseconds since epoch)
        let timestamp = entry
            .realtime_timestamp
            .as_ref()
            .and_then(|ts| ts.parse::<i64>().ok())
            .map(|us| Utc.timestamp_micros(us).single())
            .flatten()
            .unwrap_or_else(Utc::now);

        // Map priority to log level
        let level = entry.priority.as_ref().and_then(|p| {
            p.parse::<u8>().ok().map(|priority| {
                match priority {
                    0 => "EMERG",
                    1 => "ALERT",
                    2 => "CRIT",
                    3 => "ERROR",
                    4 => "WARN",
                    5 => "NOTICE",
                    6 => "INFO",
                    7 => "DEBUG",
                    _ => "UNKNOWN",
                }
                .to_string()
            })
        });

        // Get service identifier
        let service = entry
            .syslog_identifier
            .or(entry.systemd_unit)
            .or(entry.comm);

        Some(LogEntry {
            source_id: None,
            timestamp,
            level,
            service,
            hostname: entry.hostname,
            message: entry.message?,
            raw_line: Some(line.to_string()),
            structured_data: None, // Could include full JSON if needed
            fingerprint: None,
        })
    }
}

impl Default for JournaldConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for JournaldConnector {
    fn name(&self) -> &str {
        "Journald Connector"
    }

    fn source_type(&self) -> &str {
        "journald"
    }

    fn can_read(&self, path: &str) -> bool {
        path == "journald" || path.starts_with("journald:")
    }

    fn read_entries(&self, since: Option<&str>) -> Result<Vec<LogEntry>> {
        let mut cmd = self.build_command(since);

        let output = cmd
            .output()
            .context("Failed to execute journalctl")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No journal files were found") {
                return Ok(Vec::new());
            }
            anyhow::bail!("journalctl failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut entries = Vec::new();

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }
            if let Some(entry) = self.parse_entry(line) {
                entries.push(entry);
            }
        }

        // Sort by timestamp (newest first)
        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(entries)
    }

    fn get_position(&self) -> Option<String> {
        self.cursor.clone()
    }
}

/// Deserialization structure for journald JSON output
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JournaldEntry {
    #[serde(rename = "MESSAGE")]
    message: Option<String>,

    #[serde(rename = "PRIORITY")]
    priority: Option<String>,

    #[serde(rename = "_HOSTNAME")]
    hostname: Option<String>,

    #[serde(rename = "SYSLOG_IDENTIFIER")]
    syslog_identifier: Option<String>,

    #[serde(rename = "_SYSTEMD_UNIT")]
    systemd_unit: Option<String>,

    #[serde(rename = "_COMM")]
    comm: Option<String>,

    #[serde(rename = "__REALTIME_TIMESTAMP")]
    realtime_timestamp: Option<String>,

    #[serde(rename = "__CURSOR")]
    cursor: Option<String>,

    #[serde(rename = "_PID")]
    pid: Option<String>,

    #[serde(rename = "_UID")]
    uid: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_journald_entry() {
        let connector = JournaldConnector::new();
        let json = r#"{"MESSAGE":"Test message","PRIORITY":"6","_HOSTNAME":"testhost","SYSLOG_IDENTIFIER":"test-app","__REALTIME_TIMESTAMP":"1767373443242837"}"#;

        let entry = connector.parse_entry(json);
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.message, "Test message");
        assert_eq!(entry.level, Some("INFO".to_string()));
        assert_eq!(entry.hostname, Some("testhost".to_string()));
        assert_eq!(entry.service, Some("test-app".to_string()));
    }

    #[test]
    fn test_priority_mapping() {
        let connector = JournaldConnector::new();

        let error_json = r#"{"MESSAGE":"Error","PRIORITY":"3","__REALTIME_TIMESTAMP":"1767373443242837"}"#;
        let entry = connector.parse_entry(error_json).unwrap();
        assert_eq!(entry.level, Some("ERROR".to_string()));

        let warn_json = r#"{"MESSAGE":"Warning","PRIORITY":"4","__REALTIME_TIMESTAMP":"1767373443242837"}"#;
        let entry = connector.parse_entry(warn_json).unwrap();
        assert_eq!(entry.level, Some("WARN".to_string()));
    }

    #[test]
    fn test_all_priority_levels() {
        let connector = JournaldConnector::new();

        let test_cases = vec![
            ("0", "EMERG"),
            ("1", "ALERT"),
            ("2", "CRIT"),
            ("3", "ERROR"),
            ("4", "WARN"),
            ("5", "NOTICE"),
            ("6", "INFO"),
            ("7", "DEBUG"),
        ];

        for (priority, expected_level) in test_cases {
            let json = format!(r#"{{"MESSAGE":"Test","PRIORITY":"{}","__REALTIME_TIMESTAMP":"1767373443242837"}}"#, priority);
            let entry = connector.parse_entry(&json).unwrap();
            assert_eq!(entry.level, Some(expected_level.to_string()), "Priority {} should map to {}", priority, expected_level);
        }
    }

    #[test]
    fn test_connector_name() {
        let connector = JournaldConnector::new();
        assert_eq!(connector.name(), "Journald Connector");
    }

    #[test]
    fn test_connector_source_type() {
        let connector = JournaldConnector::new();
        assert_eq!(connector.source_type(), "journald");
    }

    #[test]
    fn test_can_read_journald_paths() {
        let connector = JournaldConnector::new();
        assert!(connector.can_read("journald"));
        assert!(connector.can_read("journald:nginx.service"));
        assert!(!connector.can_read("/var/log/syslog"));
    }

    #[test]
    fn test_default_trait() {
        let connector = JournaldConnector::default();
        assert_eq!(connector.source_type(), "journald");
    }

    #[test]
    fn test_with_unit_constructor() {
        let connector = JournaldConnector::with_unit("nginx.service".to_string());
        assert_eq!(connector.unit_filter, Some("nginx.service".to_string()));
    }

    #[test]
    fn test_entry_without_message_returns_none() {
        let connector = JournaldConnector::new();
        let json = r#"{"PRIORITY":"6","_HOSTNAME":"testhost","__REALTIME_TIMESTAMP":"1767373443242837"}"#;

        let entry = connector.parse_entry(json);
        assert!(entry.is_none(), "Entry without MESSAGE should return None");
    }

    #[test]
    fn test_entry_with_systemd_unit_fallback() {
        let connector = JournaldConnector::new();
        let json = r#"{"MESSAGE":"Test","PRIORITY":"6","_SYSTEMD_UNIT":"myservice.service","__REALTIME_TIMESTAMP":"1767373443242837"}"#;

        let entry = connector.parse_entry(json).unwrap();
        assert_eq!(entry.service, Some("myservice.service".to_string()));
    }

    #[test]
    fn test_entry_with_comm_fallback() {
        let connector = JournaldConnector::new();
        let json = r#"{"MESSAGE":"Test","PRIORITY":"6","_COMM":"myprocess","__REALTIME_TIMESTAMP":"1767373443242837"}"#;

        let entry = connector.parse_entry(json).unwrap();
        assert_eq!(entry.service, Some("myprocess".to_string()));
    }

    #[test]
    fn test_invalid_json_returns_none() {
        let connector = JournaldConnector::new();
        let entry = connector.parse_entry("not valid json");
        assert!(entry.is_none());
    }
}
