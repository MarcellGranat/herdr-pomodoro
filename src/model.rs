use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::attribution::RevisionMetadata;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointRole {
    Client,
    Pane,
}

/// Repository and revision facts a remote sampler resolved on its own host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoteContext {
    pub machine_label: String,
    pub runtime_workspace_id: String,
    pub workspace_path: PathBuf,
    pub repository_root: PathBuf,
    pub repository_identity: String,
    pub revision: RevisionMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TtyEndpoint {
    pub endpoint_id: String,
    pub role: EndpointRole,
    pub pid: u32,
    pub device: PathBuf,
    pub client_id: Option<String>,
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub remote: Option<RemoteContext>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PtyTopology {
    pub clients: Vec<TtyEndpoint>,
    pub panes: Vec<TtyEndpoint>,
}

impl PtyTopology {
    pub fn endpoints(&self) -> impl Iterator<Item = &TtyEndpoint> {
        self.clients.iter().chain(&self.panes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TtySample {
    pub endpoint: TtyEndpoint,
    pub observed_monotonic_ns: i64,
    pub access_time_ns: i64,
    pub modification_time_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessChange {
    pub endpoint: TtyEndpoint,
    pub observed_monotonic_ns: i64,
    pub previous_access_time_ns: i64,
    pub access_time_ns: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HumanInputObservation {
    pub observation_id: String,
    pub observed_monotonic_ns: i64,
    pub input_time_ns: i64,
    pub correlation_delta_ns: i64,
    pub client_endpoint_id: String,
    pub pane_endpoint_id: String,
    pub workspace_id: String,
    pub pane_id: String,
    pub cwd: Option<PathBuf>,
    pub provenance: String,
    #[serde(default)]
    pub remote: Option<RemoteContext>,
}
