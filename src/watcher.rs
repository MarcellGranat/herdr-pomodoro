use std::collections::HashSet;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::activity::ActivityEngine;
use crate::agent::{AgentTracker, WorkingSample};
use crate::config::{PluginConfig, PluginPaths};
use crate::correlation::CorrelationBuffer;
use crate::database::Database;
use crate::discovery::{AccessChangeTracker, discover_topology, monotonic_ns, sample_ttys};
use crate::herdr::list_working_agents;
use crate::herdr::{InputRecorder, focused_context};
use crate::model::{HumanInputObservation, PtyTopology};
use crate::pomodoro::{Phase, PomodoroEngine, TimerState, grace_warning_due};
use crate::popup::{KEEP_SESSION_BUTTON, NEW_SESSION_BUTTON, PopupKind, Popups};
use crate::remote::{RemoteRequest, RemoteSamplers};
use crate::service::WatcherLock;

pub struct ObservationWatcher {
    topology: Option<PtyTopology>,
    next_refresh_ns: i64,
    refresh_ns: i64,
    tracker: AccessChangeTracker,
    correlator: CorrelationBuffer,
    remote: Option<RemoteSamplers>,
}

impl ObservationWatcher {
    pub fn new(correlation_window_ns: i64, remote_enabled: bool) -> Self {
        Self {
            topology: None,
            next_refresh_ns: 0,
            refresh_ns: 60_000_000_000,
            tracker: AccessChangeTracker::default(),
            correlator: CorrelationBuffer::new(correlation_window_ns),
            remote: remote_enabled.then(RemoteSamplers::new),
        }
    }

    /// Pushes the tab-bar text to every connected remote machine.
    pub fn set_bar(&self, label: &str, phase: &str) {
        if let Some(remote) = self.remote.as_ref() {
            remote.set_bar(label, phase);
        }
    }

    /// Actions queued by shims on remote machines.
    pub fn take_requests(&mut self) -> Vec<RemoteRequest> {
        self.remote
            .as_mut()
            .map(RemoteSamplers::take_requests)
            .unwrap_or_default()
    }

    /// Working agents on this machine and on every sampled remote machine.
    pub fn working_agents(&self) -> Vec<WorkingSample> {
        let mut samples: Vec<WorkingSample> = list_working_agents()
            .unwrap_or_default()
            .into_iter()
            .map(|agent| WorkingSample {
                key: format!("local:{}", agent.pane_id),
                workspace_id: agent.workspace_id,
                cwd: agent.cwd,
                remote: None,
            })
            .collect();
        if let Some(remote) = self.remote.as_ref() {
            samples.extend(remote.working_agents());
        }
        samples
    }

    pub fn poll(&mut self, observed_ns: i64) -> Result<Vec<HumanInputObservation>> {
        if self.topology.is_none() || observed_ns >= self.next_refresh_ns {
            let topology = discover_topology()?;
            if let Some(remote) = self.remote.as_mut() {
                remote.sync_machines();
            }
            let mut ids: HashSet<String> = topology
                .endpoints()
                .map(|endpoint| endpoint.endpoint_id.clone())
                .collect();
            if let Some(remote) = self.remote.as_ref() {
                ids.extend(remote.endpoint_ids());
            }
            self.tracker.retain(&ids);
            self.correlator.retain(&ids);
            self.topology = Some(topology);
            self.next_refresh_ns = observed_ns.saturating_add(self.refresh_ns);
        }
        let endpoints = self
            .topology
            .as_ref()
            .expect("topology initialized")
            .endpoints()
            .cloned();
        let samples = sample_ttys(endpoints, observed_ns);
        let mut changes = self.tracker.changes(samples);
        if let Some(remote) = self.remote.as_mut() {
            changes.extend(remote.drain(observed_ns));
        }
        Ok(self.correlator.ready(changes, observed_ns))
    }
}

fn popup_key(kind: PopupKind, state: &TimerState) -> String {
    format!(
        "{kind:?}:{}:{}",
        state.session_id.as_deref().unwrap_or_default(),
        state
            .started_at
            .map(|at| at.to_rfc3339())
            .unwrap_or_default()
    )
}

