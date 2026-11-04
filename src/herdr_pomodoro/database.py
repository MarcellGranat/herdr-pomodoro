"""Private SQLite persistence with idempotent forward migrations."""

import sqlite3
from contextlib import contextmanager
from pathlib import Path
from typing import Generator

SCHEMA_VERSION = 1
BUSY_TIMEOUT_MILLISECONDS = 5_000

MIGRATION_1 = """
CREATE TABLE IF NOT EXISTS schema_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS machines (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    transport TEXT
);
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL REFERENCES machines(id),
    repository_identity TEXT NOT NULL,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL,
    UNIQUE(machine_id, repository_identity)
);
CREATE TABLE IF NOT EXISTS workspace_instances (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL REFERENCES machines(id),
    project_id TEXT NOT NULL REFERENCES projects(id),
    runtime_workspace_id TEXT NOT NULL,
    path TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    UNIQUE(machine_id, runtime_workspace_id, worktree_path)
);
CREATE TABLE IF NOT EXISTS revisions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    kind TEXT NOT NULL,
    stable_id TEXT NOT NULL,
    commit_id TEXT,
    branch TEXT,
    bookmarks TEXT,
    description TEXT,
    UNIQUE(project_id, kind, stable_id, commit_id)
);
CREATE TABLE IF NOT EXISTS processed_observations (
    observation_id TEXT PRIMARY KEY,
    observed_monotonic_ns INTEGER NOT NULL,
    processed_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS activity_anchors (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    observation_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    machine_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    workspace_instance_id TEXT NOT NULL,
    revision_id TEXT,
    input_wall_ns INTEGER NOT NULL,
    observed_monotonic_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS focus_sessions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    workspace_instance_id TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    status TEXT NOT NULL,
    termination_reason TEXT,
    presentation_state TEXT,
    presentation_degraded INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS timed_phases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT REFERENCES focus_sessions(id),
    phase TEXT NOT NULL,
    category TEXT NOT NULL,
    workspace_id TEXT,
    started_at TEXT NOT NULL,
    deadline TEXT NOT NULL,
    ended_at TEXT,
    termination_reason TEXT,
    protection_source TEXT
);
CREATE TABLE IF NOT EXISTS activity_segments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL,
    machine_id TEXT REFERENCES machines(id),
    project_id TEXT REFERENCES projects(id),
    workspace_instance_id TEXT REFERENCES workspace_instances(id),
    revision_id TEXT REFERENCES revisions(id),
    session_id TEXT REFERENCES focus_sessions(id),
    start_wall_ns INTEGER NOT NULL,
    end_wall_ns INTEGER NOT NULL,
    start_monotonic_ns INTEGER,
    end_monotonic_ns INTEGER,
    duration_ns INTEGER NOT NULL CHECK (duration_ns >= 0),
    provenance TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS activity_segments_time_idx
    ON activity_segments(start_wall_ns, end_wall_ns);
CREATE INDEX IF NOT EXISTS activity_segments_dimensions_idx
    ON activity_segments(project_id, workspace_instance_id, revision_id, category);
CREATE INDEX IF NOT EXISTS timed_phases_time_idx ON timed_phases(started_at, deadline);
"""


class DatabaseVersionError(RuntimeError):
    """Raised when a database is newer than this plugin."""


class Database:
    """Connection factory and migration owner for the private database."""

    def __init__(self, path: Path) -> None:
        """Create or migrate a private database at `path`."""
        self.path = path
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        self._migrate()
        path.chmod(0o600)

    def connect(self, *, readonly: bool = False) -> sqlite3.Connection:
        """Open a configured connection for a short transaction or query."""
        if readonly:
            connection = sqlite3.connect(f"file:{self.path}?mode=ro", uri=True)
        else:
            connection = sqlite3.connect(self.path)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute(f"PRAGMA busy_timeout = {BUSY_TIMEOUT_MILLISECONDS}")
        return connection

    @contextmanager
    def transaction(self) -> Generator[sqlite3.Connection, None, None]:
        """Yield one bounded immediate write transaction."""
        connection = self.connect()
        try:
            connection.execute("BEGIN IMMEDIATE")
            yield connection
            connection.commit()
        except BaseException:
            connection.rollback()
            raise
        finally:
            connection.close()

    def schema_version(self) -> int:
        """Return the applied schema version."""
        with self.connect(readonly=True) as connection:
            row = connection.execute(
                "SELECT version FROM schema_metadata WHERE singleton = 1"
            ).fetchone()
        return 0 if row is None else int(row["version"])

    def _migrate(self) -> None:
        connection = self.connect()
        try:
            journal_mode = connection.execute("PRAGMA journal_mode = WAL").fetchone()
            _require_wal(journal_mode)
            connection.execute(
                "CREATE TABLE IF NOT EXISTS schema_metadata ("
                "singleton INTEGER PRIMARY KEY CHECK (singleton = 1), version INTEGER NOT NULL)"
            )
            row = connection.execute(
                "SELECT version FROM schema_metadata WHERE singleton = 1"
            ).fetchone()
            version = 0 if row is None else int(row[0])
            _require_supported_version(version)
            if version < 1:
                connection.executescript(MIGRATION_1)
                connection.execute(
                    "INSERT INTO schema_metadata(singleton, version) VALUES(1, 1) "
                    "ON CONFLICT(singleton) DO UPDATE SET version = excluded.version"
                )
            connection.commit()
        except BaseException:
            connection.rollback()
            raise
        finally:
            connection.close()


def _require_wal(row: sqlite3.Row | tuple[str] | None) -> None:
    if row is None or str(row[0]).casefold() != "wal":
        msg = "SQLite WAL mode could not be enabled"
        raise RuntimeError(msg)


def _require_supported_version(version: int) -> None:
    if version > SCHEMA_VERSION:
        msg = f"database schema {version} is newer than supported version {SCHEMA_VERSION}"
        raise DatabaseVersionError(msg)
