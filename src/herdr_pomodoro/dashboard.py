"""Typed dashboard queries, filtering, and terminal rendering."""

import sqlite3
from dataclasses import dataclass
from datetime import UTC, date, datetime, time
from typing import Iterator

from herdr_pomodoro.database import Database

NANOSECONDS_PER_SECOND = 1_000_000_000


@dataclass(frozen=True, slots=True)
class DashboardFilters:
    """Immutable dashboard filter selection."""

    start_ns: int
    end_ns: int
    project_id: str | None = None
    workspace_id: str | None = None
    machine_id: str | None = None
    session_id: str | None = None
    category: str | None = None
    revision_id: str | None = None

    @classmethod
    def for_day(cls, selected: date) -> "DashboardFilters":
        """Build local-day bounds represented as UTC epoch nanoseconds."""
        local_zone = datetime.now().astimezone().tzinfo
        start = datetime.combine(selected, time.min, tzinfo=local_zone).astimezone(UTC)
        end = datetime.combine(selected, time.max, tzinfo=local_zone).astimezone(UTC)
        return cls(_timestamp_ns(start), _timestamp_ns(end))


@dataclass(frozen=True, slots=True)
class TimelineEntry:
    """One chronological activity segment or timed phase."""

    category: str
    start_ns: int
    end_ns: int
    duration_ns: int
    project_id: str | None
    project_name: str | None
    workspace_id: str | None
    workspace_path: str | None
    machine_id: str | None
    machine_label: str | None
    revision_id: str | None
    revision_label: str | None
    session_id: str | None
    provenance: str


@dataclass(frozen=True, slots=True)
class Total:
    """Named duration total."""

    identity: str
    label: str
    duration_ns: int


@dataclass(frozen=True, slots=True)
class SessionSummary:
    """Persisted focus session summary."""

    session_id: str
    workspace_id: str
    started_at: datetime
    ended_at: datetime | None
    status: str
    termination_reason: str | None


def iter_timeline(
    database: Database,
    filters: DashboardFilters,
) -> Iterator[TimelineEntry]:
    """Yield a consistent, filtered chronological projection."""
    with database.connect(readonly=True) as connection:
        connection.execute("BEGIN")
        entries = [
            *_iter_activity_rows(connection, filters),
            *_iter_phase_rows(connection, filters),
        ]
        connection.rollback()
    yield from sorted(entries, key=lambda entry: (entry.start_ns, entry.end_ns, entry.category))


def iter_category_totals(
    database: Database,
    filters: DashboardFilters,
) -> Iterator[Total]:
    """Yield duration totals by explicit activity category."""
    totals: dict[str, int] = {}
    for entry in iter_timeline(database, filters):
        totals[entry.category] = totals.get(entry.category, 0) + entry.duration_ns
    for category in sorted(totals):
        yield Total(category, category.replace("_", " "), totals[category])


def iter_project_totals(
    database: Database,
    filters: DashboardFilters,
) -> Iterator[Total]:
    """Yield human-active totals by durable project and machine provenance."""
    totals: dict[tuple[str, str], int] = {}
    for entry in iter_timeline(database, filters):
        if entry.category != "human_active" or entry.project_id is None:
            continue
        label = entry.project_name or entry.project_id
        if entry.machine_label:
            label = f"{label} [{entry.machine_label}]"
        key = (entry.project_id, label)
        totals[key] = totals.get(key, 0) + entry.duration_ns
    for (identity, label), duration_ns in sorted(totals.items(), key=lambda item: item[0][1]):
        yield Total(identity, label, duration_ns)


def iter_workspace_totals(
    database: Database,
    filters: DashboardFilters,
) -> Iterator[Total]:
    """Yield human-active totals by workspace/worktree identity."""
    totals: dict[tuple[str, str], int] = {}
    for entry in iter_timeline(database, filters):
        if entry.category != "human_active" or entry.workspace_id is None:
            continue
        key = (entry.workspace_id, entry.workspace_path or entry.workspace_id)
        totals[key] = totals.get(key, 0) + entry.duration_ns
    for (identity, label), duration_ns in sorted(totals.items(), key=lambda item: item[0][1]):
        yield Total(identity, label, duration_ns)


