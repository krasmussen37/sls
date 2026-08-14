//! CodexConnector - Parse Codex CLI log files
//!
//! Codex CLI has used two local log formats:
//! - current SQLite logs at `~/.codex/logs_*.sqlite`
//! - legacy text logs at `~/.codex/log/codex-tui.log`
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
//! Both formats are normalized into the shared SLS `LogEntry` structure. We
//! retain warnings/errors plus MCP lifecycle events needed for health checks.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};

use super::{Connector, LogEntry};

/// Connector for current and legacy Codex CLI logs.
pub struct CodexConnector {
    /// Stable Codex root or a specific Codex log file.
    log_path: PathBuf,
}

impl CodexConnector {
    pub fn new<P: AsRef<Path>>(log_path: P) -> Self {
        Self {
            log_path: log_path.as_ref().to_path_buf(),
        }
    }

    /// Resolve a stable ~/.codex source to the newest supported physical log.
    fn resolve_log_path(&self) -> Option<PathBuf> {
        if self.log_path.is_file() {
            return Some(self.log_path.clone());
        }

        if !self.log_path.is_dir() {
            return None;
        }

        let mut sqlite_logs: Vec<PathBuf> = fs::read_dir(&self.log_path)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("logs_") && name.ends_with(".sqlite"))
                    .unwrap_or(false)
            })
            .collect();

        sqlite_logs.sort_by_key(|path| fs::metadata(path).and_then(|m| m.modified()).ok());
        if let Some(path) = sqlite_logs.pop() {
            return Some(path);
        }

        let legacy = self.log_path.join("log").join("codex-tui.log");
        legacy.exists().then_some(legacy)
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

    fn read_text_entries(&self, path: &Path, since: Option<&str>) -> Result<Vec<LogEntry>> {
        let file =
            File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
        let reader = BufReader::new(file);
        let start_line: usize = since.and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut entries = Vec::new();

        for (line_num, line_result) in reader.lines().enumerate() {
            if line_num < start_line {
                continue;
            }

            let line = match line_result {
                Ok(line) if !line.is_empty() => line,
                _ => continue,
            };

            if let Some(entry) = self.parse_line(&line, line_num) {
                entries.push(entry);
            }
        }

        Ok(entries)
    }

    /// Normalize selected rows from Codex's current SQLite telemetry store.
    fn read_sqlite_entries(&self, path: &Path) -> Result<Vec<LogEntry>> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(path, flags)
            .with_context(|| format!("Failed to open Codex SQLite log {}", path.display()))?;
        let mut statement = connection.prepare(
            r#"
            SELECT id, ts, ts_nanos, level, target, feedback_log_body,
                   module_path, file, line, thread_id, process_uuid
              FROM logs
             WHERE feedback_log_body IS NOT NULL
               AND (
                    level IN ('WARN', 'ERROR')
                    OR (
                        level = 'INFO'
                        AND (
                            feedback_log_body LIKE 'Turn error:%'
                            OR target LIKE 'codex_mcp%'
                            OR target LIKE 'codex_rmcp_client%'
                            OR target LIKE 'rmcp::%'
                        )
                    )
                    OR (
                        level = 'TRACE'
                        AND target LIKE 'codex_mcp%'
                        AND (
                            feedback_log_body LIKE '%listed MCP server tools%'
                            OR feedback_log_body LIKE '%waiting for MCP server tools%'
                        )
                    )
               )
             ORDER BY ts DESC, ts_nanos DESC, id DESC
            "#,
        )?;

        let source_name = path.to_string_lossy().to_string();
        let rows = statement.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let seconds: i64 = row.get(1)?;
            let nanos: i64 = row.get(2)?;
            let level: String = row.get(3)?;
            let target: String = row.get(4)?;
            let body: String = row.get(5)?;
            let module_path: Option<String> = row.get(6)?;
            let file: Option<String> = row.get(7)?;
            let line: Option<i64> = row.get(8)?;
            let thread_id: Option<String> = row.get(9)?;
            let process_uuid: Option<String> = row.get(10)?;

            let timestamp = Utc
                .timestamp_opt(seconds, nanos.clamp(0, 999_999_999) as u32)
                .single()
                .unwrap_or_else(Utc::now);
            let effective_level = if level == "INFO" && is_error_message(&body) {
                "ERROR".to_string()
            } else {
                level
            };
            let metadata = serde_json::json!({
                "codex_log_id": id,
                "target": target,
                "module_path": module_path,
                "file": file,
                "line": line,
                "thread_id": thread_id,
                "process_uuid": process_uuid,
            });

            Ok(LogEntry {
                source_id: None,
                timestamp,
                level: Some(effective_level),
                service: Some("codex".to_string()),
                hostname: None,
                message: body.clone(),
                raw_line: Some(body),
                structured_data: Some(metadata.to_string()),
                fingerprint: Some(generate_sqlite_fingerprint(&source_name, id)),
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to normalize Codex SQLite log rows")
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

fn generate_sqlite_fingerprint(source: &str, row_id: i64) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    row_id.hash(&mut hasher);
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
        path.contains(".codex")
            && (path.ends_with(".codex")
                || path.ends_with("codex-tui.log")
                || (path.contains("logs_") && path.ends_with(".sqlite")))
    }

    fn read_entries(&self, since: Option<&str>) -> Result<Vec<LogEntry>> {
        let Some(path) = self.resolve_log_path() else {
            return Ok(Vec::new());
        };

        let mut entries = if path.extension().and_then(|ext| ext.to_str()) == Some("sqlite") {
            self.read_sqlite_entries(&path)?
        } else {
            self.read_text_entries(&path, since)?
        };

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
        assert_eq!(
            entry.level,
            Some("ERROR".to_string()),
            "Turn error should be promoted to ERROR"
        );
        assert!(entry.message.starts_with("Turn error:"));
    }

    #[test]
    fn test_parse_info_tool_call_ignored() {
        let connector = make_connector();
        let line = r#"2026-01-02T05:30:18.609993Z  INFO ToolCall: shell_command {"command":"cat file.txt","workdir":"/tmp"}"#;
        let entry = connector.parse_line(line, 0);
        assert!(
            entry.is_none(),
            "Regular ToolCall INFO lines should be ignored"
        );
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
        assert!(connector.can_read("/home/user/.codex"));
        assert!(connector.can_read("/home/user/.codex/logs_2.sqlite"));
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

    #[test]
    fn test_sqlite_rows_are_normalized_and_filtered() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("logs_9.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE logs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts INTEGER NOT NULL,
                    ts_nanos INTEGER NOT NULL,
                    level TEXT NOT NULL,
                    target TEXT NOT NULL,
                    feedback_log_body TEXT,
                    module_path TEXT,
                    file TEXT,
                    line INTEGER,
                    thread_id TEXT,
                    process_uuid TEXT
                );
                INSERT INTO logs (ts, ts_nanos, level, target, feedback_log_body)
                VALUES
                    (1784553600, 100, 'WARN', 'codex_core::test', 'warning body'),
                    (1784553601, 200, 'INFO', 'rmcp::service', 'Service initialized server_name=parallel-search'),
                    (1784553602, 300, 'TRACE', 'codex_mcp::connection_manager', 'waiting for MCP server tools server_name=parallel-search'),
                    (1784553603, 400, 'INFO', 'codex_core::routine', 'ordinary info');
                "#,
            )
            .unwrap();
        drop(connection);

        let connector = CodexConnector::new(temp.path());
        let entries = connector.read_entries(None).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].level, Some("TRACE".to_string()));
        assert_eq!(entries[0].service, Some("codex".to_string()));
        assert!(entries[0].message.contains("parallel-search"));
        assert!(entries.iter().all(|entry| entry.structured_data.is_some()));
        assert!(entries.iter().all(|entry| entry.fingerprint.is_some()));
    }
}
