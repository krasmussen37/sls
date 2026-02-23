//! GeminiConnector - Parse Gemini CLI session JSON files
//!
//! Gemini CLI writes session files to `~/.gemini/tmp/*/chats/session-*.json`.
//! Each file is a single JSON object (not JSONL) representing one chat session.
//!
//! ## Format
//!
//! ```json
//! {
//!   "sessionId": "...",
//!   "startTime": "2026-02-16T03:02:22.221Z",
//!   "messages": [
//!     {
//!       "id": "...",
//!       "timestamp": "...",
//!       "type": "user" | "gemini",
//!       "content": "...",
//!       "toolCalls": [
//!         {
//!           "id": "...",
//!           "name": "tool_name",
//!           "status": "success" | "error",
//!           "resultDisplay": "...",
//!           "timestamp": "..."
//!         }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! We extract:
//! - Tool calls with `status != "success"` (errors)
//! - All tool call activity when `include_all` is set

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{Connector, LogEntry};

/// Connector for Gemini CLI session JSON files
pub struct GeminiConnector {
    /// Base path to search for Gemini sessions (e.g., ~/.gemini/tmp/)
    base_path: PathBuf,
    /// Whether to include non-error entries
    include_all: bool,
}

impl GeminiConnector {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            include_all: false,
        }
    }

    #[allow(dead_code)]
    pub fn all_entries<P: AsRef<Path>>(base_path: P) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            include_all: true,
        }
    }

    /// Find all session JSON files under the base path
    fn find_session_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        if !self.base_path.exists() {
            return Ok(files);
        }

        self.collect_session_files(&self.base_path, &mut files)?;

        // Sort by modification time (newest first)
        files.sort_by(|a, b| {
            let time_a = fs::metadata(a).and_then(|m| m.modified()).ok();
            let time_b = fs::metadata(b).and_then(|m| m.modified()).ok();
            time_b.cmp(&time_a)
        });

        Ok(files)
    }

    fn collect_session_files(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.collect_session_files(&path, files)?;
            } else if path.extension().map_or(false, |e| e == "json") {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with("session-") {
                    files.push(path);
                }
            }
        }
        Ok(())
    }

    /// Parse a session file and extract log entries
    fn parse_file(&self, path: &Path) -> Result<Vec<LogEntry>> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let session: GeminiSession = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse JSON in {}", path.display()))?;

        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let mut entries = Vec::new();

        for message in &session.messages {
            let msg_time = DateTime::parse_from_rfc3339(&message.timestamp)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            // Extract tool call events
            for tool_call in &message.tool_calls {
                let is_error = tool_call.status.as_deref() != Some("success")
                    && tool_call.status.is_some();

                if is_error {
                    let display = tool_call
                        .result_display
                        .clone()
                        .unwrap_or_else(|| format!("Tool '{}' failed", tool_call.name));

                    let msg = format!("Tool '{}' error: {}", tool_call.name, display);

                    entries.push(LogEntry {
                        source_id: None,
                        timestamp: parse_tool_timestamp(tool_call, msg_time),
                        level: Some("ERROR".to_string()),
                        service: Some(format!("gemini:{}", file_name)),
                        hostname: None,
                        message: msg.clone(),
                        raw_line: None,
                        structured_data: None,
                        fingerprint: Some(generate_fingerprint(&msg)),
                    });
                } else if self.include_all {
                    let display = tool_call.result_display.clone().unwrap_or_default();
                    let msg = format!(
                        "Tool '{}': {}",
                        tool_call.name,
                        display.chars().take(300).collect::<String>()
                    );

                    entries.push(LogEntry {
                        source_id: None,
                        timestamp: parse_tool_timestamp(tool_call, msg_time),
                        level: Some("INFO".to_string()),
                        service: Some(format!("gemini:{}", file_name)),
                        hostname: None,
                        message: msg,
                        raw_line: None,
                        structured_data: None,
                        fingerprint: None,
                    });
                }
            }
        }

        Ok(entries)
    }
}

/// Parse a tool call's timestamp, falling back to the message timestamp
fn parse_tool_timestamp(tool_call: &GeminiToolCall, fallback: DateTime<Utc>) -> DateTime<Utc> {
    tool_call
        .timestamp
        .as_deref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(fallback)
}

