"""Tests for private path discovery and SQLite migrations."""

import sqlite3
from pathlib import Path

import pytest

from herdr_pomodoro.config import PluginConfig, discover_paths, load_config
from herdr_pomodoro.database import SCHEMA_VERSION, Database, DatabaseVersionError


def test_injected_paths_create_private_config_and_state(tmp_path: Path) -> None:
    paths = discover_paths(
        config_directory=tmp_path / "config",
        state_directory=tmp_path / "state",
    )

    config = load_config(paths)

    assert config == PluginConfig()
    assert paths.config_file.exists()
    assert paths.config_file.stat().st_mode & 0o777 == 0o600
    assert paths.state_directory.stat().st_mode & 0o777 == 0o700


def test_migrations_are_idempotent_and_enable_safety_pragmas(tmp_path: Path) -> None:
    database_path = tmp_path / "private" / "activity.sqlite3"
    first = Database(database_path)
    second = Database(database_path)

    assert first.schema_version() == SCHEMA_VERSION
    assert second.schema_version() == SCHEMA_VERSION
    with second.connect() as connection:
        assert connection.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
        assert connection.execute("PRAGMA foreign_keys").fetchone()[0] == 1
        tables = {
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table'"
            ).fetchall()
        }
    assert {
        "activity_anchors",
        "activity_segments",
        "focus_sessions",
        "machines",
        "processed_observations",
        "projects",
        "revisions",
        "timed_phases",
        "workspace_instances",
    } <= tables


def test_rejects_newer_database_schema(tmp_path: Path) -> None:
    path = tmp_path / "future.sqlite3"
    database = Database(path)
    with database.transaction() as connection:
        connection.execute("UPDATE schema_metadata SET version = 999 WHERE singleton = 1")

    with pytest.raises(DatabaseVersionError, match="newer"):
        Database(path)


def test_dashboard_reader_and_writer_can_overlap(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    reader = database.connect(readonly=True)
    try:
        reader.execute("BEGIN")
        reader.execute("SELECT COUNT(*) FROM machines").fetchone()
        with database.transaction() as writer:
            writer.execute(
                "INSERT INTO machines(id, kind, label) VALUES(?, ?, ?)",
                ("machine", "local", "test"),
            )
        assert reader.execute("SELECT COUNT(*) FROM machines").fetchone()[0] == 0
        reader.rollback()
        assert reader.execute("SELECT COUNT(*) FROM machines").fetchone()[0] == 1
    finally:
        reader.close()


def test_processed_observation_primary_key_deduplicates(tmp_path: Path) -> None:
    database = Database(tmp_path / "activity.sqlite3")
    with database.transaction() as connection:
        connection.execute(
            "INSERT INTO processed_observations VALUES(?, ?, ?)",
            ("observation", 1, "2026-11-04T00:00:00+00:00"),
        )
    with pytest.raises(sqlite3.IntegrityError), database.transaction() as connection:
        connection.execute(
            "INSERT INTO processed_observations VALUES(?, ?, ?)",
            ("observation", 1, "2026-11-04T00:00:00+00:00"),
        )
