use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::attribution::RevisionMetadata;
use crate::herdr::run_herdr;
use crate::model::{AccessChange, EndpointRole, RemoteContext, TtyEndpoint};

const SAMPLER_SCRIPT: &str = include_str!("../remote/sampler.py");
const RECONNECT_DELAY: Duration = Duration::from_secs(15);
const SSH_OPTIONS: [&str; 8] = [
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=8",
    "-o",
    "ServerAliveInterval=10",
    "-o",
    "ServerAliveCountMax=3",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteMachine {
    pub label: String,
    pub target: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RemotePane {
    pub pane_id: String,
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub device: PathBuf,
    pub repo_root: PathBuf,
    pub repo_identity: String,
    pub revision: RevisionMetadata,
}

#[derive(Debug)]
enum RemoteEvent {
    Topology(Vec<RemotePane>),
    Access { pane_id: String, ns: i64 },
}

struct RemoteMessage {
    machine: String,
    event: RemoteEvent,
}

/// Streams metadata-only input events from remote Herdr hosts over SSH.
pub struct RemoteSamplers {
    sender: Sender<RemoteMessage>,
    receiver: Receiver<RemoteMessage>,
    running: HashSet<String>,
    panes: HashMap<String, HashMap<String, RemotePane>>,
}

impl RemoteSamplers {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            running: HashSet::new(),
            panes: HashMap::new(),
        }
    }

    /// Starts a sampler thread for every enabled saved machine not yet running.
    pub fn sync_machines(&mut self) {
        let Ok(machines) = list_machines() else {
            return;
        };
        for machine in machines {
            if self.running.insert(machine.label.clone()) {
                let sender = self.sender.clone();
                thread::spawn(move || run_sampler(&machine, &sender));
            }
        }
    }

    pub fn endpoint_ids(&self) -> HashSet<String> {
        self.panes
            .iter()
            .flat_map(|(machine, panes)| {
                panes
                    .keys()
                    .map(move |pane_id| endpoint_id(machine, pane_id))
            })
            .collect()
    }

    /// Applies queued messages and returns input events as access changes.
    pub fn drain(&mut self, observed_monotonic_ns: i64) -> Vec<AccessChange> {
        let mut changes = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            match message.event {
                RemoteEvent::Topology(panes) => {
                    let known = panes
                        .into_iter()
                        .map(|pane| (pane.pane_id.clone(), pane))
                        .collect();
                    self.panes.insert(message.machine, known);
                }
                RemoteEvent::Access { pane_id, ns } => {
                    let Some(pane) = self
                        .panes
                        .get(&message.machine)
                        .and_then(|panes| panes.get(&pane_id))
                    else {
                        continue;
                    };
                    changes.push(AccessChange {
                        endpoint: endpoint(&message.machine, pane),
                        observed_monotonic_ns,
                        previous_access_time_ns: ns - 1,
                        access_time_ns: ns,
                    });
                }
            }
        }
        changes
    }
}

fn endpoint_id(machine: &str, pane_id: &str) -> String {
    format!("remote:{machine}:{pane_id}")
}

fn endpoint(machine: &str, pane: &RemotePane) -> TtyEndpoint {
    TtyEndpoint {
        endpoint_id: endpoint_id(machine, &pane.pane_id),
        role: EndpointRole::Pane,
        pid: 0,
        device: pane.device.clone(),
        client_id: None,
        workspace_id: Some(format!("{machine}/{}", pane.workspace_id)),
        pane_id: Some(format!("{machine}/{}", pane.pane_id)),
        cwd: Some(pane.cwd.clone()),
        remote: Some(RemoteContext {
            machine_label: machine.to_owned(),
            runtime_workspace_id: pane.workspace_id.clone(),
            workspace_path: pane.cwd.clone(),
            repository_root: pane.repo_root.clone(),
            repository_identity: pane.repo_identity.clone(),
            revision: pane.revision.clone(),
        }),
    }
}

