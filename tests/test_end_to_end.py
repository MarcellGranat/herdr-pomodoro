"""End-to-end accounting and Pomodoro validation sequences."""

from datetime import UTC, datetime
from pathlib import Path, PurePosixPath

from herdr_pomodoro.activity import NANOSECONDS_PER_SECOND, ActivityEngine
from herdr_pomodoro.attribution import (
    RevisionKind,
    RevisionMetadata,
    WorkspaceAttribution,
    local_machine_identity,
)
from herdr_pomodoro.config import PluginConfig
from herdr_pomodoro.database import Database
from herdr_pomodoro.models import HumanInputObservation
from herdr_pomodoro.pomodoro import Phase, PomodoroEngine


def test_navigation_sequence_preserves_a_chain(tmp_path: Path) -> None:
    database = Database(tmp_path / "navigation.sqlite3")
    engine = ActivityEngine(database)
    workspace_a = _attribution(tmp_path / "a", "A")

    engine.ingest(_observation("a1", "A", 0), attribution=workspace_a)
    # Navigation A -> B -> A is consumed by Herdr, so no observation is ingested.
    result = engine.ingest(_observation("a2", "A", 90), attribution=workspace_a)

    assert result.credited_duration_ns == 90 * NANOSECONDS_PER_SECOND


def test_cross_workspace_input_sequence_breaks_each_chain(tmp_path: Path) -> None:
    database = Database(tmp_path / "cross-workspace.sqlite3")
    engine = ActivityEngine(database)
    workspace_a = _attribution(tmp_path / "a", "A")
    workspace_b = _attribution(tmp_path / "b", "B")

    results = (
        engine.ingest(_observation("a1", "A", 0), attribution=workspace_a),
        engine.ingest(_observation("b1", "B", 30), attribution=workspace_b),
        engine.ingest(_observation("a2", "A", 60), attribution=workspace_a),
    )

    assert sum(result.credited_duration_ns for result in results) == 0


def test_focus_agent_grace_early_end_and_break_sequence(tmp_path: Path) -> None:
    database = Database(tmp_path / "focus.sqlite3")
    config = PluginConfig(start_break_after_early_end=True)
    engine = PomodoroEngine(database, config)

    engine.start_focus("A")
    engine.set_agent_protection(active=True)
    engine.set_agent_protection(active=False)
    engine.start_grace(Phase.AWAY_GRACE)
    engine.on_human_input("A")
    engine.start_grace(Phase.WORKSPACE_GRACE)
    assert engine.on_human_input("B").phase is Phase.WORKSPACE_GRACE
    assert engine.on_human_input("A").phase is Phase.FOCUS
    assert engine.end_session(reason="ended_early").phase is Phase.BREAK
    assert engine.end_session(reason="break_cancelled").phase is Phase.IDLE


def _observation(identity: str, workspace_id: str, seconds: int) -> HumanInputObservation:
    timestamp = seconds * NANOSECONDS_PER_SECOND
    return HumanInputObservation(
        observation_id=identity,
        observed_monotonic_ns=timestamp,
        input_time_ns=timestamp,
        correlation_delta_ns=1,
        client_endpoint_id="client",
        pane_endpoint_id=f"pane-{workspace_id}",
        workspace_id=workspace_id,
        pane_id=f"pane-{workspace_id}",
        cwd=PurePosixPath(f"/work/{workspace_id}"),
    )


def _attribution(path: Path, workspace_id: str) -> WorkspaceAttribution:
    machine = local_machine_identity("test-machine")
    return WorkspaceAttribution(
        machine=machine,
        runtime_workspace_id=workspace_id,
        workspace_path=path,
        repository_root=path,
        repository_identity=f"repo-{workspace_id}",
        project_id=f"project-{workspace_id}",
        project_name=workspace_id,
        workspace_instance_id=f"workspace-{workspace_id}",
        revision=RevisionMetadata(
            RevisionKind.GIT,
            datetime(2026, 11, 4, tzinfo=UTC).isoformat(),
        ),
    )
