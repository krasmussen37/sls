//! On-demand indexing module for SLS
//!
//! Implements the query-triggered reverse-chronological indexing pattern:
//! 1. When a query comes in, sync last N events from each source (instant)
//! 2. Return results immediately
//! 3. Kick off background builder for historical data
//! 4. Periodic refresh every 30 minutes to prevent stale gaps

use anyhow::Result;
use chrono::Utc;
use rusqlite::params;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use crate::connectors::{
    agent_stderr::AgentStderrConnector, journald::JournaldConnector, syslog::SyslogConnector,
    Connector, LogEntry,
};
use crate::db::models::StoredLogEntry;
use crate::db::Database;

/// Configuration for the indexer
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    /// Number of recent events to sync on query (default: 10 per source)
    pub quick_sync_limit: usize,
    /// Number of events to sync on periodic refresh (default: 100 per source)
    pub refresh_sync_limit: usize,
    /// Refresh interval in seconds (default: 1800 = 30 minutes)
    pub refresh_interval_secs: u64,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            quick_sync_limit: 10,
            refresh_sync_limit: 100,
            refresh_interval_secs: 1800, // 30 minutes
        }
    }
}

/// The main indexer that implements on-demand indexing
pub struct Indexer {
    db: Arc<Mutex<Database>>,
    connectors: Arc<Vec<SourceConnector>>,
    config: IndexerConfig,
    background_running: Arc<AtomicBool>,
}

struct SourceConnector {
    source_id: Option<i64>,
    connector: Arc<dyn Connector + Send + Sync>,
}

// Safety: SourceConnector is Send + Sync because Arc<dyn Connector + Send + Sync> is
unsafe impl Send for SourceConnector {}
unsafe impl Sync for SourceConnector {}

impl Indexer {
    /// Create a new indexer with the given database and connectors
    pub fn new(db: Database, connectors: Vec<SourceConnector>, config: IndexerConfig) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            connectors: Arc::new(connectors),
            config,
            background_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Open or create an indexer at the default location
    pub fn open_default() -> Result<Self> {
        let db_path = crate::db::default_db_path();
        Self::open(&db_path)
    }

