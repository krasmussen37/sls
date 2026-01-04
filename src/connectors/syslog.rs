//! SyslogConnector - Parse /var/log/syslog and similar files
//!
//! This connector reads traditional syslog format files.
//!
//! ## Syslog Format
//!
//! Traditional BSD syslog format:
//! ```
//! <month> <day> <time> <hostname> <process>[<pid>]: <message>
//! ```
//!
//! Example:
//! ```
//! Jan  2 17:00:00 myhost sshd[1234]: Accepted publickey for user
//! ```

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDateTime, TimeZone, Utc};
use regex::Regex;

use super::{Connector, LogEntry};

/// Connector for syslog format files
pub struct SyslogConnector {
    /// Path to the syslog file
    file_path: PathBuf,
    /// Current byte offset position
    position: Option<u64>,
    /// Maximum entries to read per call
    max_entries: usize,
}

impl SyslogConnector {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            file_path: path.as_ref().to_path_buf(),
            position: None,
            max_entries: 1000,
        }
    }

    /// Parse a syslog line into a LogEntry
    pub(crate) fn parse_line(&self, line: &str) -> Option<LogEntry> {
        // Standard syslog regex: "Mon DD HH:MM:SS hostname process[pid]: message"
        // Some variants don't have pid: "Mon DD HH:MM:SS hostname process: message"
        lazy_static::lazy_static! {
            static ref SYSLOG_RE: Regex = Regex::new(
                r"^([A-Z][a-z]{2})\s+(\d{1,2})\s+(\d{2}:\d{2}:\d{2})\s+(\S+)\s+([^\[:]+)(?:\[(\d+)\])?:\s*(.*)$"
            ).unwrap();
        }

        let caps = SYSLOG_RE.captures(line)?;

        let month = caps.get(1)?.as_str();
        let day: u32 = caps.get(2)?.as_str().parse().ok()?;
        let time = caps.get(3)?.as_str();
        let hostname = caps.get(4)?.as_str().to_string();
        let process = caps.get(5)?.as_str().to_string();
        let _pid: Option<&str> = caps.get(6).map(|m| m.as_str());
        let message = caps.get(7)?.as_str().to_string();

        // Parse timestamp (assuming current year)
        let month_num = match month {
            "Jan" => 1,
            "Feb" => 2,
            "Mar" => 3,
            "Apr" => 4,
            "May" => 5,
            "Jun" => 6,
            "Jul" => 7,
            "Aug" => 8,
            "Sep" => 9,
            "Oct" => 10,
            "Nov" => 11,
            "Dec" => 12,
            _ => return None,
        };

        let year = Utc::now().year();
        let datetime_str = format!("{}-{:02}-{:02} {}", year, month_num, day, time);
        let timestamp = NaiveDateTime::parse_from_str(&datetime_str, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|dt| Utc.from_utc_datetime(&dt))
            .unwrap_or_else(Utc::now);

        // Infer log level from message content
        let level = infer_log_level(&message);

        Some(LogEntry {
            source_id: None,
            timestamp,
            level,
            service: Some(process),
            hostname: Some(hostname),
            message,
            raw_line: Some(line.to_string()),
            structured_data: None,
            fingerprint: None,
        })
    }
}

impl Connector for SyslogConnector {
    fn name(&self) -> &str {
        "Syslog Connector"
    }

    fn source_type(&self) -> &str {
        "syslog"
    }

    fn can_read(&self, path: &str) -> bool {
        path.ends_with("syslog")
            || path.ends_with("messages")
            || path.contains("/var/log/")
            || path.ends_with(".log")
    }

    fn read_entries(&self, since: Option<&str>) -> Result<Vec<LogEntry>> {
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.file_path)
            .with_context(|| format!("Failed to open {}", self.file_path.display()))?;

        let mut reader = BufReader::new(file);

        // Seek to position if specified
        if let Some(pos_str) = since {
            if let Ok(pos) = pos_str.parse::<u64>() {
                reader.seek(SeekFrom::Start(pos))?;
            }
        }

        let mut entries = Vec::new();
        let mut line = String::new();

        while entries.len() < self.max_entries {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        if let Some(entry) = self.parse_line(trimmed) {
                            entries.push(entry);
                        }
                    }
                }
                Err(_) => break,
            }
        }

        // Sort by timestamp (newest first)
        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(entries)
    }

    fn get_position(&self) -> Option<String> {
        self.position.map(|p| p.to_string())
    }
}

