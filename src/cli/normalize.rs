//! Normalize arbitrary log formats to SLS schema
//!
//! Converts various log formats (syslog, JSON, plain text) to the SLS schema format.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// Normalized log entry in SLS schema format
#[derive(Debug, Serialize, Deserialize)]
pub struct NormalizedEntry {
    pub timestamp_utc: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_data: Option<serde_json::Value>,
}

/// Detect and parse log format
fn parse_log_line(line: &str) -> Result<NormalizedEntry> {
    // Try JSON first
    if let Ok(json_entry) = parse_json_log(line) {
        return Ok(json_entry);
    }

    // Try syslog format: "Jan  2 15:04:05 hostname service[pid]: message"
    if let Ok(syslog_entry) = parse_syslog(line) {
        return Ok(syslog_entry);
    }

    // Fallback: plain text with current timestamp
    Ok(NormalizedEntry {
        timestamp_utc: Utc::now().timestamp(),
        level: None,
        service: None,
        hostname: None,
        message: line.to_string(),
        raw_line: Some(line.to_string()),
        structured_data: None,
    })
}

/// Parse JSON log format
fn parse_json_log(line: &str) -> Result<NormalizedEntry> {
    let json: serde_json::Value = serde_json::from_str(line)?;

    // Extract timestamp (try common field names)
    let timestamp_utc = if let Some(ts) = json.get("timestamp").or_else(|| json.get("time")) {
        if let Some(ts_str) = ts.as_str() {
            DateTime::parse_from_rfc3339(ts_str)
                .map(|dt| dt.timestamp())
                .unwrap_or_else(|_| Utc::now().timestamp())
        } else if let Some(ts_num) = ts.as_i64() {
            ts_num
        } else {
            Utc::now().timestamp()
        }
    } else {
        Utc::now().timestamp()
    };

    // Extract level
    let level = json
        .get("level")
        .or_else(|| json.get("severity"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_uppercase());

    // Extract service
    let service = json
        .get("service")
        .or_else(|| json.get("logger"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Extract message
    let message = json
        .get("message")
        .or_else(|| json.get("msg"))
        .and_then(|v| v.as_str())
        .unwrap_or(line)
        .to_string();

    // Extract hostname
    let hostname = json
        .get("hostname")
        .or_else(|| json.get("host"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(NormalizedEntry {
        timestamp_utc,
        level,
        service,
        hostname,
        message,
        raw_line: Some(line.to_string()),
        structured_data: Some(json),
    })
}

/// Parse syslog format
fn parse_syslog(line: &str) -> Result<NormalizedEntry> {
    // Very basic syslog parsing - can be enhanced with proper syslog parser
    // Check if starts with month abbreviation (basic validation)
    const MONTHS: &[&str] = &["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if !MONTHS.iter().any(|m| line.starts_with(m)) {
        anyhow::bail!("Not syslog format");
    }

    // Split and filter out empty strings to handle double spaces
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        anyhow::bail!("Not syslog format");
    }

    // parts[0] = month, parts[1] = day, parts[2] = time
    // parts[3] = hostname or service (if no hostname)
    // parts[4]+ = service and message or just message

    // Try to extract hostname and service from parts[3] and parts[4]
    let hostname = if parts[3].contains(':') {
        None
    } else {
        Some(parts[3].to_string())
    };

    let message_part = if hostname.is_some() {
        parts[4..].join(" ")
    } else {
        parts[3..].join(" ")
    };

    // Extract service name if format is "service[pid]:" or "service:"
    let (service, message) = if let Some(colon_pos) = message_part.find(':') {
        let service_part = &message_part[..colon_pos];
        let service_name = if let Some(bracket_pos) = service_part.find('[') {
            service_part[..bracket_pos].to_string()
        } else {
            service_part.to_string()
        };
        let msg = message_part[colon_pos + 1..].trim().to_string();
        (Some(service_name), msg)
    } else {
        (None, message_part.to_string())
    };

    // Use current time as we don't have year in syslog timestamp
    let timestamp_utc = Utc::now().timestamp();

    Ok(NormalizedEntry {
        timestamp_utc,
        level: None,
        service,
        hostname,
        message,
        raw_line: Some(line.to_string()),
        structured_data: None,
    })
}

/// Run the normalize command
pub async fn run(
    input_path: Option<PathBuf>,
    format: Option<String>,
    json_output: bool,
) -> Result<()> {
    let reader: Box<dyn BufRead> = if let Some(path) = input_path {
        let file = std::fs::File::open(&path)?;
        Box::new(BufReader::new(file))
    } else {
        Box::new(BufReader::new(std::io::stdin()))
    };

    let mut entries = Vec::new();

    for line_result in reader.lines() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }

        match parse_log_line(&line) {
            Ok(entry) => {
                if json_output {
                    println!("{}", serde_json::to_string(&entry)?);
                } else {
                    entries.push(entry);
                }
            }
            Err(e) => {
                if !json_output {
                    eprintln!("Warning: Failed to parse line: {}", e);
                }
            }
        }
    }

    // If not streaming JSON, output all entries as JSON array
    if !json_output && !entries.is_empty() {
        if let Some(fmt) = format {
            match fmt.as_str() {
                "json" => println!("{}", serde_json::to_string_pretty(&entries)?),
                "ndjson" => {
                    for entry in entries {
                        println!("{}", serde_json::to_string(&entry)?);
                    }
                }
                _ => {
                    println!("Normalized {} log entries", entries.len());
                    println!("Use --format json or --format ndjson to see output");
                }
            }
        } else {
            println!("Normalized {} log entries", entries.len());
            println!("Use --format json or --format ndjson to see output");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_log() {
        let line = r#"{"timestamp":"2024-01-02T15:04:05Z","level":"ERROR","service":"api","message":"Connection failed"}"#;
        let entry = parse_json_log(line).unwrap();
        assert_eq!(entry.level, Some("ERROR".to_string()));
        assert_eq!(entry.service, Some("api".to_string()));
        assert_eq!(entry.message, "Connection failed");
    }

    #[test]
    fn test_parse_plain_text() {
        let line = "This is a plain text log message";
        let entry = parse_log_line(line).unwrap();
        assert_eq!(entry.message, line);
        assert!(entry.level.is_none());
    }

    #[test]
    fn test_parse_syslog() {
        let line = "Jan  2 15:04:05 myhost sshd[1234]: Connection from 192.168.1.1";
        let entry = parse_syslog(line).unwrap();
        assert_eq!(entry.service, Some("sshd".to_string()));
        assert!(entry.message.contains("Connection from"));
    }
}
