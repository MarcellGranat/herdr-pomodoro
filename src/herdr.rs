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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkingAgent {
    pub pane_id: String,
    pub workspace_id: String,
    pub cwd: std::path::PathBuf,
}

/// Local agents whose status is currently `working`.
pub fn list_working_agents() -> Result<Vec<WorkingAgent>> {
    let result = run_herdr(&["agent", "list"]);
    if !result.success {
        bail!("could not list Herdr agents: {}", result.stderr.trim());
    }
    parse_working_agents(&result.stdout)
}

fn parse_working_agents(raw: &str) -> Result<Vec<WorkingAgent>> {
    let payload: Value = serde_json::from_str(raw).context("agent list is not JSON")?;
    let agents = payload
        .pointer("/result/agents")
        .and_then(Value::as_array)
        .context("agent list has no agents")?;
    Ok(agents
        .iter()
        .filter(|agent| agent.get("agent_status").and_then(Value::as_str) == Some("working"))
        .filter_map(|agent| {
            let pane_id = agent.get("pane_id")?.as_str()?;
            let cwd = agent
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())?;
            Some(WorkingAgent {
                pane_id: pane_id.to_owned(),
                workspace_id: pane_id.split(':').next()?.to_owned(),
                cwd: std::path::PathBuf::from(cwd),
            })
        })
        .collect())
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
    play_system_sound(sound);
    if result.success {
        Ok(())
    } else {
        bail!(
            "could not show Pomodoro notification: {}",
            result.stderr.trim()
        )
    }
}

/// Plays a macOS system sound directly, because Herdr's own notification sound
/// is not reliably audible with terminal toast delivery.
fn play_system_sound(sound: &str) {
    #[cfg(target_os = "macos")]
    {
        let file = match sound {
            "request" => "/System/Library/Sounds/Sosumi.aiff",
            "done" => "/System/Library/Sounds/Glass.aiff",
            _ => return,
        };
        let _ = std::process::Command::new("afplay")
            .arg(file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = sound;
}

pub const SIDEBAR_LOCK_MARKER: &str = "# pomodoro-sidebar-lock";
const HERDR_CONFIG_ENV: &str = "HERDR_CONFIG_PATH";
const UNLOCKED_PREFIX: &str = "# ";

/// Returns the config text with the marked sidebar-lock line enabled or
/// commented out, or `None` when no marked line exists or nothing changes.
pub fn apply_sidebar_lock(text: &str, locked: bool) -> Option<String> {
    let mut changed = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            if !line.contains(SIDEBAR_LOCK_MARKER) {
                return line.to_owned();
            }
            let commented = line.trim_start().starts_with('#');
            let updated = match (locked, commented) {
                (true, true) => line
                    .trim_start()
                    .trim_start_matches('#')
                    .trim_start()
                    .to_owned(),
                (false, false) => format!("{UNLOCKED_PREFIX}{line}"),
                _ => return line.to_owned(),
            };
            changed = true;
            updated
        })
        .collect();
    if !changed {
        return None;
    }
    let mut result = lines.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

/// Disables Herdr's sidebar toggle while a Pomodoro session is active and
/// restores it when idle, reloading the running server only on change.
pub fn sync_sidebar_lock(locked: bool) -> Result<bool> {
    if cfg!(test) {
        return Ok(false);
    }
    let path = match env::var_os(HERDR_CONFIG_ENV) {
        Some(path) => std::path::PathBuf::from(path),
        None => std::path::PathBuf::from(env::var_os("HOME").context("HOME is not set")?)
            .join(".config/herdr/config.toml"),
    };
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let Some(updated) = apply_sidebar_lock(&text, locked) else {
        return Ok(false);
    };
    std::fs::write(&path, updated)
        .with_context(|| format!("could not write {}", path.display()))?;
    let result = run_herdr(&["server", "reload-config"]);
    if !result.success {
        let _ = std::fs::write(&path, text);
        bail!("could not reload Herdr config: {}", result.stderr.trim());
    }
    Ok(true)
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
    #[test]
    fn working_agents_are_filtered_by_status() {
        let raw = r#"{"result":{"agents":[{"pane_id":"w1:p1","agent_status":"working","cwd":"/a"},{"pane_id":"w2:p1","agent_status":"idle","cwd":"/b"}]}}"#;
        let agents = super::parse_working_agents(raw).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].workspace_id, "w1");
    }

    #[test]
    fn sidebar_lock_toggles_marked_line_only() {
        use super::apply_sidebar_lock;
        let locked = "[keys]\ntoggle_sidebar = \"ctrl+b\" # pomodoro-sidebar-lock\nother = 1\n";
        let unlocked = apply_sidebar_lock(locked, false).unwrap();
        assert_eq!(
            unlocked,
            "[keys]\n# toggle_sidebar = \"ctrl+b\" # pomodoro-sidebar-lock\nother = 1\n"
        );
        assert_eq!(apply_sidebar_lock(&unlocked, false), None);
        assert_eq!(apply_sidebar_lock(&unlocked, true).unwrap(), locked);
        assert_eq!(apply_sidebar_lock("other = 1\n", true), None);
    }

    use super::*;

    #[test]
    fn plugin_identity_is_stable() {
        assert_eq!(PRESENTATION_SOURCE, format!("plugin:{PLUGIN_ID}"));
    }
}
