//! sls similar - Find similar log patterns
//!
//! Performs pattern-based matching to find log entries with similar content.
//! Uses keyword extraction and fuzzy matching rather than full semantic search.

use anyhow::Result;
use chrono::{TimeZone, Utc};
use serde::Serialize;

use crate::db::default_db_path;
use crate::indexer::Indexer;

#[derive(Serialize)]
struct SimilarResult {
    success: bool,
    query: String,
    keywords: Vec<String>,
    matches: Vec<SimilarMatch>,
}

#[derive(Serialize)]
struct SimilarMatch {
    id: i64,
    score: f32,
    timestamp: String,
    level: Option<String>,
    service: Option<String>,
    message: String,
}

/// Extract keywords from a query string for pattern matching
fn extract_keywords(query: &str) -> Vec<String> {
    // Common stop words to filter out
    let stop_words = [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "must", "shall", "can", "need", "dare",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
        "into", "through", "during", "before", "after", "above", "below",
        "between", "under", "again", "further", "then", "once", "here",
        "there", "when", "where", "why", "how", "all", "each", "few", "more",
        "most", "other", "some", "such", "no", "nor", "not", "only", "own",
        "same", "so", "than", "too", "very", "just", "and", "but", "or",
    ];

    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| {
            !word.is_empty()
                && word.len() >= 3
                && !stop_words.contains(&word.as_ref())
        })
        .map(|s| s.to_string())
        .collect()
}

/// Calculate a simple similarity score based on keyword matches
fn calculate_score(message: &str, keywords: &[String]) -> f32 {
    if keywords.is_empty() {
        return 0.0;
    }

    let message_lower = message.to_lowercase();
    let matches: usize = keywords
        .iter()
        .filter(|kw| message_lower.contains(kw.as_str()))
        .count();

    (matches as f32 / keywords.len() as f32) * 100.0
}

pub async fn run(query: String, limit: usize, json_output: bool) -> Result<()> {
    let db_path = default_db_path();

    // Extract keywords from query
    let keywords = extract_keywords(&query);

    if keywords.is_empty() {
        let result = SimilarResult {
            success: true,
            query: query.clone(),
            keywords: vec![],
            matches: vec![],
        };

        if json_output {
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            println!("Finding entries similar to: \"{}\"", query);
            println!("  No meaningful keywords extracted from query.");
        }
        return Ok(());
    }

    // Quick sync before searching
    if db_path.exists() {
        let indexer = Indexer::open_default()?;
        let _ = indexer.sync_recent(10).await;
    }

    // Build SQL query with OR conditions for each keyword
    let mut matches: Vec<SimilarMatch> = Vec::new();

    if db_path.exists() {
        let db = crate::db::Database::open(&db_path)?;

        // Build query for any keyword match
        let conditions: Vec<String> = keywords
            .iter()
            .map(|_| "message LIKE ?".to_string())
            .collect();

        let sql = format!(
            "SELECT id, timestamp_utc, level, service, message
             FROM log_entries
             WHERE {}
             ORDER BY timestamp_utc DESC
             LIMIT ?",
            conditions.join(" OR ")
        );

        // Build params with LIKE patterns
        let patterns: Vec<String> = keywords
            .iter()
            .map(|kw| format!("%{}%", kw))
            .collect();

        let mut stmt = db.conn().prepare(&sql)?;

        // Execute query - we need to handle the params dynamically
        let fetch_limit = (limit * 3) as i64; // Fetch more to allow for scoring/sorting
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = patterns
            .iter()
            .map(|p| Box::new(p.clone()) as Box<dyn rusqlite::ToSql>)
            .collect();
        params_vec.push(Box::new(fetch_limit));

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        // Score and collect results
        let mut scored_matches: Vec<(f32, SimilarMatch)> = rows
            .filter_map(|r| r.ok())
            .map(|(id, ts, level, service, message)| {
                let score = calculate_score(&message, &keywords);
                let timestamp = Utc
                    .timestamp_opt(ts, 0)
                    .single()
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|| ts.to_string());

                (score, SimilarMatch {
                    id,
                    score,
                    timestamp,
                    level,
                    service,
                    message,
                })
            })
            .collect();

        // Sort by score (highest first) and take top matches
        scored_matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        matches = scored_matches
            .into_iter()
            .take(limit)
            .map(|(_, m)| m)
            .collect();
    }

    let result = SimilarResult {
        success: true,
        query: query.clone(),
        keywords: keywords.clone(),
        matches,
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Finding entries similar to: \"{}\"", query);
        println!("  Keywords: {}", keywords.join(", "));
        println!("  Limit: {}", limit);
        println!();

        if result.matches.is_empty() {
            println!("No similar entries found.");
            if !db_path.exists() {
                println!();
                println!("Tip: Database not found. Run 'sls discover' to set up log sources.");
            }
        } else {
            println!("Found {} similar entries:", result.matches.len());
            println!();
            for m in &result.matches {
                let level_str = m
                    .level
                    .as_ref()
                    .map(|l| format!("[{}]", l))
                    .unwrap_or_default();
                let service_str = m
                    .service
                    .as_ref()
                    .map(|s| format!("{}: ", s))
                    .unwrap_or_default();
                println!(
                    "{:.0}% {} {} {}{}",
                    m.score, m.timestamp, level_str, service_str, m.message
                );
            }
        }
    }

    Ok(())
}
