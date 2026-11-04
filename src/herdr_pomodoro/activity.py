"""Global consecutive-input accounting and compact segment persistence."""

import sqlite3
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

from herdr_pomodoro.attribution import (
    AttributionResolver,
    RevisionKind,
    RevisionMetadata,
    WorkspaceAttribution,
    local_machine_identity,
    revision_for_interval,
    revision_id,
)
from herdr_pomodoro.constants import DEFAULT_MAXIMUM_GAP_SECONDS
from herdr_pomodoro.database import Database
from herdr_pomodoro.models import HumanInputObservation

NANOSECONDS_PER_SECOND = 1_000_000_000
HUMAN_ACTIVE_CATEGORY = "human_active"


@dataclass(frozen=True, slots=True)
class IngestResult:
    """Outcome of processing one correlated observation."""

    duplicate: bool
    credited_duration_ns: int
    segment_id: int | None


@dataclass(frozen=True, slots=True)
class _StoredAnchor:
    """Persisted global input anchor and its revision details."""

    workspace_id: str
    machine_id: str
    project_id: str
    workspace_instance_id: str
    revision_id: str | None
    revision: RevisionMetadata
    input_wall_ns: int
    observed_monotonic_ns: int


@dataclass(frozen=True, slots=True)
class _CreditedInterval:
    """Complete compact segment input for one credited gap."""

    anchor: _StoredAnchor
    end_wall_ns: int
    end_monotonic_ns: int
    duration_ns: int
    revision_id: str
    provenance: str


class ActivityEngine:
    """Process correlated observations under the global anchor rule."""

    def __init__(
        self,
        database: Database,
        *,
        maximum_gap_seconds: int = DEFAULT_MAXIMUM_GAP_SECONDS,
        resolver: AttributionResolver | None = None,
    ) -> None:
        """Initialize an accounting engine over a private database."""
        self.database = database
        self.maximum_gap_ns = maximum_gap_seconds * NANOSECONDS_PER_SECOND
        self.resolver = AttributionResolver() if resolver is None else resolver
        self.machine = local_machine_identity()

    def ingest(
        self,
        observation: HumanInputObservation,
        *,
        attribution: WorkspaceAttribution | None = None,
    ) -> IngestResult:
        """Deduplicate, account, merge, and replace the global anchor atomically."""
        resolved = attribution or self.resolver.resolve(
            machine=self.machine,
            runtime_workspace_id=observation.workspace_id,
            workspace_path=Path(str(observation.cwd or Path.home())),
            refresh_revision=True,
        )
        with self.database.transaction() as connection:
            if not _claim_observation(connection, observation):
                return IngestResult(duplicate=True, credited_duration_ns=0, segment_id=None)
            current_revision_id = _upsert_attribution(connection, resolved)
            anchor = _load_anchor(connection)
            duration_ns = 0
            segment_id: int | None = None
            if anchor is not None:
                gap_ns = observation.observed_monotonic_ns - anchor.observed_monotonic_ns
                if (
                    anchor.workspace_id == observation.workspace_id
                    and 0 < gap_ns <= self.maximum_gap_ns
                ):
                    interval_revision = revision_for_interval(anchor.revision, resolved.revision)
                    interval_revision_id = _upsert_revision(
                        connection, resolved.project_id, interval_revision
                    )
                    segment_id = _merge_or_insert_segment(
                        connection,
                        _CreditedInterval(
                            anchor=anchor,
                            end_wall_ns=observation.input_time_ns,
                            end_monotonic_ns=observation.observed_monotonic_ns,
                            duration_ns=gap_ns,
                            revision_id=interval_revision_id,
                            provenance=observation.provenance,
                        ),
                    )
                    duration_ns = gap_ns
            _replace_anchor(
                connection,
                observation=observation,
                attribution=resolved,
                revision_id_value=current_revision_id,
            )
        return IngestResult(
            duplicate=False,
            credited_duration_ns=duration_ns,
            segment_id=segment_id,
        )


def _claim_observation(
    connection: sqlite3.Connection,
    observation: HumanInputObservation,
) -> bool:
    processed_at = datetime.now(tz=UTC).isoformat()
    cursor = connection.execute(
        "INSERT OR IGNORE INTO processed_observations "
        "(observation_id, observed_monotonic_ns, processed_at) VALUES(?, ?, ?)",
        (observation.observation_id, observation.observed_monotonic_ns, processed_at),
    )
    return cursor.rowcount == 1


def _upsert_attribution(
    connection: sqlite3.Connection,
    attribution: WorkspaceAttribution,
) -> str:
    machine = attribution.machine
    connection.execute(
        "INSERT INTO machines(id, kind, label, transport) VALUES(?, ?, ?, ?) "
        "ON CONFLICT(id) DO UPDATE SET label = excluded.label, transport = excluded.transport",
        (machine.machine_id, machine.kind, machine.label, machine.transport),
    )
    connection.execute(
        "INSERT INTO projects(id, machine_id, repository_identity, name, root_path) "
        "VALUES(?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET "
        "name = excluded.name, root_path = excluded.root_path",
        (
            attribution.project_id,
            machine.machine_id,
            attribution.repository_identity,
            attribution.project_name,
            str(attribution.repository_root),
        ),
    )
    connection.execute(
        "INSERT INTO workspace_instances(id, machine_id, project_id, runtime_workspace_id, "
        "path, worktree_path) VALUES(?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET "
        "runtime_workspace_id = excluded.runtime_workspace_id, path = excluded.path",
        (
            attribution.workspace_instance_id,
            machine.machine_id,
            attribution.project_id,
            attribution.runtime_workspace_id,
            str(attribution.workspace_path),
            str(attribution.workspace_path),
        ),
    )
    return _upsert_revision(connection, attribution.project_id, attribution.revision)


