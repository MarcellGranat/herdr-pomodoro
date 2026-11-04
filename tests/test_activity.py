"""Tests for global consecutive-input accounting."""

from pathlib import Path, PurePosixPath

from herdr_pomodoro.activity import NANOSECONDS_PER_SECOND, ActivityEngine
from herdr_pomodoro.attribution import (
    RevisionKind,
    RevisionMetadata,
    WorkspaceAttribution,
    local_machine_identity,
)
from herdr_pomodoro.database import Database
from herdr_pomodoro.models import HumanInputObservation


def test_same_workspace_credits_and_merges_continuous_input(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    engine = ActivityEngine(database)
    attribution = _attribution(tmp_path, "A")

    engine.ingest(_observation("one", "A", 0), attribution=attribution)
    second = engine.ingest(_observation("two", "A", 10), attribution=attribution)
    third = engine.ingest(_observation("three", "A", 20), attribution=attribution)

    assert second.credited_duration_ns == 10 * NANOSECONDS_PER_SECOND
    assert third.segment_id == second.segment_id
    with database.connect(readonly=True) as connection:
        rows = connection.execute("SELECT duration_ns FROM activity_segments").fetchall()
    assert [row["duration_ns"] for row in rows] == [20 * NANOSECONDS_PER_SECOND]


def test_other_workspace_breaks_global_chain(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    engine = ActivityEngine(database)
    workspace_a = _attribution(tmp_path / "a", "A")
    workspace_b = _attribution(tmp_path / "b", "B")

    engine.ingest(_observation("a1", "A", 0), attribution=workspace_a)
    assert engine.ingest(_observation("b1", "B", 10), attribution=workspace_b).segment_id is None
    assert engine.ingest(_observation("a2", "A", 20), attribution=workspace_a).segment_id is None


def test_navigation_without_input_does_not_change_anchor(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    engine = ActivityEngine(database)
    attribution = _attribution(tmp_path, "A")

    engine.ingest(_observation("a1", "A", 0), attribution=attribution)
    result = engine.ingest(_observation("a2", "A", 60), attribution=attribution)

    assert result.credited_duration_ns == 60 * NANOSECONDS_PER_SECOND


def test_over_limit_and_out_of_order_replace_without_credit(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    engine = ActivityEngine(database)
    attribution = _attribution(tmp_path, "A")

    engine.ingest(_observation("one", "A", 200), attribution=attribution)
    assert engine.ingest(_observation("old", "A", 100), attribution=attribution).segment_id is None
    assert engine.ingest(_observation("late", "A", 300), attribution=attribution).segment_id is None


def test_duplicate_observation_is_idempotent_across_restart(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    attribution = _attribution(tmp_path, "A")
    observation = _observation("same", "A", 10)

    ActivityEngine(database).ingest(observation, attribution=attribution)
    result = ActivityEngine(database).ingest(observation, attribution=attribution)

    assert result.duplicate
    with database.connect(readonly=True) as connection:
        assert connection.execute("SELECT COUNT(*) FROM processed_observations").fetchone()[0] == 1


def test_wall_clock_change_does_not_determine_duration(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    engine = ActivityEngine(database)
    attribution = _attribution(tmp_path, "A")
    first = _observation("one", "A", 0, wall_seconds=1_000)
    second = _observation("two", "A", 10, wall_seconds=900)

    engine.ingest(first, attribution=attribution)
    result = engine.ingest(second, attribution=attribution)

    assert result.credited_duration_ns == 10 * NANOSECONDS_PER_SECOND


def test_revision_change_creates_transition_attribution(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    engine = ActivityEngine(database)
    first = _attribution(tmp_path, "A", revision="one")
    second = _attribution(tmp_path, "A", revision="two")

    engine.ingest(_observation("one", "A", 0), attribution=first)
    engine.ingest(_observation("two", "A", 10), attribution=second)

    with database.connect(readonly=True) as connection:
        row = connection.execute(
            "SELECT r.kind FROM activity_segments s JOIN revisions r ON r.id = s.revision_id"
        ).fetchone()
    assert row["kind"] == RevisionKind.TRANSITION


def _observation(
    identity: str,
    workspace_id: str,
    monotonic_seconds: int,
    *,
    wall_seconds: int | None = None,
) -> HumanInputObservation:
    wall = monotonic_seconds if wall_seconds is None else wall_seconds
    return HumanInputObservation(
        observation_id=identity,
        observed_monotonic_ns=monotonic_seconds * NANOSECONDS_PER_SECOND,
        input_time_ns=wall * NANOSECONDS_PER_SECOND,
        correlation_delta_ns=1,
        client_endpoint_id="client",
        pane_endpoint_id=f"pane-{workspace_id}",
        workspace_id=workspace_id,
        pane_id=f"pane-{workspace_id}",
        cwd=PurePosixPath(f"/work/{workspace_id}"),
    )


def _attribution(
    path: Path,
    workspace_id: str,
    *,
    revision: str = "revision",
) -> WorkspaceAttribution:
    machine = local_machine_identity("test-machine")
    return WorkspaceAttribution(
        machine=machine,
        runtime_workspace_id=workspace_id,
        workspace_path=path,
        repository_root=path,
        repository_identity=f"repo-{workspace_id}",
        project_id=f"project-{workspace_id}",
        project_name=f"Project {workspace_id}",
        workspace_instance_id=f"workspace-{workspace_id}",
        revision=RevisionMetadata(RevisionKind.GIT, revision, commit_id=revision),
    )