fn list_machines() -> Result<Vec<RemoteMachine>> {
    let result = run_herdr(&["machine", "list", "--json"]);
    if !result.success {
        anyhow::bail!("could not list Herdr machines: {}", result.stderr.trim());
    }
    parse_machines(&result.stdout)
}

fn parse_machines(raw: &str) -> Result<Vec<RemoteMachine>> {
    let payload: Value = serde_json::from_str(raw).context("machine list is not JSON")?;
    Ok(payload
        .as_array()
        .context("machine list must be an array")?
        .iter()
        .filter(|machine| machine.get("enabled").and_then(Value::as_bool) == Some(true))
        .filter_map(|machine| {
            Some(RemoteMachine {
                label: machine.get("label")?.as_str()?.to_owned(),
                target: machine.get("target")?.as_str()?.to_owned(),
            })
        })
        .collect())
}

fn parse_line(line: &str) -> Option<RemoteEvent> {
    let value: Value = serde_json::from_str(line).ok()?;
    match value.get("type")?.as_str()? {
        "topology" => {
            let panes = serde_json::from_value(value.get("panes")?.clone()).ok()?;
            Some(RemoteEvent::Topology(panes))
        }
        "access" => Some(RemoteEvent::Access {
            pane_id: value.get("pane_id")?.as_str()?.to_owned(),
            ns: value.get("ns")?.as_i64()?,
        }),
        _ => None,
    }
}

fn run_sampler(machine: &RemoteMachine, sender: &Sender<RemoteMessage>) {
    loop {
        if stream_once(machine, sender).is_err() {
            return;
        }
        thread::sleep(RECONNECT_DELAY);
    }
}

/// Runs one SSH session. `Err` means the receiver is gone and the thread should stop.
fn stream_once(machine: &RemoteMachine, sender: &Sender<RemoteMessage>) -> Result<(), ()> {
    let Ok(mut child) = Command::new("ssh")
        .args(SSH_OPTIONS)
        .arg("-T")
        .arg(&machine.target)
        .args(["python3", "-u", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(());
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(SAMPLER_SCRIPT.as_bytes());
    }
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if let Some(event) = parse_line(&line) {
                let message = RemoteMessage {
                    machine: machine.label.clone(),
                    event,
                };
                if sender.send(message).is_err() {
                    let _ = child.kill();
                    return Err(());
                }
            }
        }
    }
    let _ = child.wait();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enabled_machines_only() {
        let raw = r#"[{"id":"a","label":"m1","target":"m1","enabled":true},{"id":"b","label":"m2","target":"m2","enabled":false}]"#;
        let machines = parse_machines(raw).unwrap();
        assert_eq!(
            machines,
            vec![RemoteMachine {
                label: "m1".into(),
                target: "m1".into()
            }]
        );
    }

    #[test]
    fn access_events_become_remote_changes_after_topology() {
        let topology = r#"{"type":"topology","panes":[{"pane_id":"w1:p1","workspace_id":"w1","cwd":"/home/x/proj","device":"/dev/pts/1","repo_root":"/home/x/proj","repo_identity":"/home/x/proj/.git","revision":{"kind":"git","stable_id":"abc","commit_id":"abc","branch":"main","bookmarks":null,"description":"Fix"}}]}"#;
        let mut samplers = RemoteSamplers::new();
        for line in [topology, r#"{"type":"access","pane_id":"w1:p1","ns":1000}"#] {
            samplers
                .sender
                .send(RemoteMessage {
                    machine: "m1".into(),
                    event: parse_line(line).unwrap(),
                })
                .unwrap();
        }
        let changes = samplers.drain(5);
        assert_eq!(changes.len(), 1);
        let endpoint = &changes[0].endpoint;
        assert_eq!(endpoint.workspace_id.as_deref(), Some("m1/w1"));
        assert_eq!(endpoint.remote.as_ref().unwrap().machine_label, "m1");
        assert_eq!(changes[0].access_time_ns, 1000);
    }
}
