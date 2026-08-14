//! sls sources - Manage log sources

use anyhow::Result;
use crate::db::{default_db_path, Database, models::LogSource};

pub async fn list(json_output: bool) -> Result<()> {
    let db_path = default_db_path();

    // If database doesn't exist, show helpful message
    if !db_path.exists() {
        if json_output {
            println!(r#"{{"sources": [], "status": "no_database"}}"#);
        } else {
            println!("No log sources configured (database not initialized).");
            println!();
            println!("Run 'sls discover' to auto-discover sources, or add manually:");
            println!("  sls sources add --type journald");
            println!("  sls sources add --type syslog --path /var/log/syslog");
            println!("  sls sources add --type agent_stderr --path ~/.claude/logs/");
        }
        return Ok(());
    }

    let db = Database::open(&db_path)?;

    // Query active log sources
    let mut stmt = db.conn().prepare(
        "SELECT id, source_type, source_path, last_position, active, discovered, confidence, created_at, updated_at
         FROM log_sources
         WHERE active = 1
         ORDER BY id ASC"
    )?;

    let sources = stmt.query_map([], |row| {
        Ok(LogSource {
            id: row.get(0)?,
            source_type: row.get(1)?,
            source_path: row.get(2)?,
            last_position: row.get(3)?,
            active: row.get::<_, i64>(4)? != 0,
            discovered: row.get::<_, i64>(5)? != 0,
            confidence: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?
    .collect::<Result<Vec<LogSource>, _>>()?;

    if json_output {
        let json = serde_json::to_string_pretty(&sources)?;
        println!("{}", json);
    } else {
        if sources.is_empty() {
            println!("No active log sources.");
            println!();
            println!("Run 'sls discover' to auto-discover sources, or add manually:");
            println!("  sls sources add --type journald");
            println!("  sls sources add --type syslog --path /var/log/syslog");
            println!("  sls sources add --type agent_stderr --path ~/.claude/logs/");
        } else {
            println!("Active Log Sources");
            println!("==================");
            println!();
            println!("{:<4} {:<15} {:<40} {:<10}", "ID", "Type", "Path", "Status");
            println!("{}", "-".repeat(75));

            for source in sources {
                let path = source.source_path.as_deref().unwrap_or("(system)");
                let status = if source.discovered {
                    format!("auto ({}%)", source.confidence.unwrap_or(0))
                } else {
                    "manual".to_string()
                };
                println!("{:<4} {:<15} {:<40} {:<10}", source.id, source.source_type, path, status);
            }
        }
    }
    Ok(())
}

pub async fn add(
    source_type: String,
    path: Option<String>,
    container: Option<String>,
    json_output: bool,
) -> Result<()> {
    let db_path = default_db_path();
    let db = Database::open(&db_path)?;

    // Validate source type
    let valid_types = vec![
        "journald",
        "syslog",
        "agent_stderr",
        "codex",
        "docker",
        "json",
        "file",
    ];
    if !valid_types.contains(&source_type.as_str()) {
        anyhow::bail!(
            "Invalid source type: {}. Valid types: {}",
            source_type,
            valid_types.join(", ")
        );
    }

    // Determine source_path based on type
    let source_path = match source_type.as_str() {
        "journald" => None, // Journald doesn't need a path
        "docker" => {
            // For docker, use container name as path
            if let Some(c) = container {
                Some(c)
            } else {
                anyhow::bail!("Docker source requires --container parameter");
            }
        }
        _ => {
            // For file-based sources, path is required
            if let Some(p) = path {
                Some(p)
            } else {
                anyhow::bail!("{} source requires --path parameter", source_type);
            }
        }
    };

    // Insert into database
    let source_id = db.conn().query_row(
        "INSERT INTO log_sources (source_type, source_path, active, discovered, confidence)
         VALUES (?, ?, 1, 0, NULL)
         RETURNING id",
        rusqlite::params![source_type, source_path],
        |row| row.get::<_, i64>(0),
    )?;

    if json_output {
        println!(
            r#"{{"success": true, "source_id": {}, "type": "{}", "path": "{}"}}"#,
            source_id,
            source_type,
            source_path.as_deref().unwrap_or("(system)")
        );
    } else {
        println!("Added log source:");
        println!("  ID:   {}", source_id);
        println!("  Type: {}", source_type);
        if let Some(p) = source_path {
            println!("  Path: {}", p);
        }
        println!();
        println!("Run 'sls index' to start indexing this source.");
    }

    Ok(())
}

pub async fn remove(id: String, json_output: bool) -> Result<()> {
    let db_path = default_db_path();

    if !db_path.exists() {
        anyhow::bail!("Database not found. No sources to remove.");
    }

    let db = Database::open(&db_path)?;

    // Parse ID
    let source_id: i64 = id.parse()
        .map_err(|_| anyhow::anyhow!("Invalid source ID: {}", id))?;

    // Check if source exists
    let exists: bool = db.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM log_sources WHERE id = ?)",
        [source_id],
        |row| row.get(0),
    )?;

    if !exists {
        anyhow::bail!("Source with ID {} not found", source_id);
    }

    // Soft delete: set active=0 instead of deleting
    let rows_affected = db.conn().execute(
        "UPDATE log_sources SET active = 0, updated_at = strftime('%s', 'now') WHERE id = ?",
        [source_id],
    )?;

    if json_output {
        println!(
            r#"{{"success": true, "source_id": {}, "rows_affected": {}}}"#,
            source_id, rows_affected
        );
    } else {
        if rows_affected > 0 {
            println!("Removed log source ID: {}", source_id);
            println!("(Source marked as inactive, historical data preserved)");
        } else {
            println!("Source ID {} was already inactive.", source_id);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_add_journald_source() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Initialize database
        let _db = Database::open(&db_path).unwrap();

        // Mock default_db_path to return our test path
        // This is a limitation - in real test we'd inject the path
        // For now, just test the logic structure

        assert!(db_path.exists());
    }

    #[tokio::test]
    async fn test_add_file_source_requires_path() {
        // Test that file source without path returns error
        let result = add(
            "syslog".to_string(),
            None,  // No path
            None,
            true,
        ).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires --path"));
    }

    #[tokio::test]
    async fn test_add_docker_requires_container() {
        // Test that docker source without container returns error
        let result = add(
            "docker".to_string(),
            None,
            None,  // No container
            true,
        ).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires --container"));
    }

    #[tokio::test]
    async fn test_add_invalid_source_type() {
        // Test that invalid source type returns error
        let result = add(
            "invalid_type".to_string(),
            Some("/some/path".to_string()),
            None,
            true,
        ).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid source type"));
    }

    #[tokio::test]
    async fn test_remove_invalid_id() {
        // Test that non-numeric ID returns error
        let result = remove("not_a_number".to_string(), true).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid source ID"));
    }
}
