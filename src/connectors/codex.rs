//! CodexConnector - Parse Codex CLI log files
//!
//! Codex CLI writes logs to `~/.codex/log/codex-tui.log`.
//! The format is plain text with one event per line.
//!
//! ## Format
//!
//! ```text
//! 2026-01-02T05:30:21.287462Z  WARN Unknown model gpt-4 is used.
//! 2026-01-02T05:30:18.609993Z  INFO ToolCall: shell_command {"command":"cat ...","workdir":"..."}
//! 2026-01-02T05:30:21.287462Z  INFO Turn error: {"detail":"The 'gpt-4' model is not supported"}
//! ```
//!
//! Each line: `{timestamp}  {LEVEL} {message}`
//!
//! We extract:
//! - Lines at WARN or ERROR level
//! - Lines containing "Turn error:" (appear at INFO level but signal actual errors)

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::{Connector, LogEntry};

/// Connector for Codex CLI TUI log files
pub struct CodexConnector {
    /// Path to the log file (e.g., ~/.codex/log/codex-tui.log)
    log_path: PathBuf,
}

impl CodexConnector {
    pub fn new<P: AsRef<Path>>(log_path: P) -> Self {
        Self {
            log_path: log_path.as_ref().to_path_buf(),
        }
    }

    /// Parse a single log line, returning a LogEntry if it's noteworthy
    fn parse_line(&self, line: &str, line_num: usize) -> Option<LogEntry> {
        // Format: "2026-01-02T05:30:21.287462Z  WARN message..."
        // Split on double-space between timestamp and level
        let parts: Vec<&str> = line.splitn(2, "  ").collect();
        if parts.len() < 2 {
            return None;
        }

        let timestamp_str = parts[0].trim();
        let rest = parts[1].trim();

        // Parse timestamp (Codex uses microsecond precision)
        let timestamp = parse_codex_timestamp(timestamp_str)?;

        // Split level from message
        let (level, message) = split_level(rest)?;

        // Decide whether to emit this entry
        let emit = match level.as_str() {
            "ERROR" => true,
            "WARN" => true,
            "INFO" => is_error_message(message),
            _ => false,
        };

        if !emit {
            return None;
        }

        // For INFO "Turn error:" lines, promote to ERROR
        let effective_level = if level == "INFO" && is_error_message(message) {
            "ERROR".to_string()
        } else {
            level
        };

        Some(LogEntry {
            source_id: None,
            timestamp,
            level: Some(effective_level),
            service: Some("codex".to_string()),
            hostname: None,
            message: message.to_string(),
            raw_line: Some(line.to_string()),
            structured_data: None,
            fingerprint: Some(generate_fingerprint(message, line_num)),
        })
    }
}

/// Parse a Codex timestamp (ISO-8601 with microseconds, e.g. "2026-01-02T05:30:21.287462Z")
fn parse_codex_timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Split "WARN some message here" into ("WARN", "some message here")
fn split_level(rest: &str) -> Option<(String, &str)> {
    let levels = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"];
    for level in &levels {
        if let Some(stripped) = rest.strip_prefix(level) {
            let message = stripped.trim_start();
            return Some((level.to_string(), message));
        }
    }
    None
}

/// Return true if an INFO-level message should be promoted to an error
fn is_error_message(message: &str) -> bool {
    message.starts_with("Turn error:")
}

