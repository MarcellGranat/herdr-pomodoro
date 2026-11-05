use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::PluginConfig;
use crate::database::Database;
use crate::herdr::{HerdrPresentation, show_notification};

const WORKSPACE_LEFT_MARKER: &str = "left_focus_workspace";
const GRACE_WARNING_REMAINING_SECONDS: i64 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PhaseAlert {
    title: &'static str,
    body: &'static str,
    sound: &'static str,
}

fn alert_for_kind(kind: &str) -> Option<PhaseAlert> {
    match kind {
        "grace_warning_02_40" => Some(PhaseAlert {
            title: "Pomodoro grace break",
            body: "2 minutes 40 seconds remaining.",
            sound: "request",
        }),
        "focus_completed" => Some(PhaseAlert {
            title: "Focus complete",
            body: "Your five-minute break has started.",
            sound: "done",
        }),
        "break_completed" => Some(PhaseAlert {
            title: "Pomodoro break ended",
            body: "The five-minute pause is over.",
            sound: "none",
        }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Focus,
    AwayGrace,
    WorkspaceGrace,
    Break,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentViewMode {
    FocusWorkspace,
    All,
}

fn agent_view_mode(phase: Phase) -> AgentViewMode {
    match phase {
        Phase::Focus => AgentViewMode::FocusWorkspace,
        Phase::Idle | Phase::AwayGrace | Phase::WorkspaceGrace | Phase::Break => AgentViewMode::All,
    }
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Focus => "focus",
            Self::AwayGrace => "away_grace",
            Self::WorkspaceGrace => "workspace_grace",
            Self::Break => "break",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "idle" => Ok(Self::Idle),
            "focus" => Ok(Self::Focus),
            "away_grace" => Ok(Self::AwayGrace),
            "workspace_grace" => Ok(Self::WorkspaceGrace),
            "break" => Ok(Self::Break),
            _ => bail!("unknown Pomodoro phase {value}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimerState {
    pub phase: Phase,
    pub session_id: Option<String>,
    pub workspace_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
    pub remaining_seconds: i64,
    pub presentation_degraded: bool,
}

impl TimerState {
    pub fn idle() -> Self {
        Self {
            phase: Phase::Idle,
            session_id: None,
            workspace_id: None,
            started_at: None,
            deadline: None,
            remaining_seconds: 0,
            presentation_degraded: false,
        }
    }

    pub fn label(&self) -> String {
        let icon = match self.phase {
            Phase::Focus => "🍅",
            Phase::AwayGrace | Phase::WorkspaceGrace => "⚠",
            Phase::Break => "☕",
            Phase::Idle => "",
        };
        format!(
            "{icon} {} {:02}:{:02}",
            self.phase.as_str().replace('_', " "),
            self.remaining_seconds / 60,
            self.remaining_seconds % 60
        )
        .trim()
        .to_owned()
    }
}

#[derive(Clone)]
pub struct PomodoroEngine {
    database: Database,
    config: PluginConfig,
    presentation: HerdrPresentation,
}

impl PomodoroEngine {
    pub fn new(database: Database, config: PluginConfig) -> Self {
        Self {
            database,
            config,
            presentation: HerdrPresentation,
        }
    }

    pub fn start_focus(
        &self,
        workspace_id: &str,
        workspace_instance_id: Option<&str>,
    ) -> Result<TimerState> {
        let current = self.state(true)?;
        if current.phase != Phase::Idle {
            bail!(
                "a Pomodoro session is already active ({})",
                current.phase.as_str()
            );
        }
        let now = Utc::now();
        let session_id = Uuid::new_v4().to_string();
        let presentation_changed = self.presentation.reduce(workspace_id);
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("INSERT INTO focus_sessions(id, workspace_id, workspace_instance_id, started_at, status, presentation_state, presentation_degraded) VALUES(?1, ?2, ?3, ?4, 'active', ?5, ?6)", params![session_id, workspace_id, workspace_instance_id, now.to_rfc3339(), format!("{{\"workspace_id\":\"{workspace_id}\"}}"), i64::from(!presentation_changed)])?;
        insert_phase(
            &transaction,
            &session_id,
            Phase::Focus,
            "focus_elapsed",
            Some(workspace_id),
            now,
            now + Duration::seconds(self.config.focus_seconds),
            None,
        )?;
        transaction.commit()?;
        let state = self.state(false)?;
        let _ = self.publish(&state);
        Ok(state)
    }

    pub fn start_grace(&self, phase: Phase) -> Result<TimerState> {
        if !matches!(phase, Phase::AwayGrace | Phase::WorkspaceGrace) {
            bail!("invalid grace phase");
        }
        let state = self.state(true)?;
        let session = state
            .session_id
            .context("cannot start grace without an active focus session")?;
        let workspace = state
            .workspace_id
            .context("active session has no workspace")?;
        let seconds = if phase == Phase::AwayGrace {
            self.config.away_grace_seconds
        } else {
            self.config.workspace_grace_seconds
        };
        let now = Utc::now();
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        close_active_phase(&transaction, now, "grace_started")?;
        insert_phase(
            &transaction,
            &session,
            phase,
            "grace",
            Some(&workspace),
            now,
            now + Duration::seconds(seconds),
            None,
        )?;
        transaction.commit()?;
        let next = self.state(false)?;
        let _ = self.sync_agent_view(&next);
        let _ = self.publish(&next);
        Ok(next)
    }

    pub fn on_human_input(&self, workspace_id: &str) -> Result<TimerState> {
        let state = self.state(true)?;
        let in_focus_workspace = state.workspace_id.as_deref() == Some(workspace_id);
        match state.phase {
            Phase::Focus if !in_focus_workspace => {
                let next = self.start_grace(Phase::WorkspaceGrace)?;
                self.mark_workspace_left()?;
                Ok(next)
            }
            Phase::WorkspaceGrace if !in_focus_workspace => {
                self.mark_workspace_left()?;
                Ok(state)
            }
            Phase::WorkspaceGrace if in_focus_workspace && self.workspace_was_left()? => {
                self.resume_focus(state)
            }
            Phase::AwayGrace if in_focus_workspace => self.resume_focus(state),
            _ => Ok(state),
        }
    }

    fn mark_workspace_left(&self) -> Result<()> {
        let connection = self.database.connect()?;
        connection.execute(
            "UPDATE timed_phases SET protection_source=?1 WHERE ended_at IS NULL AND phase=?2",
            params![WORKSPACE_LEFT_MARKER, Phase::WorkspaceGrace.as_str()],
        )?;
        Ok(())
    }

    fn workspace_was_left(&self) -> Result<bool> {
        let connection = self.database.connect_readonly()?;
        let marker: Option<String> = connection
            .query_row(
                "SELECT protection_source FROM timed_phases WHERE ended_at IS NULL AND phase=?1 ORDER BY id DESC LIMIT 1",
                [Phase::WorkspaceGrace.as_str()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(marker.as_deref() == Some(WORKSPACE_LEFT_MARKER))
    }

    fn resume_focus(&self, state: TimerState) -> Result<TimerState> {
        let session = state.session_id.context("grace has no session")?;
        let workspace = state.workspace_id.context("grace has no workspace")?;
        let connection = self.database.connect_readonly()?;
        let raw: String = connection.query_row("SELECT deadline FROM timed_phases WHERE session_id=?1 AND phase='focus' ORDER BY id LIMIT 1", [&session], |row| row.get(0))?;
        drop(connection);
        let original = parse_time(&raw)?;
        let now = Utc::now();
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        close_active_phase(&transaction, now, "returned")?;
        insert_phase(
            &transaction,
            &session,
            Phase::Focus,
            "focus_elapsed",
            Some(&workspace),
            now,
            original.max(now),
            None,
        )?;
        transaction.commit()?;
        let next = self.state(true)?;
        let _ = self.sync_agent_view(&next);
        let _ = self.publish(&next);
        Ok(next)
    }

    pub fn return_to_focus(&self) -> Result<TimerState> {
        let state = self.state(true)?;
        if !matches!(state.phase, Phase::AwayGrace | Phase::WorkspaceGrace) {
            bail!("cannot return to focus outside a grace break");
        }
        self.resume_focus(state)
    }

    pub fn set_agent_protection(&self, active: bool, source: &str) -> Result<TimerState> {
        let state = self.state(true)?;
        let Some(session) = state.session_id.as_deref() else {
            return Ok(state);
        };
        let now = Utc::now();
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let protection: Option<(i64, String)> = transaction.query_row("SELECT id, started_at FROM timed_phases WHERE session_id=?1 AND phase='agent_supervision' AND ended_at IS NULL ORDER BY id DESC LIMIT 1", [session], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if active && protection.is_none() {
            insert_phase(
                &transaction,
                session,
                Phase::Focus,
                "agent_supervision",
                state.workspace_id.as_deref(),
                now,
                state.deadline.unwrap_or(now),
                Some(source),
            )?;
            transaction.execute(
                "UPDATE timed_phases SET phase='agent_supervision' WHERE id=last_insert_rowid()",
                [],
            )?;
        } else if !active && let Some((id, started)) = protection {
            close_protection(
                &transaction,
                id,
                parse_time(&started)?,
                now,
                session,
                source,
            )?;
        }
        transaction.commit()?;
        self.state(false)
    }

    pub fn end_session(&self, reason: &str, start_break: bool) -> Result<TimerState> {
        let state = self.state(false)?;
        let Some(session) = state.session_id.as_deref() else {
            return Ok(state);
        };
        let now = Utc::now();
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        close_all_protection(&transaction, session, now)?;
        if reason == "completed" && state.phase == Phase::Focus {
            queue_active_phase_notification(
                &transaction,
                session,
                Phase::Focus,
                "focus_completed",
                now,
            )?;
        }
        close_active_phase(&transaction, now, reason)?;
        let use_break = start_break && state.phase != Phase::Break;
        if use_break {
            insert_phase(
                &transaction,
                session,
                Phase::Break,
                "break",
                state.workspace_id.as_deref(),
                now,
                now + Duration::seconds(self.config.break_seconds),
                None,
            )?;
        } else {
            transaction.execute("UPDATE focus_sessions SET ended_at=?1, status='ended', termination_reason=?2 WHERE id=?3", params![now.to_rfc3339(), reason, session])?;
        }
        transaction.commit()?;
        if let Some(workspace) = state.workspace_id.as_deref() {
            let _ = self.presentation.restore(workspace);
        }
        let next = self.state(false)?;
        let _ = self.publish(&next);
        Ok(next)
    }

    pub fn state(&self, reconcile: bool) -> Result<TimerState> {
        if reconcile {
            self.reconcile_deadline()?;
        }
        let connection = self.database.connect_readonly()?;
        let row: Option<(String, String, String, String, String, i64)> = connection.query_row("SELECT p.phase, p.session_id, s.workspace_id, p.started_at, p.deadline, s.presentation_degraded FROM timed_phases p JOIN focus_sessions s ON s.id=p.session_id WHERE s.status='active' AND p.ended_at IS NULL AND p.phase!='agent_supervision' ORDER BY p.id DESC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))).optional()?;
        let Some((phase, session, workspace, started, deadline, degraded)) = row else {
            return Ok(TimerState::idle());
        };
        let started_at = parse_time(&started)?;
        let deadline = parse_time(&deadline)?;
        Ok(TimerState {
            phase: Phase::from_str(&phase)?,
            session_id: Some(session),
            workspace_id: Some(workspace),
            started_at: Some(started_at),
            deadline: Some(deadline),
            remaining_seconds: (deadline - Utc::now()).num_seconds().max(0),
            presentation_degraded: degraded != 0,
        })
    }

    pub fn publish(&self, state: &TimerState) -> Result<()> {
        let _ =
            crate::herdr::sync_sidebar_lock(matches!(state.phase, Phase::Focus | Phase::AwayGrace));
        if let Some(workspace) = state.workspace_id.as_deref() {
            if state.phase == Phase::Idle {
                self.presentation.restore(workspace)
            } else {
                self.presentation.publish(workspace, &state.label())
            }
        } else {
            Ok(())
        }
    }

    pub fn notify_due(&self, state: &TimerState) -> Result<()> {
        if grace_warning_due(state) {
            self.queue_phase_notification(state, "grace_warning_02_40")?;
        }
        Ok(())
    }

    pub fn deliver_pending_notifications(&self) -> Result<usize> {
        self.deliver_pending_with(|alert| show_notification(alert.title, alert.body, alert.sound))
    }

    pub fn sync_agent_view(&self, state: &TimerState) -> bool {
        match agent_view_mode(state.phase) {
            AgentViewMode::FocusWorkspace => state
                .workspace_id
                .as_deref()
                .is_some_and(|workspace| self.presentation.reduce(workspace)),
            AgentViewMode::All => self.presentation.show_all_agents(),
        }
    }

    fn reconcile_deadline(&self) -> Result<()> {
        let state = self.state(false)?;
        if state.phase == Phase::Idle
            || state.deadline.is_some_and(|deadline| deadline > Utc::now())
        {
            return Ok(());
        }
        match state.phase {
            Phase::Focus => {
                self.end_session("completed", true)?;
            }
            Phase::AwayGrace | Phase::WorkspaceGrace => {
                self.end_session("grace_expired", false)?;
            }
            Phase::Break => {
                self.complete_break(&state)?;
            }
            Phase::Idle => {}
        }
        Ok(())
    }

    fn complete_break(&self, state: &TimerState) -> Result<()> {
        let Some(session) = state.session_id.as_deref() else {
            return Ok(());
        };
        let now = Utc::now();
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        queue_active_phase_notification(
            &transaction,
            session,
            Phase::Break,
            "break_completed",
            now,
        )?;
        close_active_phase(&transaction, now, "completed")?;
        transaction.execute("UPDATE focus_sessions SET ended_at=?1, status='completed', termination_reason=COALESCE(termination_reason, 'completed') WHERE id=?2", params![now.to_rfc3339(), session])?;
        transaction.commit()?;
        if let Some(workspace) = state.workspace_id.as_deref() {
            let _ = self.presentation.restore(workspace);
        }
        Ok(())
    }

    fn queue_phase_notification(&self, state: &TimerState, kind: &str) -> Result<bool> {
        let Some(session) = state.session_id.as_deref() else {
            return Ok(false);
        };
        let mut connection = self.database.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted =
            queue_active_phase_notification(&transaction, session, state.phase, kind, Utc::now())?;
        transaction.commit()?;
        Ok(inserted)
    }

    fn deliver_pending_with<F>(&self, mut send: F) -> Result<usize>
    where
        F: FnMut(&PhaseAlert) -> Result<()>,
    {
        let pending: Vec<(i64, String)> = {
            let connection = self.database.connect_readonly()?;
            let mut statement = connection.prepare(
                "SELECT phase_id, kind FROM phase_notifications WHERE delivered_at IS NULL ORDER BY created_at, phase_id",
            )?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut delivered = 0;
        for (phase_id, kind) in pending {
            let alert = alert_for_kind(&kind).context("unknown Pomodoro alert kind")?;
            if send(&alert).is_ok() {
                self.database.connect()?.execute(
                    "UPDATE phase_notifications SET delivered_at=?1 WHERE phase_id=?2 AND kind=?3 AND delivered_at IS NULL",
                    params![Utc::now().to_rfc3339(), phase_id, kind],
                )?;
                delivered += 1;
            }
        }
        Ok(delivered)
    }
}

fn grace_warning_due(state: &TimerState) -> bool {
    let grace = matches!(state.phase, Phase::AwayGrace | Phase::WorkspaceGrace);
    let long_enough = state
        .started_at
        .zip(state.deadline)
        .is_some_and(|(started, deadline)| {
            (deadline - started).num_seconds() >= GRACE_WARNING_REMAINING_SECONDS
        });
    grace
        && long_enough
        && state.remaining_seconds > 0
        && state.remaining_seconds <= GRACE_WARNING_REMAINING_SECONDS
}

fn queue_active_phase_notification(
    transaction: &rusqlite::Transaction<'_>,
    session: &str,
    phase: Phase,
    kind: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let phase_id: Option<i64> = transaction
        .query_row(
            "SELECT id FROM timed_phases WHERE session_id=?1 AND phase=?2 AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
            params![session, phase.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(phase_id) = phase_id else {
        return Ok(false);
    };
    Ok(transaction.execute(
        "INSERT OR IGNORE INTO phase_notifications(phase_id, kind, created_at) VALUES(?1, ?2, ?3)",
        params![phase_id, kind, now.to_rfc3339()],
    )? > 0)
}

#[allow(clippy::too_many_arguments)]
fn insert_phase(
    transaction: &rusqlite::Transaction<'_>,
    session: &str,
    phase: Phase,
    category: &str,
    workspace: Option<&str>,
    started: DateTime<Utc>,
    deadline: DateTime<Utc>,
    protection_source: Option<&str>,
) -> Result<()> {
    transaction.execute("INSERT INTO timed_phases(session_id, phase, category, workspace_id, started_at, deadline, protection_source) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![session, phase.as_str(), category, workspace, started.to_rfc3339(), deadline.to_rfc3339(), protection_source])?;
    Ok(())
}

fn close_active_phase(
    transaction: &rusqlite::Transaction<'_>,
    ended: DateTime<Utc>,
    reason: &str,
) -> Result<()> {
    transaction.execute("UPDATE timed_phases SET ended_at=?1, termination_reason=?2 WHERE ended_at IS NULL AND phase!='agent_supervision'", params![ended.to_rfc3339(), reason])?;
    Ok(())
}

fn close_protection(
    transaction: &rusqlite::Transaction<'_>,
    id: i64,
    started: DateTime<Utc>,
    ended: DateTime<Utc>,
    session: &str,
    source: &str,
) -> Result<()> {
    let duration = (ended - started).num_nanoseconds().unwrap_or(0).max(0);
    transaction.execute(
        "UPDATE timed_phases SET ended_at=?1, termination_reason='protection_stopped' WHERE id=?2",
        params![ended.to_rfc3339(), id],
    )?;
    transaction.execute("INSERT INTO activity_segments(category, session_id, start_wall_ns, end_wall_ns, duration_ns, provenance) VALUES('agent_supervision', ?1, ?2, ?3, ?4, ?5)", params![session, timestamp_ns(started), timestamp_ns(ended), duration, source])?;
    Ok(())
}

fn close_all_protection(
    transaction: &rusqlite::Transaction<'_>,
    session: &str,
    ended: DateTime<Utc>,
) -> Result<()> {
    let values: Vec<(i64, String, String)> = {
        let mut statement = transaction.prepare("SELECT id, started_at, COALESCE(protection_source, 'agent') FROM timed_phases WHERE session_id=?1 AND phase='agent_supervision' AND ended_at IS NULL")?;
        statement
            .query_map([session], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, started, source) in values {
        close_protection(
            transaction,
            id,
            parse_time(&started)?,
            ended,
            session,
            &source,
        )?;
    }
    Ok(())
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid timestamp {value}"))?
        .with_timezone(&Utc))
}

fn timestamp_ns(value: DateTime<Utc>) -> i64 {
    value.timestamp_nanos_opt().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_persists_and_cross_workspace_input_starts_grace() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(db, PluginConfig::default());
        let state = engine.start_focus("A", None).unwrap();
        assert_eq!(state.phase, Phase::Focus);
        let grace = engine.on_human_input("B").unwrap();
        assert_eq!(grace.phase, Phase::WorkspaceGrace);
        let resumed = engine.on_human_input("A").unwrap();
        assert_eq!(resumed.phase, Phase::Focus);
    }

    #[test]
    fn popup_started_workspace_grace_needs_leaving_before_input_resumes_focus() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(db, PluginConfig::default());
        engine.start_focus("A", None).unwrap();
        engine.start_grace(Phase::WorkspaceGrace).unwrap();
        assert_eq!(
            engine.on_human_input("A").unwrap().phase,
            Phase::WorkspaceGrace
        );
        engine.on_human_input("B").unwrap();
        assert_eq!(engine.on_human_input("A").unwrap().phase, Phase::Focus);
    }

    #[test]
    fn expired_workspace_grace_ends_focus_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(database.clone(), PluginConfig::default());
        engine.start_focus("A", None).unwrap();
        engine.on_human_input("B").unwrap();
        database
            .connect()
            .unwrap()
            .execute(
                "UPDATE timed_phases SET deadline='2000-01-01T00:00:00+00:00' WHERE ended_at IS NULL AND phase='workspace_grace'",
                [],
            )
            .unwrap();
        let restarted = PomodoroEngine::new(database, PluginConfig::default());
        assert_eq!(restarted.state(true).unwrap().phase, Phase::Idle);
    }

    #[test]
    fn grace_phases_reveal_agents_and_focus_filters_them() {
        assert_eq!(agent_view_mode(Phase::Focus), AgentViewMode::FocusWorkspace);
        assert_eq!(agent_view_mode(Phase::AwayGrace), AgentViewMode::All);
        assert_eq!(agent_view_mode(Phase::WorkspaceGrace), AgentViewMode::All);
        assert_eq!(agent_view_mode(Phase::Break), AgentViewMode::All);
        assert_eq!(agent_view_mode(Phase::Idle), AgentViewMode::All);
    }

    #[test]
    fn starting_focus_does_not_replace_an_active_session() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(database, PluginConfig::default());
        let first = engine.start_focus("A", None).unwrap();

        let error = engine.start_focus("B", None).unwrap_err();
        let current = engine.state(false).unwrap();

        assert!(error.to_string().contains("already active"));
        assert_eq!(current.session_id, first.session_id);
        assert_eq!(current.workspace_id.as_deref(), Some("A"));
        assert_eq!(current.deadline, first.deadline);
    }

    #[test]
    fn explicit_return_resumes_focus_from_either_grace() {
        for grace_phase in [Phase::AwayGrace, Phase::WorkspaceGrace] {
            let directory = tempfile::tempdir().unwrap();
            let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
            let engine = PomodoroEngine::new(database, PluginConfig::default());
            let focus = engine.start_focus("A", None).unwrap();
            engine.start_grace(grace_phase).unwrap();

            let resumed = engine.return_to_focus().unwrap();

            assert_eq!(resumed.phase, Phase::Focus);
            assert_eq!(resumed.deadline, focus.deadline);
        }
    }

    #[test]
    fn stopping_focus_or_break_returns_to_idle() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(database, PluginConfig::default());
        engine.start_focus("A", None).unwrap();
        assert_eq!(
            engine.end_session("stopped", false).unwrap().phase,
            Phase::Idle
        );

        engine.start_focus("A", None).unwrap();
        let break_state = engine.end_session("ended_early", true).unwrap();
        assert_eq!(break_state.phase, Phase::Break);
        assert_eq!(
            engine.end_session("break_stopped", false).unwrap().phase,
            Phase::Idle
        );
    }

    #[test]
    fn phase_notification_is_delivered_once() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(database, PluginConfig::default());
        let state = engine.start_focus("A", None).unwrap();
        assert!(
            engine
                .queue_phase_notification(&state, "focus_completed")
                .unwrap()
        );
        assert!(
            !engine
                .queue_phase_notification(&state, "focus_completed")
                .unwrap()
        );
        let mut deliveries = 0;
        assert_eq!(
            engine
                .deliver_pending_with(|_| {
                    deliveries += 1;
                    Ok(())
                })
                .unwrap(),
            1
        );
        assert_eq!(engine.deliver_pending_with(|_| Ok(())).unwrap(), 0);
        assert_eq!(deliveries, 1);
    }

    #[test]
    fn failed_notification_can_be_retried() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(database, PluginConfig::default());
        let state = engine.start_focus("A", None).unwrap();

        assert!(
            engine
                .queue_phase_notification(&state, "focus_completed")
                .unwrap()
        );
        assert_eq!(
            engine.deliver_pending_with(|_| bail!("offline")).unwrap(),
            0
        );
        assert_eq!(engine.deliver_pending_with(|_| Ok(())).unwrap(), 1);
    }

    #[test]
    fn grace_warning_becomes_due_at_two_forty_remaining() {
        let now = Utc::now();
        let mut state = TimerState {
            phase: Phase::AwayGrace,
            session_id: Some("session".to_owned()),
            workspace_id: Some("A".to_owned()),
            started_at: Some(now),
            deadline: Some(now + Duration::seconds(180)),
            remaining_seconds: 21,
            presentation_degraded: false,
        };
        assert!(!grace_warning_due(&state));
        state.remaining_seconds = 20;
        assert!(grace_warning_due(&state));
    }

    #[test]
    fn completed_focus_and_break_queue_their_alerts() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let engine = PomodoroEngine::new(database.clone(), PluginConfig::default());
        engine.start_focus("A", None).unwrap();
        database
            .connect()
            .unwrap()
            .execute(
                "UPDATE timed_phases SET deadline='2000-01-01T00:00:00+00:00' WHERE ended_at IS NULL AND phase='focus'",
                [],
            )
            .unwrap();

        assert_eq!(engine.state(true).unwrap().phase, Phase::Break);
        let mut focus_alert = None;
        assert_eq!(
            engine
                .deliver_pending_with(|alert| {
                    focus_alert = Some(*alert);
                    Ok(())
                })
                .unwrap(),
            1
        );
        assert_eq!(focus_alert.unwrap().sound, "done");

        database
            .connect()
            .unwrap()
            .execute(
                "UPDATE timed_phases SET deadline='2000-01-01T00:00:00+00:00' WHERE ended_at IS NULL AND phase='break'",
                [],
            )
            .unwrap();
        assert_eq!(engine.state(true).unwrap().phase, Phase::Idle);
        let mut break_alert = None;
        assert_eq!(
            engine
                .deliver_pending_with(|alert| {
                    break_alert = Some(*alert);
                    Ok(())
                })
                .unwrap(),
            1
        );
        assert_eq!(break_alert.unwrap().title, "Pomodoro break ended");
    }
}
