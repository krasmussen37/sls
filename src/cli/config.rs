//! SLS Configuration Management
//!
//! Manages ~/.sls/config.yaml for SLS settings including:
//! - Discovery mode (cron, daemon, manual, hybrid)
//! - Index settings
//! - Output preferences

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Discovery mode for automatic log source detection
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryMode {
    /// Manual discovery only (user runs `sls discover`)
    #[default]
    Manual,
    /// Run discovery periodically via cron
    Cron,
    /// Run as a background daemon
    Daemon,
    /// Hybrid: daemon for hot sources, cron for cold
    Hybrid,
}

impl std::fmt::Display for DiscoveryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryMode::Manual => write!(f, "manual"),
            DiscoveryMode::Cron => write!(f, "cron"),
            DiscoveryMode::Daemon => write!(f, "daemon"),
            DiscoveryMode::Hybrid => write!(f, "hybrid"),
        }
    }
}

impl std::str::FromStr for DiscoveryMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "manual" => Ok(DiscoveryMode::Manual),
            "cron" => Ok(DiscoveryMode::Cron),
            "daemon" => Ok(DiscoveryMode::Daemon),
            "hybrid" => Ok(DiscoveryMode::Hybrid),
            _ => Err(format!(
                "Invalid discovery mode: {}. Use: manual, cron, daemon, or hybrid",
                s
            )),
        }
    }
}

/// SLS Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlsConfig {
    /// Version of the config format
    #[serde(default = "default_version")]
    pub version: String,

    /// Discovery mode for log sources
    #[serde(default)]
    pub discovery_mode: DiscoveryMode,

    /// Cron schedule for cron/hybrid modes (crontab syntax)
    #[serde(default = "default_cron_schedule")]
    pub cron_schedule: String,

    /// Daemon refresh interval in seconds
    #[serde(default = "default_daemon_interval")]
    pub daemon_interval_secs: u64,

    /// Auto-accept high-confidence sources (>80%)
    #[serde(default)]
    pub auto_accept_sources: bool,

    /// Default output format (table, json, csv)
    #[serde(default = "default_output_format")]
    pub default_output_format: String,

    /// Index settings
    #[serde(default)]
    pub index: IndexConfig,
}

/// Index-related configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexConfig {
    /// Maximum entries to keep (0 = unlimited)
    #[serde(default)]
    pub max_entries: u64,

    /// Retention period in days (0 = unlimited)
    #[serde(default)]
    pub retention_days: u64,

    /// Batch size for indexing
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_version() -> String {
    "1".to_string()
}

fn default_cron_schedule() -> String {
    "0 * * * *".to_string() // Every hour
}

fn default_daemon_interval() -> u64 {
    60 // 1 minute
}

fn default_output_format() -> String {
    "table".to_string()
}

fn default_batch_size() -> usize {
    1000
}

impl Default for SlsConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            discovery_mode: DiscoveryMode::default(),
            cron_schedule: default_cron_schedule(),
            daemon_interval_secs: default_daemon_interval(),
            auto_accept_sources: false,
            default_output_format: default_output_format(),
            index: IndexConfig::default(),
        }
    }
}