fn generate_fingerprint(message: &str, line_num: usize) -> String {
    // Combine message prefix with line number so each occurrence is unique
    // but normalize numeric parts within the message
    let normalized: String = message
        .chars()
        .take(80)
        .map(|c| if c.is_numeric() { '#' } else { c })
        .collect();

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    line_num.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl Connector for CodexConnector {
    fn name(&self) -> &str {
        "Codex CLI Connector"
    }

    fn source_type(&self) -> &str {
        "codex"
    }

    fn can_read(&self, path: &str) -> bool {
        path.contains(".codex") && path.ends_with("codex-tui.log")
    }

    fn read_entries(&self, since: Option<&str>) -> Result<Vec<LogEntry>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.log_path)
            .with_context(|| format!("Failed to open {}", self.log_path.display()))?;
        let reader = BufReader::new(file);

        // `since` is a line number offset
        let start_line: usize = since.and_then(|s| s.parse().ok()).unwrap_or(0);

        let mut entries = Vec::new();

        for (line_num, line_result) in reader.lines().enumerate() {
            if line_num < start_line {
                continue;
            }

            let line = match line_result {
                Ok(l) => l,
                Err(_) => continue,
            };

            if line.is_empty() {
                continue;
            }

            if let Some(entry) = self.parse_line(&line, line_num) {
                entries.push(entry);
            }
        }

        // Sort newest first
        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(entries)
    }

    fn get_position(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_connector() -> CodexConnector {
        CodexConnector::new("/tmp/nonexistent-codex.log")
    }

    #[test]
    fn test_parse_warn_line() {
        let connector = make_connector();
        let line = "2026-01-02T05:16:28.233715Z  WARN Unknown model gpt-4 is used. This will degrade the performance of Codex.";
        let entry = connector.parse_line(line, 0);
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.level, Some("WARN".to_string()));
        assert!(entry.message.contains("Unknown model gpt-4"));
        assert_eq!(entry.service, Some("codex".to_string()));
    }

    #[test]
    fn test_parse_info_turn_error_promoted_to_error() {
        let connector = make_connector();
        let line = r#"2026-01-02T05:30:21.287462Z  INFO Turn error: {"detail":"The 'gpt-4' model is not supported when using Codex with a ChatGPT account."}"#;
        let entry = connector.parse_line(line, 0);
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.level, Some("ERROR".to_string()), "Turn error should be promoted to ERROR");
        assert!(entry.message.starts_with("Turn error:"));
    }

    #[test]
    fn test_parse_info_tool_call_ignored() {
        let connector = make_connector();
        let line = r#"2026-01-02T05:30:18.609993Z  INFO ToolCall: shell_command {"command":"cat file.txt","workdir":"/tmp"}"#;
        let entry = connector.parse_line(line, 0);
        assert!(entry.is_none(), "Regular ToolCall INFO lines should be ignored");
    }

    #[test]
    fn test_parse_info_esc_pressed_ignored() {
        let connector = make_connector();
        let line = "2026-01-01T06:21:25.182952Z  INFO Esc pressed";
        let entry = connector.parse_line(line, 0);
        assert!(entry.is_none(), "Routine INFO lines should be ignored");
    }

    #[test]
    fn test_parse_invalid_line_returns_none() {
        let connector = make_connector();
        assert!(connector.parse_line("not a log line", 0).is_none());
        assert!(connector.parse_line("", 0).is_none());
    }

    #[test]
    fn test_connector_name() {
        let connector = make_connector();
        assert_eq!(connector.name(), "Codex CLI Connector");
    }

    #[test]
    fn test_connector_source_type() {
        let connector = make_connector();
        assert_eq!(connector.source_type(), "codex");
    }

    #[test]
    fn test_can_read_codex_paths() {
        let connector = make_connector();
        assert!(connector.can_read("/home/user/.codex/log/codex-tui.log"));
        assert!(!connector.can_read("/home/user/.codex/history.jsonl"));
        assert!(!connector.can_read("/var/log/syslog"));
    }

    #[test]
    fn test_timestamp_parsing() {
        let ts = parse_codex_timestamp("2026-01-02T05:30:21.287462Z");
        assert!(ts.is_some());
    }

    #[test]
    fn test_read_entries_nonexistent_file_returns_empty() {
        let connector = make_connector();
        let result = connector.read_entries(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_fingerprint_is_16_hex_chars() {
        let fp = generate_fingerprint("Unknown model gpt-4 is used", 42);
        assert_eq!(fp.len(), 16);
    }
}
