//! sls index - Index logs from configured sources

use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
struct IndexResult {
    success: bool,
    entries_indexed: usize,
    sources_processed: usize,
    message: String,
}

pub async fn run(full: bool, watch: bool, json_output: bool) -> Result<()> {
    let result = IndexResult {
        success: true,
        entries_indexed: 0,
        sources_processed: 0,
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

    Ok(())
}
