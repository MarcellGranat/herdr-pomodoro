"""Plugin configuration and private path discovery."""

import json
from dataclasses import dataclass
from os import environ
from pathlib import Path
from typing import cast

from herdr_pomodoro.constants import (
    DEFAULT_AWAY_GRACE_SECONDS,
    DEFAULT_BREAK_SECONDS,
    DEFAULT_CORRELATION_WINDOW_MILLISECONDS,
    DEFAULT_FOCUS_SECONDS,
    DEFAULT_MAXIMUM_GAP_SECONDS,
    DEFAULT_SAMPLE_INTERVAL_MILLISECONDS,
    DEFAULT_WORKSPACE_GRACE_SECONDS,
)
from herdr_pomodoro.json_types import JsonValue

CONFIG_DIRECTORY_ENV = "HERDR_PLUGIN_CONFIG_DIR"
STATE_DIRECTORY_ENV = "HERDR_PLUGIN_STATE_DIR"
CONFIG_FILE_NAME = "config.json"
DATABASE_FILE_NAME = "activity.sqlite3"
LOCK_FILE_NAME = "watcher.lock"


@dataclass(frozen=True, slots=True)
class PluginPaths:
    """Private plugin filesystem locations."""

    config_directory: Path
    state_directory: Path

    @property
    def config_file(self) -> Path:
        """Return the JSON configuration path."""
        return self.config_directory / CONFIG_FILE_NAME

    @property
    def database_file(self) -> Path:
        """Return the private SQLite database path."""
        return self.state_directory / DATABASE_FILE_NAME

    @property
    def watcher_lock_file(self) -> Path:
        """Return the single-watcher lock path."""
        return self.state_directory / LOCK_FILE_NAME

    def ensure(self) -> None:
        """Create private plugin directories when absent."""
        self.config_directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.state_directory.mkdir(mode=0o700, parents=True, exist_ok=True)


@dataclass(frozen=True, slots=True)
class PluginConfig:
    """Validated runtime settings with conservative defaults."""

    maximum_gap_seconds: int = DEFAULT_MAXIMUM_GAP_SECONDS
    sample_interval_milliseconds: int = DEFAULT_SAMPLE_INTERVAL_MILLISECONDS
    correlation_window_milliseconds: int = DEFAULT_CORRELATION_WINDOW_MILLISECONDS
    focus_seconds: int = DEFAULT_FOCUS_SECONDS
    away_grace_seconds: int = DEFAULT_AWAY_GRACE_SECONDS
    workspace_grace_seconds: int = DEFAULT_WORKSPACE_GRACE_SECONDS
    break_seconds: int = DEFAULT_BREAK_SECONDS
    grace_expiry_policy: str = "end_focus"
    start_break_after_early_end: bool = True


def discover_paths(
    *,
    config_directory: Path | None = None,
    state_directory: Path | None = None,
) -> PluginPaths:
    """Resolve injected, Herdr-provided, or private user directories."""
    home = Path.home()
    resolved_config = config_directory or Path(
        environ.get(CONFIG_DIRECTORY_ENV, home / ".config" / "herdr-pomodoro")
    )
    resolved_state = state_directory or Path(
        environ.get(STATE_DIRECTORY_ENV, home / ".local" / "state" / "herdr-pomodoro")
    )
    return PluginPaths(resolved_config.expanduser(), resolved_state.expanduser())


def load_config(paths: PluginPaths) -> PluginConfig:
    """Load configuration, creating a documented default file on first use."""
    paths.ensure()
    if not paths.config_file.exists():
        config = PluginConfig()
        paths.config_file.write_text(_config_json(config), encoding="utf-8")
        paths.config_file.chmod(0o600)
        return config
    payload = cast("JsonValue", json.loads(paths.config_file.read_text(encoding="utf-8")))
    if not isinstance(payload, dict):
        msg = "plugin config must be a JSON object"
        raise TypeError(msg)
    defaults = PluginConfig()
    return PluginConfig(
        maximum_gap_seconds=_positive_int(
            payload, "maximum_gap_seconds", defaults.maximum_gap_seconds
        ),
        sample_interval_milliseconds=_positive_int(
            payload, "sample_interval_milliseconds", defaults.sample_interval_milliseconds
        ),
        correlation_window_milliseconds=_positive_int(
            payload, "correlation_window_milliseconds", defaults.correlation_window_milliseconds
        ),
        focus_seconds=_positive_int(payload, "focus_seconds", defaults.focus_seconds),
        away_grace_seconds=_positive_int(
            payload, "away_grace_seconds", defaults.away_grace_seconds
        ),
        workspace_grace_seconds=_positive_int(
            payload, "workspace_grace_seconds", defaults.workspace_grace_seconds
        ),
        break_seconds=_positive_int(payload, "break_seconds", defaults.break_seconds),
        grace_expiry_policy=_string(payload, "grace_expiry_policy", defaults.grace_expiry_policy),
        start_break_after_early_end=_boolean(
            payload,
            "start_break_after_early_end",
            default=defaults.start_break_after_early_end,
        ),
    )


def _config_json(config: PluginConfig) -> str:
    payload = {
        "away_grace_seconds": config.away_grace_seconds,
        "break_seconds": config.break_seconds,
        "correlation_window_milliseconds": config.correlation_window_milliseconds,
        "focus_seconds": config.focus_seconds,
        "grace_expiry_policy": config.grace_expiry_policy,
        "maximum_gap_seconds": config.maximum_gap_seconds,
        "sample_interval_milliseconds": config.sample_interval_milliseconds,
        "start_break_after_early_end": config.start_break_after_early_end,
        "workspace_grace_seconds": config.workspace_grace_seconds,
    }
    return f"{json.dumps(payload, indent=2, sort_keys=True)}\n"


def _positive_int(payload: dict[str, JsonValue], key: str, default: int) -> int:
    value = payload.get(key, default)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        msg = f"config field {key!r} must be a positive integer"
        raise ValueError(msg)
    return value


def _string(payload: dict[str, JsonValue], key: str, default: str) -> str:
    value = payload.get(key, default)
    if not isinstance(value, str) or not value:
        msg = f"config field {key!r} must be a non-empty string"
        raise ValueError(msg)
    return value


def _boolean(payload: dict[str, JsonValue], key: str, *, default: bool) -> bool:
    value = payload.get(key, default)
    if not isinstance(value, bool):
        msg = f"config field {key!r} must be a boolean"
        raise TypeError(msg)
    return value
