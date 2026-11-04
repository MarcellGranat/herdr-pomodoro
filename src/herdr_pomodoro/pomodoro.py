"""Persisted Pomodoro, grace, break, and supervision state machine."""

import json
import sqlite3
from contextlib import suppress
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from enum import StrEnum
from typing import Callable, Protocol
from uuid import uuid4

from herdr_pomodoro.config import PluginConfig
from herdr_pomodoro.database import Database

FOCUS_ELAPSED_CATEGORY = "focus_elapsed"
GRACE_CATEGORY = "grace"
BREAK_CATEGORY = "break"
AGENT_SUPERVISION_CATEGORY = "agent_supervision"


class Phase(StrEnum):
    """Persisted Pomodoro phases."""

    IDLE = "idle"
    FOCUS = "focus"
    AWAY_GRACE = "away_grace"
    WORKSPACE_GRACE = "workspace_grace"
    BREAK = "break"


@dataclass(frozen=True, slots=True)
class TimerState:
    """Current reconstructed timer state."""

    phase: Phase
    session_id: str | None
    workspace_id: str | None
    started_at: datetime | None
    deadline: datetime | None
    remaining_seconds: int
    presentation_degraded: bool = False


class PresentationController(Protocol):
    """Best-effort presentation capture/reduction/restoration boundary."""

    def capture(self) -> str:
        """Return serializable presentation state."""
        raise NotImplementedError

    def reduce(self, focused_workspace_id: str) -> bool:
        """Reduce distractions and report whether state changed."""
        raise NotImplementedError

    def restore(self, state: str) -> None:
        """Restore only state previously changed by the plugin."""
        raise NotImplementedError


class ProtectionAdapter(Protocol):
    """Optional integration boundary for monitored notification targets."""

    @property
    def source(self) -> str:
        """Return a stable protection-source label."""
        raise NotImplementedError

    def is_active(self) -> bool:
        """Return whether the external target currently protects focus."""
        raise NotImplementedError


@dataclass(slots=True)
class UnsupportedPresentationController:
    """Capability-aware fallback when Herdr presentation APIs are unavailable."""

    def capture(self) -> str:
        """Return an explicit empty snapshot."""
        return json.dumps({"supported": False})

    def reduce(self, focused_workspace_id: str) -> bool:
        """Report that no presentation state could be changed."""
        del focused_workspace_id
        return False

    def restore(self, state: str) -> None:
        """Do nothing because this controller never changes state."""
        del state


Clock = Callable[[], datetime]


def utc_now() -> datetime:
    """Return the current aware UTC time."""
    return datetime.now(tz=UTC)


