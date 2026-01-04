//! sls coverage - Show log coverage status
//!
//! Compares the service catalog with discovered/indexed sources
//! to show which services have logs indexed, discovered but not indexed,
//! or not discovered at all.

use anyhow::Result;
use serde::Serialize;

use crate::cli::catalog::ServiceCatalog;
use crate::cli::discover::{discover_sources, DiscoveredSource};
use crate::db::{default_db_path, Database};

/// Coverage status for a service
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CoverageStatus {
    /// Logs are indexed and being tracked
    Indexed,
    /// Source discovered but not yet indexed
    Discovered,
    /// Service defined but no sources found
    Missing,
    /// Not in catalog but discovered
    Uncataloged,
}

/// Coverage info for a single service
#[derive(Debug, Clone, Serialize)]
pub struct ServiceCoverage {
    pub id: String,
    pub name: String,
    pub status: CoverageStatus,
    pub sources_found: usize,
    pub sources_expected: usize,
    pub required: bool,
    pub tags: Vec<String>,
}

/// Summary of overall coverage
#[derive(Debug, Clone, Serialize)]
pub struct CoverageSummary {
    pub total_services: usize,
    pub indexed: usize,
    pub discovered: usize,
    pub missing: usize,
    pub uncataloged: usize,
    pub required_missing: usize,
}

/// Full coverage report
#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    pub success: bool,
    pub summary: CoverageSummary,
    pub services: Vec<ServiceCoverage>,
    pub catalog_path: String,
}

/// Run the coverage command
pub async fn run(json_output: bool, init: bool) -> Result<()> {
    // If --init, create sample catalog
    if init {
        return initialize_catalog(json_output);
    }

    // Load catalog
    let catalog = ServiceCatalog::load()?;

    // Get discovered sources
    let discovered = discover_sources();

    // Get indexed sources from database
    let indexed = get_indexed_sources()?;

    // Build coverage report
    let report = build_coverage_report(&catalog, &discovered, &indexed);

    // Output
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_coverage_report(&report);
    }

    Ok(())
}

/// Initialize a sample service catalog
fn initialize_catalog(json_output: bool) -> Result<()> {
    let path = ServiceCatalog::default_path();

    if path.exists() {
        if json_output {
            println!(
                r#"{{"success": false, "error": "Catalog already exists", "path": "{}"}}"#,
                path.display()
            );
        } else {
            println!(
                "Service catalog already exists at: {}",
                path.display()
            );
            println!("Use 'sls coverage' to view coverage status.");
        }
        return Ok(());
    }

    let catalog = ServiceCatalog::sample();
    catalog.save()?;

    if json_output {
        println!(
            r#"{{"success": true, "message": "Catalog created", "path": "{}", "services": {}}}"#,
            path.display(),
            catalog.services.len()
        );
    } else {
        println!("Created sample service catalog at: {}", path.display());
        println!();
        println!("Included {} services:", catalog.services.len());
        for (id, svc) in &catalog.services {
            println!("  - {}: {}", id, svc.description);
        }
        println!();
        println!("Edit the catalog to customize services for your environment.");
        println!("Then run 'sls coverage' to check log coverage.");
    }

    Ok(())
}

