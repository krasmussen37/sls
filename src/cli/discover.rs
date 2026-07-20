//! sls discover - Auto-discover log sources on this machine
//!
//! Scans known log locations and uses heuristics to identify log files:
//! - Known paths (/var/log/syslog, journald, ~/.claude/logs/)
//! - File extensions (.log, .jsonl, .err)
//! - Content patterns (timestamps, log levels, stack traces)
//! - Directory conventions (logs/, stderr/, error/)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::db::{default_db_path, Database};

/// A discovered log source with confidence score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSource {
    pub id: String,
    pub source_type: String,
    pub path: Option<String>,
    pub description: String,
    pub confidence: u8,
    pub selected: bool,
}

#[derive(Serialize)]
struct DiscoverOutput {
    success: bool,
    sources: Vec<DiscoveredSource>,
    auto_mode: bool,
}

pub async fn run(auto: bool, json_output: bool) -> Result<()> {
    // Discover all potential log sources
    let mut sources = discover_sources();

    // In auto mode, auto-select high-confidence sources
    if auto {
        for source in &mut sources {
            source.selected = source.confidence >= 80;
        }
    }

    let output = DiscoverOutput {
        success: true,
        sources: sources.clone(),
        auto_mode: auto,
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("SLS Log Source Discovery");
        println!("========================");
        println!();

        if sources.is_empty() {
            println!("No log sources discovered.");
        } else {
            println!("Discovered {} potential log sources:", sources.len());
            println!();

            for source in &sources {
                let status = if source.selected { "[+]" } else { "[ ]" };
                let path_info = source
                    .path
                    .as_ref()
                    .map(|p| format!(" ({})", p))
                    .unwrap_or_default();

                println!(
                    "  {} {:20} {:3}% - {}{}",
                    status, source.id, source.confidence, source.description, path_info
                );
            }

            println!();

            // Save selected sources if any
            let selected: Vec<_> = sources.iter().filter(|s| s.selected).collect();
            if !selected.is_empty() {
                save_sources(&selected)?;
                println!(
                    "Saved {} source(s) to database.",
                    selected.len()
                );
            } else if auto {
                println!("No high-confidence sources found (>80%).");
                println!("Run without --auto to manually select sources.");
            } else {
                println!("Run with --auto to auto-accept high-confidence sources.");
                println!("Or use 'sls sources add' to manually add sources.");
            }
        }
    }

    Ok(())
}

/// Discover all potential log sources on this machine
pub fn discover_sources() -> Vec<DiscoveredSource> {
    let mut sources = Vec::new();

    // Check journald
    if check_journald_available() {
        sources.push(DiscoveredSource {
            id: "journald".to_string(),
            source_type: "journald".to_string(),
            path: None,
            description: "systemd journal".to_string(),
            confidence: 90,
            selected: false,
        });
    }

    // Check syslog
    let syslog_paths = ["/var/log/syslog", "/var/log/messages"];
    for path in &syslog_paths {
        if Path::new(path).exists() {
            let confidence = analyze_log_file(path);
            if confidence > 50 {
                sources.push(DiscoveredSource {
                    id: format!("syslog-{}", path.replace('/', "-").trim_matches('-')),
                    source_type: "syslog".to_string(),
                    path: Some(path.to_string()),
                    description: "syslog format".to_string(),
                    confidence,
                    selected: false,
                });
            }
        }
    }

    // Check Claude Code logs
    if let Some(home) = dirs::home_dir() {
        let claude_logs = home.join(".claude").join("logs");
        if claude_logs.exists() {
            sources.push(DiscoveredSource {
                id: "claude-sessions".to_string(),
                source_type: "agent_stderr".to_string(),
                path: Some(claude_logs.to_string_lossy().to_string()),
                description: "Claude Code session JSONL".to_string(),
                confidence: 92,
                selected: false,
            });
        }

        // Check for .claude/projects (newer Claude Code)
        let claude_projects = home.join(".claude").join("projects");
        if claude_projects.exists() {
            sources.push(DiscoveredSource {
                id: "claude-projects".to_string(),
                source_type: "agent_stderr".to_string(),
                path: Some(claude_projects.to_string_lossy().to_string()),
                description: "Claude Code project logs".to_string(),
                confidence: 88,
                selected: false,
            });
        }

        // Codex has used both a legacy text log and rotating SQLite logs.
        // Register the stable ~/.codex root so the connector follows rotations.
        let codex_root = home.join(".codex");
        let codex_tui_log = codex_root.join("log").join("codex-tui.log");
        let codex_sqlite_log = fs::read_dir(&codex_root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .any(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("logs_") && name.ends_with(".sqlite"))
                    .unwrap_or(false)
            });
        if codex_tui_log.exists() || codex_sqlite_log {
            sources.push(DiscoveredSource {
                id: "codex".to_string(),
                source_type: "codex".to_string(),
                path: Some(codex_root.to_string_lossy().to_string()),
                description: "Codex CLI logs".to_string(),
                confidence: 90,
                selected: false,
            });
        }

        // Check Gemini CLI session files (~/.gemini/tmp/)
        let gemini_tmp = home.join(".gemini").join("tmp");
        if gemini_tmp.exists() {
            sources.push(DiscoveredSource {
                id: "gemini-sessions".to_string(),
                source_type: "gemini".to_string(),
                path: Some(gemini_tmp.to_string_lossy().to_string()),
                description: "Gemini CLI chat sessions".to_string(),
                confidence: 88,
                selected: false,
            });
        }

        // Check ~/.local/state for various app logs
        let local_state = home.join(".local").join("state");
        if local_state.exists() {
            scan_directory_for_logs(&local_state, &mut sources, 3);
        }
    }

    // Check /var/log for additional logs
    let var_log = Path::new("/var/log");
    if var_log.exists() {
        scan_var_log(&mut sources);
    }

    // Sort by confidence descending
    sources.sort_by(|a, b| b.confidence.cmp(&a.confidence));

    sources
}

