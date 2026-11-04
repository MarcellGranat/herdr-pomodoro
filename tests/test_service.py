"""Tests for single-owner watcher and macOS service reconciliation."""

import plistlib
from pathlib import Path
from typing import cast

import pytest

from herdr_pomodoro.config import discover_paths
from herdr_pomodoro.discovery import CommandResult
from herdr_pomodoro.json_types import JsonValue
from herdr_pomodoro.service import (
    LAUNCH_AGENT_LABEL,
    WatcherAlreadyRunningError,
    WatcherLock,
    reconcile_macos_service,
)


def test_only_one_watcher_owns_state_directory(tmp_path: Path) -> None:
    lock_path = tmp_path / "watcher.lock"

    with (
        WatcherLock(lock_path),
        pytest.raises(WatcherAlreadyRunningError),
        WatcherLock(lock_path),
    ):
        pass


def test_reconciles_launch_agent_with_private_paths(tmp_path: Path) -> None:
    paths = discover_paths(
        config_directory=tmp_path / "config",
        state_directory=tmp_path / "state",
    )
    calls: list[tuple[str, ...]] = []

    def runner(arguments: tuple[str, ...]) -> CommandResult:
        calls.append(arguments)
        return CommandResult(0, "", "")

    plist_path = reconcile_macos_service(
        paths,
        launch_agents_directory=tmp_path / "LaunchAgents",
        runner=runner,
        user_id=501,
    )
    payload = cast("dict[str, JsonValue]", plistlib.loads(plist_path.read_bytes()))

    assert payload["Label"] == LAUNCH_AGENT_LABEL
    environment = payload["EnvironmentVariables"]
    assert isinstance(environment, dict)
    assert environment["HERDR_PLUGIN_STATE_DIR"] == str(paths.state_directory)
    assert isinstance(environment["PATH"], str)
    assert ("launchctl", "bootstrap", "gui/501", str(plist_path)) in calls
    assert calls[-1] == (
        "launchctl",
        "kickstart",
        "-k",
        f"gui/501/{LAUNCH_AGENT_LABEL}",
    )
