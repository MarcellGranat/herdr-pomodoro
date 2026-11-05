mod activity;
mod attribution;
mod config;
mod correlation;
mod dashboard;
mod database;
mod discovery;
mod herdr;
mod menu;
mod model;
mod pomodoro;
mod service;
mod watcher;

#[cfg(test)]
mod live_measurements;

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use clap::{Args, Parser, Subcommand};
use serde_json::Value;
use std::env;
use std::io::{self, Read};

use crate::config::{PluginConfig, PluginPaths};
use crate::dashboard::{DashboardFilters, run_dashboard};
use crate::database::Database;
use crate::discovery::{capability_report, discover_topology};
use crate::herdr::{focus_workspace, focused_context, list_other_workspaces, open_dashboard};
use crate::menu::{MenuAction, run_menu, run_workspace_picker};
use crate::pomodoro::{Phase, PomodoroEngine, TimerState};

#[derive(Debug, Parser)]
#[command(name = "herdr-pomodoro", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    AgentEvent,
    Away,
    Dashboard(DashboardArgs),
    End,
    Menu,
    OpenDashboard,
    OpenMenu,
    Reconcile,
    Start,
    Status,
    Watch(WatchArgs),
    WorkspaceGrace,
}

#[derive(Clone, Debug, Args)]
struct DashboardArgs {
    #[arg(long)]
    date: Option<NaiveDate>,
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long)]
    machine: Option<String>,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    category: Option<String>,
    #[arg(long)]
    revision: Option<String>,
}

#[derive(Clone, Copy, Debug, Args)]
struct WatchArgs {
    #[arg(long)]
    duration: Option<f64>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Herdr Pomodoro: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Status => {
            let report = capability_report(&discover_topology()?);
            println!("Herdr Pomodoro: {}", report.diagnostic);
        }
        Commands::Reconcile => reconcile()?,
        Commands::Watch(arguments) => {
            let (paths, config) = runtime()?;
            watcher::run(
                &paths,
                &config,
                arguments.duration,
                arguments.duration.is_some(),
            )?;
        }
        Commands::Start => {
            let engine = engine()?;
            let context = focused_context()?;
            print_state(&engine.start_focus(&context.workspace_id, None)?);
        }
        Commands::Away => print_state(&engine()?.start_grace(Phase::AwayGrace)?),
        Commands::WorkspaceGrace => print_state(&engine()?.start_grace(Phase::WorkspaceGrace)?),
        Commands::End => {
            let engine = engine()?;
            let start_break = engine_config()?.start_break_after_early_end;
            print_state(&engine.end_session("ended_early", start_break)?);
        }
        Commands::AgentEvent => {
            let payload = event_payload()?;
            let status = payload
                .get("agent_status")
                .or_else(|| payload.get("status"))
                .and_then(Value::as_str);
            let source = payload
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("herdr-agent");
            print_state(&engine()?.set_agent_protection(status == Some("working"), source)?);
        }
        Commands::OpenDashboard => {
            if !open_dashboard() {
                bail!("could not open dashboard pane");
            }
        }
        Commands::OpenMenu => {
            if !crate::herdr::open_menu() {
                bail!("could not open Pomodoro command popup");
            }
        }
        Commands::Menu => run_menu_command()?,
        Commands::Dashboard(arguments) => {
            let paths = PluginPaths::discover()?;
            let database = Database::open(paths.database_file())?;
            let mut filters = DashboardFilters::for_day(
                arguments.date.unwrap_or_else(|| Local::now().date_naive()),
            )?;
            filters.project_id = arguments.project;
            filters.workspace_id = arguments.workspace;
            filters.machine_id = arguments.machine;
            filters.session_id = arguments.session;
            filters.category = arguments.category;
            filters.revision_id = arguments.revision;
            run_dashboard(database, filters)?;
        }
    }
    Ok(())
}

fn run_menu_command() -> Result<()> {
    let engine = engine()?;
    let state = engine.state(true)?;
    match run_menu(state.phase)? {
        Some(MenuAction::Start) => {
            let context = focused_context()?;
            print_state(&engine.start_focus(&context.workspace_id, None)?);
        }
        Some(MenuAction::Dashboard) => {
            let paths = PluginPaths::discover()?;
            run_dashboard(
                Database::open(paths.database_file())?,
                DashboardFilters::for_day(Local::now().date_naive())?,
            )?;
        }
        Some(MenuAction::WorkspaceGrace) => {
            let state = engine.start_grace(Phase::WorkspaceGrace)?;
            print_state(&state);
            pick_other_workspace(&state)?;
        }
        Some(MenuAction::AwayGrace) => {
            print_state(&engine.start_grace(Phase::AwayGrace)?);
        }
        Some(MenuAction::ReturnToFocus) => {
            print_state(&engine.return_to_focus()?);
        }
        Some(MenuAction::StopSession) => {
            print_state(&engine.end_session("stopped_from_menu", false)?);
        }
        Some(MenuAction::PickWorkspace) => {
            pick_other_workspace(&state)?;
        }
        None => {}
    }
    Ok(())
}

fn pick_other_workspace(state: &TimerState) -> Result<()> {
    let focus_workspace_id = state
        .workspace_id
        .as_deref()
        .context("workspace grace has no focus workspace")?;
    let workspaces = list_other_workspaces(focus_workspace_id)?;
    if let Some(workspace_id) = run_workspace_picker(&workspaces)? {
        focus_workspace(&workspace_id)?;
    }
    Ok(())
}

fn runtime() -> Result<(PluginPaths, PluginConfig)> {
    let paths = PluginPaths::discover()?;
    let config = PluginConfig::load(&paths)?;
    Ok((paths, config))
}

fn engine_config() -> Result<PluginConfig> {
    let paths = PluginPaths::discover()?;
    PluginConfig::load(&paths)
}

fn engine() -> Result<PomodoroEngine> {
    let (paths, config) = runtime()?;
    Ok(PomodoroEngine::new(
        Database::open(paths.database_file())?,
        config,
    ))
}

fn reconcile() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let paths = PluginPaths::discover()?;
        let plist = service::reconcile_macos_service(&paths)?;
        println!(
            "Herdr Pomodoro: watcher service reconciled at {}.",
            plist.display()
        );
    }
    #[cfg(not(target_os = "macos"))]
    println!("Herdr Pomodoro: automatic watcher service currently requires macOS.");
    Ok(())
}

fn print_state(state: &TimerState) {
    let degraded = if state.presentation_degraded {
        " (presentation limited)"
    } else {
        ""
    };
    println!(
        "Herdr Pomodoro: {} {:02}:{:02}{degraded}",
        state.phase.as_str(),
        state.remaining_seconds / 60,
        state.remaining_seconds % 60
    );
}

fn event_payload() -> Result<Value> {
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    if raw.trim().is_empty() {
        raw = env::var("HERDR_PLUGIN_EVENT_JSON").unwrap_or_default();
    }
    if raw.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let payload: Value = serde_json::from_str(&raw).context("agent event must be valid JSON")?;
    if !payload.is_object() {
        bail!("agent event must be a JSON object");
    }
    Ok(payload)
}
