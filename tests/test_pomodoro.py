"""Tests for persisted Pomodoro lifecycle and supervision."""

from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path

from herdr_pomodoro.config import PluginConfig
from herdr_pomodoro.database import Database
from herdr_pomodoro.pomodoro import Phase, PomodoroEngine


@dataclass(slots=True)
class _Clock:
    now: datetime

    def __call__(self) -> datetime:
        return self.now

    def advance(self, seconds: int) -> None:
        self.now += timedelta(seconds=seconds)


@dataclass(slots=True)
class _Presentation:
    changed: bool = True
    restored: bool = False
    fail: bool = False

    def capture(self) -> str:
        return '{"sidebar":true}'

    def reduce(self, focused_workspace_id: str) -> bool:
        del focused_workspace_id
        if self.fail:
            msg = "unsupported"
            raise RuntimeError(msg)
        return self.changed

    def restore(self, state: str) -> None:
        del state
        self.restored = True


@dataclass(frozen=True, slots=True)
class _NotifyAdapter:
    active: bool
    source: str = "herdr-notify:build"

    def is_active(self) -> bool:
        return self.active


def test_focus_completion_starts_break_and_restart_recovers(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    clock = _Clock(datetime(2026, 11, 4, tzinfo=UTC))
    config = PluginConfig(focus_seconds=10, break_seconds=5)
    engine = PomodoroEngine(database, config, clock=clock)

    engine.start_focus("A")
    clock.advance(11)
    restarted = PomodoroEngine(database, config, clock=clock)

    assert restarted.state().phase is Phase.BREAK
    clock.advance(6)
    assert restarted.state().phase is Phase.IDLE


def test_end_early_restores_presentation_and_records_reason(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    clock = _Clock(datetime(2026, 11, 4, tzinfo=UTC))
    presentation = _Presentation()
    engine = PomodoroEngine(
        database,
        PluginConfig(break_seconds=5),
        clock=clock,
        presentation=presentation,
    )
    engine.start_focus("A")

    state = engine.end_session(reason="ended_early")

    assert state.phase is Phase.BREAK
    assert presentation.restored


def test_workspace_grace_allows_other_input_and_return(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    clock = _Clock(datetime(2026, 11, 4, tzinfo=UTC))
    engine = PomodoroEngine(database, PluginConfig(), clock=clock)
    engine.start_focus("A")
    engine.start_grace(Phase.WORKSPACE_GRACE)

    assert engine.on_human_input("B").phase is Phase.WORKSPACE_GRACE
    assert engine.on_human_input("A").phase is Phase.FOCUS


def test_grace_expiry_ends_focus(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    clock = _Clock(datetime(2026, 11, 4, tzinfo=UTC))
    engine = PomodoroEngine(database, PluginConfig(away_grace_seconds=3), clock=clock)
    engine.start_focus("A")
    engine.start_grace(Phase.AWAY_GRACE)
    clock.advance(4)

    assert engine.state().phase is Phase.IDLE


def test_agent_protection_is_separate_from_human_activity(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    clock = _Clock(datetime(2026, 11, 4, tzinfo=UTC))
    engine = PomodoroEngine(database, PluginConfig(), clock=clock)
    engine.start_focus("A")
    engine.set_agent_protection(active=True)
    clock.advance(7)
    engine.set_agent_protection(active=False)

    with database.connect(readonly=True) as connection:
        rows = connection.execute("SELECT category, duration_ns FROM activity_segments").fetchall()
    assert [(row["category"], row["duration_ns"]) for row in rows] == [
        ("agent_supervision", 7_000_000_000)
    ]


def test_optional_notify_adapter_uses_separate_protection_source(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    clock = _Clock(datetime(2026, 11, 4, tzinfo=UTC))
    engine = PomodoroEngine(database, PluginConfig(), clock=clock)
    engine.start_focus("A")
    engine.sync_protection(_NotifyAdapter(active=True))
    clock.advance(2)
    engine.sync_protection(_NotifyAdapter(active=False))

    with database.connect(readonly=True) as connection:
        row = connection.execute("SELECT provenance FROM activity_segments").fetchone()
    assert row["provenance"] == "herdr-notify:build"


def test_failed_presentation_is_nonfatal_and_not_restored(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    presentation = _Presentation(fail=True)
    engine = PomodoroEngine(database, PluginConfig(), presentation=presentation)

    state = engine.start_focus("A")
    engine.end_session(start_break=False)

    assert state.presentation_degraded
    assert not presentation.restored
