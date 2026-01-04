//! sls search - Search indexed logs
//!
//! Implements the on-demand indexing pattern:
//! 1. Quick sync recent events before searching
//! 2. Return results immediately
//! 3. Spawn background indexer for historical data

use anyhow::Result;
use chrono::{Local, TimeZone, Utc};
use serde::Serialize;

use crate::indexer::{parse_relative_time, Indexer, SearchRequest};

#[derive(Serialize)]
struct SearchOutput {
    success: bool,
    query: String,
    total_matches: usize,
    synced_before_search: bool,
    entries: Vec<LogEntryOutput>,
}

#[derive(Serialize)]
struct LogEntryOutput {
    id: i64,
    timestamp: String,
    level: Option<String>,
    service: Option<String>,
    message: String,
}

pub async fn run(
    query: String,
    level: Option<String>,
    service: Option<String>,
    since: Option<String>,
    last: Option<String>,
    today: bool,
    limit: usize,
    format: Option<String>,
    json_output: bool,
) -> Result<()> {
    // Open the indexer (creates DB if needed)
    let indexer = Indexer::open_default()?;

    // Build search request
    let mut request = SearchRequest::new(&query).with_limit(limit);

    if let Some(ref lvl) = level {
        request = request.with_level(lvl);
    }
    if let Some(ref svc) = service {
        request = request.with_service(svc);
    }

    // Handle time filters: --today takes priority, then --since, then --last
    if today {
        // Get start of today in local time, convert to UTC timestamp
        // Use earliest() to handle DST transitions safely
        let naive_today = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
        if let Some(local_dt) = naive_today.and_local_timezone(Local).earliest() {
            request = request.with_since(local_dt.timestamp());
        }
    } else if let Some(ref s) = since {
        if let Some(ts) = parse_relative_time(s) {
            request = request.with_since(ts);
        }
    } else if let Some(ref l) = last {
        if let Some(ts) = parse_relative_time(l) {
            request = request.with_since(ts);
        }
    }

    // Perform the search (with on-demand sync)
    let result = indexer.search(&request).await?;

    // Convert entries to output format
    let entries: Vec<LogEntryOutput> = result
        .entries
        .iter()
        .map(|e| LogEntryOutput {
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

    let output = SearchOutput {
        success: true,
        query: query.clone(),
        total_matches: entries.len(),
        synced_before_search: result.synced_before_search,
        entries,
    };

    // Determine output format: explicit --format takes priority, then --json flag
    let output_format = format.as_deref().unwrap_or(if json_output { "json" } else { "table" });

    match output_format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        "csv" => {
            // CSV header
            println!("id,timestamp,level,service,message");
            for entry in &output.entries {
                // Escape CSV fields
                let level = entry.level.as_deref().unwrap_or("");
                let service = entry.service.as_deref().unwrap_or("");
                let message = escape_csv(&entry.message);
                println!(
                    "{},{},{},{},{}",
                    entry.id, entry.timestamp, level, service, message
                );
            }
        }
        _ => {
            // Default table format
            println!("Searching for: \"{}\"", query);
            if let Some(ref lvl) = level {
                println!("  Level filter: {}", lvl);
            }
            if let Some(ref svc) = service {
                println!("  Service filter: {}", svc);
            }
            if today {
                println!("  Time filter: today");
            } else if let Some(ref s) = since {
                println!("  Since: {}", s);
            } else if let Some(ref l) = last {
                println!("  Last: {}", l);
            }
            println!("  Limit: {}", limit);
            println!();

            if output.entries.is_empty() {
                println!("No matches found.");
                if !result.synced_before_search {
                    println!();
                    println!("Tip: No sources configured. Run 'sls discover' to find log sources.");
                }
            } else {
                println!("Found {} matches:", output.entries.len());
                println!();
                for entry in &output.entries {
                    let level_str = entry
                        .level
                        .as_ref()
                        .map(|l| format!("[{}]", l))
                        .unwrap_or_default();
                    let service_str = entry
                        .service
                        .as_ref()
                        .map(|s| format!("{}: ", s))
                        .unwrap_or_default();
                    println!(
                        "{} {} {}{}",
                        entry.timestamp, level_str, service_str, entry.message
                    );
                }
            }
        }
    }

    Ok(())
}

/// Escape a string for CSV output
fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