    /// Open or create an indexer at the given path
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Database::open(db_path)?;
        let connectors = Self::load_connectors(&db)?;
        Ok(Self::new(db, connectors, IndexerConfig::default()))
    }

    /// Add a connector to this indexer
    /// Note: This should only be called during setup before any async operations
    pub fn add_connector(&mut self, connector: Arc<dyn Connector + Send + Sync>) {
        if let Some(connectors) = Arc::get_mut(&mut self.connectors) {
            connectors.push(SourceConnector {
                source_id: None,
                connector,
            });
        } else {
            tracing::warn!("Cannot add connector: indexer is already in use");
        }
    }

    /// Perform a quick sync of recent events from all sources
    /// This is called before every query to ensure fresh data
    pub async fn sync_recent(&self, limit: usize) -> Result<usize> {
        let mut total_indexed = 0;

        for connector in self.connectors.iter() {
            match connector.connector.read_entries(None) {
                Ok(entries) => {
                    let entries_to_index = Self::prepare_entries(entries, connector.source_id, limit);
                    let count = Self::store_entries(&self.db, &entries_to_index)?;
                    total_indexed += count;
                    tracing::debug!(
                        "Quick synced {} entries from {}",
                        count,
                        connector.connector.source_type()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to read from {}: {}",
                        connector.connector.source_type(),
                        e
                    );
                }
            }
        }

        Ok(total_indexed)
    }

    /// Store log entries in the database
    fn store_entries(db: &Mutex<Database>, entries: &[LogEntry]) -> Result<usize> {
        let db = db.lock().unwrap();
        let mut count = 0;

        for entry in entries {
            // Check for duplicates by fingerprint if available
            if let Some(ref fp) = entry.fingerprint {
                let exists: bool = db.conn().query_row(
                    "SELECT EXISTS(SELECT 1 FROM log_entries WHERE fingerprint = ?)",
                    [fp],
                    |row| row.get(0),
                )?;
                if exists {
                    continue;
                }
            }

            db.conn().execute(
                "INSERT INTO log_entries (
                    source_id, timestamp_utc, level, service, hostname, 
                    message, raw_line, structured_data, fingerprint
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    entry.source_id,
                    entry.timestamp.timestamp(),
                    entry.level,
                    entry.service,
                    entry.hostname,
                    entry.message,
                    entry.raw_line,
                    entry.structured_data,
                    entry.fingerprint,
                ],
            )?;
            count += 1;
        }

        Ok(count)
    }

    /// Search for log entries matching the query
    /// This implements the on-demand pattern:
    /// 1. Quick sync recent events
    /// 2. Search immediately
    /// 3. Spawn background indexer
    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResult> {
        // 1. Quick sync recent events before searching
        let synced = self.sync_recent(self.config.quick_sync_limit).await?;
        tracing::debug!("Quick synced {} entries before search", synced);

        // 2. Perform the search
        let mut results = self.execute_search(request).await?;
        results.synced_before_search = synced > 0;

        // 3. Spawn background indexer if not already running
        self.spawn_background_indexer();

        Ok(results)
    }

    /// Execute the actual search query against the database
    async fn execute_search(&self, request: &SearchRequest) -> Result<SearchResult> {
        let db = self.db.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, source_id, timestamp_utc, level, service, hostname, 
                    message, raw_line, structured_data, fingerprint, indexed_at
             FROM log_entries WHERE 1=1",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        // Add query filter (simple LIKE for now, could use FTS later)
        if !request.query.is_empty() {
            sql.push_str(" AND message LIKE ?");
            params_vec.push(Box::new(format!("%{}%", request.query)));
        }

        // Add level filter
        if let Some(ref level) = request.level {
            sql.push_str(" AND level = ?");
            params_vec.push(Box::new(level.clone()));
        }

        // Add service filter
        if let Some(ref service) = request.service {
            sql.push_str(" AND service = ?");
            params_vec.push(Box::new(service.clone()));
        }

        // Add time filter
        if let Some(since) = request.since_timestamp {
            sql.push_str(" AND timestamp_utc >= ?");
            params_vec.push(Box::new(since));
        }

        // Order by timestamp descending (most recent first)
        sql.push_str(" ORDER BY timestamp_utc DESC LIMIT ?");
        params_vec.push(Box::new(request.limit as i64));

        // Execute query
        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = db.conn().prepare(&sql)?;
        let entries = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(StoredLogEntry {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    timestamp_utc: row.get(2)?,
                    level: row.get(3)?,
                    service: row.get(4)?,
                    hostname: row.get(5)?,
                    message: row.get(6)?,
                    raw_line: row.get(7)?,
                    structured_data: row.get(8)?,
                    fingerprint: row.get(9)?,
                    indexed_at: row.get(10)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(SearchResult {
            entries,
            total_matches: 0, // TODO: Add count query
            synced_before_search: false, // Set by caller
        })
    }

    /// Spawn a background indexer task to continue building the index
    pub fn spawn_background_indexer(&self) {
        // Only run one background indexer at a time
        if self
            .background_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Background indexer already running");
            return;
        }

        let db = Arc::clone(&self.db);
        let connectors = Arc::clone(&self.connectors);
        let refresh_limit = self.config.refresh_sync_limit;
        let running = Arc::clone(&self.background_running);

        tokio::spawn(async move {
            tracing::info!("Background indexer started");

            let mut total_indexed = 0;
            for source in connectors.iter() {
                match source.connector.read_entries(None) {
                    Ok(entries) => {
                        let entries_to_index =
                            Self::prepare_entries(entries, source.source_id, refresh_limit);
                        match Self::store_entries(&db, &entries_to_index) {
                            Ok(count) => {
                                total_indexed += count;
                                tracing::debug!(
                                    "Background indexed {} entries from {}",
                                    count,
                                    source.connector.source_type()
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to store entries for {}: {}",
                                    source.connector.source_type(),
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to read from {}: {}",
                            source.connector.source_type(),
                            e
                        );
                    }
                }
            }

            running.store(false, Ordering::SeqCst);
            tracing::info!("Background indexer completed (indexed {})", total_indexed);
        });
    }

    /// Periodic refresh - called every 30 minutes to prevent stale gaps
    pub async fn periodic_refresh(&self) -> Result<usize> {
        self.sync_recent(self.config.refresh_sync_limit).await
    }

    /// Get indexing statistics
    pub async fn stats(&self) -> Result<IndexerStats> {
        let db = self.db.lock().unwrap();

        let total_entries: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM log_entries", [], |row| row.get(0))?;

        let total_sources: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM log_sources", [], |row| row.get(0))?;

        let last_indexed: Option<i64> = db.conn().query_row(
            "SELECT MAX(indexed_at) FROM log_entries",
            [],
            |row| row.get(0),
        )?;

        Ok(IndexerStats {
            total_entries: total_entries as usize,
            total_sources: total_sources as usize,
            last_indexed_timestamp: last_indexed,
            background_running: self.background_running.load(Ordering::SeqCst),
        })
    }
}

/// Search request parameters
#[derive(Debug, Clone, Default)]
pub struct SearchRequest {
    pub query: String,
    pub level: Option<String>,
    pub service: Option<String>,
    pub hostname: Option<String>,
    pub since_timestamp: Option<i64>,
    pub limit: usize,
}

impl SearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: 50,
            ..Default::default()
        }
    }

    pub fn with_level(mut self, level: impl Into<String>) -> Self {
        self.level = Some(level.into());
        self
    }

    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    pub fn with_hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = Some(hostname.into());
        self
    }

    pub fn with_since(mut self, timestamp: i64) -> Self {
        self.since_timestamp = Some(timestamp);
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// Search result
#[derive(Debug)]
pub struct SearchResult {
    pub entries: Vec<StoredLogEntry>,
    pub total_matches: usize,
    pub synced_before_search: bool,
}

/// Indexer statistics
#[derive(Debug, Clone)]
pub struct IndexerStats {
    pub total_entries: usize,
    pub total_sources: usize,
    pub last_indexed_timestamp: Option<i64>,
    pub background_running: bool,
}

/// Parse a relative time string like "1h", "30m", "1d" to a timestamp
    pub fn parse_relative_time(s: &str) -> Option<i64> {
    let now = Utc::now().timestamp();
    let s = s.trim();

    if s.is_empty() {
        return None;
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str.parse().ok()?;

    let seconds = match unit {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        "w" => num * 604800,
        _ => return None,
    };

    Some(now - seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_relative_time() {
        let now = Utc::now().timestamp();

        // Test various units
        let result = parse_relative_time("1h").unwrap();
        assert!(now - result >= 3599 && now - result <= 3601);

        let result = parse_relative_time("30m").unwrap();
        assert!(now - result >= 1799 && now - result <= 1801);

        let result = parse_relative_time("1d").unwrap();
        assert!(now - result >= 86399 && now - result <= 86401);

        // Invalid input
        assert!(parse_relative_time("").is_none());
        assert!(parse_relative_time("abc").is_none());
    }

    #[test]
    fn test_parse_relative_time_seconds_and_weeks() {
        let now = Utc::now().timestamp();

        // Test seconds
        let result = parse_relative_time("30s").unwrap();
        assert!(now - result >= 29 && now - result <= 31);

        // Test weeks
        let result = parse_relative_time("1w").unwrap();
        assert!(now - result >= 604799 && now - result <= 604801);
    }

    #[test]
    fn test_search_request_builder() {
        let req = SearchRequest::new("error")
            .with_level("ERROR")
            .with_service("myapp")
            .with_limit(100);

        assert_eq!(req.query, "error");
        assert_eq!(req.level, Some("ERROR".to_string()));
        assert_eq!(req.service, Some("myapp".to_string()));
        assert_eq!(req.limit, 100);
    }

    #[test]
    fn test_search_request_with_since() {
        let timestamp = 1704067200i64;
        let req = SearchRequest::new("test")
            .with_since(timestamp);

        assert_eq!(req.since_timestamp, Some(timestamp));
    }

    #[test]
    fn test_search_request_default() {
        let req = SearchRequest::default();

        assert_eq!(req.query, "");
        assert_eq!(req.level, None);
        assert_eq!(req.service, None);
        assert_eq!(req.since_timestamp, None);
        assert_eq!(req.limit, 0);
    }

    #[test]
    fn test_search_request_new_sets_default_limit() {
        let req = SearchRequest::new("test");

        assert_eq!(req.query, "test");
        assert_eq!(req.limit, 50);
    }

    #[test]
    fn test_indexer_config_default() {
        let config = IndexerConfig::default();

        assert_eq!(config.quick_sync_limit, 10);
        assert_eq!(config.refresh_sync_limit, 100);
        assert_eq!(config.refresh_interval_secs, 1800);
    }

    #[test]
    fn test_indexer_stats_clone() {
        let stats = IndexerStats {
            total_entries: 100,
            total_sources: 3,
            last_indexed_timestamp: Some(1704067200),
            background_running: false,
        };

        let cloned = stats.clone();
        assert_eq!(cloned.total_entries, 100);
        assert_eq!(cloned.total_sources, 3);
        assert_eq!(cloned.last_indexed_timestamp, Some(1704067200));
        assert!(!cloned.background_running);
    }
}

impl Indexer {
    fn load_connectors(db: &Database) -> Result<Vec<SourceConnector>> {
        let mut connectors = Vec::new();
        let mut stmt = db.conn().prepare(
            "SELECT id, source_type, source_path FROM log_sources WHERE active = 1",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        for row in rows {
            let (id, source_type, source_path) = row?;
            match source_type.as_str() {
                "agent_stderr" => {
                    if let Some(path) = source_path {
                        connectors.push(SourceConnector {
                            source_id: Some(id),
                            connector: Arc::new(AgentStderrConnector::new(path)),
                        });
                    }
                }
                "journald" => {
                    connectors.push(SourceConnector {
                        source_id: Some(id),
                        connector: Arc::new(JournaldConnector::new()),
                    });
                }
                "syslog" => {
                    let path = source_path.unwrap_or_else(|| "/var/log/syslog".to_string());
                    connectors.push(SourceConnector {
                        source_id: Some(id),
                        connector: Arc::new(SyslogConnector::new(path)),
                    });
                }
                _ => {
                    tracing::debug!("Unknown source type {}, skipping", source_type);
                }
            }
        }

        Ok(connectors)
    }

    fn prepare_entries(
        mut entries: Vec<LogEntry>,
        source_id: Option<i64>,
        limit: usize,
    ) -> Vec<LogEntry> {
        if let Some(id) = source_id {
            for entry in entries.iter_mut() {
                if entry.source_id.is_none() {
                    entry.source_id = Some(id);
                }
            }
        }

        if limit > 0 && entries.len() > limit {
            entries.truncate(limit);
        }

        entries
    }
}
