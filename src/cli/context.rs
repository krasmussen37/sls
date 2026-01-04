//! sls context - Get logs around a specific timestamp or session
//!
//! Use this command for root cause analysis by viewing logs surrounding
//! a specific point in time or from a particular agent session.

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::params;
use serde::Serialize;

use crate::db::{default_db_path, Database};
use crate::indexer::{parse_relative_time, Indexer};

#[derive(Serialize)]
struct ContextResult {
    success: bool,
    center_timestamp: Option<String>,
    session_id: Option<String>,
    window_size: usize,
    before: Vec<LogEntryOutput>,
    after: Vec<LogEntryOutput>,
}

#[derive(Serialize, Clone)]
struct LogEntryOutput {
    id: i64,
    timestamp: String,
    level: Option<String>,
    service: Option<String>,
    message: String,
}

/// Parse a timestamp string (ISO-8601 or relative like "-5m")
fn parse_timestamp(s: &str) -> Option<i64> {
    // Try relative time first
    if s.starts_with('-') || s.starts_with('+') {
        return parse_relative_time(&s[1..]).map(|ts| {
            if s.starts_with('+') {
                Utc::now().timestamp() + (Utc::now().timestamp() - ts)
            } else {
                ts
            }
        });
    }

    // Try ISO-8601 format
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }

    // Try common formats
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp());
    }

    // Try epoch timestamp
    if let Ok(ts) = s.parse::<i64>() {
        return Some(ts);
    }

    None
}