def iter_revision_totals(
    database: Database,
    filters: DashboardFilters,
) -> Iterator[Total]:
    """Yield human-active totals by Git commit, Jujutsu change, or transition."""
    totals: dict[tuple[str, str], int] = {}
    for entry in iter_timeline(database, filters):
        if entry.category != "human_active" or entry.revision_id is None:
            continue
        key = (entry.revision_id, entry.revision_label or entry.revision_id)
        totals[key] = totals.get(key, 0) + entry.duration_ns
    for (identity, label), duration_ns in sorted(totals.items(), key=lambda item: item[0][1]):
        yield Total(identity, label, duration_ns)


def iter_sessions(database: Database, filters: DashboardFilters) -> Iterator[SessionSummary]:
    """Yield sessions overlapping the selected period."""
    start = _datetime_from_ns(filters.start_ns).isoformat()
    end = _datetime_from_ns(filters.end_ns).isoformat()
    with database.connect(readonly=True) as connection:
        rows = connection.execute(
            "SELECT id, workspace_id, started_at, ended_at, status, termination_reason "
            "FROM focus_sessions WHERE started_at <= ? AND COALESCE(ended_at, ?) >= ? "
            "AND (? IS NULL OR id = ?) ORDER BY started_at",
            (end, end, start, filters.session_id, filters.session_id),
        ).fetchall()
    for row in rows:
        yield SessionSummary(
            session_id=str(row["id"]),
            workspace_id=str(row["workspace_id"]),
            started_at=datetime.fromisoformat(str(row["started_at"])),
            ended_at=(
                None if row["ended_at"] is None else datetime.fromisoformat(str(row["ended_at"]))
            ),
            status=str(row["status"]),
            termination_reason=row["termination_reason"],
        )


def render_dashboard(database: Database, filters: DashboardFilters) -> str:
    """Render a compact terminal dashboard without inferring missing activity."""
    entries = tuple(iter_timeline(database, filters))
    categories = tuple(iter_category_totals(database, filters))
    lines = ["Herdr Pomodoro — activity dashboard", ""]
    if not entries:
        return "\n".join((*lines, "No recorded activity for the selected period.", ""))
    lines.append("Summary")
    category_map = {total.identity: total.duration_ns for total in categories}
    lines.extend(
        (f"  {category.replace('_', ' '):18} {_duration(category_map.get(category, 0))}")
        for category in ("human_active", "focus_elapsed", "agent_supervision", "grace", "break")
    )
    _append_totals(lines, "Projects", iter_project_totals(database, filters))
    _append_totals(lines, "Workspaces", iter_workspace_totals(database, filters))
    _append_totals(lines, "Revisions", iter_revision_totals(database, filters))
    lines.extend(("", "Timeline"))
    for entry in entries:
        start = _datetime_from_ns(entry.start_ns).astimezone().strftime("%H:%M:%S")
        label = entry.workspace_path or entry.workspace_id or "session"
        lines.append(f"  {start}  {entry.category:18} {_duration(entry.duration_ns):>9}  {label}")
    lines.append("")
    return "\n".join(lines)


def _iter_activity_rows(
    connection: sqlite3.Connection,
    filters: DashboardFilters,
) -> Iterator[TimelineEntry]:
    rows = connection.execute(
        "SELECT s.*, p.name project_name, w.path workspace_path, m.label machine_label, "
        "r.kind revision_kind, r.stable_id revision_stable_id FROM activity_segments s "
        "LEFT JOIN projects p ON p.id = s.project_id "
        "LEFT JOIN workspace_instances w ON w.id = s.workspace_instance_id "
        "LEFT JOIN machines m ON m.id = s.machine_id "
        "LEFT JOIN revisions r ON r.id = s.revision_id "
        "WHERE s.end_wall_ns >= ? AND s.start_wall_ns <= ? "
        "AND (? IS NULL OR s.project_id = ?) "
        "AND (? IS NULL OR s.workspace_instance_id = ?) "
        "AND (? IS NULL OR s.machine_id = ?) "
        "AND (? IS NULL OR s.session_id = ?) "
        "AND (? IS NULL OR s.category = ?) "
        "AND (? IS NULL OR s.revision_id = ?)",
        (
            filters.start_ns,
            filters.end_ns,
            filters.project_id,
            filters.project_id,
            filters.workspace_id,
            filters.workspace_id,
            filters.machine_id,
            filters.machine_id,
            filters.session_id,
            filters.session_id,
            filters.category,
            filters.category,
            filters.revision_id,
            filters.revision_id,
        ),
    ).fetchall()
    for row in rows:
        start_ns = max(int(row["start_wall_ns"]), filters.start_ns)
        end_ns = min(int(row["end_wall_ns"]), filters.end_ns)
        original_ns = max(1, int(row["end_wall_ns"]) - int(row["start_wall_ns"]))
        clipped_ns = max(0, end_ns - start_ns)
        duration_ns = int(int(row["duration_ns"]) * clipped_ns / original_ns)
        revision_label = None
        if row["revision_kind"] is not None:
            revision_label = f"{row['revision_kind']}:{row['revision_stable_id']}"
        yield TimelineEntry(
            category=str(row["category"]),
            start_ns=start_ns,
            end_ns=end_ns,
            duration_ns=duration_ns,
            project_id=row["project_id"],
            project_name=row["project_name"],
            workspace_id=row["workspace_instance_id"],
            workspace_path=row["workspace_path"],
            machine_id=row["machine_id"],
            machine_label=row["machine_label"],
            revision_id=row["revision_id"],
            revision_label=revision_label,
            session_id=row["session_id"],
            provenance=str(row["provenance"]),
        )