fn generate_fingerprint(message: &str) -> String {
    let normalized = message
        .chars()
        .take(100)
        .map(|c| if c.is_numeric() { '#' } else { c })
        .collect::<String>();

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl Connector for GeminiConnector {
    fn name(&self) -> &str {
        "Gemini CLI Connector"
    }

    fn source_type(&self) -> &str {
        "gemini"
    }

    fn can_read(&self, path: &str) -> bool {
        path.contains(".gemini") && path.contains("chats")
    }

    fn read_entries(&self, _since: Option<&str>) -> Result<Vec<LogEntry>> {
        let files = self.find_session_files()?;
        let mut all_entries = Vec::new();

        for file_path in files {
            match self.parse_file(&file_path) {
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
        None
    }
}

/// Top-level Gemini session file structure
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiSession {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    messages: Vec<GeminiMessage>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiMessage {
    #[serde(default)]
    id: Option<String>,
    timestamp: String,
    #[serde(rename = "type")]
    #[serde(default)]
    message_type: Option<String>,
    #[serde(default)]
    tool_calls: Vec<GeminiToolCall>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiToolCall {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    result_display: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session_json(tool_status: &str, result_display: &str) -> String {
        format!(
            r#"{{
  "sessionId": "test-session-id",
  "startTime": "2026-02-16T03:00:00.000Z",
  "messages": [
    {{
      "id": "msg-1",
      "timestamp": "2026-02-16T03:00:01.000Z",
      "type": "gemini",
      "toolCalls": [
        {{
          "id": "tool-1",
          "name": "shell_command",
          "status": "{}",
          "timestamp": "2026-02-16T03:00:01.500Z",
          "resultDisplay": "{}"
        }}
      ]
    }}
  ]
}}"#,
            tool_status, result_display
        )
    }

    #[test]
    fn test_parse_error_tool_call() {
        let connector = GeminiConnector::new("/tmp");
        let json = make_session_json("error", "Command not found: nonexistent-tool");

        let session: GeminiSession = serde_json::from_str(&json).unwrap();
        let file_name = "session-test.json";
        let mut entries = Vec::new();

        for message in &session.messages {
            let msg_time = DateTime::parse_from_rfc3339(&message.timestamp)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);

            for tool_call in &message.tool_calls {
                let is_error = tool_call.status.as_deref() != Some("success")
                    && tool_call.status.is_some();
                if is_error {
                    let display = tool_call.result_display.clone().unwrap_or_default();
                    let msg = format!("Tool '{}' error: {}", tool_call.name, display);
                    entries.push(LogEntry {
                        source_id: None,
                        timestamp: parse_tool_timestamp(tool_call, msg_time),
                        level: Some("ERROR".to_string()),
                        service: Some(format!("gemini:{}", file_name)),
                        hostname: None,
                        message: msg,
                        raw_line: None,
                        structured_data: None,
                        fingerprint: None,
                    });
                }
            }
        }

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, Some("ERROR".to_string()));
        assert!(entries[0].message.contains("shell_command"));
        assert!(entries[0].message.contains("Command not found"));
        let _ = connector; // silence unused warning
    }

    #[test]
    fn test_success_tool_call_ignored_in_errors_only_mode() {
        let json = make_session_json("success", "Files listed successfully");
        let session: GeminiSession = serde_json::from_str(&json).unwrap();

        // In errors-only mode, success tool calls should produce no entries
        let connector = GeminiConnector::new("/tmp");
        let mut entries = Vec::new();

        for message in &session.messages {
            let msg_time = Utc::now();
            for tool_call in &message.tool_calls {
                let is_error = tool_call.status.as_deref() != Some("success")
                    && tool_call.status.is_some();
                if is_error {
                    entries.push(tool_call.name.clone());
                } else if connector.include_all {
                    entries.push(tool_call.name.clone());
                }
            }
        }

        assert!(entries.is_empty(), "Success tool calls should be ignored in errors-only mode");
    }

    #[test]
    fn test_connector_name() {
        let connector = GeminiConnector::new("/tmp");
        assert_eq!(connector.name(), "Gemini CLI Connector");
    }

    #[test]
    fn test_connector_source_type() {
        let connector = GeminiConnector::new("/tmp");
        assert_eq!(connector.source_type(), "gemini");
    }

    #[test]
    fn test_can_read_gemini_paths() {
        let connector = GeminiConnector::new("/tmp");
        assert!(connector.can_read("/home/user/.gemini/tmp/workspace/chats/session-x.json"));
        assert!(!connector.can_read("/var/log/syslog"));
        assert!(!connector.can_read("/home/user/.gemini/config.json")); // no "chats"
    }

    #[test]
    fn test_fingerprint_generates_16_char_hex() {
        let fp = generate_fingerprint("Tool 'shell_command' error: not found");
        assert_eq!(fp.len(), 16);
    }

    #[test]
    fn test_parse_session_with_no_tool_calls() {
        let json = r#"{
  "sessionId": "abc",
  "startTime": "2026-02-16T03:00:00.000Z",
  "messages": [
    {
      "id": "m1",
      "timestamp": "2026-02-16T03:00:01.000Z",
      "type": "user",
      "toolCalls": []
    }
  ]
}"#;
        let session: GeminiSession = serde_json::from_str(json).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert!(session.messages[0].tool_calls.is_empty());
    }
}
