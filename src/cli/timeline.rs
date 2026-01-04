//! sls timeline - Show activity timeline
//!
//! Aggregates log entries by time buckets to show activity patterns.

use anyhow::Result;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::Serialize;

use crate::db::{default_db_path, Database};
use crate::indexer::parse_relative_time;

#[derive(Serialize)]
struct TimelineOutput {
    success: bool,
    period: String,
    group_by: String,
    total_entries: usize,
    buckets: Vec<TimelineBucket>,
}

#[derive(Serialize, Clone)]
struct TimelineBucket {
    timestamp: String,
    label: String,
    total: usize,
    errors: usize,
    warnings: usize,
    info: usize,
    debug: usize,
}

pub async fn run(
    today: bool,
    since: Option<String>,
    group_by: String,
    json_output: bool,
) -> Result<()> {
    // Determine time range
    let (period_str, since_ts) = if today {
        let start_of_today = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        ("today".to_string(), start_of_today)
    } else {
        let since_str = since.clone().unwrap_or_else(|| "24h".to_string());
        let ts = parse_relative_time(&since_str).unwrap_or_else(|| Utc::now().timestamp() - 86400);
        (since_str, ts)
    };

    // Get bucket duration based on group_by
    let bucket_secs = match group_by.as_str() {
        "minute" => 60,
        "hour" => 3600,
        "day" => 86400,
        _ => 3600, // Default to hour
    };

    // Query the database
    let db_path = default_db_path();
    let buckets = if db_path.exists() {
        let db = Database::open(&db_path)?;
        query_timeline(&db, since_ts, bucket_secs)?
    } else {
        Vec::new()
    };

    let total_entries: usize = buckets.iter().map(|b| b.total).sum();

    let output = TimelineOutput {
        success: true,
        period: period_str.clone(),
        group_by: group_by.clone(),
        total_entries,
        buckets: buckets.clone(),
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Activity Timeline");
        println!("=================");
        println!("Period: {}", period_str);
        println!("Grouped by: {}", group_by);
        println!("Total entries: {}", total_entries);
        println!();

        if buckets.is_empty() {
            println!("No activity data available.");
            println!();
            println!("Tip: Run 'sls discover --auto' to set up log sources,");
            println!("     then 'sls index' to populate the timeline.");
        } else {
            // Print header
            println!(
                "{:20} {:>6} {:>6} {:>6} {:>6} {:>6}",
                "Time", "Total", "Error", "Warn", "Info", "Debug"
            );
            println!("{}", "-".repeat(60));

            // Print each bucket with a simple bar chart
            for bucket in &buckets {
                let bar = generate_bar(bucket.total, buckets.iter().map(|b| b.total).max().unwrap_or(1));
                println!(
                    "{:20} {:>6} {:>6} {:>6} {:>6} {:>6}  {}",
                    bucket.label,
                    bucket.total,
                    bucket.errors,
                    bucket.warnings,
                    bucket.info,
                    bucket.debug,
                    bar
                );
            }
        }
    }

    Ok(())
}

/// Query the database for timeline buckets
fn query_timeline(db: &Database, since_ts: i64, bucket_secs: i64) -> Result<Vec<TimelineBucket>> {
    let mut stmt = db.conn().prepare(
        "SELECT
            (timestamp_utc / ?) * ? as bucket_ts,
            COUNT(*) as total,
            SUM(CASE WHEN UPPER(level) = 'ERROR' OR UPPER(level) = 'FATAL' OR UPPER(level) = 'CRITICAL' THEN 1 ELSE 0 END) as errors,
            SUM(CASE WHEN UPPER(level) = 'WARN' OR UPPER(level) = 'WARNING' THEN 1 ELSE 0 END) as warnings,
            SUM(CASE WHEN UPPER(level) = 'INFO' THEN 1 ELSE 0 END) as info,
            SUM(CASE WHEN UPPER(level) = 'DEBUG' OR UPPER(level) = 'TRACE' THEN 1 ELSE 0 END) as debug
         FROM log_entries
         WHERE timestamp_utc >= ?
         GROUP BY bucket_ts
         ORDER BY bucket_ts"
    )?;

    let buckets = stmt
        .query_map(rusqlite::params![bucket_secs, bucket_secs, since_ts], |row| {
            let bucket_ts: i64 = row.get(0)?;
            let total: usize = row.get::<_, i64>(1)? as usize;
            let errors: usize = row.get::<_, i64>(2)? as usize;
            let warnings: usize = row.get::<_, i64>(3)? as usize;
            let info: usize = row.get::<_, i64>(4)? as usize;
            let debug: usize = row.get::<_, i64>(5)? as usize;

            let dt = Utc.timestamp_opt(bucket_ts, 0).single().unwrap_or_else(Utc::now);
            let label = format_bucket_label(&dt, bucket_secs);

            Ok(TimelineBucket {
                timestamp: dt.to_rfc3339(),
                label,
                total,
                errors,
                warnings,
                info,
                debug,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(buckets)
}

/// Format a bucket label based on the bucket size
fn format_bucket_label(dt: &DateTime<Utc>, bucket_secs: i64) -> String {
    match bucket_secs {
        60 => dt.format("%Y-%m-%d %H:%M").to_string(),
        3600 => dt.format("%Y-%m-%d %H:00").to_string(),
        86400 => dt.format("%Y-%m-%d").to_string(),
        _ => dt.format("%Y-%m-%d %H:%M").to_string(),
    }
}

/// Generate a simple ASCII bar for the histogram
fn generate_bar(value: usize, max_value: usize) -> String {
    if max_value == 0 {
        return String::new();
    }

    let width = 20;
    let filled = (value * width) / max_value;
    let filled = filled.max(if value > 0 { 1 } else { 0 }); // At least 1 char if non-zero

    format!("{}", "█".repeat(filled))
}