class PomodoroEngine:
    """Persist and reconcile one active Pomodoro session."""

    def __init__(
        self,
        database: Database,
        config: PluginConfig,
        *,
        clock: Clock = utc_now,
        presentation: PresentationController | None = None,
    ) -> None:
        """Initialize the state machine with injectable time and presentation."""
        self.database = database
        self.config = config
        self.clock = clock
        self.presentation = presentation or UnsupportedPresentationController()

    def start_focus(
        self, workspace_id: str, workspace_instance_id: str | None = None
    ) -> TimerState:
        """Start a new focus session after ending any prior active session."""
        now = self.clock()
        active = self.state(reconcile=False)
        if active.phase is not Phase.IDLE:
            self.end_session(reason="replaced", start_break=False)
        session_id = str(uuid4())
        captured = self.presentation.capture()
        changed = False
        try:
            changed = self.presentation.reduce(workspace_id)
        except RuntimeError:
            changed = False
        deadline = now + timedelta(seconds=self.config.focus_seconds)
        with self.database.transaction() as connection:
            connection.execute(
                "INSERT INTO focus_sessions(id, workspace_id, workspace_instance_id, started_at, "
                "status, presentation_state, presentation_degraded) VALUES(?, ?, ?, ?, ?, ?, ?)",
                (
                    session_id,
                    workspace_id,
                    workspace_instance_id,
                    now.isoformat(),
                    "active",
                    captured,
                    int(not changed),
                ),
            )
            _insert_phase(
                connection,
                session_id=session_id,
                phase=Phase.FOCUS,
                category=FOCUS_ELAPSED_CATEGORY,
                workspace_id=workspace_id,
                started_at=now,
                deadline=deadline,
            )
        return self.state(reconcile=False)

    def start_grace(self, phase: Phase) -> TimerState:
        """Replace active focus with a bounded away or workspace grace phase."""
        if phase not in {Phase.AWAY_GRACE, Phase.WORKSPACE_GRACE}:
            msg = "grace phase must be away_grace or workspace_grace"
            raise ValueError(msg)
        state = self.state()
        if state.session_id is None or state.workspace_id is None:
            msg = "cannot start grace without an active focus session"
            raise RuntimeError(msg)
        now = self.clock()
        seconds = (
            self.config.away_grace_seconds
            if phase is Phase.AWAY_GRACE
            else self.config.workspace_grace_seconds
        )
        with self.database.transaction() as connection:
            _close_active_phase(connection, now, "grace_started")
            _insert_phase(
                connection,
                session_id=state.session_id,
                phase=phase,
                category=GRACE_CATEGORY,
                workspace_id=state.workspace_id,
                started_at=now,
                deadline=now + timedelta(seconds=seconds),
            )
        return self.state(reconcile=False)

    def on_human_input(self, workspace_id: str) -> TimerState:
        """Resume focus from grace only when input returns to the focused workspace."""
        state = self.state()
        if state.phase not in {Phase.AWAY_GRACE, Phase.WORKSPACE_GRACE}:
            return state
        if workspace_id != state.workspace_id or state.session_id is None:
            return state
        now = self.clock()
        focus_deadline = self._original_focus_deadline(state.session_id)
        with self.database.transaction() as connection:
            _close_active_phase(connection, now, "returned")
            _insert_phase(
                connection,
                session_id=state.session_id,
                phase=Phase.FOCUS,
                category=FOCUS_ELAPSED_CATEGORY,
                workspace_id=workspace_id,
                started_at=now,
                deadline=max(focus_deadline, now),
            )
        return self.state()

    def set_agent_protection(self, *, active: bool, source: str = "herdr-agent") -> TimerState:
        """Start or stop a separately classified supervision interval."""
        state = self.state()
        if state.session_id is None:
            return state
        now = self.clock()
        with self.database.transaction() as connection:
            protection = connection.execute(
                "SELECT id, started_at FROM timed_phases WHERE session_id = ? "
                "AND phase = 'agent_supervision' AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
                (state.session_id,),
            ).fetchone()
            if active and protection is None:
                _insert_phase(
                    connection,
                    session_id=state.session_id,
                    phase="agent_supervision",
                    category=AGENT_SUPERVISION_CATEGORY,
                    workspace_id=state.workspace_id,
                    started_at=now,
                    deadline=state.deadline or now,
                    protection_source=source,
                )
            elif not active and protection is not None:
                _close_protection(connection, int(protection["id"]), now, state.session_id, source)
        return self.state(reconcile=False)

    def sync_protection(self, adapter: ProtectionAdapter) -> TimerState:
        """Synchronize an optional notification-monitoring protection adapter."""
        return self.set_agent_protection(active=adapter.is_active(), source=adapter.source)

    def end_session(
        self, *, reason: str = "ended_early", start_break: bool | None = None
    ) -> TimerState:
        """End the active phase, restore presentation, and optionally begin break."""
        state = self.state(reconcile=False)
        if state.session_id is None:
            return state
        now = self.clock()
        with self.database.transaction() as connection:
            _close_all_protection(connection, state.session_id, now)
            _close_active_phase(connection, now, reason)
            session = connection.execute(
                "SELECT presentation_state, presentation_degraded FROM focus_sessions WHERE id = ?",
                (state.session_id,),
            ).fetchone()
            use_break = (
                self.config.start_break_after_early_end if start_break is None else start_break
            )
            if state.phase is Phase.BREAK:
                use_break = False
            if use_break:
                _insert_phase(
                    connection,
                    session_id=state.session_id,
                    phase=Phase.BREAK,
                    category=BREAK_CATEGORY,
                    workspace_id=state.workspace_id,
                    started_at=now,
                    deadline=now + timedelta(seconds=self.config.break_seconds),
                )
            else:
                connection.execute(
                    "UPDATE focus_sessions SET ended_at = ?, status = 'ended', "
                    "termination_reason = ? WHERE id = ?",
                    (now.isoformat(), reason, state.session_id),
                )
        if session is not None and not bool(session["presentation_degraded"]):
            with suppress(RuntimeError):
                self.presentation.restore(str(session["presentation_state"]))
        return self.state(reconcile=False)

    def state(self, *, reconcile: bool = True) -> TimerState:
        """Reconstruct current state from persisted deadlines."""
        if reconcile:
            self._reconcile_deadline()
        with self.database.connect(readonly=True) as connection:
            row = connection.execute(
                "SELECT p.*, s.workspace_id, s.presentation_degraded FROM timed_phases p "
                "JOIN focus_sessions s ON s.id = p.session_id WHERE s.status = 'active' "
                "AND p.ended_at IS NULL AND p.phase != 'agent_supervision' "
                "ORDER BY p.id DESC LIMIT 1"
            ).fetchone()
        if row is None:
            return TimerState(Phase.IDLE, None, None, None, None, 0)
        deadline = datetime.fromisoformat(str(row["deadline"]))
        started_at = datetime.fromisoformat(str(row["started_at"]))
        remaining = max(0, int((deadline - self.clock()).total_seconds()))
        return TimerState(
            phase=Phase(str(row["phase"])),
            session_id=str(row["session_id"]),
            workspace_id=str(row["workspace_id"]),
            started_at=started_at,
            deadline=deadline,
            remaining_seconds=remaining,
            presentation_degraded=bool(row["presentation_degraded"]),
        )

    def _reconcile_deadline(self) -> None:
        with self.database.connect(readonly=True) as connection:
            row = connection.execute(
                "SELECT p.phase, p.deadline FROM timed_phases p JOIN focus_sessions s "
                "ON s.id = p.session_id WHERE s.status = 'active' AND p.ended_at IS NULL "
                "AND p.phase != 'agent_supervision' ORDER BY p.id DESC LIMIT 1"
            ).fetchone()
        if row is None or datetime.fromisoformat(str(row["deadline"])) > self.clock():
            return
        phase = Phase(str(row["phase"]))
        if phase is Phase.FOCUS:
            self.end_session(reason="completed", start_break=True)
        elif phase in {Phase.AWAY_GRACE, Phase.WORKSPACE_GRACE}:
            self.end_session(reason="grace_expired", start_break=False)
        elif phase is Phase.BREAK:
            self._complete_break()

    def _complete_break(self) -> None:
        state = self.state(reconcile=False)
        if state.session_id is None:
            return
        now = self.clock()
        with self.database.transaction() as connection:
            _close_active_phase(connection, now, "completed")
            connection.execute(
                "UPDATE focus_sessions SET ended_at = ?, status = 'completed', "
                "termination_reason = COALESCE(termination_reason, 'completed') WHERE id = ?",
                (now.isoformat(), state.session_id),
            )

    def _original_focus_deadline(self, session_id: str) -> datetime:
        with self.database.connect(readonly=True) as connection:
            row = connection.execute(
                "SELECT deadline FROM timed_phases WHERE session_id = ? AND phase = 'focus' "
                "ORDER BY id LIMIT 1",
                (session_id,),
            ).fetchone()
        if row is None:
            return self.clock()
        return datetime.fromisoformat(str(row["deadline"]))