def _upsert_revision(
    connection: sqlite3.Connection,
    project_id: str,
    revision: RevisionMetadata,
) -> str:
    identity = revision_id(project_id, revision)
    connection.execute(
        "INSERT INTO revisions(id, project_id, kind, stable_id, commit_id, branch, bookmarks, "
        "description) VALUES(?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET "
        "commit_id = excluded.commit_id, branch = excluded.branch, bookmarks = excluded.bookmarks, "
        "description = excluded.description",
        (
            identity,
            project_id,
            revision.kind,
            revision.stable_id,
            revision.commit_id,
            revision.branch,
            revision.bookmarks,
            revision.description,
        ),
    )
    return identity


def _load_anchor(connection: sqlite3.Connection) -> _StoredAnchor | None:
    row = connection.execute(
        "SELECT a.*, r.kind, r.stable_id, r.commit_id, r.branch, r.bookmarks, r.description "
        "FROM activity_anchors a LEFT JOIN revisions r ON r.id = a.revision_id "
        "WHERE a.singleton = 1"
    ).fetchone()
    if row is None:
        return None
    kind_value = row["kind"]
    revision = (
        RevisionMetadata(RevisionKind.NONE, "unversioned")
        if kind_value is None
        else RevisionMetadata(
            kind=RevisionKind(str(kind_value)),
            stable_id=str(row["stable_id"]),
            commit_id=row["commit_id"],
            branch=row["branch"],
            bookmarks=row["bookmarks"],
            description=row["description"],
        )
    )
    return _StoredAnchor(
        workspace_id=str(row["workspace_id"]),
        machine_id=str(row["machine_id"]),
        project_id=str(row["project_id"]),
        workspace_instance_id=str(row["workspace_instance_id"]),
        revision_id=row["revision_id"],
        revision=revision,
        input_wall_ns=int(row["input_wall_ns"]),
        observed_monotonic_ns=int(row["observed_monotonic_ns"]),
    )


def _merge_or_insert_segment(
    connection: sqlite3.Connection,
    interval: _CreditedInterval,
) -> int:
    anchor = interval.anchor
    session_row = connection.execute(
        "SELECT id FROM focus_sessions WHERE status = 'active' ORDER BY started_at DESC LIMIT 1"
    ).fetchone()
    session_id = None if session_row is None else str(session_row["id"])
    previous = connection.execute(
        "SELECT id FROM activity_segments WHERE category = ? AND machine_id = ? AND project_id = ? "
        "AND workspace_instance_id = ? AND revision_id = ? AND session_id IS ? "
        "AND end_monotonic_ns = ? AND provenance = ? ORDER BY id DESC LIMIT 1",
        (
            HUMAN_ACTIVE_CATEGORY,
            anchor.machine_id,
            anchor.project_id,
            anchor.workspace_instance_id,
            interval.revision_id,
            session_id,
            anchor.observed_monotonic_ns,
            interval.provenance,
        ),
    ).fetchone()
    if previous is not None:
        segment_id = int(previous["id"])
        connection.execute(
            "UPDATE activity_segments SET end_wall_ns = ?, end_monotonic_ns = ?, "
            "duration_ns = duration_ns + ? WHERE id = ?",
            (
                interval.end_wall_ns,
                interval.end_monotonic_ns,
                interval.duration_ns,
                segment_id,
            ),
        )
        return segment_id
    cursor = connection.execute(
        "INSERT INTO activity_segments(category, machine_id, project_id, workspace_instance_id, "
        "revision_id, session_id, start_wall_ns, end_wall_ns, start_monotonic_ns, "
        "end_monotonic_ns, duration_ns, provenance) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            HUMAN_ACTIVE_CATEGORY,
            anchor.machine_id,
            anchor.project_id,
            anchor.workspace_instance_id,
            interval.revision_id,
            session_id,
            anchor.input_wall_ns,
            interval.end_wall_ns,
            anchor.observed_monotonic_ns,
            interval.end_monotonic_ns,
            interval.duration_ns,
            interval.provenance,
        ),
    )
    if cursor.lastrowid is None:
        msg = "SQLite did not return an activity segment identity"
        raise RuntimeError(msg)
    return int(cursor.lastrowid)


def _replace_anchor(
    connection: sqlite3.Connection,
    *,
    observation: HumanInputObservation,
    attribution: WorkspaceAttribution,
    revision_id_value: str,
) -> None:
    connection.execute(
        "INSERT INTO activity_anchors(singleton, observation_id, workspace_id, machine_id, "
        "project_id, workspace_instance_id, revision_id, input_wall_ns, observed_monotonic_ns) "
        "VALUES(1, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET "
        "observation_id = excluded.observation_id, workspace_id = excluded.workspace_id, "
        "machine_id = excluded.machine_id, project_id = excluded.project_id, "
        "workspace_instance_id = excluded.workspace_instance_id, "
        "revision_id = excluded.revision_id, "
        "input_wall_ns = excluded.input_wall_ns, "
        "observed_monotonic_ns = excluded.observed_monotonic_ns",
        (
            observation.observation_id,
            observation.workspace_id,
            attribution.machine.machine_id,
            attribution.project_id,
            attribution.workspace_instance_id,
            revision_id_value,
            observation.input_time_ns,
            observation.observed_monotonic_ns,
        ),
    )
