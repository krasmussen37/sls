//! sls status - Show indexing status and health

use anyhow::Result;
use chrono::{TimeZone, Utc};
use serde::Serialize;

use crate::db::default_db_path;
use crate::indexer::Indexer;

#[derive(Serialize)]
struct StatusOutput {
    healthy: bool,
    indexed_entries: usize,
    sources_count: usize,
    last_indexed: Option<String>,
    background_running: bool,
    database_path: String,
}

pub async fn run(json_output: bool) -> Result<()> {
    let db_path = default_db_path();
    let db_exists = db_path.exists();

    // Try to get stats from the indexer
    let (indexed_entries, sources_count, last_indexed, background_running) = if db_exists {
        match Indexer::open_default() {
            Ok(indexer) => {
                let stats = indexer.stats().await?;
                let last_ts = stats.last_indexed_timestamp.and_then(|ts| {
                    Utc.timestamp_opt(ts, 0)
                        .single()
                        .map(|dt| dt.to_rfc3339())
                });
                (
                    stats.total_entries,
                    stats.total_sources,
                    last_ts,
                    stats.background_running,
                )
            }
            Err(_) => (0, 0, None, false),
        }
    } else {
        (0, 0, None, false)
    };

    let healthy = db_exists && sources_count > 0;

    let status = StatusOutput {
        healthy,
        indexed_entries,
        sources_count,
        last_indexed,
        background_running,
        database_path: db_path.display().to_string(),
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("SLS Status");
        println!("==========");
        println!(
            "Healthy: {}",
            if status.healthy {
                "Yes"
            } else if !db_exists {
                "No (database not found)"
            } else if sources_count == 0 {
                "No (no sources configured)"
            } else {
                "No"
            }
        );
        println!("Indexed entries: {}", status.indexed_entries);
        println!("Sources configured: {}", status.sources_count);
        println!(
            "Last indexed: {}",
            status.last_indexed.as_deref().unwrap_or("never")
        );
        println!(
            "Background indexer: {}",
            if status.background_running {
                "running"
            } else {
                "idle"
            }
        );
        println!("Database: {}", status.database_path);

        if !db_exists {
            println!();
            println!("Run 'sls discover' to set up log sources.");
        } else if sources_count == 0 {
            println!();
            println!("No sources configured. Run 'sls discover' to find log sources.");
        }
    }
    Ok(())
}