/// Opens the popups that fit the current phase and applies the answers.
fn drive_popups(pomodoro: &PomodoroEngine, popups: &mut Popups, state: &TimerState) {
    let mut wanted = vec![PopupKind::NewSession];
    match state.phase {
        Phase::AwaitBreak => {
            wanted.push(PopupKind::BreakTime);
            let key = popup_key(PopupKind::BreakTime, state);
            if !popups.show(PopupKind::BreakTime, &key, state.remaining_seconds) {
                // No way to ask: do not discard a finished focus session.
                let _ = pomodoro.confirm_break();
            }
        }
        Phase::AwayGrace | Phase::WorkspaceGrace => {
            wanted.push(PopupKind::KeepSession);
            if grace_warning_due(state) {
                let key = popup_key(PopupKind::KeepSession, state);
                popups.show(PopupKind::KeepSession, &key, state.remaining_seconds);
            }
        }
        Phase::Idle | Phase::Focus | Phase::Break => {}
    }
    if state.phase != Phase::Idle {
        wanted.retain(|kind| *kind != PopupKind::NewSession);
    }
    popups.cancel_unwanted(&wanted);
    for outcome in popups.poll() {
        let Some(button) = outcome.button else {
            continue;
        };
        let _ = match (outcome.kind, button.as_str()) {
            (PopupKind::BreakTime, _) => pomodoro.confirm_break().map(drop),
            (PopupKind::KeepSession, KEEP_SESSION_BUTTON) => pomodoro.return_to_focus().map(drop),
            (PopupKind::NewSession, NEW_SESSION_BUTTON) => focused_context()
                .and_then(|context| pomodoro.start_focus(&context.workspace_id, None))
                .map(drop),
            _ => Ok(()),
        };
    }
}

/// Applies a Pomodoro action requested from a remote workspace.
fn apply_request(pomodoro: &PomodoroEngine, config: &PluginConfig, request: &RemoteRequest) {
    let _ = match request.action.as_str() {
        "start" => pomodoro.start_focus(&request.workspace_id, None).map(drop),
        "end" => pomodoro
            .end_session("ended_early", config.start_break_after_early_end)
            .map(drop),
        "stop" => pomodoro.end_session("stopped_from_menu", false).map(drop),
        "return" => pomodoro.return_to_focus().map(drop),
        "break" => pomodoro.confirm_break().map(drop),
        "away" => pomodoro.start_grace(Phase::AwayGrace).map(drop),
        "workspace-grace" => pomodoro.start_grace(Phase::WorkspaceGrace).map(drop),
        _ => Ok(()),
    };
}

pub fn run(
    paths: &PluginPaths,
    config: &PluginConfig,
    duration_seconds: Option<f64>,
    emit_json: bool,
) -> Result<()> {
    paths.ensure()?;
    let _lock = WatcherLock::acquire(&paths.watcher_lock_file())?;
    let database = Database::open(paths.database_file())?;
    let mut activity = ActivityEngine::new(database.clone(), config.maximum_gap_seconds);
    let mut agents = AgentTracker::new(database.clone());
    let pomodoro = PomodoroEngine::new(database, config.clone());
    let initial_state = pomodoro.state(true)?;
    let _ = pomodoro.sync_agent_view(&initial_state);
    let mut watcher = ObservationWatcher::new(
        i64::try_from(config.correlation_window_milliseconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000_000),
        config.remote_sampling_enabled,
    );
    let started = Instant::now();
    let duration = duration_seconds.map(Duration::from_secs_f64);
    let sleep = Duration::from_millis(config.sample_interval_milliseconds);
    let mut popups = Popups::default();
    let mut recorder = InputRecorder::default();
    let mut next_presentation = Instant::now();
    loop {
        if duration.is_some_and(|value| started.elapsed() >= value) {
            break;
        }
        let observed = monotonic_ns();
        for observation in watcher.poll(observed)? {
            activity.ingest(&observation)?;
            recorder.record(&paths.state_directory, &observation.workspace_id);
            pomodoro.on_human_input(&observation.workspace_id)?;
            if emit_json {
                println!("{}", serde_json::to_string(&observation)?);
                io::stdout().flush()?;
            }
        }
        if Instant::now() >= next_presentation {
            let state = pomodoro.state(true)?;
            let _ = pomodoro.notify_due(&state);
            let _ = pomodoro.deliver_pending_notifications(&mut popups);
            drive_popups(&pomodoro, &mut popups, &state);
            let _ = pomodoro.publish(&state);
            watcher.set_bar(&state.bar_text(), state.phase.as_str());
            for request in watcher.take_requests() {
                apply_request(&pomodoro, config, &request);
            }
            let _ = agents.tick(&watcher.working_agents());
            next_presentation = Instant::now() + Duration::from_secs(1);
        }
        thread::sleep(sleep);
    }
    Ok(())
}
