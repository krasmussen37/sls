//! Database module for SLS
//!
//! Uses SQLite for metadata storage and Tantivy for full-text search.

pub mod migrations;
pub mod models;

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Database manager for SLS
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create the database at the given path
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }
    
    /// Open an in-memory database (for testing)
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }
    
    /// Run migrations
    fn migrate(&self) -> Result<()> {
        migrations::run(&self.conn)
    }
    
    /// Get a reference to the connection
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Get the default database path
pub fn default_db_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".sls")
        .join("sls.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn test_open_in_memory() {
        let db = Database::open_in_memory().expect("Failed to open in-memory database");
        assert!(db.conn().is_autocommit());
    }

    #[test]
    fn test_migrations_run() {
        let db = Database::open_in_memory().expect("Failed to open database");

        // Check that migrations table exists
        let count: i64 = db.conn().query_row(
            "SELECT COUNT(*) FROM migrations",
            [],
            |row| row.get(0)
        ).expect("Failed to query migrations");

        assert!(count > 0, "Migrations should have been applied");
    }

    #[test]
    fn test_log_sources_table_exists() {
        let db = Database::open_in_memory().expect("Failed to open database");

        // Insert a test log source
        db.conn().execute(
            "INSERT INTO log_sources (source_type, source_path) VALUES (?, ?)",
            params!["syslog", "/var/log/syslog"],
        ).expect("Failed to insert log source");

        let source_type: String = db.conn().query_row(
            "SELECT source_type FROM log_sources WHERE id = 1",
            [],
            |row| row.get(0)
        ).expect("Failed to query log source");

        assert_eq!(source_type, "syslog");
    }

    #[test]
    fn test_log_entries_table_exists() {
        let db = Database::open_in_memory().expect("Failed to open database");

        // Insert a test log entry
        db.conn().execute(
            "INSERT INTO log_entries (timestamp_utc, message) VALUES (?, ?)",
            params![1704067200i64, "Test log message"],
        ).expect("Failed to insert log entry");

        let message: String = db.conn().query_row(
            "SELECT message FROM log_entries WHERE id = 1",
            [],
            |row| row.get(0)
        ).expect("Failed to query log entry");

        assert_eq!(message, "Test log message");
    }

    #[test]
    fn test_agent_tool_outputs_table_exists() {
        let db = Database::open_in_memory().expect("Failed to open database");

        // First insert a log entry (required for foreign key)
        db.conn().execute(
            "INSERT INTO log_entries (timestamp_utc, message) VALUES (?, ?)",
            params![1704067200i64, "Test log"],
        ).expect("Failed to insert log entry");

        // Insert agent tool output
        db.conn().execute(
            "INSERT INTO agent_tool_outputs (log_entry_id, agent_type, session_id, tool_name) VALUES (?, ?, ?, ?)",
            params![1i64, "claude-code", "session-123", "Bash"],
        ).expect("Failed to insert agent tool output");

        let tool_name: String = db.conn().query_row(
            "SELECT tool_name FROM agent_tool_outputs WHERE id = 1",
            [],
            |row| row.get(0)
        ).expect("Failed to query agent tool output");

        assert_eq!(tool_name, "Bash");
    }

    #[test]
    fn test_default_db_path() {
        let path = default_db_path();
        assert!(path.to_string_lossy().contains(".sls"));
        assert!(path.to_string_lossy().ends_with("sls.db"));
    }
}
