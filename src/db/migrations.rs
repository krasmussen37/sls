//! Database migrations for SLS

use anyhow::Result;
use rusqlite::Connection;

/// Run all migrations
pub fn run(conn: &Connection) -> Result<()> {
    // Create migrations table if it doesn't exist
    conn.execute(
        "CREATE TABLE IF NOT EXISTS migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        )",
        [],
    )?;
    
    // Run each migration
    run_migration(conn, "001_initial_schema", migrate_001_initial_schema)?;
    
    Ok(())
}

fn run_migration<F>(conn: &Connection, name: &str, migration: F) -> Result<()>
where
    F: FnOnce(&Connection) -> Result<()>,
{
    // Check if already applied
    let applied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM migrations WHERE name = ?)",
        [name],
        |row| row.get(0),
    )?;
    
    if !applied {
        migration(conn)?;
        conn.execute("INSERT INTO migrations (name) VALUES (?)", [name])?;
        tracing::info!("Applied migration: {}", name);
    }
    
    Ok(())
}

fn migrate_001_initial_schema(conn: &Connection) -> Result<()> {
    // Log sources table
    conn.execute_batch(
        "CREATE TABLE log_sources (
            id INTEGER PRIMARY KEY,
            source_type TEXT NOT NULL,
            source_path TEXT,
            last_position TEXT,
            active INTEGER DEFAULT 1,
            discovered INTEGER DEFAULT 0,
            confidence INTEGER,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );
        
        CREATE INDEX idx_sources_type ON log_sources(source_type);
        CREATE INDEX idx_sources_active ON log_sources(active);
        "
    )?;
    
    // Log entries table
    conn.execute_batch(
        "CREATE TABLE log_entries (
            id INTEGER PRIMARY KEY,
            source_id INTEGER REFERENCES log_sources(id),
            timestamp_utc INTEGER NOT NULL,
            level TEXT,
            service TEXT,
            hostname TEXT,
            message TEXT NOT NULL,
            raw_line TEXT,
            structured_data TEXT,
            fingerprint TEXT,
            indexed_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );
        
        CREATE INDEX idx_entries_timestamp ON log_entries(timestamp_utc);
        CREATE INDEX idx_entries_level ON log_entries(level);
        CREATE INDEX idx_entries_service ON log_entries(service);
        CREATE INDEX idx_entries_source ON log_entries(source_id);
        CREATE INDEX idx_entries_fingerprint ON log_entries(fingerprint);
        "
    )?;
    
    // Agent tool outputs table (for stderr from agent tools)
    conn.execute_batch(
        "CREATE TABLE agent_tool_outputs (
            id INTEGER PRIMARY KEY,
            log_entry_id INTEGER REFERENCES log_entries(id),
            agent_type TEXT,
            session_id TEXT,
            tool_name TEXT,
            exit_code INTEGER,
            stderr TEXT,
            workspace TEXT,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        );
        
        CREATE INDEX idx_agent_outputs_session ON agent_tool_outputs(session_id);
        CREATE INDEX idx_agent_outputs_tool ON agent_tool_outputs(tool_name);
        CREATE INDEX idx_agent_outputs_agent ON agent_tool_outputs(agent_type);
        "
    )?;
    
    Ok(())
}