/// Get indexed sources from the database
fn get_indexed_sources() -> Result<Vec<IndexedSource>> {
    let db_path = default_db_path();

    if !db_path.exists() {
        return Ok(vec![]);
    }

    let db = Database::open(&db_path)?;

    let mut stmt = db.conn().prepare(
        "SELECT id, source_type, source_path, active FROM log_sources WHERE active = 1",
    )?;

    let sources = stmt
        .query_map([], |row| {
            Ok(IndexedSource {
                id: row.get(0)?,
                source_type: row.get(1)?,
                path: row.get(2)?,
                active: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(sources)
}

#[derive(Debug)]
struct IndexedSource {
    id: i64,
    source_type: String,
    path: Option<String>,
    active: bool,
}

/// Build the coverage report by comparing catalog, discovered, and indexed sources
fn build_coverage_report(
    catalog: &ServiceCatalog,
    discovered: &[DiscoveredSource],
    indexed: &[IndexedSource],
) -> CoverageReport {
    let mut services = Vec::new();
    let mut summary = CoverageSummary {
        total_services: 0,
        indexed: 0,
        discovered: 0,
        missing: 0,
        uncataloged: 0,
        required_missing: 0,
    };

    // Process cataloged services
    for (id, svc) in &catalog.services {
        let sources_expected = svc.sources.len();

        // Check how many sources are indexed
        let sources_indexed = svc
            .sources
            .iter()
            .filter(|src| {
                indexed.iter().any(|idx| {
                    idx.source_type == **src
                        || idx.path.as_ref().map(|p| p.contains(*src)).unwrap_or(false)
                })
            })
            .count();

        // Check how many sources are discovered
        let sources_discovered = svc
            .sources
            .iter()
            .filter(|src| {
                discovered.iter().any(|disc| {
                    disc.id == **src
                        || disc.source_type == **src
                        || disc.path.as_ref().map(|p| p.contains(*src)).unwrap_or(false)
                })
            })
            .count();

        let status = if sources_indexed > 0 {
            CoverageStatus::Indexed
        } else if sources_discovered > 0 {
            CoverageStatus::Discovered
        } else {
            CoverageStatus::Missing
        };

        match &status {
            CoverageStatus::Indexed => summary.indexed += 1,
            CoverageStatus::Discovered => summary.discovered += 1,
            CoverageStatus::Missing => {
                summary.missing += 1;
                if svc.required {
                    summary.required_missing += 1;
                }
            }
            CoverageStatus::Uncataloged => summary.uncataloged += 1,
        }

        services.push(ServiceCoverage {
            id: id.clone(),
            name: svc.name.clone(),
            status,
            sources_found: sources_indexed.max(sources_discovered),
            sources_expected,
            required: svc.required,
            tags: svc.tags.clone(),
        });

        summary.total_services += 1;
    }

    // Add discovered but uncataloged sources
    for disc in discovered {
        let in_catalog = catalog.services.values().any(|svc| {
            svc.sources.iter().any(|src| {
                *src == disc.id
                    || *src == disc.source_type
                    || disc.path.as_ref().map(|p| p.contains(src)).unwrap_or(false)
            })
        });

        if !in_catalog && disc.confidence > 60 {
            services.push(ServiceCoverage {
                id: disc.id.clone(),
                name: disc.description.clone(),
                status: CoverageStatus::Uncataloged,
                sources_found: 1,
                sources_expected: 0,
                required: false,
                tags: vec![],
            });
            summary.uncataloged += 1;
            summary.total_services += 1;
        }
    }

    // Sort by status (missing first, then discovered, then indexed, then uncataloged)
    services.sort_by(|a, b| {
        let order = |s: &CoverageStatus| match s {
            CoverageStatus::Missing => 0,
            CoverageStatus::Discovered => 1,
            CoverageStatus::Indexed => 2,
            CoverageStatus::Uncataloged => 3,
        };
        order(&a.status).cmp(&order(&b.status))
    });

    CoverageReport {
        success: true,
        summary,
        services,
        catalog_path: ServiceCatalog::default_path().to_string_lossy().to_string(),
    }
}

/// Print the coverage report in human-readable format
fn print_coverage_report(report: &CoverageReport) {
    println!("SLS Log Coverage Report");
    println!("=======================");
    println!();

    // Summary
    let s = &report.summary;
    println!(
        "Summary: {} indexed, {} discovered, {} missing, {} uncataloged",
        s.indexed, s.discovered, s.missing, s.uncataloged
    );

    if s.required_missing > 0 {
        println!(
            "⚠️  {} required service(s) have no logs!",
            s.required_missing
        );
    }
    println!();

    if report.services.is_empty() {
        println!("No services in catalog.");
        println!();
        println!("Run 'sls coverage --init' to create a sample catalog.");
        return;
    }

    // Services by status
    println!("Services:");
    for svc in &report.services {
        let icon = match svc.status {
            CoverageStatus::Indexed => "✓",
            CoverageStatus::Discovered => "○",
            CoverageStatus::Missing => "✗",
            CoverageStatus::Uncataloged => "?",
        };

        let status_str = match svc.status {
            CoverageStatus::Indexed => "indexed",
            CoverageStatus::Discovered => "discovered",
            CoverageStatus::Missing => "missing",
            CoverageStatus::Uncataloged => "uncataloged",
        };

        let required_marker = if svc.required { " [required]" } else { "" };

        println!(
            "  {} {:20} {:12} ({}/{} sources){}",
            icon, svc.id, status_str, svc.sources_found, svc.sources_expected, required_marker
        );
    }

    println!();
    println!("Catalog: {}", report.catalog_path);
    println!();
    println!("Legend:");
    println!("  ✓ indexed     - Logs are being tracked");
    println!("  ○ discovered  - Source found but not indexed (run 'sls index')");
    println!("  ✗ missing     - No logs found for this service");
    println!("  ? uncataloged - Discovered source not in catalog");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coverage_status_order() {
        assert!(CoverageStatus::Missing != CoverageStatus::Indexed);
    }
}
