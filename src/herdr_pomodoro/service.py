"""Single-owner watcher lock and macOS per-user service reconciliation."""

import fcntl
import plistlib
import subprocess
import sys
from dataclasses import dataclass
from os import environ
from pathlib import Path
from typing import IO, Callable, Self

from herdr_pomodoro.config import PluginPaths
from herdr_pomodoro.discovery import CommandResult

LAUNCH_AGENT_LABEL = "com.chmarax.herdr-pomodoro.watcher"
LAUNCH_AGENT_FILE_NAME = f"{LAUNCH_AGENT_LABEL}.plist"


class WatcherAlreadyRunningError(RuntimeError):
    """Raised when another watcher already holds the ownership lock."""


@dataclass(slots=True)
class WatcherLock:
    """Advisory single-owner lock for one plugin state directory."""

    path: Path
    _handle: IO[str] | None = None

    def __enter__(self) -> Self:
        """Acquire the non-blocking exclusive lock."""
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        handle = self.path.open("a+", encoding="utf-8")
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            handle.close()
            msg = f"another watcher owns {self.path}"
            raise WatcherAlreadyRunningError(msg) from error
        self._handle = handle
        return self

    def __exit__(self, _exc_type: object, _exc_value: object, _traceback: object) -> None:
        """Release ownership when the watcher exits."""
        if self._handle is not None:
            fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
            self._handle.close()
            self._handle = None


ServiceRunner = Callable[[tuple[str, ...]], CommandResult]


def run_service_command(arguments: tuple[str, ...]) -> CommandResult:
    """Run one launchctl lifecycle command."""
    completed = subprocess.run(  # noqa: S603
        arguments,
        check=False,
        capture_output=True,
        text=True,
    )
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def reconcile_macos_service(
    paths: PluginPaths,
    *,
    launch_agents_directory: Path | None = None,
    runner: ServiceRunner = run_service_command,
    user_id: int | None = None,
) -> Path:
    """Write and bootstrap the current per-user watcher LaunchAgent."""
    paths.ensure()
    agents_directory = launch_agents_directory or Path.home() / "Library" / "LaunchAgents"
    agents_directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    plist_path = agents_directory / LAUNCH_AGENT_FILE_NAME
    payload = {
        "EnvironmentVariables": {
            "HERDR_PLUGIN_CONFIG_DIR": str(paths.config_directory),
            "HERDR_PLUGIN_STATE_DIR": str(paths.state_directory),
            "HERDR_BIN_PATH": environ.get("HERDR_BIN_PATH", "herdr"),
            "PATH": environ.get("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"),
        },
        "KeepAlive": True,
        "Label": LAUNCH_AGENT_LABEL,
        "ProcessType": "Background",
        "ProgramArguments": [
            sys.executable,
            "-m",
            "herdr_pomodoro.cli",
            "watch",
        ],
        "RunAtLoad": True,
        "StandardErrorPath": str(paths.state_directory / "watcher.stderr.log"),
        "StandardOutPath": str(paths.state_directory / "watcher.stdout.log"),
        "ThrottleInterval": 10,
    }
    rendered = plistlib.dumps(payload, sort_keys=True)
    if not plist_path.exists() or plist_path.read_bytes() != rendered:
        plist_path.write_bytes(rendered)
        plist_path.chmod(0o600)
    uid = user_id if user_id is not None else _user_id()
    domain = f"gui/{uid}"
    service = f"{domain}/{LAUNCH_AGENT_LABEL}"
    runner(("launchctl", "bootout", service))
    result = runner(("launchctl", "bootstrap", domain, str(plist_path)))
    if result.returncode != 0:
        detail = result.stderr.strip() or "unknown launchctl error"
        msg = f"could not bootstrap watcher service: {detail}"
        raise RuntimeError(msg)
    runner(("launchctl", "enable", service))
    runner(("launchctl", "kickstart", "-k", service))
    return plist_path


def _user_id() -> int:
    result = subprocess.run(
        ("/usr/bin/id", "-u"),
        check=True,
        capture_output=True,
        text=True,
    )
    return int(result.stdout.strip())
