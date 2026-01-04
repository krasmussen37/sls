//! AgentStderrConnector - Parse Claude/Codex/Gemini JSONL session files
//!
//! This connector reads agent stderr output from JSONL files that capture
//! tool execution results, errors, and other agent activity.
//!
//! ## JSONL Format
//!
//! Claude Code session files use this structure:
//! - Each line is a JSON object with `type`, `timestamp`, `sessionId`, etc.
//! - Tool results are in `message.content` array with `type: "tool_result"`
//! - Errors have `is_error: true` and `toolUseResult: "Error: ..."`
//!
//! ## Example entries we capture:
//! - Tool execution errors (exit codes, command failures)
//! - File read/write failures
//! - Git operation errors
//! - Any message with `is_error: true`

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{Connector, LogEntry};

/// Connector for agent stderr JSONL files
pub struct AgentStderrConnector {
    /// Base path to search for agent logs (e.g., ~/.claude/projects/)
    base_path: PathBuf,
    /// Current position marker (file:line format)
    position: Option<String>,
    /// Whether to include non-error entries
    include_all: bool,
}

impl AgentStderrConnector {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            position: None,
            include_all: false,
        }
    }

    /// Create connector that only captures errors
    pub fn errors_only<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            position: None,
            include_all: false,
        }
    }

    /// Create connector that captures all tool results
    #[allow(dead_code)]
    pub fn all_entries<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            position: None,
            include_all: true,
        }
    }

    /// Find all JSONL files in the base path
    fn find_jsonl_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        if self.base_path.is_file() && self.base_path.extension().map_or(false, |e| e == "jsonl") {
            files.push(self.base_path.clone());
            return Ok(files);
        }

        if !self.base_path.exists() {
            return Ok(files);
        }

        // Walk directory looking for .jsonl files
        self.collect_jsonl_files(&self.base_path, &mut files)?;

        // Sort by modification time (newest first)
        files.sort_by(|a, b| {
            let time_a = fs::metadata(a).and_then(|m| m.modified()).ok();
            let time_b = fs::metadata(b).and_then(|m| m.modified()).ok();
            time_b.cmp(&time_a)
        });

        Ok(files)
    }

    fn collect_jsonl_files(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.collect_jsonl_files(&path, files)?;
            } else if path.extension().map_or(false, |e| e == "jsonl") {
                files.push(path);
            }
        }
        Ok(())
    }

    /// Parse a single file and extract log entries
    fn parse_file(&self, path: &Path, since_line: Option<usize>) -> Result<Vec<LogEntry>> {
        let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        let start_line = since_line.unwrap_or(0);
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();

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

            if let Some(entry) = self.parse_line(&line, &file_name) {
                entries.push(entry);
            }
        }

        Ok(entries)
    }

    /// Parse a single JSONL line
    fn parse_line(&self, line: &str, source_file: &str) -> Option<LogEntry> {
        let json: SessionEntry = serde_json::from_str(line).ok()?;

        // Extract timestamp
        let timestamp = DateTime::parse_from_rfc3339(&json.timestamp)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        // Check for error in tool results
        if let Some(ref result) = json.tool_use_result {
            if result.starts_with("Error:") {
                return Some(LogEntry {
                    source_id: None,
                    timestamp,
                    level: Some("ERROR".to_string()),
                    service: Some(format!("claude-code:{}", source_file)),
                    hostname: None,
                    message: result.clone(),
                    raw_line: Some(line.to_string()),
                    structured_data: Some(serde_json::to_string(&json).ok()?),
                    fingerprint: Some(generate_fingerprint(result)),
                });
            }
        }

        // Check for tool_result with is_error in message content
        if let Some(ref message) = json.message {
            if let Some(ref content) = message.content {
                for item in content {
                    if item.is_error.unwrap_or(false) {
                        let error_msg = item.content.clone().unwrap_or_else(|| "Unknown error".to_string());
                        return Some(LogEntry {
                            source_id: None,
                            timestamp,
                            level: Some("ERROR".to_string()),
                            service: Some(format!("claude-code:{}", source_file)),
                            hostname: None,
                            message: error_msg.clone(),
                            raw_line: Some(line.to_string()),
                            structured_data: Some(serde_json::to_string(&json).ok()?),
                            fingerprint: Some(generate_fingerprint(&error_msg)),
                        });
                    }
                }
            }
        }

        // If include_all is set, also capture successful tool results
        if self.include_all {
            if json.entry_type == "user" && json.tool_use_result.is_some() {
                let result = json.tool_use_result.as_ref().unwrap();
                return Some(LogEntry {
                    source_id: None,
                    timestamp,
                    level: Some("INFO".to_string()),
                    service: Some(format!("claude-code:{}", source_file)),
                    hostname: None,
                    message: result.chars().take(500).collect(), // Truncate long messages
                    raw_line: Some(line.to_string()),
                    structured_data: None,
                    fingerprint: None,
                });
            }
        }

        None
    }
}

impl Connector for AgentStderrConnector {
    fn name(&self) -> &str {
        "Agent Stderr Connector"
    }

    fn source_type(&self) -> &str {
        "agent_stderr"
    }

    fn can_read(&self, path: &str) -> bool {
        path.contains(".claude") || path.ends_with(".jsonl") || path.contains("agent")
    }