impl SlsConfig {
    /// Get the default config file path
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".sls")
            .join("config.yaml")
    }

    /// Load config from the default path
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_path())
    }

    /// Load config from a specific path
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)?;
        let config: SlsConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Save config to the default path
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    /// Save config to a specific path
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_yaml::to_string(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

/// Run the config command
pub async fn run(
    key: Option<String>,
    value: Option<String>,
    json_output: bool,
) -> Result<()> {
    let mut config = SlsConfig::load()?;
    let config_path = SlsConfig::default_path();

    match (key.as_deref(), value) {
        // Show all config
        (None, None) => {
            if json_output {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                println!("SLS Configuration");
                println!("=================");
                println!("Path: {}", config_path.display());
                println!();
                println!("discovery-mode:      {}", config.discovery_mode);
                println!("cron-schedule:       {}", config.cron_schedule);
                println!("daemon-interval:     {}s", config.daemon_interval_secs);
                println!("auto-accept-sources: {}", config.auto_accept_sources);
                println!("default-format:      {}", config.default_output_format);
                println!();
                println!("Index Settings:");
                println!("  max-entries:    {}", if config.index.max_entries == 0 {
                    "unlimited".to_string()
                } else {
                    config.index.max_entries.to_string()
                });
                println!("  retention-days: {}", if config.index.retention_days == 0 {
                    "unlimited".to_string()
                } else {
                    config.index.retention_days.to_string()
                });
                println!("  batch-size:     {}", config.index.batch_size);
            }
        }
        // Get a specific value
        (Some(key), None) => {
            let value = match key {
                "discovery-mode" => config.discovery_mode.to_string(),
                "cron-schedule" => config.cron_schedule.clone(),
                "daemon-interval" => config.daemon_interval_secs.to_string(),
                "auto-accept-sources" => config.auto_accept_sources.to_string(),
                "default-format" => config.default_output_format.clone(),
                "index.max-entries" => config.index.max_entries.to_string(),
                "index.retention-days" => config.index.retention_days.to_string(),
                "index.batch-size" => config.index.batch_size.to_string(),
                _ => {
                    anyhow::bail!("Unknown config key: {}", key);
                }
            };

            if json_output {
                println!(r#"{{"key": "{}", "value": "{}"}}"#, key, value);
            } else {
                println!("{}", value);
            }
        }
        // Set a value
        (Some(key), Some(value)) => {
            match key {
                "discovery-mode" => {
                    config.discovery_mode = value.parse()
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                }
                "cron-schedule" => {
                    config.cron_schedule = value;
                }
                "daemon-interval" => {
                    config.daemon_interval_secs = value.parse()
                        .map_err(|_| anyhow::anyhow!("Invalid number: {}", value))?;
                }
                "auto-accept-sources" => {
                    config.auto_accept_sources = value.parse()
                        .map_err(|_| anyhow::anyhow!("Invalid boolean: {}", value))?;
                }
                "default-format" => {
                    if !["table", "json", "csv"].contains(&value.as_str()) {
                        anyhow::bail!("Invalid format: {}. Use: table, json, or csv", value);
                    }
                    config.default_output_format = value;
                }
                "index.max-entries" => {
                    config.index.max_entries = value.parse()
                        .map_err(|_| anyhow::anyhow!("Invalid number: {}", value))?;
                }
                "index.retention-days" => {
                    config.index.retention_days = value.parse()
                        .map_err(|_| anyhow::anyhow!("Invalid number: {}", value))?;
                }
                "index.batch-size" => {
                    config.index.batch_size = value.parse()
                        .map_err(|_| anyhow::anyhow!("Invalid number: {}", value))?;
                }
                _ => {
                    anyhow::bail!("Unknown config key: {}", key);
                }
            }

            config.save()?;

            if json_output {
                println!(r#"{{"success": true, "key": "{}", "value": "{}"}}"#, key,
                    match key {
                        "discovery-mode" => config.discovery_mode.to_string(),
                        _ => "updated".to_string()
                    });
            } else {
                println!("Set {} = {}", key, match key {
                    "discovery-mode" => config.discovery_mode.to_string(),
                    "cron-schedule" => config.cron_schedule.clone(),
                    "daemon-interval" => config.daemon_interval_secs.to_string(),
                    "auto-accept-sources" => config.auto_accept_sources.to_string(),
                    "default-format" => config.default_output_format.clone(),
                    "index.max-entries" => config.index.max_entries.to_string(),
                    "index.retention-days" => config.index.retention_days.to_string(),
                    "index.batch-size" => config.index.batch_size.to_string(),
                    _ => "?".to_string()
                });

                // Show setup instructions for discovery mode changes
                if key == "discovery-mode" {
                    println!();
                    print_discovery_setup(&config.discovery_mode);
                }
            }
        }
        (None, Some(_)) => {
            anyhow::bail!("Cannot set value without key");
        }
    }

    Ok(())
}

/// Print setup instructions for a discovery mode
fn print_discovery_setup(mode: &DiscoveryMode) {
    match mode {
        DiscoveryMode::Manual => {
            println!("Manual mode: Run 'sls discover' when you want to find new sources.");
        }
        DiscoveryMode::Cron => {
            println!("Cron mode setup:");
            println!("  1. Add to crontab: crontab -e");
            println!("  2. Add line: 0 * * * * sls index --watch=false");
            println!("  Or use: sls setup cron");
        }
        DiscoveryMode::Daemon => {
            println!("Daemon mode setup:");
            println!("  1. Create systemd service: sudo sls setup daemon");
            println!("  2. Enable: sudo systemctl enable sls-indexer");
            println!("  3. Start: sudo systemctl start sls-indexer");
        }
        DiscoveryMode::Hybrid => {
            println!("Hybrid mode setup:");
            println!("  1. Set up daemon for hot sources: sls setup daemon");
            println!("  2. Set up cron for cold sources: sls setup cron");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_discovery_mode_parse() {
        assert_eq!("manual".parse::<DiscoveryMode>().unwrap(), DiscoveryMode::Manual);
        assert_eq!("cron".parse::<DiscoveryMode>().unwrap(), DiscoveryMode::Cron);
        assert_eq!("daemon".parse::<DiscoveryMode>().unwrap(), DiscoveryMode::Daemon);
        assert_eq!("hybrid".parse::<DiscoveryMode>().unwrap(), DiscoveryMode::Hybrid);
        assert!("invalid".parse::<DiscoveryMode>().is_err());
    }

    #[test]
    fn test_config_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        let mut config = SlsConfig::default();
        config.discovery_mode = DiscoveryMode::Daemon;
        config.daemon_interval_secs = 120;

        config.save_to(&path).unwrap();

        let loaded = SlsConfig::load_from(&path).unwrap();
        assert_eq!(loaded.discovery_mode, DiscoveryMode::Daemon);
        assert_eq!(loaded.daemon_interval_secs, 120);
    }

    #[test]
    fn test_default_config() {
        let config = SlsConfig::default();
        assert_eq!(config.discovery_mode, DiscoveryMode::Manual);
        assert_eq!(config.default_output_format, "table");
    }
}
