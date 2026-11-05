use std::collections::HashMap;
use std::env;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::model::{AccessChange, EndpointRole, PtyTopology, TtyEndpoint, TtySample};

const NON_CLIENT_SUBCOMMANDS: [&str; 2] = ["remote-client-bridge", "server"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub parent_pid: u32,
    pub tty: String,
    pub command: String,
    pub arguments: String,
}

pub fn run_command(program: &str, arguments: &[&str]) -> CommandResult {
    match Command::new(program).args(arguments).output() {
        Ok(output) => CommandResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(error) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

pub fn parse_process_records(output: &str) -> Vec<ProcessRecord> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent_pid = fields.next()?.parse().ok()?;
            let tty = fields.next()?.to_owned();
            let command = fields.next()?.to_owned();
            let arguments = fields.collect::<Vec<_>>().join(" ");
            if arguments.is_empty() {
                return None;
            }
            Some(ProcessRecord {
                pid,
                parent_pid,
                tty,
                command,
                arguments,
            })
        })
        .collect()
}

pub fn discover_topology() -> Result<PtyTopology> {
    let process_result = run_command("ps", &["-axo", "pid=,ppid=,tty=,comm=,args="]);
    if !process_result.success {
        bail!(
            "could not inspect process table: {}",
            process_result.stderr.trim()
        );
    }
    let processes = parse_process_records(&process_result.stdout);
    let process_by_pid: HashMap<u32, ProcessRecord> = processes
        .iter()
        .cloned()
        .map(|record| (record.pid, record))
        .collect();
    let clients = processes.iter().filter_map(client_endpoint).collect();
    let herdr = env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_owned());
    let snapshot_result = run_command(&herdr, &["api", "snapshot"]);
    if !snapshot_result.success {
        bail!(
            "could not inspect Herdr snapshot: {}",
            snapshot_result.stderr.trim()
        );
    }
    let payload: Value = serde_json::from_str(&snapshot_result.stdout)
        .context("Herdr snapshot returned invalid JSON")?;
    let panes = payload
        .pointer("/result/snapshot/panes")
        .and_then(Value::as_array)
        .context("Herdr snapshot panes must be an array")?;
    let mut endpoints = Vec::new();
    for pane in panes {
        let pane_id = pane
            .get("pane_id")
            .and_then(Value::as_str)
            .context("pane_id must be a string")?;
        let workspace_id = pane
            .get("workspace_id")
            .and_then(Value::as_str)
            .context("workspace_id must be a string")?;
        let result = run_command(&herdr, &["pane", "process-info", "--pane", pane_id]);
        if !result.success {
            continue;
        }
        let info: Value = serde_json::from_str(&result.stdout)
            .context("pane process info returned invalid JSON")?;
        let Some(shell_pid) = info
            .pointer("/result/process_info/shell_pid")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
        else {
            continue;
        };
        let Some(process) = process_by_pid
            .get(&shell_pid)
            .filter(|record| record.tty != "?" && record.tty != "-")
        else {
            continue;
        };
        endpoints.push(TtyEndpoint {
            endpoint_id: format!("pane:{pane_id}:{shell_pid}:{}", process.tty),
            role: EndpointRole::Pane,
            pid: shell_pid,
            device: Path::new("/dev").join(&process.tty),
            client_id: None,
            workspace_id: Some(workspace_id.to_owned()),
            pane_id: Some(pane_id.to_owned()),
            cwd: pane
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            remote: None,
        });
    }
    Ok(PtyTopology {
        clients,
        panes: endpoints,
    })
}

fn client_endpoint(process: &ProcessRecord) -> Option<TtyEndpoint> {
    if process.tty == "?" || process.tty == "-" {
        return None;
    }
    let command_name = Path::new(&process.command).file_name()?.to_str()?;
    let arguments: Vec<&str> = process.arguments.split_whitespace().collect();
    let first_name = arguments
        .first()
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str());
    if command_name != "herdr" && first_name != Some("herdr") {
        return None;
    }
    if arguments
        .get(1)
        .is_some_and(|value| NON_CLIENT_SUBCOMMANDS.contains(value))
    {
        return None;
    }
    Some(TtyEndpoint {
        endpoint_id: format!("client:{}:{}", process.pid, process.tty),
        role: EndpointRole::Client,
        pid: process.pid,
        device: Path::new("/dev").join(&process.tty),
        client_id: Some(format!("pid-{}", process.pid)),
        workspace_id: None,
        pane_id: None,
        cwd: None,
        remote: None,
    })
}

pub fn sample_ttys(
    endpoints: impl Iterator<Item = TtyEndpoint>,
    observed_monotonic_ns: i64,
) -> Vec<TtySample> {
    endpoints
        .filter_map(|endpoint| {
            let metadata = fs::metadata(&endpoint.device).ok()?;
            Some(TtySample {
                endpoint,
                observed_monotonic_ns,
                access_time_ns: metadata
                    .atime()
                    .saturating_mul(1_000_000_000)
                    .saturating_add(metadata.atime_nsec()),
                modification_time_ns: metadata
                    .mtime()
                    .saturating_mul(1_000_000_000)
                    .saturating_add(metadata.mtime_nsec()),
            })
        })
        .collect()
}

#[derive(Default)]
pub struct AccessChangeTracker {
    access_times: HashMap<String, i64>,
}

impl AccessChangeTracker {
    pub fn changes(&mut self, samples: impl IntoIterator<Item = TtySample>) -> Vec<AccessChange> {
        let mut changes = Vec::new();
        for sample in samples {
            let previous = self
                .access_times
                .insert(sample.endpoint.endpoint_id.clone(), sample.access_time_ns);
            if let Some(previous) = previous.filter(|previous| sample.access_time_ns > *previous) {
                changes.push(AccessChange {
                    endpoint: sample.endpoint,
                    observed_monotonic_ns: sample.observed_monotonic_ns,
                    previous_access_time_ns: previous,
                    access_time_ns: sample.access_time_ns,
                });
            }
        }
        changes
    }

    pub fn retain(&mut self, endpoint_ids: &std::collections::HashSet<String>) {
        self.access_times
            .retain(|endpoint, _| endpoint_ids.contains(endpoint));
    }
}

pub fn monotonic_ns() -> i64 {
    static START: OnceLock<Instant> = OnceLock::new();
    i64::try_from(START.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(i64::MAX)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityReport {
    pub capture_supported: bool,
    pub client_count: usize,
    pub pane_count: usize,
    pub diagnostic: String,
}

pub fn capability_report(topology: &PtyTopology) -> CapabilityReport {
    let client_count = topology.clients.len();
    let pane_count = topology.panes.len();
    let diagnostic = if client_count == 0 {
        "capture is disabled: no local Herdr client PTY was resolved".to_owned()
    } else if pane_count == 0 {
        "capture is disabled: no local pane PTY was resolved; native remote panes require the optional remote metadata sampler".to_owned()
    } else {
        format!(
            "human input capture is supported for {client_count} client PTY(s) and {pane_count} local pane PTY(s)"
        )
    };
    CapabilityReport {
        capture_supported: client_count > 0 && pane_count > 0,
        client_count,
        pane_count,
        diagnostic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_process_rows() {
        let values = parse_process_records(" 12 1 ttys001 /usr/bin/herdr herdr\ninvalid\n");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].pid, 12);
    }

    #[test]
    fn capability_fails_closed() {
        let report = capability_report(&PtyTopology::default());
        assert!(!report.capture_supported);
    }
}