/// Infer log level from message content
fn infer_log_level(message: &str) -> Option<String> {
    let lower = message.to_lowercase();
    
    if lower.contains("error") || lower.contains("fail") || lower.contains("fatal") {
        Some("ERROR".to_string())
    } else if lower.contains("warn") {
        Some("WARN".to_string())
    } else if lower.contains("debug") {
        Some("DEBUG".to_string())
    } else if lower.contains("notice") {
        Some("NOTICE".to_string())
    } else {
        Some("INFO".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_syslog_line() {
        let connector = SyslogConnector::new("/var/log/syslog");

        let line = "Jan  2 17:00:00 myhost sshd[1234]: Accepted publickey for user";
        let entry = connector.parse_line(line);

        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert!(entry.message.contains("Accepted publickey"));
        assert_eq!(entry.service, Some("sshd".to_string()));
        assert_eq!(entry.hostname, Some("myhost".to_string()));
    }

    #[test]
    fn test_parse_syslog_no_pid() {
        let connector = SyslogConnector::new("/var/log/syslog");

        let line = "Dec 31 23:59:59 server kernel: Something happened";
        let entry = connector.parse_line(line);

        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.service, Some("kernel".to_string()));
        assert!(entry.message.contains("Something happened"));
    }

    #[test]
    fn test_infer_log_level() {
        assert_eq!(infer_log_level("Connection error"), Some("ERROR".to_string()));
        assert_eq!(infer_log_level("Warning: disk space low"), Some("WARN".to_string()));
        assert_eq!(infer_log_level("Starting service"), Some("INFO".to_string()));
    }

    #[test]
    fn test_infer_log_level_all_types() {
        // ERROR cases
        assert_eq!(infer_log_level("Error: something failed"), Some("ERROR".to_string()));
        assert_eq!(infer_log_level("Fatal error occurred"), Some("ERROR".to_string()));
        assert_eq!(infer_log_level("Authentication failed"), Some("ERROR".to_string()));

        // WARN cases
        assert_eq!(infer_log_level("Warning message"), Some("WARN".to_string()));

        // DEBUG cases
        assert_eq!(infer_log_level("Debug: variable x = 5"), Some("DEBUG".to_string()));

        // NOTICE cases
        assert_eq!(infer_log_level("Notice: system updated"), Some("NOTICE".to_string()));

        // INFO cases (default)
        assert_eq!(infer_log_level("Service started"), Some("INFO".to_string()));
    }

    #[test]
    fn test_parse_all_months() {
        let connector = SyslogConnector::new("/var/log/syslog");

        let months = vec![
            "Jan", "Feb", "Mar", "Apr", "May", "Jun",
            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"
        ];

        for month in months {
            let line = format!("{}  1 12:00:00 host proc: message", month);
            let entry = connector.parse_line(&line);
            assert!(entry.is_some(), "Failed to parse month: {}", month);
        }
    }

    #[test]
    fn test_connector_name() {
        let connector = SyslogConnector::new("/var/log/syslog");
        assert_eq!(connector.name(), "Syslog Connector");
    }

    #[test]
    fn test_connector_source_type() {
        let connector = SyslogConnector::new("/var/log/syslog");
        assert_eq!(connector.source_type(), "syslog");
    }

    #[test]
    fn test_can_read_syslog_paths() {
        let connector = SyslogConnector::new("/var/log/syslog");
        assert!(connector.can_read("/var/log/syslog"));
        assert!(connector.can_read("/var/log/messages"));
        assert!(connector.can_read("/var/log/auth.log"));
        assert!(!connector.can_read("/home/user/.claude/session.jsonl"));
    }

    #[test]
    fn test_parse_single_digit_day() {
        let connector = SyslogConnector::new("/var/log/syslog");

        // Single digit day with double space padding
        let line = "Jan  5 10:30:00 host sshd[123]: Test";
        let entry = connector.parse_line(line);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().service, Some("sshd".to_string()));
    }

    #[test]
    fn test_parse_double_digit_day() {
        let connector = SyslogConnector::new("/var/log/syslog");

        // Double digit day
        let line = "Jan 25 10:30:00 host sshd[123]: Test";
        let entry = connector.parse_line(line);
        assert!(entry.is_some());
    }

    #[test]
    fn test_invalid_line_returns_none() {
        let connector = SyslogConnector::new("/var/log/syslog");

        assert!(connector.parse_line("not a valid syslog line").is_none());
        assert!(connector.parse_line("").is_none());
        assert!(connector.parse_line("Invalid Month 01 00:00:00 host proc: msg").is_none());
    }

    #[test]
    fn test_raw_line_preserved() {
        let connector = SyslogConnector::new("/var/log/syslog");

        let line = "Jan  1 00:00:00 host proc: message";
        let entry = connector.parse_line(line).unwrap();

        assert_eq!(entry.raw_line, Some(line.to_string()));
    }

    #[test]
    fn test_get_position_initially_none() {
        let connector = SyslogConnector::new("/var/log/syslog");
        assert!(connector.get_position().is_none());
    }
}
