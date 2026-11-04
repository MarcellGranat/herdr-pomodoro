"""Tests for dashboard totals, filtering, provenance, and empty states."""

from datetime import UTC, date, datetime, timedelta
from pathlib import Path

from herdr_pomodoro.dashboard import (
    DashboardFilters,
    iter_category_totals,
    iter_project_totals,
    iter_revision_totals,
    iter_timeline,
    render_dashboard,
)
from herdr_pomodoro.database import Database


def test_totals_reconcile_and_filters_do_not_mutate_source(tmp_path: Path) -> None:
    database, filters = _database_with_activity(tmp_path)

    categories = {
        total.identity: total.duration_ns for total in iter_category_totals(database, filters)
    }
    filtered = DashboardFilters(
        filters.start_ns,
        filters.end_ns,
        category="human_active",
        revision_id="revision-a",
    )
    entries = tuple(iter_timeline(database, filtered))

    assert categories["human_active"] == 10_000_000_000
    assert categories["focus_elapsed"] == 20_000_000_000
    assert len(entries) == 1
    with database.connect(readonly=True) as connection:
        assert connection.execute("SELECT COUNT(*) FROM activity_segments").fetchone()[0] == 2


def test_local_and_remote_projects_are_labeled_separately(tmp_path: Path) -> None:
    database, filters = _database_with_activity(tmp_path)

    totals = tuple(iter_project_totals(database, filters))

    assert {total.label for total in totals} == {"Same [local]", "Same [remote]"}


def test_revision_totals_and_timeline_are_chronological(tmp_path: Path) -> None:
    database, filters = _database_with_activity(tmp_path)

    revisions = tuple(iter_revision_totals(database, filters))
    entries = tuple(iter_timeline(database, filters))

    assert {total.label for total in revisions} == {"git:a", "jj:b"}
    assert [entry.start_ns for entry in entries] == sorted(entry.start_ns for entry in entries)


def test_empty_dashboard_does_not_infer_work(tmp_path: Path) -> None:
    database = Database(tmp_path / "empty.sqlite3")
    filters = DashboardFilters.for_day(date(2026, 11, 4))

    assert "No recorded activity" in render_dashboard(database, filters)


def _database_with_activity(tmp_path: Path) -> tuple[Database, DashboardFilters]:
    database = Database(tmp_path / "activity.sqlite3")
    start = datetime(2026, 11, 4, 10, tzinfo=UTC)
    start_ns = int(start.timestamp() * 1_000_000_000)
    with database.transaction() as connection:
        for machine_id, label in (("local", "local"), ("remote", "remote")):
            connection.execute(
                "INSERT INTO machines(id, kind, label) VALUES(?, ?, ?)",
                (machine_id, label, label),
            )
            project_id = f"project-{machine_id}"
            workspace_id = f"workspace-{machine_id}"
            revision_id = "revision-a" if machine_id == "local" else "revision-b"
            revision_kind = "git" if machine_id == "local" else "jj"
            stable_id = "a" if machine_id == "local" else "b"
            connection.execute(
                "INSERT INTO projects VALUES(?, ?, ?, ?, ?)",
                (project_id, machine_id, f"repo-{machine_id}", "Same", f"/{machine_id}"),
            )
            connection.execute(
                "INSERT INTO workspace_instances VALUES(?, ?, ?, ?, ?, ?)",
                (
                    workspace_id,
                    machine_id,
                    project_id,
                    machine_id,
                    f"/{machine_id}",
                    f"/{machine_id}",
                ),
            )
            connection.execute(
                "INSERT INTO revisions(id, project_id, kind, stable_id) VALUES(?, ?, ?, ?)",
                (revision_id, project_id, revision_kind, stable_id),
            )
            offset = 0 if machine_id == "local" else 20_000_000_000
            connection.execute(
                "INSERT INTO activity_segments(category, machine_id, project_id, "
                "workspace_instance_id, revision_id, start_wall_ns, end_wall_ns, duration_ns, "
                "provenance) VALUES('human_active', ?, ?, ?, ?, ?, ?, ?, 'test')",
                (
                    machine_id,
                    project_id,
                    workspace_id,
                    revision_id,
                    start_ns + offset,
                    start_ns + offset + 5_000_000_000,
                    5_000_000_000,
                ),
            )
        connection.execute(
            "INSERT INTO focus_sessions(id, workspace_id, workspace_instance_id, "
            "started_at, status) "
            "VALUES('session', 'local', 'workspace-local', ?, 'ended')",
            (start.isoformat(),),
        )
        connection.execute(
            "INSERT INTO timed_phases(session_id, phase, category, workspace_id, started_at, "
            "deadline, ended_at) VALUES('session', 'focus', 'focus_elapsed', 'local', ?, ?, ?)",
            (
                start.isoformat(),
                (start + timedelta(seconds=20)).isoformat(),
                (start + timedelta(seconds=20)).isoformat(),
            ),
        )
    return database, DashboardFilters(
        start_ns - 1,
        start_ns + 60_000_000_000,
    )