pub async fn run(
    at: Option<String>,
    session_id: Option<String>,
    window: usize,
    json_output: bool,
) -> Result<()> {
    let db_path = default_db_path();

    // Quick sync before searching
    if db_path.exists() {
        let indexer = Indexer::open_default()?;
        let _ = indexer.sync_recent(10).await;
    }

    let mut before_entries: Vec<LogEntryOutput> = Vec::new();
    let mut after_entries: Vec<LogEntryOutput> = Vec::new();
    let mut center_ts: Option<i64> = None;

    if db_path.exists() {
        let db = Database::open(&db_path)?;

        // Determine the center timestamp
        if let Some(ref ts_str) = at {
            center_ts = parse_timestamp(ts_str);
            if center_ts.is_none() {
                anyhow::bail!("Invalid timestamp format: {}. Use ISO-8601, epoch, or relative time (e.g., -5m)", ts_str);
            }
        } else if session_id.is_some() {
            // For session-based context, find the time span for this session
            let sid = session_id.as_ref().unwrap();
            let (min_ts, max_ts): (Option<i64>, Option<i64>) = db.conn().query_row(
                "SELECT MIN(le.timestamp_utc), MAX(le.timestamp_utc)
                 FROM log_entries le
                 INNER JOIN agent_tool_outputs ato ON ato.log_entry_id = le.id
                 WHERE ato.session_id = ?1",
                params![sid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if let (Some(start), Some(end)) = (min_ts, max_ts) {
                center_ts = Some(start + ((end - start) / 2));
            }
        } else {
            // Default to current time if nothing specified
            center_ts = Some(Utc::now().timestamp());
        }

        if let Some(ts) = center_ts {
            // Get entries before the timestamp
            before_entries = if let Some(ref sid) = session_id {
                let sql_before = "SELECT le.id, le.timestamp_utc, le.level, le.service, le.message
                                 FROM log_entries le
                                 INNER JOIN agent_tool_outputs ato ON ato.log_entry_id = le.id
                                 WHERE ato.session_id = ?1 AND le.timestamp_utc < ?2
                                 ORDER BY le.timestamp_utc DESC
                                 LIMIT ?3";
                let mut stmt = db.conn().prepare(sql_before)?;
                let rows: Vec<_> = stmt.query_map(params![sid, ts, window as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
                rows.into_iter()
                    .map(|(id, ts, level, service, message)| LogEntryOutput {
                        id,
                        timestamp: Utc
                            .timestamp_opt(ts, 0)
                            .single()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| ts.to_string()),
                        level,
                        service,
                        message,
                    })
                    .collect()
            } else {
                let sql_before = "SELECT id, timestamp_utc, level, service, message
                                 FROM log_entries
                                 WHERE timestamp_utc < ?1
                                 ORDER BY timestamp_utc DESC
                                 LIMIT ?2";
                let mut stmt = db.conn().prepare(sql_before)?;
                let rows: Vec<_> = stmt.query_map(params![ts, window as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
                rows.into_iter()
                    .map(|(id, ts, level, service, message)| LogEntryOutput {
                        id,
                        timestamp: Utc
                            .timestamp_opt(ts, 0)
                            .single()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| ts.to_string()),
                        level,
                        service,
                        message,
                    })
                    .collect()
            };

            // Reverse so oldest is first
            before_entries.reverse();

            // Get entries after the timestamp
            after_entries = if let Some(ref sid) = session_id {
                let sql_after = "SELECT le.id, le.timestamp_utc, le.level, le.service, le.message
                                FROM log_entries le
                                INNER JOIN agent_tool_outputs ato ON ato.log_entry_id = le.id
                                WHERE ato.session_id = ?1 AND le.timestamp_utc >= ?2
                                ORDER BY le.timestamp_utc ASC
                                LIMIT ?3";
                let mut stmt = db.conn().prepare(sql_after)?;
                let rows: Vec<_> = stmt.query_map(params![sid, ts, window as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
                rows.into_iter()
                    .map(|(id, ts, level, service, message)| LogEntryOutput {
                        id,
                        timestamp: Utc
                            .timestamp_opt(ts, 0)
                            .single()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| ts.to_string()),
                        level,
                        service,
                        message,
                    })
                    .collect()
            } else {
                let sql_after = "SELECT id, timestamp_utc, level, service, message
                                FROM log_entries
                                WHERE timestamp_utc >= ?1
                                ORDER BY timestamp_utc ASC
                                LIMIT ?2";
                let mut stmt = db.conn().prepare(sql_after)?;
                let rows: Vec<_> = stmt.query_map(params![ts, window as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
                rows.into_iter()
                    .map(|(id, ts, level, service, message)| LogEntryOutput {
                        id,
                        timestamp: Utc
                            .timestamp_opt(ts, 0)
                            .single()
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| ts.to_string()),
                        level,
                        service,
                        message,
                    })
                    .collect()
            };
        }
    }

    let center_timestamp_str = center_ts.and_then(|ts| {
        Utc.timestamp_opt(ts, 0)
            .single()
            .map(|dt| dt.to_rfc3339())
    });

    let result = ContextResult {
        success: true,
        center_timestamp: center_timestamp_str.clone(),
        session_id: session_id.clone(),
        window_size: window,
        before: before_entries.clone(),
        after: after_entries.clone(),
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Context Search");
        println!("==============");
        if let Some(ref ts) = center_timestamp_str {
            println!("Center: {}", ts);
        }
        if let Some(ref sid) = session_id {
            println!("Session: {}", sid);
        }
        println!("Window: {} entries before/after", window);
        println!();

        if before_entries.is_empty() && after_entries.is_empty() {
            println!("No log entries found.");
            if !db_path.exists() {
                println!();
                println!("Tip: Database not found. Run 'sls discover' to set up log sources.");
            }
        } else {
            // Print before entries
            if !before_entries.is_empty() {
                println!("=== Before ({} entries) ===", before_entries.len());
                for entry in &before_entries {
                    print_entry(entry);
                }
                println!();
            }

            // Print separator for center point
            if let Some(ref ts) = center_timestamp_str {
                println!(">>> CENTER: {} <<<", ts);
            }
            println!();

            // Print after entries
            if !after_entries.is_empty() {
                println!("=== After ({} entries) ===", after_entries.len());
                for entry in &after_entries {
                    print_entry(entry);
                }
            }
        }
    }

    Ok(())
}

fn print_entry(entry: &LogEntryOutput) {
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
    println!("{} {} {}{}", entry.timestamp, level_str, service_str, entry.message);
}