/// Check if journald is available
fn check_journald_available() -> bool {
    std::process::Command::new("journalctl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Analyze a log file to determine confidence score
fn analyze_log_file(path: &str) -> u8 {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };

    let reader = BufReader::new(file);
    let mut score = 50u8;
    let mut lines_checked = 0;

    for line in reader.lines().take(20) {
        if let Ok(line) = line {
            lines_checked += 1;

            // Check for timestamp patterns
            if has_timestamp_pattern(&line) {
                score = score.saturating_add(5);
            }

            // Check for log level keywords
            if has_log_level(&line) {
                score = score.saturating_add(5);
            }

            // Check for common log patterns
            if line.contains('[') && line.contains(']') {
                score = score.saturating_add(2);
            }
        }
    }

    if lines_checked == 0 {
        return 0;
    }

    score.min(99)
}

/// Check if a line has timestamp patterns
fn has_timestamp_pattern(line: &str) -> bool {
    // ISO-8601: 2024-01-15T10:30:00
    // Syslog: Jan 15 10:30:00
    // Common: [2024-01-15 10:30:00]
    let patterns = [
        r"\d{4}-\d{2}-\d{2}",      // ISO date
        r"\d{2}:\d{2}:\d{2}",      // Time HH:MM:SS
        r"[A-Z][a-z]{2}\s+\d{1,2}", // Month Day
    ];

    for pattern in &patterns {
        if regex::Regex::new(pattern)
            .map(|r| r.is_match(line))
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Check if a line has log level keywords
fn has_log_level(line: &str) -> bool {
    let levels = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE", "FATAL", "CRITICAL"];
    let upper = line.to_uppercase();
    levels.iter().any(|l| upper.contains(l))
}

/// Scan a directory for potential log files
fn scan_directory_for_logs(dir: &Path, sources: &mut Vec<DiscoveredSource>, depth: u8) {
    if depth == 0 {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Check if directory name suggests logs
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.contains("log") || name.contains("stderr") || name.contains("error") {
                scan_directory_for_logs(&path, sources, depth - 1);
            }
        } else if path.is_file() {
            // Check file extension
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            if ext == "log" || ext == "jsonl" || ext == "err" || name.ends_with(".log") {
                let confidence = analyze_log_file(path.to_str().unwrap_or(""));
                if confidence > 60 {
                    let id = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    sources.push(DiscoveredSource {
                        id: format!("file-{}", id),
                        source_type: if ext == "jsonl" { "json" } else { "syslog" }.to_string(),
                        path: Some(path.to_string_lossy().to_string()),
                        description: format!("{} file", ext.to_uppercase()),
                        confidence,
                        selected: false,
                    });
                }
            }
        }
    }
}

/// Scan /var/log for common log files
fn scan_var_log(sources: &mut Vec<DiscoveredSource>) {
    let common_logs = [
        ("/var/log/auth.log", "authentication logs", "syslog"),
        ("/var/log/kern.log", "kernel logs", "syslog"),
        ("/var/log/dpkg.log", "package manager logs", "syslog"),
        ("/var/log/nginx/access.log", "nginx access logs", "nginx"),
        ("/var/log/nginx/error.log", "nginx error logs", "nginx"),
        ("/var/log/apache2/access.log", "apache access logs", "apache"),
        ("/var/log/apache2/error.log", "apache error logs", "apache"),
    ];

    for (path, desc, source_type) in &common_logs {
        if Path::new(path).exists() {
            let confidence = analyze_log_file(path);
            if confidence > 40 {
                let id = path
                    .split('/')
                    .last()
                    .unwrap_or("unknown")
                    .replace(".log", "");

                sources.push(DiscoveredSource {
                    id,
                    source_type: source_type.to_string(),
                    path: Some(path.to_string()),
                    description: desc.to_string(),
                    confidence,
                    selected: false,
                });
            }
        }
    }
}

/// Save discovered sources to the database
fn save_sources(sources: &[&DiscoveredSource]) -> Result<()> {
    let db_path = default_db_path();
    let db = Database::open(&db_path)?;

    for source in sources {
        db.conn().execute(
            "INSERT OR REPLACE INTO log_sources (source_type, source_path, active, discovered, confidence)
             VALUES (?, ?, 1, 1, ?)",
            rusqlite::params![source.source_type, source.path, source.confidence as i32],
        )?;
    }

    Ok(())
}
