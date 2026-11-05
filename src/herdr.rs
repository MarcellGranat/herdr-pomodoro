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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceChoice {
    pub workspace_id: String,
    pub number: u64,
    pub label: String,
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

pub fn open_menu() -> bool {
    run_herdr(&[
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        "menu",
    ])
    .success
}

pub fn list_other_workspaces(focus_workspace_id: &str) -> Result<Vec<WorkspaceChoice>> {
    let result = run_herdr(&["workspace", "list"]);
    if !result.success {
        bail!("could not list Herdr workspaces: {}", result.stderr.trim());
    }
    parse_workspace_choices(&result.stdout, focus_workspace_id)
}

pub fn focus_workspace(workspace_id: &str) -> Result<()> {
    let result = run_herdr(&["workspace", "focus", workspace_id]);
    if result.success {
        Ok(())
    } else {
        bail!("could not focus workspace: {}", result.stderr.trim())
    }
}

fn parse_workspace_choices(raw: &str, focus_workspace_id: &str) -> Result<Vec<WorkspaceChoice>> {
    let payload: Value =
        serde_json::from_str(raw).context("Herdr workspace list returned invalid JSON")?;
    let workspaces = payload
        .pointer("/result/workspaces")
        .and_then(Value::as_array)
        .context("Herdr workspace list has no workspaces")?;
    let mut choices = Vec::new();
    for workspace in workspaces {
        let workspace_id = workspace
            .get("workspace_id")
            .and_then(Value::as_str)
            .context("Herdr workspace has no id")?;
        if workspace_id == focus_workspace_id {
            continue;
        }
        choices.push(WorkspaceChoice {
            workspace_id: workspace_id.to_owned(),
            number: workspace
                .get("number")
                .and_then(Value::as_u64)
                .context("Herdr workspace has no number")?,
            label: workspace
                .get("label")
                .and_then(Value::as_str)
                .context("Herdr workspace has no label")?
                .to_owned(),
        });
    }
    choices.sort_by_key(|choice| choice.number);
    Ok(choices)
}

pub fn show_notification(title: &str, body: &str, sound: &str) -> Result<()> {
    let result = run_herdr(&[
        "notification",
        "show",
        title,
        "--body",
        body,
        "--position",
        "top-right",
        "--sound",
        sound,
    ]);
    if result.success {
        Ok(())
    } else {
        bail!(
            "could not show Pomodoro notification: {}",
            result.stderr.trim()
        )
    }
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

    pub fn show_all_agents(&self) -> bool {
        self.clear_agent_view().is_ok()
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

    #[test]
    fn workspace_choices_exclude_focus_and_follow_sidebar_order() {
        let raw = r#"{"result":{"type":"workspace_list","workspaces":[{"workspace_id":"w3","number":3,"label":"Gamma"},{"workspace_id":"w1","number":1,"label":"Focus"},{"workspace_id":"w2","number":2,"label":"Beta"}]}}"#;

        let choices = parse_workspace_choices(raw, "w1").unwrap();

        assert_eq!(
            choices,
            vec![
                WorkspaceChoice {
                    workspace_id: "w2".to_owned(),
                    number: 2,
                    label: "Beta".to_owned(),
                },
                WorkspaceChoice {
                    workspace_id: "w3".to_owned(),
                    number: 3,
                    label: "Gamma".to_owned(),
                },
            ]
        );
    }
}
