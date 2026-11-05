use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};

use crate::config::set_private_file_permissions;

pub const SCHEMA_VERSION: i64 = 2;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1), version INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS machines (
    id TEXT PRIMARY KEY, kind TEXT NOT NULL, label TEXT NOT NULL, transport TEXT
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
"#;

pub const MIGRATION_2: &str = r#"
CREATE TABLE IF NOT EXISTS phase_notifications (
    phase_id INTEGER NOT NULL REFERENCES timed_phases(id),
    kind TEXT NOT NULL,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    PRIMARY KEY(phase_id, kind)
);
"#;

#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let database = Self { path };
        database.migrate()?;
        set_private_file_permissions(&database.path)?;
        Ok(database)
    }

    pub fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)?;
        configure(&connection)?;
        Ok(connection)
    }

    pub fn connect_readonly(&self) -> Result<Connection> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        configure(&connection)?;
        Ok(connection)
    }

    #[cfg(test)]
    pub fn schema_version(&self) -> Result<i64> {
        let connection = self.connect_readonly()?;
        Ok(connection
            .query_row(
                "SELECT version FROM schema_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0))
    }

    fn migrate(&self) -> Result<()> {
        let connection = self.connect()?;
        let journal: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !journal.eq_ignore_ascii_case("wal") {
            bail!("SQLite WAL mode could not be enabled");
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_metadata (\
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1), version INTEGER NOT NULL);",
        )?;
        let version = connection
            .query_row(
                "SELECT version FROM schema_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0);
        if version > SCHEMA_VERSION {
            bail!("database schema {version} is newer than supported version {SCHEMA_VERSION}");
        }
        if version < 1 {
            connection.execute_batch(MIGRATION_1)?;
            connection.execute(
                "INSERT INTO schema_metadata(singleton, version) VALUES(1, 1) \
                 ON CONFLICT(singleton) DO UPDATE SET version = excluded.version",
                [],
            )?;
        }
        if version < 2 {
            connection.execute_batch(MIGRATION_2)?;
            connection.execute(
                "INSERT INTO schema_metadata(singleton, version) VALUES(1, 2) \
                 ON CONFLICT(singleton) DO UPDATE SET version = excluded.version",
                [],
            )?;
        }
        Ok(())
    }
}

fn configure(connection: &Connection) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("activity.sqlite3");
        let first = Database::open(&path).unwrap();
        assert_eq!(first.schema_version().unwrap(), SCHEMA_VERSION);
        let second = Database::open(&path).unwrap();
        assert_eq!(second.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn opens_python_schema_without_losing_rows() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("activity.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(MIGRATION_1).unwrap();
        connection
            .execute(
                "INSERT INTO schema_metadata(singleton, version) VALUES(1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO machines(id, kind, label) VALUES('m1', 'local', 'Mac')",
                [],
            )
            .unwrap();
        drop(connection);
        let database = Database::open(&path).unwrap();
        assert_eq!(database.schema_version().unwrap(), SCHEMA_VERSION);
        let count: i64 = database
            .connect_readonly()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM machines", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
