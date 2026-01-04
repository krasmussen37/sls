//! Service Catalog - Maps services to log sources
//!
//! Manages ~/.sls/service_catalog.yaml which defines which services
//! should have logs tracked and maps them to log sources.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// A service definition in the catalog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDef {
    /// Human-readable name of the service
    pub name: String,

    /// Description of the service
    #[serde(default)]
    pub description: String,

    /// Log sources for this service (source IDs or paths)
    #[serde(default)]
    pub sources: Vec<String>,

    /// Expected log patterns (optional)
    #[serde(default)]
    pub patterns: Vec<String>,

    /// Tags for grouping services
    #[serde(default)]
    pub tags: Vec<String>,

    /// Whether this service is required (show warning if no logs)
    #[serde(default)]
    pub required: bool,
}

/// The service catalog configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceCatalog {
    /// Version of the catalog format
    #[serde(default = "default_version")]
    pub version: String,

    /// Map of service ID to service definition
    #[serde(default)]
    pub services: HashMap<String, ServiceDef>,

    /// Default tags to apply to all services
    #[serde(default)]
    pub default_tags: Vec<String>,
}

fn default_version() -> String {
    "1".to_string()
}

impl ServiceCatalog {
    /// Get the default catalog file path
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".sls")
            .join("service_catalog.yaml")
    }

    /// Load catalog from the default path
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_path())
    }

    /// Load catalog from a specific path
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(path)?;
        let catalog: ServiceCatalog = serde_yaml::from_str(&content)?;
        Ok(catalog)
    }

    /// Save catalog to the default path
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    /// Save catalog to a specific path
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_yaml::to_string(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Create a sample catalog with common services
    pub fn sample() -> Self {
        let mut services = HashMap::new();

        services.insert(
            "journald".to_string(),
            ServiceDef {
                name: "System Journal".to_string(),
                description: "systemd journal for all system services".to_string(),
                sources: vec!["journald".to_string()],
                patterns: vec![],
                tags: vec!["system".to_string()],
                required: true,
            },
        );

        services.insert(
            "syslog".to_string(),
            ServiceDef {
                name: "Syslog".to_string(),
                description: "Traditional syslog messages".to_string(),
                sources: vec!["/var/log/syslog".to_string(), "/var/log/messages".to_string()],
                patterns: vec![],
                tags: vec!["system".to_string()],
                required: false,
            },
        );

        services.insert(
            "claude-code".to_string(),
            ServiceDef {
                name: "Claude Code".to_string(),
                description: "Claude Code CLI session logs".to_string(),
                sources: vec!["~/.claude/logs".to_string(), "~/.claude/projects".to_string()],
                patterns: vec![],
                tags: vec!["agent".to_string(), "development".to_string()],
                required: false,
            },
        );

        services.insert(
            "nginx".to_string(),
            ServiceDef {
                name: "Nginx".to_string(),
                description: "Nginx web server logs".to_string(),
                sources: vec![
                    "/var/log/nginx/access.log".to_string(),
                    "/var/log/nginx/error.log".to_string(),
                ],
                patterns: vec![],
                tags: vec!["webserver".to_string()],
                required: false,
            },
        );

        Self {
            version: "1".to_string(),
            services,
            default_tags: vec![],
        }
    }

    /// Get list of all service IDs
    pub fn service_ids(&self) -> Vec<&String> {
        self.services.keys().collect()
    }

    /// Check if a service exists in the catalog
    pub fn has_service(&self, id: &str) -> bool {
        self.services.contains_key(id)
    }

    /// Get a service by ID
    pub fn get_service(&self, id: &str) -> Option<&ServiceDef> {
        self.services.get(id)
    }

    /// Add or update a service
    pub fn upsert_service(&mut self, id: String, service: ServiceDef) {
        self.services.insert(id, service);
    }

    /// Remove a service
    pub fn remove_service(&mut self, id: &str) -> Option<ServiceDef> {
        self.services.remove(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sample_catalog() {
        let catalog = ServiceCatalog::sample();
        assert!(catalog.services.len() >= 3);
        assert!(catalog.has_service("journald"));
        assert!(catalog.has_service("claude-code"));
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_catalog.yaml");

        let catalog = ServiceCatalog::sample();
        catalog.save_to(&path).unwrap();

        let loaded = ServiceCatalog::load_from(&path).unwrap();
        assert_eq!(loaded.services.len(), catalog.services.len());
    }

    #[test]
    fn test_empty_catalog() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.yaml");

        let catalog = ServiceCatalog::load_from(&path).unwrap();
        assert!(catalog.services.is_empty());
    }
}
