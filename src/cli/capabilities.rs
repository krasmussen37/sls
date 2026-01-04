//! sls capabilities - Print capabilities for agent introspection

use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
struct Capabilities {
    name: String,
    version: String,
    description: String,
    commands: Vec<CommandInfo>,
}

#[derive(Serialize)]
struct CommandInfo {
    name: String,
    description: String,
    args: Vec<ArgInfo>,
}

#[derive(Serialize)]
struct ArgInfo {
    name: String,
    description: String,
    required: bool,
}

pub async fn run(json_output: bool) -> Result<()> {
    let caps = Capabilities {
        name: "sls".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "System Log Search - Index and search system logs, agent stderr, and application logs".to_string(),
        commands: vec![
            CommandInfo {
                name: "search".to_string(),
                description: "Search indexed logs with keyword queries".to_string(),
                args: vec![
                    ArgInfo { name: "query".to_string(), description: "Search query".to_string(), required: true },
                    ArgInfo { name: "--level".to_string(), description: "Filter by log level".to_string(), required: false },
                    ArgInfo { name: "--service".to_string(), description: "Filter by service".to_string(), required: false },
                    ArgInfo { name: "--since".to_string(), description: "Time range (e.g., 1h, 30m)".to_string(), required: false },
                    ArgInfo { name: "--last".to_string(), description: "Time range preset (alias for --since)".to_string(), required: false },
                    ArgInfo { name: "--today".to_string(), description: "Show only today's logs".to_string(), required: false },
                    ArgInfo { name: "--format".to_string(), description: "Output format (table, json, csv)".to_string(), required: false },
                ],
            },
            CommandInfo {
                name: "alert".to_string(),
                description: "Check error/warning thresholds for alerting".to_string(),
                args: vec![
                    ArgInfo { name: "--since".to_string(), description: "Time range (e.g., 1h, 30m)".to_string(), required: false },
                    ArgInfo { name: "--last".to_string(), description: "Time range preset (alias for --since)".to_string(), required: false },
                    ArgInfo { name: "--today".to_string(), description: "Check only today's logs".to_string(), required: false },
                    ArgInfo { name: "--error-threshold".to_string(), description: "Error count for CRITICAL (default: 1)".to_string(), required: false },
                    ArgInfo { name: "--warning-threshold".to_string(), description: "Warning count for WARNING (default: 10)".to_string(), required: false },
                    ArgInfo { name: "--service".to_string(), description: "Filter by service".to_string(), required: false },
                ],
            },
            CommandInfo {
                name: "similar".to_string(),
                description: "Find similar log patterns using semantic search".to_string(),
                args: vec![
                    ArgInfo { name: "query".to_string(), description: "Query to match".to_string(), required: true },
                    ArgInfo { name: "--limit".to_string(), description: "Max results".to_string(), required: false },
                ],
            },
            CommandInfo {
                name: "context".to_string(),
                description: "Get logs around a timestamp for root cause analysis".to_string(),
                args: vec![
                    ArgInfo { name: "--at".to_string(), description: "Timestamp to center on".to_string(), required: false },
                    ArgInfo { name: "--session-id".to_string(), description: "Agent session ID".to_string(), required: false },
                ],
            },
            CommandInfo {
                name: "status".to_string(),
                description: "Show indexing status and health".to_string(),
                args: vec![],
            },
            CommandInfo {
                name: "discover".to_string(),
                description: "Auto-discover log sources on this machine".to_string(),
                args: vec![
                    ArgInfo { name: "--auto".to_string(), description: "Auto-accept high-confidence sources".to_string(), required: false },
                ],
            },
            CommandInfo {
                name: "tail".to_string(),
                description: "Stream logs from a source in real time".to_string(),
                args: vec![
                    ArgInfo { name: "--source".to_string(), description: "Source type (journald, syslog, file)".to_string(), required: false },
                    ArgInfo { name: "--path".to_string(), description: "Path to log file (syslog/file)".to_string(), required: false },
                    ArgInfo { name: "--unit".to_string(), description: "Systemd unit filter (journald)".to_string(), required: false },
                    ArgInfo { name: "--lines".to_string(), description: "Number of recent lines to show before following".to_string(), required: false },
                ],
            },
            CommandInfo {
                name: "coverage".to_string(),
                description: "Show log coverage status (indexed vs discovered vs missing)".to_string(),
                args: vec![
                    ArgInfo { name: "--init".to_string(), description: "Initialize a sample service catalog".to_string(), required: false },
                ],
            },
        ],
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&caps)?);
    } else {
        println!("SLS - System Log Search");
        println!("=======================");
        println!("Version: {}", caps.version);
        println!();
        println!("Commands:");
        for cmd in &caps.commands {
            println!("  {} - {}", cmd.name, cmd.description);
        }
        println!();
        println!("Use --json for machine-readable output.");
    }
    Ok(())
}
