use std::collections::HashSet;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::activity::ActivityEngine;
use crate::config::{PluginConfig, PluginPaths};
use crate::correlation::CorrelationBuffer;
use crate::database::Database;
use crate::discovery::{AccessChangeTracker, discover_topology, monotonic_ns, sample_ttys};
use crate::model::{HumanInputObservation, PtyTopology};
use crate::pomodoro::PomodoroEngine;
use crate::service::WatcherLock;

pub struct ObservationWatcher {
    topology: Option<PtyTopology>,
    next_refresh_ns: i64,
    refresh_ns: i64,
    tracker: AccessChangeTracker,
    correlator: CorrelationBuffer,
}

impl ObservationWatcher {
    pub fn new(correlation_window_ns: i64) -> Self {
        Self {
            topology: None,
            next_refresh_ns: 0,
            refresh_ns: 60_000_000_000,
            tracker: AccessChangeTracker::default(),
            correlator: CorrelationBuffer::new(correlation_window_ns),
        }
    }

    pub fn poll(&mut self, observed_ns: i64) -> Result<Vec<HumanInputObservation>> {
        if self.topology.is_none() || observed_ns >= self.next_refresh_ns {
            let topology = discover_topology()?;
            let ids: HashSet<String> = topology
                .endpoints()
                .map(|endpoint| endpoint.endpoint_id.clone())
                .collect();
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
        let changes = self.tracker.changes(samples);
        Ok(self.correlator.ready(changes, observed_ns))
    }
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
    let pomodoro = PomodoroEngine::new(database, config.clone());
    let initial_state = pomodoro.state(true)?;
    let _ = pomodoro.sync_agent_view(&initial_state);
    let mut watcher = ObservationWatcher::new(
        i64::try_from(config.correlation_window_milliseconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1_000_000),
    );
    let started = Instant::now();
    let duration = duration_seconds.map(Duration::from_secs_f64);
    let sleep = Duration::from_millis(config.sample_interval_milliseconds);
    let mut next_presentation = Instant::now();
    loop {
        if duration.is_some_and(|value| started.elapsed() >= value) {
            break;
        }
        let observed = monotonic_ns();
        for observation in watcher.poll(observed)? {
            activity.ingest(&observation)?;
            pomodoro.on_human_input(&observation.workspace_id)?;
            if emit_json {
                println!("{}", serde_json::to_string(&observation)?);
                io::stdout().flush()?;
            }
        }
        if Instant::now() >= next_presentation {
            let state = pomodoro.state(true)?;
            let _ = pomodoro.notify_due(&state);
            let _ = pomodoro.deliver_pending_notifications();
            let _ = pomodoro.publish(&state);
            next_presentation = Instant::now() + Duration::from_secs(1);
        }
        thread::sleep(sleep);
    }
    Ok(())
}