def _iter_phase_rows(
    connection: sqlite3.Connection,
    filters: DashboardFilters,
) -> Iterator[TimelineEntry]:
    now = datetime.now(tz=UTC)
    rows = connection.execute(
        "SELECT p.*, s.workspace_instance_id, w.path workspace_path, w.project_id, w.machine_id, "
        "pr.name project_name, m.label machine_label FROM timed_phases p "
        "JOIN focus_sessions s ON s.id = p.session_id "
        "LEFT JOIN workspace_instances w ON w.id = s.workspace_instance_id "
        "LEFT JOIN projects pr ON pr.id = w.project_id LEFT JOIN machines m ON m.id = w.machine_id"
    ).fetchall()
    for row in rows:
        if row["category"] == "agent_supervision":
            continue
        started = datetime.fromisoformat(str(row["started_at"]))
        deadline = datetime.fromisoformat(str(row["deadline"]))
        ended = (
            deadline if row["ended_at"] is None else datetime.fromisoformat(str(row["ended_at"]))
        )
        end_time = min(deadline, ended, now)
        start_ns = max(_timestamp_ns(started), filters.start_ns)
        end_ns = min(_timestamp_ns(end_time), filters.end_ns)
        if end_ns < filters.start_ns or start_ns > filters.end_ns or end_ns <= start_ns:
            continue
        entry = TimelineEntry(
            category=str(row["category"]),
            start_ns=start_ns,
            end_ns=end_ns,
            duration_ns=end_ns - start_ns,
            project_id=row["project_id"],
            project_name=row["project_name"],
            workspace_id=row["workspace_instance_id"],
            workspace_path=row["workspace_path"],
            machine_id=row["machine_id"],
            machine_label=row["machine_label"],
            revision_id=None,
            revision_label=None,
            session_id=row["session_id"],
            provenance=str(row["protection_source"] or "pomodoro"),
        )
        if _matches(entry, filters):
            yield entry


def _matches(entry: TimelineEntry, filters: DashboardFilters) -> bool:
    return all(
        expected is None or actual == expected
        for actual, expected in (
            (entry.project_id, filters.project_id),
            (entry.workspace_id, filters.workspace_id),
            (entry.machine_id, filters.machine_id),
            (entry.session_id, filters.session_id),
            (entry.category, filters.category),
            (entry.revision_id, filters.revision_id),
        )
    )


def _append_totals(lines: list[str], title: str, totals: Iterator[Total]) -> None:
    values = tuple(totals)
    if not values:
        return
    lines.extend(("", title))
    lines.extend(f"  {total.label}: {_duration(total.duration_ns)}" for total in values)


def _duration(value_ns: int) -> str:
    total_seconds = value_ns // NANOSECONDS_PER_SECOND
    hours, remainder = divmod(total_seconds, 3600)
    minutes, seconds = divmod(remainder, 60)
    return f"{hours:02d}:{minutes:02d}:{seconds:02d}"


def _timestamp_ns(value: datetime) -> int:
    return int(value.timestamp() * NANOSECONDS_PER_SECOND)


def _datetime_from_ns(value: int) -> datetime:
    return datetime.fromtimestamp(value / NANOSECONDS_PER_SECOND, tz=UTC)
