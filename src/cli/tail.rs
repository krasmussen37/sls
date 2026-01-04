//! sls tail - Stream logs from a source

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::SecondsFormat;
use serde::Serialize;

use crate::connectors::{journald::JournaldConnector, syslog::SyslogConnector, LogEntry};

#[derive(Serialize)]
struct TailEntryOutput {
    timestamp: String,
    level: Option<String>,
    service: Option<String>,
    message: String,
    source: String,
}

#[derive(Serialize)]
struct TailLineOutput {
    line: String,
    source: String,
}

pub async fn run(
    source: String,
    path: Option<String>,
    unit: Option<String>,
    lines: usize,
    json_output: bool,
) -> Result<()> {
    match source.as_str() {
        "journald" => tail_journald(unit, lines, json_output),
        "syslog" => {
            let path = path.unwrap_or_else(|| "/var/log/syslog".to_string());
            tail_syslog(PathBuf::from(path), lines, json_output)
        }
        "file" => {
            let path = path.context("file source requires --path")?;
            tail_file(PathBuf::from(path), lines, json_output)
        }
        other => {
            anyhow::bail!(
                "Unknown source type: {} (supported: journald, syslog, file)",
                other
            );
        }
    }
}

fn tail_journald(unit: Option<String>, lines: usize, json_output: bool) -> Result<()> {
    let mut cmd = Command::new("journalctl");
    cmd.arg("--no-pager");
    cmd.arg("--output=json");
    cmd.arg("--follow");
    cmd.arg(format!("--lines={}", lines));
    if let Some(unit) = unit {
        cmd.arg(format!("--unit={}", unit));
    }

    let mut child = cmd
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn journalctl")?;

    let stdout = child
        .stdout
        .take()
        .context("Failed to capture journalctl stdout")?;

    let reader = BufReader::new(stdout);
    let connector = JournaldConnector::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(entry) = connector.parse_entry(trimmed) {
            print_entry(&entry, "journald", json_output)?;
        } else if !json_output {
            println!("{}", trimmed);
        }
    }

    Ok(())
}

fn tail_syslog(path: PathBuf, lines: usize, json_output: bool) -> Result<()> {
    let connector = SyslogConnector::new(&path);
    let mut reader = BufReader::new(
        File::open(&path).with_context(|| format!("Failed to open {}", path.display()))?,
    );

    let recent = read_last_lines(&mut reader, lines)?;
    for line in recent {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(entry) = connector.parse_line(trimmed) {
            print_entry(&entry, "syslog", json_output)?;
        } else {
            print_line(trimmed, &path, json_output)?;
        }
    }

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            thread::sleep(Duration::from_millis(250));
            continue;
        }

        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(entry) = connector.parse_line(trimmed) {
            print_entry(&entry, "syslog", json_output)?;
        } else {
            print_line(trimmed, &path, json_output)?;
        }
    }
}

fn tail_file(path: PathBuf, lines: usize, json_output: bool) -> Result<()> {
    let mut reader = BufReader::new(
        File::open(&path).with_context(|| format!("Failed to open {}", path.display()))?,
    );

    let recent = read_last_lines(&mut reader, lines)?;
    for line in recent {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        print_line(trimmed, &path, json_output)?;
    }

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            thread::sleep(Duration::from_millis(250));
            continue;
        }

        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        print_line(trimmed, &path, json_output)?;
    }
}

fn read_last_lines(reader: &mut BufReader<File>, lines: usize) -> Result<Vec<String>> {
    if lines == 0 {
        return Ok(Vec::new());
    }

    let mut buffer = String::new();
    let mut recent: VecDeque<String> = VecDeque::with_capacity(lines);

    while reader.read_line(&mut buffer)? > 0 {
        let trimmed = buffer.trim_end().to_string();
        if recent.len() == lines {
            recent.pop_front();
        }
        recent.push_back(trimmed);
        buffer.clear();
    }

    Ok(recent.into_iter().collect())
}

fn print_entry(entry: &LogEntry, source: &str, json_output: bool) -> Result<()> {
    let timestamp = entry
        .timestamp
        .to_rfc3339_opts(SecondsFormat::Secs, true);

    if json_output {
        let payload = TailEntryOutput {
            timestamp,
            level: entry.level.clone(),
            service: entry.service.clone(),
            message: entry.message.clone(),
            source: source.to_string(),
        };
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        let level_str = entry
            .level
            .as_ref()
            .map(|l| format!("[{}]", l))
            .unwrap_or_default();
        let service_str = entry
            .service
            .as_ref()
            .map(|s| format!("{}: ", s))
            .unwrap_or_default();
        println!("{} {} {}{}", timestamp, level_str, service_str, entry.message);
    }

    Ok(())
}

fn print_line(line: &str, path: &Path, json_output: bool) -> Result<()> {
    if json_output {
        let payload = TailLineOutput {
            line: line.to_string(),
            source: path.display().to_string(),
        };
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        println!("{}", line);
    }

    Ok(())
}
