use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod cli;
mod connectors;
mod db;
mod indexer;
mod mcp;

use cli::{alert, config, context, coverage, dashboard, discover, index, normalize, search, similar, status, tail, timeline};

/// SLS - System Log Search
///
/// Index and search system logs, agent stderr, and application logs.
/// Surfaces hidden errors and root causes that agents can't normally see.
#[derive(Parser)]
#[command(name = "sls")]
#[command(version, about, long_about = None)]
struct Cli {
    /// Output format
    #[arg(long, global = true)]
    json: bool,

    /// Machine-readable output (alias for --json)
    #[arg(long, global = true)]
    robot: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index logs from configured sources
    Index {
        /// Full reindex (ignore last position)
        #[arg(long)]
        full: bool,

        /// Watch for new log entries continuously
        #[arg(long)]
        watch: bool,
    },

    /// Search indexed logs
    Search {
        /// Search query
        query: String,

        /// Filter by log level (ERROR, WARN, INFO, DEBUG)
        #[arg(long)]
        level: Option<String>,

        /// Filter by service name
        #[arg(long)]
        service: Option<String>,

        /// Time range (e.g., "1h", "30m", "1d")
        #[arg(long)]
        since: Option<String>,

        /// Time range preset (e.g., "1h", "30m", "1d") - alias for --since
        #[arg(long)]
        last: Option<String>,

        /// Show only today's logs
        #[arg(long)]
        today: bool,

        /// Maximum results to return
        #[arg(long, default_value = "50")]
        limit: usize,

        /// Output format (table, json, csv)
        #[arg(long, value_name = "FORMAT")]
        format: Option<String>,
    },

    /// Find similar log patterns (semantic search)
    Similar {
        /// Query to find similar entries for
        query: String,

        /// Maximum results to return
        #[arg(long, default_value = "10")]
        limit: usize,
    },

    /// Get logs around a specific timestamp for context
    Context {
        /// Timestamp to center on (ISO-8601 or relative like "-5m")
        #[arg(long)]
        at: Option<String>,

        /// Session ID to get context for
        #[arg(long)]
        session_id: Option<String>,

        /// Number of entries before/after
        #[arg(long, default_value = "10")]
        window: usize,
    },

    /// Show activity timeline
    Timeline {
        /// Show today's activity
        #[arg(long)]
        today: bool,

        /// Time range (e.g., "1h", "1d", "7d")
        #[arg(long)]
        since: Option<String>,

        /// Group by interval (minute, hour, day)
        #[arg(long, default_value = "hour")]
        group_by: String,
    },

    /// Stream logs from a source
    Tail {
        /// Source type (journald, syslog, file)
        #[arg(long, default_value = "journald")]
        source: String,

        /// Path to log file (for syslog or file sources)
        #[arg(long)]
        path: Option<String>,

        /// Filter journald by systemd unit (e.g., "nginx.service")
        #[arg(long)]
        unit: Option<String>,

        /// Number of recent lines to show before following
        #[arg(long, default_value = "50")]
        lines: usize,
    },

    /// Show indexing status and health
    Status,

    /// Live dashboard with updating stats
    Dashboard {
        /// Refresh interval in seconds
        #[arg(long, default_value = "3")]
        refresh: u64,
    },

    /// Discover log sources on this machine
    Discover {
        /// Automatically accept high-confidence sources (>80%)
        #[arg(long)]
        auto: bool,
    },

    /// Show log coverage status (indexed vs discovered vs missing)
    Coverage {
        /// Initialize a sample service catalog
        #[arg(long)]
        init: bool,
    },

    /// Manage log sources
    Sources {
        #[command(subcommand)]
        action: SourcesAction,
    },

    /// Print capabilities for agent introspection
    Capabilities,

    /// Manage SLS configuration
    Config {
        /// Configuration key to get or set
        key: Option<String>,

        /// Value to set (omit to get current value)
        value: Option<String>,
    },

    /// Normalize arbitrary log formats to SLS schema
    Normalize {
        /// Input file path (reads from stdin if not provided)
        #[arg(long)]
        input: Option<PathBuf>,

        /// Output format (json, ndjson)
        #[arg(long)]
        format: Option<String>,
    },

    /// Run as MCP server (stdio transport)
    Mcp,

    /// Check error/warning thresholds for alerting
    Alert {
        /// Time range (e.g., "1h", "30m", "1d")
        #[arg(long)]
        since: Option<String>,

        /// Time range preset (e.g., "1h", "30m", "1d") - alias for --since
        #[arg(long)]
        last: Option<String>,

        /// Check only today's logs
        #[arg(long)]
        today: bool,

        /// Error count threshold for CRITICAL status (default: 1)
        #[arg(long, default_value = "1")]
        error_threshold: usize,

        /// Warning count threshold for WARNING status (default: 10)
        #[arg(long, default_value = "10")]
        warning_threshold: usize,

        /// Filter by service name
        #[arg(long)]
        service: Option<String>,
    },
}

#[derive(Subcommand)]
enum SourcesAction {
    /// List configured sources
    List,

    /// Add a new source
    Add {
        /// Source type (journald, syslog, agent_stderr, codex, docker, json)
        #[arg(long)]
        r#type: String,

        /// Path to log file or directory
        #[arg(long)]
        path: Option<String>,

        /// Container name (for docker type)
        #[arg(long)]
        container: Option<String>,
    },

    /// Remove a source
    Remove {
        /// Source ID to remove
        id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let cli = Cli::parse();
    let json_output = cli.json || cli.robot;

    match cli.command {
        Commands::Index { full, watch } => {
            index::run(full, watch, json_output).await
        }
        Commands::Search {
            query,
            level,
            service,
            since,
            last,
            today,
            limit,
            format,
        } => search::run(query, level, service, since, last, today, limit, format, json_output).await,
        Commands::Similar { query, limit } => {
            similar::run(query, limit, json_output).await
        }
        Commands::Context {
            at,
            session_id,
            window,
        } => context::run(at, session_id, window, json_output).await,
        Commands::Timeline {
            today,
            since,
            group_by,
        } => timeline::run(today, since, group_by, json_output).await,
        Commands::Tail {
            source,
            path,
            unit,
            lines,
        } => tail::run(source, path, unit, lines, json_output).await,
        Commands::Status => status::run(json_output).await,
        Commands::Dashboard { refresh } => dashboard::run(refresh).await,
        Commands::Discover { auto } => discover::run(auto, json_output).await,
        Commands::Coverage { init } => coverage::run(json_output, init).await,
        Commands::Sources { action } => match action {
            SourcesAction::List => cli::sources::list(json_output).await,
            SourcesAction::Add {
                r#type,
                path,
                container,
            } => cli::sources::add(r#type, path, container, json_output).await,
            SourcesAction::Remove { id } => cli::sources::remove(id, json_output).await,
        },
        Commands::Capabilities => {
            cli::capabilities::run(json_output).await
        }
        Commands::Config { key, value } => {
            config::run(key, value, json_output).await
        }
        Commands::Normalize { input, format } => {
            normalize::run(input, format, json_output).await
        }
        Commands::Mcp => {
            mcp::run_mcp_server().await
        }
        Commands::Alert {
            since,
            last,
            today,
            error_threshold,
            warning_threshold,
            service,
        } => alert::run(since, last, today, error_threshold, warning_threshold, service, json_output).await,
    }
}
