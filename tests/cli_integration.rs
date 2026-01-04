//! Integration tests for SLS CLI commands
//!
//! Tests each CLI command end-to-end using real fixtures.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

/// Get a Command for the sls binary
fn sls() -> Command {
    Command::cargo_bin("sls").unwrap()
}

/// Create a temp directory with a sample syslog file
fn create_test_fixtures() -> TempDir {
    let temp = TempDir::new().unwrap();

    // Create a sample syslog file
    let syslog_content = r#"Jan  2 10:00:00 testhost kernel: Starting system
Jan  2 10:00:01 testhost systemd[1]: Started Session 1 of user root.
Jan  2 10:00:02 testhost sshd[1234]: Accepted publickey for user from 192.168.1.1
Jan  2 10:00:03 testhost kernel: ERROR: disk I/O error on /dev/sda1
Jan  2 10:00:04 testhost myapp[5678]: Connection established to database
Jan  2 10:00:05 testhost myapp[5678]: ERROR: Query failed: timeout
Jan  2 10:00:06 testhost myapp[5678]: WARNING: Retrying connection
Jan  2 10:00:07 testhost kernel: Network interface eth0 up
"#;

    let log_dir = temp.path().join("var/log");
    fs::create_dir_all(&log_dir).unwrap();
    fs::write(log_dir.join("syslog"), syslog_content).unwrap();

    temp
}

mod discover {
    use super::*;

    #[test]
    fn test_discover_help() {
        sls()
            .arg("discover")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Discover log sources"));
    }

    #[test]
    fn test_discover_json_output() {
        sls()
            .arg("discover")
            .arg("--json")
            .assert()
            .success()
            .stdout(predicate::str::contains("{"));
    }

    #[test]
    fn test_discover_auto_mode() {
        sls()
            .arg("discover")
            .arg("--auto")
            .arg("--json")
            .assert()
            .success();
    }
}

mod search {
    use super::*;

    #[test]
    fn test_search_help() {
        sls()
            .arg("search")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Search indexed logs"));
    }

    #[test]
    fn test_search_empty_query() {
        sls()
            .arg("search")
            .arg("")
            .arg("--json")
            .assert()
            .success();
    }

    #[test]
    fn test_search_with_query() {
        sls()
            .arg("search")
            .arg("error")
            .arg("--json")
            .assert()
            .success();
    }

    #[test]
    fn test_search_with_limit() {
        sls()
            .arg("search")
            .arg("test")
            .arg("--limit")
            .arg("10")
            .arg("--json")
            .assert()
            .success();
    }
}

mod similar {
    use super::*;

    #[test]
    fn test_similar_help() {
        sls()
            .arg("similar")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Find similar log patterns"));
    }

    #[test]
    fn test_similar_with_pattern() {
        sls()
            .arg("similar")
            .arg("connection error")
            .arg("--json")
            .assert()
            .success();
    }
}

mod context {
    use super::*;

    #[test]
    fn test_context_help() {
        sls()
            .arg("context")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Get logs around a specific timestamp"));
    }

    #[test]
    fn test_context_with_relative_time() {
        // Use --at=VALUE format to avoid clap parsing -5m as flags
        sls()
            .arg("context")
            .arg("--at=-5m")
            .arg("--json")
            .assert()
            .success();
    }

    #[test]
    fn test_context_with_window() {
        sls()
            .arg("context")
            .arg("--at=-1h")
            .arg("--window")
            .arg("20")
            .arg("--json")
            .assert()
            .success();
    }

    #[test]
    fn test_context_default() {
        // Test context with no specific timestamp (uses current time)
        sls()
            .arg("context")
            .arg("--json")
            .assert()
            .success();
    }
}

mod timeline {
    use super::*;

    #[test]
    fn test_timeline_help() {
        sls()
            .arg("timeline")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Show activity timeline"));
    }

    #[test]
    fn test_timeline_json_output() {
        sls()
            .arg("timeline")
            .arg("--json")
            .assert()
            .success();
    }

    #[test]
    fn test_timeline_with_since() {
        sls()
            .arg("timeline")
            .arg("--since")
            .arg("1h")
            .arg("--json")
            .assert()
            .success();
    }

    #[test]
    fn test_timeline_today() {
        sls()
            .arg("timeline")
            .arg("--today")
            .arg("--json")
            .assert()
            .success();
    }
}

mod status {
    use super::*;

    #[test]
    fn test_status_help() {
        sls()
            .arg("status")
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Show indexing status"));
    }

    #[test]
    fn test_status_json_output() {
        sls()
            .arg("status")
            .arg("--json")
            .assert()
            .success()
            .stdout(predicate::str::contains("{"));
    }
}

mod capabilities {
    use super::*;

    #[test]
    fn test_capabilities_output() {
        sls()
            .arg("capabilities")
            .assert()
            .success()
            .stdout(predicate::str::contains("Commands:"));
    }
}

mod global_flags {
    use super::*;

    #[test]
    fn test_version() {
        sls()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("sls"));
    }

    #[test]
    fn test_help() {
        sls()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("System Log Search"));
    }

    #[test]
    fn test_robot_flag() {
        sls()
            .arg("--robot")
            .arg("status")
            .assert()
            .success();
    }
}
