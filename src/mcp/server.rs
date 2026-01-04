//! SLS MCP Server implementation
//!
//! Implements a simple MCP server using JSON-RPC over stdio.
//! Based on the Model Context Protocol specification.

use anyhow::Result;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

use crate::indexer::{parse_relative_time, Indexer, SearchRequest};
use crate::mcp::tools::*;

/// JSON-RPC request
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
            }),
        }
    }
}

/// MCP Server state
struct McpServer {
    indexer: Indexer,
}

impl McpServer {
    fn new() -> Result<Self> {
        let indexer = Indexer::open_default()?;
        Ok(Self { indexer })
    }

    /// Handle a JSON-RPC request
    async fn handle_request(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(request.id.clone()),
            "initialized" => JsonRpcResponse::success(request.id.clone(), json!({})),
            "tools/list" => self.handle_tools_list(request.id.clone()),
            "tools/call" => self.handle_tools_call(request.id.clone(), &request.params).await,
            _ => JsonRpcResponse::error(
                request.id.clone(),
                -32601,
                &format!("Method not found: {}", request.method),
            ),
        }
    }

    /// Handle initialize request
    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "sls-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "SLS MCP Server - Search and analyze system logs. Use sls_search to find log entries, sls_alert to check thresholds, sls_tail for recent logs, and sls_capabilities for introspection."
            }),
        )
    }

    /// Handle tools/list request
    fn handle_tools_list(&self, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "tools": [
                    {
                        "name": "sls_search",
                        "description": "Search indexed system logs with keyword queries. Returns matching log entries with timestamps, levels, and messages.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "Search query to match against log messages"
                                },
                                "level": {
                                    "type": "string",
                                    "description": "Filter by log level (ERROR, WARN, INFO, DEBUG)"
                                },
                                "service": {
                                    "type": "string",
                                    "description": "Filter by service name"
                                },
                                "since": {
                                    "type": "string",
                                    "description": "Time range filter (e.g., '1h', '30m', '1d')"
                                },
                                "limit": {
                                    "type": "integer",
                                    "description": "Maximum number of results to return (default: 50)"
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "sls_alert",
                        "description": "Check log error/warning counts against thresholds. Returns OK, WARNING, or CRITICAL status based on counts.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "since": {
                                    "type": "string",
                                    "description": "Time range to check (e.g., '1h', '30m', '1d')"
                                },
                                "error_threshold": {
                                    "type": "integer",
                                    "description": "Error count threshold for CRITICAL status (default: 1)"
                                },
                                "warning_threshold": {
                                    "type": "integer",
                                    "description": "Warning count threshold for WARNING status (default: 10)"
                                },
                                "service": {
                                    "type": "string",
                                    "description": "Filter by service name"
                                }
                            }
                        }
                    },
                    {
                        "name": "sls_tail",
                        "description": "Get recent log entries from a source. Returns a snapshot of recent logs, not a live stream.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "source": {
                                    "type": "string",
                                    "description": "Log source type: journald, syslog, or file"
                                },
                                "path": {
                                    "type": "string",
                                    "description": "Path to log file (for syslog or file sources)"
                                },
                                "unit": {
                                    "type": "string",
                                    "description": "Systemd unit filter (for journald source)"
                                },
                                "lines": {
                                    "type": "integer",
                                    "description": "Number of recent lines to return (default: 50)"
                                }
                            }
                        }
                    },
                    {
                        "name": "sls_capabilities",
                        "description": "Get SLS capabilities and available commands for agent introspection.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                ]
            }),
        )
    }

    /// Handle tools/call request
    async fn handle_tools_call(&self, id: Option<Value>, params: &Option<Value>) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => return JsonRpcResponse::error(id, -32602, "Missing params"),
        };

        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        match tool_name {
            "sls_search" => self.call_search(id, arguments).await,
            "sls_alert" => self.call_alert(id, arguments).await,
            "sls_tail" => self.call_tail(id, arguments).await,
            "sls_capabilities" => self.call_capabilities(id).await,
            _ => JsonRpcResponse::error(id, -32602, &format!("Unknown tool: {}", tool_name)),
        }
    }

    /// Call sls_search tool
    async fn call_search(&self, id: Option<Value>, args: Value) -> JsonRpcResponse {
        let params: SearchParams = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(id, -32602, &format!("Invalid params: {}", e)),
        };

        let mut request = SearchRequest::new(&params.query).with_limit(params.limit.unwrap_or(50));

        if let Some(level) = params.level {
            request = request.with_level(&level);
        }
        if let Some(service) = params.service {
            request = request.with_service(&service);
        }
        if let Some(since) = params.since {
            if let Some(ts) = parse_relative_time(&since) {
                request = request.with_since(ts);
            }
        }

        match self.indexer.search(&request).await {
            Ok(result) => {
                let entries: Vec<SearchResultEntry> = result
                    .entries
                    .iter()
                    .map(|e| SearchResultEntry {
                        id: e.id,
                        timestamp: Utc
                            .timestamp_opt(e.timestamp_utc, 0)
                            .single()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| e.timestamp_utc.to_string()),
                        level: e.level.clone(),
                        service: e.service.clone(),
                        message: e.message.clone(),
                    })
                    .collect();

                let output = SearchResult {
                    success: true,
                    query: params.query,
                    total_matches: entries.len(),
                    entries,
                };

                JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&output).unwrap()
                        }]
                    }),
                )
            }
            Err(e) => JsonRpcResponse::error(id, -32000, &format!("Search failed: {}", e)),
        }
    }

    /// Call sls_alert tool
    async fn call_alert(&self, id: Option<Value>, args: Value) -> JsonRpcResponse {
        let params: AlertParams = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(id, -32602, &format!("Invalid params: {}", e)),
        };

        let error_threshold = params.error_threshold.unwrap_or(1);
        let warning_threshold = params.warning_threshold.unwrap_or(10);

        let time_filter = if let Some(s) = &params.since {
            parse_relative_time(s)
        } else {
            parse_relative_time("1h")
        };

        let time_range = params.since.clone().unwrap_or_else(|| "1h".to_string());

        // Count errors
        let mut error_request = SearchRequest::new("").with_level("ERROR").with_limit(1_000_000);
        if let Some(ts) = time_filter {
            error_request = error_request.with_since(ts);
        }
        if let Some(svc) = &params.service {
            error_request = error_request.with_service(svc);
        }

        let error_count = match self.indexer.search(&error_request).await {
            Ok(r) => r.entries.len(),
            Err(_) => 0,
        };

        // Count warnings
        let mut warn_request = SearchRequest::new("").with_level("WARN").with_limit(1_000_000);
        if let Some(ts) = time_filter {
            warn_request = warn_request.with_since(ts);
        }
        if let Some(svc) = &params.service {
            warn_request = warn_request.with_service(svc);
        }

        let warning_count = match self.indexer.search(&warn_request).await {
            Ok(r) => r.entries.len(),
            Err(_) => 0,
        };

        // Determine status
        let (status, message) = if error_count >= error_threshold {
            (
                AlertStatus::Critical,
                format!(
                    "CRITICAL: {} errors in last {} (threshold: {})",
                    error_count, time_range, error_threshold
                ),
            )
        } else if warning_count >= warning_threshold {
            (
                AlertStatus::Warning,
                format!(
                    "WARNING: {} warnings in last {} (threshold: {})",
                    warning_count, time_range, warning_threshold
                ),
            )
        } else {
            (
                AlertStatus::Ok,
                format!(
                    "OK: {} errors, {} warnings in last {}",
                    error_count, warning_count, time_range
                ),
            )
        };

        let output = AlertResult {
            status,
            error_count,
            warning_count,
            error_threshold,
            warning_threshold,
            time_range,
            message,
        };

        JsonRpcResponse::success(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&output).unwrap()
                }]
            }),
        )
    }

    /// Call sls_tail tool
    async fn call_tail(&self, id: Option<Value>, args: Value) -> JsonRpcResponse {
        let params: TailParams = match serde_json::from_value(args) {
            Ok(p) => p,
            Err(e) => return JsonRpcResponse::error(id, -32602, &format!("Invalid params: {}", e)),
        };

        let source = params.source.unwrap_or_else(|| "journald".to_string());
        let lines = params.lines.unwrap_or(50);

        let request = SearchRequest::new("").with_limit(lines);

        match self.indexer.search(&request).await {
            Ok(result) => {
                let entries: Vec<TailEntry> = result
                    .entries
                    .iter()
                    .map(|e| TailEntry {
                        timestamp: Utc
                            .timestamp_opt(e.timestamp_utc, 0)
                            .single()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| e.timestamp_utc.to_string()),
                        level: e.level.clone(),
                        message: e.message.clone(),
                    })
                    .collect();

                let output = TailResult {
                    success: true,
                    source,
                    entries,
                };

                JsonRpcResponse::success(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&output).unwrap()
                        }]
                    }),
                )
            }
            Err(e) => JsonRpcResponse::error(id, -32000, &format!("Tail failed: {}", e)),
        }
    }

    /// Call sls_capabilities tool
    async fn call_capabilities(&self, id: Option<Value>) -> JsonRpcResponse {
        let output = CapabilitiesResult {
            name: "sls".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "System Log Search - Index and search system logs, agent stderr, and application logs".to_string(),
            commands: vec![
                CommandInfo {
                    name: "search".to_string(),
                    description: "Search indexed logs with keyword queries".to_string(),
                },
                CommandInfo {
                    name: "alert".to_string(),
                    description: "Check error/warning thresholds for alerting".to_string(),
                },
                CommandInfo {
                    name: "tail".to_string(),
                    description: "Get recent log entries from a source".to_string(),
                },
                CommandInfo {
                    name: "similar".to_string(),
                    description: "Find similar log patterns using semantic search".to_string(),
                },
                CommandInfo {
                    name: "context".to_string(),
                    description: "Get logs around a timestamp for root cause analysis".to_string(),
                },
                CommandInfo {
                    name: "status".to_string(),
                    description: "Show indexing status and health".to_string(),
                },
                CommandInfo {
                    name: "discover".to_string(),
                    description: "Auto-discover log sources on this machine".to_string(),
                },
            ],
        };

        JsonRpcResponse::success(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&output).unwrap()
                }]
            }),
        )
    }
}

/// Run the MCP server on stdio
pub async fn run_mcp_server() -> Result<()> {
    let server = McpServer::new()?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // MCP uses newline-delimited JSON-RPC
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Error reading stdin: {}", e);
                continue;
            }
        };

        if line.is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let response = JsonRpcResponse::error(None, -32700, &format!("Parse error: {}", e));
                let response_str = serde_json::to_string(&response)?;
                writeln!(stdout, "{}", response_str)?;
                stdout.flush()?;
                continue;
            }
        };

        // Skip notifications (no id)
        if request.id.is_none() && request.method == "notifications/initialized" {
            continue;
        }

        let response = server.handle_request(&request).await;

        // Only send response if there was an id (not a notification)
        if request.id.is_some() || response.error.is_some() {
            let response_str = serde_json::to_string(&response)?;
            writeln!(stdout, "{}", response_str)?;
            stdout.flush()?;
        }
    }

    Ok(())
}
