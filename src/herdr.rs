use std::env;
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::discovery::CommandResult;

pub const PLUGIN_ID: &str = "chmarax.herdr-pomodoro";
pub const PRESENTATION_SOURCE: &str = "plugin:chmarax.herdr-pomodoro";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusContext {
    pub workspace_id: String,
    pub pane_id: String,
}

pub fn run_herdr(arguments: &[&str]) -> CommandResult {
    let executable = env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_owned());
    match Command::new(executable).args(arguments).output() {
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

pub fn focused_context() -> Result<FocusContext> {
    let result = run_herdr(&["api", "snapshot"]);
    if !result.success {
        bail!("{}", result.stderr.trim());
    }
    let payload: Value =
        serde_json::from_str(&result.stdout).context("Herdr snapshot returned invalid JSON")?;
    let snapshot = payload
        .pointer("/result/snapshot")
        .context("Herdr snapshot has no result")?;
    let workspace_id = snapshot
        .get("focused_workspace_id")
        .and_then(Value::as_str)
        .context("Herdr snapshot has no focused workspace")?;
    let pane_id = snapshot
        .get("focused_pane_id")
        .and_then(Value::as_str)
        .context("Herdr snapshot has no focused pane")?;
    Ok(FocusContext {
        workspace_id: workspace_id.to_owned(),
        pane_id: pane_id.to_owned(),
    })
}

pub fn open_dashboard() -> bool {
    run_herdr(&[
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        "dashboard",
    ])
    .success
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HerdrPresentation;

impl HerdrPresentation {
    pub fn reduce(&self, workspace_id: &str) -> bool {
        set_agent_view(workspace_id).is_ok()
    }

    pub fn restore(&self, workspace_id: &str) -> Result<()> {
        let view = self.clear_agent_view();
        let metadata = clear_pomodoro(workspace_id);
        view.and(metadata)
    }

    pub fn clear_agent_view(&self) -> Result<()> {
        clear_agent_view().map(|_| ())
    }

    pub fn publish(&self, workspace_id: &str, value: &str) -> Result<()> {
        let token = format!("pomodoro={value}");
        let result = run_herdr(&[
            "workspace",
            "report-metadata",
            workspace_id,
            "--source",
            PRESENTATION_SOURCE,
            "--token",
            &token,
            "--ttl-ms",
            "5000",
        ]);
        if result.success {
            Ok(())
        } else {
            bail!(
                "could not publish Pomodoro metadata: {}",
                result.stderr.trim()
            )
        }
    }
}

fn clear_pomodoro(workspace_id: &str) -> Result<()> {
    let result = run_herdr(&[
        "workspace",
        "report-metadata",
        workspace_id,
        "--source",
        PRESENTATION_SOURCE,
        "--clear-token",
        "pomodoro",
    ]);
    if result.success {
        Ok(())
    } else {
        bail!(
            "could not clear Pomodoro metadata: {}",
            result.stderr.trim()
        )
    }
}

fn set_agent_view(workspace_id: &str) -> Result<Value> {
    socket_request(json!({
        "id": "herdr-pomodoro-agent-view",
        "method": "agent.view.set",
        "params": {
            "source": PRESENTATION_SOURCE,
            "label": "Pomodoro focus",
            "filter": {"op": "eq", "field": "workspace_id", "value": workspace_id}
        }
    }))
}

fn clear_agent_view() -> Result<Value> {
    socket_request(json!({
        "id": "herdr-pomodoro-agent-view-clear",
        "method": "agent.view.clear",
        "params": {"source": PRESENTATION_SOURCE}
    }))
}

#[cfg(unix)]
fn socket_request(request: Value) -> Result<Value> {
    let path = env::var("HERDR_SOCKET_PATH").context("HERDR_SOCKET_PATH is not available")?;
    let mut stream = UnixStream::connect(path).context("connect to Herdr socket")?;
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    let value: Value =
        serde_json::from_str(&response).context("Herdr socket returned invalid JSON")?;
    if let Some(error) = value.get("error") {
        bail!("Herdr API error: {error}");
    }
    Ok(value)
}

#[cfg(not(unix))]
fn socket_request(_request: Value) -> Result<Value> {
    bail!("Herdr Agent view requires a Unix socket on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_identity_is_stable() {
        assert_eq!(PRESENTATION_SOURCE, format!("plugin:{PLUGIN_ID}"));
    }
}
