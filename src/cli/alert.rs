//! sls alert - Basic threshold alerting for log monitoring
//!
//! Checks if error/warning counts exceed configured thresholds.
//! Useful for CI/CD pipelines and monitoring scripts.

use anyhow::Result;
use chrono::Local;
use serde::Serialize;

use crate::indexer::{parse_relative_time, Indexer, SearchRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Ok,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertStatus::Ok => write!(f, "OK"),
            AlertStatus::Warning => write!(f, "WARNING"),
            AlertStatus::Critical => write!(f, "CRITICAL"),
        }
    }
}

#[derive(Serialize)]
struct AlertOutput {
    status: AlertStatus,
    exit_code: i32,
    error_count: usize,
    warning_count: usize,
    error_threshold: usize,
    warning_threshold: usize,
    time_range: String,
    message: String,
}

pub async fn run(
    since: Option<String>,
    last: Option<String>,
    today: bool,
    error_threshold: usize,
    warning_threshold: usize,
    service: Option<String>,
    json_output: bool,
) -> Result<()> {
    let indexer = Indexer::open_default()?;

    // Determine time filter
    // Use earliest() to handle DST transitions safely
    let time_filter = if today {
        let naive_today = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
        naive_today.and_local_timezone(Local).earliest().map(|dt| dt.timestamp())
    } else if let Some(ref s) = since {
        parse_relative_time(s)
    } else if let Some(ref l) = last {
        parse_relative_time(l)
    } else {
        // Default to last 1 hour if no time range specified
        parse_relative_time("1h")
    };

    let time_range = if today {
        "today".to_string()
    } else if let Some(ref s) = since {
        s.clone()
    } else if let Some(ref l) = last {
        l.clone()
    } else {
        "1h".to_string()
    };

    // Count errors - use high limit to get accurate counts
    // For very high volume logs, consider using a dedicated count query
    let mut error_request = SearchRequest::new("").with_level("ERROR").with_limit(1_000_000);
    if let Some(ts) = time_filter {
        error_request = error_request.with_since(ts);
    }
    if let Some(ref svc) = service {
        error_request = error_request.with_service(svc);
    }
    let error_result = indexer.search(&error_request).await?;
    let error_count = error_result.entries.len();

    // Count warnings
    let mut warn_request = SearchRequest::new("").with_level("WARN").with_limit(1_000_000);
    if let Some(ts) = time_filter {
        warn_request = warn_request.with_since(ts);
    }
    if let Some(ref svc) = service {
        warn_request = warn_request.with_service(svc);
    }
    let warn_result = indexer.search(&warn_request).await?;
    let warning_count = warn_result.entries.len();

    // Determine alert status
    let (status, exit_code, message) = if error_count >= error_threshold {
        (
            AlertStatus::Critical,
            2,
            format!(
                "CRITICAL: {} errors in last {} (threshold: {})",
                error_count, time_range, error_threshold
            ),
        )
    } else if warning_count >= warning_threshold {
        (
            AlertStatus::Warning,
            1,
            format!(
                "WARNING: {} warnings in last {} (threshold: {})",
                warning_count, time_range, warning_threshold
            ),
        )
    } else {
        (
            AlertStatus::Ok,
            0,
            format!(
                "OK: {} errors, {} warnings in last {}",
                error_count, warning_count, time_range
            ),
        )
    };

    let output = AlertOutput {
        status,
        exit_code,
        error_count,
        warning_count,
        error_threshold,
        warning_threshold,
        time_range,
        message: message.clone(),
    };

    if json_output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", message);
        if error_count > 0 || warning_count > 0 {
            println!();
            println!("  Errors:   {}", error_count);
            println!("  Warnings: {}", warning_count);
        }
    }

    // Exit with appropriate code for shell scripts
    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_status_display() {
        assert_eq!(format!("{}", AlertStatus::Ok), "OK");
        assert_eq!(format!("{}", AlertStatus::Warning), "WARNING");
        assert_eq!(format!("{}", AlertStatus::Critical), "CRITICAL");
    }

    #[test]
    fn test_alert_status_serialize() {
        assert_eq!(
            serde_json::to_string(&AlertStatus::Ok).unwrap(),
            "\"ok\""
        );
        assert_eq!(
            serde_json::to_string(&AlertStatus::Warning).unwrap(),
            "\"warning\""
        );
        assert_eq!(
            serde_json::to_string(&AlertStatus::Critical).unwrap(),
            "\"critical\""
        );
    }
}