def _insert_phase(  # noqa: PLR0913
    connection: sqlite3.Connection,
    *,
    session_id: str,
    phase: Phase | str,
    category: str,
    workspace_id: str | None,
    started_at: datetime,
    deadline: datetime,
    protection_source: str | None = None,
) -> None:
    connection.execute(
        "INSERT INTO timed_phases(session_id, phase, category, workspace_id, started_at, deadline, "
        "protection_source) VALUES(?, ?, ?, ?, ?, ?, ?)",
        (
            session_id,
            str(phase),
            category,
            workspace_id,
            started_at.isoformat(),
            deadline.isoformat(),
            protection_source,
        ),
    )


def _close_active_phase(
    connection: sqlite3.Connection,
    ended_at: datetime,
    reason: str,
) -> None:
    connection.execute(
        "UPDATE timed_phases SET ended_at = ?, termination_reason = ? WHERE ended_at IS NULL "
        "AND phase != 'agent_supervision'",
        (ended_at.isoformat(), reason),
    )


def _close_protection(
    connection: sqlite3.Connection,
    phase_id: int,
    ended_at: datetime,
    session_id: str,
    source: str,
) -> None:
    row = connection.execute(
        "SELECT started_at FROM timed_phases WHERE id = ?", (phase_id,)
    ).fetchone()
    if row is None:
        return
    started_at = datetime.fromisoformat(str(row["started_at"]))
    duration_ns = max(0, int((ended_at - started_at).total_seconds() * 1_000_000_000))
    connection.execute(
        "UPDATE timed_phases SET ended_at = ?, termination_reason = 'protection_stopped' "
        "WHERE id = ?",
        (ended_at.isoformat(), phase_id),
    )
    connection.execute(
        "INSERT INTO activity_segments(category, session_id, start_wall_ns, end_wall_ns, "
        "duration_ns, provenance) VALUES(?, ?, ?, ?, ?, ?)",
        (
            AGENT_SUPERVISION_CATEGORY,
            session_id,
            _timestamp_ns(started_at),
            _timestamp_ns(ended_at),
            duration_ns,
            source,
        ),
    )


def _close_all_protection(
    connection: sqlite3.Connection,
    session_id: str,
    ended_at: datetime,
) -> None:
    rows = connection.execute(
        "SELECT id, protection_source FROM timed_phases WHERE session_id = ? "
        "AND phase = 'agent_supervision' AND ended_at IS NULL",
        (session_id,),
    ).fetchall()
    for row in rows:
        _close_protection(
            connection,
            int(row["id"]),
            ended_at,
            session_id,
            str(row["protection_source"] or "agent"),
        )


def _timestamp_ns(value: datetime) -> int:
    return int(value.timestamp() * 1_000_000_000)