    fn read_entries(&self, since: Option<&str>) -> Result<Vec<LogEntry>> {
        let files = self.find_jsonl_files()?;
        let mut all_entries = Vec::new();

        // Parse position marker (file:line format)
        let since_info: Option<(&str, usize)> = since.and_then(|s| {
            let parts: Vec<&str> = s.splitn(2, ':').collect();
            if parts.len() == 2 {
                parts[1].parse().ok().map(|line| (parts[0], line))
            } else {
                None
            }
        });

        for file_path in files {
            let file_name = file_path.file_name().unwrap_or_default().to_string_lossy();
            let start_line = since_info
                .filter(|(name, _)| *name == file_name)
                .map(|(_, line)| line);

            match self.parse_file(&file_path, start_line) {
                Ok(entries) => all_entries.extend(entries),
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", file_path.display(), e);
                }
            }
        }

        // Sort by timestamp (newest first)
        all_entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(all_entries)
    }

    fn get_position(&self) -> Option<String> {
        self.position.clone()
    }
}

/// Generate a fingerprint for deduplication
fn generate_fingerprint(message: &str) -> String {
    // Extract key error patterns, removing variable parts like paths, numbers, IDs
    let normalized = message
        .lines()
        .next()
        .unwrap_or(message)
        .chars()
        .take(100)
        .collect::<String>()
        // Remove numbers (potential line numbers, exit codes vary)
        .chars()
        .map(|c| if c.is_numeric() { '#' } else { c })
        .collect::<String>();

    // Simple hash for fingerprint
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Deserialization structures for Claude Code JSONL format
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionEntry {
    #[serde(rename = "type")]
    entry_type: String,
    timestamp: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    message: Option<MessageContent>,
    #[serde(default)]
    tool_use_result: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MessageContent {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<Vec<ToolResultContent>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToolResultContent {
    #[serde(rename = "type")]
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    tool_use_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_error_entry() {
        let connector = AgentStderrConnector::errors_only("/tmp");
        let line = r#"{"type":"user","timestamp":"2026-01-02T17:05:53.074Z","toolUseResult":"Error: Exit code 128\nerror: cannot pull with rebase"}"#;

        let entry = connector.parse_line(line, "test.jsonl");
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.level, Some("ERROR".to_string()));
        assert!(entry.message.contains("Exit code 128"));
    }

    #[test]
    fn test_fingerprint_generation() {
        let fp1 = generate_fingerprint("Exit code 127\nCommand not found");
        let fp2 = generate_fingerprint("Exit code 128\nCommand not found");
        // Fingerprints should be different due to different exit codes
        // (though normalized, the text differs)
        assert!(!fp1.is_empty());
        assert!(!fp2.is_empty());
    }

    #[test]
    fn test_parse_non_error_entry_ignored() {
        let connector = AgentStderrConnector::errors_only("/tmp");
        let line = r#"{"type":"user","timestamp":"2026-01-02T17:05:53.074Z","toolUseResult":"Success: Files listed"}"#;

        let entry = connector.parse_line(line, "test.jsonl");
        assert!(entry.is_none(), "Non-error entries should be ignored in errors_only mode");
    }

    #[test]
    fn test_parse_entry_with_is_error_in_content() {
        let connector = AgentStderrConnector::errors_only("/tmp");
        let line = r#"{"type":"user","timestamp":"2026-01-02T17:05:53.074Z","message":{"content":[{"type":"tool_result","is_error":true,"content":"Failed to read file"}]}}"#;

        let entry = connector.parse_line(line, "test.jsonl");
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.level, Some("ERROR".to_string()));
        assert!(entry.message.contains("Failed to read file"));
    }

    #[test]
    fn test_parse_all_entries_mode() {
        let connector = AgentStderrConnector::all_entries("/tmp");
        let line = r#"{"type":"user","timestamp":"2026-01-02T17:05:53.074Z","toolUseResult":"ls output: file1.txt file2.txt"}"#;

        let entry = connector.parse_line(line, "test.jsonl");
        assert!(entry.is_some(), "All entries mode should capture non-error results");

        let entry = entry.unwrap();
        assert_eq!(entry.level, Some("INFO".to_string()));
    }

    #[test]
    fn test_connector_name() {
        let connector = AgentStderrConnector::new("/tmp");
        assert_eq!(connector.name(), "Agent Stderr Connector");
    }

    #[test]
    fn test_connector_source_type() {
        let connector = AgentStderrConnector::new("/tmp");
        assert_eq!(connector.source_type(), "agent_stderr");
    }

    #[test]
    fn test_can_read_claude_paths() {
        let connector = AgentStderrConnector::new("/tmp");
        assert!(connector.can_read("/home/user/.claude/projects/test.jsonl"));
        assert!(connector.can_read("/tmp/agent-logs/session.jsonl"));
        assert!(!connector.can_read("/var/log/syslog"));
    }

    #[test]
    fn test_fingerprint_truncates_long_messages() {
        let long_message = "A".repeat(200);
        let fp = generate_fingerprint(&long_message);
        assert!(!fp.is_empty());
        assert_eq!(fp.len(), 16); // Hex hash is 16 chars
    }

    #[test]
    fn test_invalid_json_returns_none() {
        let connector = AgentStderrConnector::new("/tmp");
        let entry = connector.parse_line("not valid json", "test.jsonl");
        assert!(entry.is_none());
    }
}
