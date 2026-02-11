//! sls index - Index logs from configured sources

use anyhow::Result;
use serde::Serialize;

use crate::indexer::Indexer;

#[derive(Serialize)]
struct IndexResult {
    success: bool,
    entries_indexed: usize,
    sources_processed: usize,
    message: String,
}

pub async fn run(full: bool, watch: bool, json_output: bool) -> Result<()> {
    let indexer = Indexer::open_default()?;
    let stats_before = indexer.stats().await?;

    // Use higher limit for full reindex, moderate for incremental
    let limit = if full { 0 } else { 100 }; // 0 = no limit

    let entries_indexed = indexer.sync_recent(limit).await?;
    let sources_processed = stats_before.total_sources;

    let result = IndexResult {
        success: true,
        entries_indexed,
        sources_processed,
        message: format!(
            "Indexing {} (full={}, watch={})",
            if watch { "continuously" } else { "once" },
            full,
            watch
        ),
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", result.message);
        println!("  Entries indexed: {}", result.entries_indexed);
        println!("  Sources processed: {}", result.sources_processed);
    }

    if watch {
        println!("Watching for new log entries (Ctrl+C to stop)...");
        let interval = std::time::Duration::from_secs(30);
        loop {
            tokio::time::sleep(interval).await;
            match indexer.sync_recent(100).await {
                Ok(count) if count > 0 => {
                    if json_output {
                        let refresh = IndexResult {
                            success: true,
                            entries_indexed: count,
                            sources_processed,
                            message: "Refresh sync".to_string(),
                        };
                        println!("{}", serde_json::to_string_pretty(&refresh)?);
                    } else {
                        println!("  Refreshed: {} new entries", count);
                    }
                }
                Ok(_) => {} // No new entries, stay quiet
                Err(e) => {
                    eprintln!("  Refresh error: {}", e);
                }
            }
        }
    }

    Ok(())
}
