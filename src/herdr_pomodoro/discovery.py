"""Discover same-user Herdr client and pane PTYs."""

import json
import subprocess
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Callable, Iterator, cast

from herdr_pomodoro.constants import HERDR_COMMAND
from herdr_pomodoro.json_types import JsonValue
from herdr_pomodoro.models import EndpointRole, PtyTopology, TtyEndpoint

PROCESS_COMMAND = ("ps", "-axo", "pid=,ppid=,tty=,comm=,args=")
SNAPSHOT_COMMAND = ("api", "snapshot")
PANE_PROCESS_COMMAND = ("pane", "process-info", "--pane")
NON_CLIENT_SUBCOMMANDS = frozenset({"remote-client-bridge", "server"})
PROCESS_FIELD_COUNT = 5
MAX_PARALLEL_PANE_QUERIES = 8


class DiscoveryError(RuntimeError):
    """Raised when a safe local PTY topology cannot be discovered."""


@dataclass(frozen=True, slots=True)
class CommandResult:
    """Minimal subprocess result used by injectable discovery commands."""

    returncode: int
    stdout: str
    stderr: str


CommandRunner = Callable[[tuple[str, ...]], CommandResult]


@dataclass(frozen=True, slots=True)
class ProcessRecord:
    """Relevant fields from one same-user process table row."""

    pid: int
    parent_pid: int
    tty: str
    command: str
    arguments: str


def run_command(arguments: tuple[str, ...]) -> CommandResult:
    """Run one read-only topology command."""
    completed = subprocess.run(  # noqa: S603
        arguments,
        check=False,
        capture_output=True,
        text=True,
    )
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def iter_process_records(output: str) -> Iterator[ProcessRecord]:
    """Yield valid process records from portable `ps` output."""
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=4)
        if len(fields) != PROCESS_FIELD_COUNT:
            continue
        pid, parent_pid, tty, command, arguments = fields
        try:
            yield ProcessRecord(
                pid=int(pid),
                parent_pid=int(parent_pid),
                tty=tty,
                command=command,
                arguments=arguments,
            )
        except ValueError:
            continue


def discover_topology(
    runner: CommandRunner = run_command,
    herdr_command: str = HERDR_COMMAND,
) -> PtyTopology:
    """Discover local Herdr clients and panes without reading PTY contents."""
    process_result = runner(PROCESS_COMMAND)
    _require_success(process_result, "process table")
    processes = tuple(iter_process_records(process_result.stdout))
    process_by_pid = {process.pid: process for process in processes}
    clients = tuple(_iter_clients(processes))

    snapshot_result = runner((herdr_command, *SNAPSHOT_COMMAND))
    _require_success(snapshot_result, "Herdr snapshot")
    snapshot = _json_object(snapshot_result.stdout, "Herdr snapshot")
    snapshot_panes = _snapshot_panes(snapshot)
    process_results = _pane_process_results(snapshot_panes, runner, herdr_command)
    panes = tuple(
        _iter_panes(
            snapshot_panes,
            process_by_pid=process_by_pid,
            process_results=process_results,
        )
    )
    return PtyTopology(clients=clients, panes=panes)


def _iter_clients(processes: tuple[ProcessRecord, ...]) -> Iterator[TtyEndpoint]:
    for process in processes:
        if process.tty in {"?", "-"}:
            continue
        command_name = Path(process.command).name
        arguments = process.arguments.split()
        if command_name != HERDR_COMMAND and (
            not arguments or Path(arguments[0]).name != HERDR_COMMAND
        ):
            continue
        if len(arguments) > 1 and arguments[1] in NON_CLIENT_SUBCOMMANDS:
            continue
        device = Path("/dev") / process.tty
        yield TtyEndpoint(
            endpoint_id=f"client:{process.pid}:{process.tty}",
            role=EndpointRole.CLIENT,
            pid=process.pid,
            device=device,
            client_id=f"pid-{process.pid}",
        )


def _iter_panes(
    panes: tuple[dict[str, JsonValue], ...],
    *,
    process_by_pid: dict[int, ProcessRecord],
    process_results: dict[str, CommandResult],
) -> Iterator[TtyEndpoint]:
    for pane in panes:
        pane_id = _required_string(pane, "pane_id")
        process_result = process_results[pane_id]
        if process_result.returncode != 0:
            continue
        process_payload = _json_object(process_result.stdout, f"process info for {pane_id}")
        process_info = _nested_object(process_payload, ("result", "process_info"))
        shell_pid = _required_integer(process_info, "shell_pid")
        process = process_by_pid.get(shell_pid)
        if process is None or process.tty in {"?", "-"}:
            continue
        workspace_id = _required_string(pane, "workspace_id")
        cwd_value = pane.get("cwd")
        cwd = PurePosixPath(cwd_value) if isinstance(cwd_value, str) and cwd_value else None
        yield TtyEndpoint(
            endpoint_id=f"pane:{pane_id}:{shell_pid}:{process.tty}",
            role=EndpointRole.PANE,
            pid=shell_pid,
            device=Path("/dev") / process.tty,
            workspace_id=workspace_id,
            pane_id=pane_id,
            cwd=cwd,
        )


def _pane_process_results(
    panes: tuple[dict[str, JsonValue], ...],
    runner: CommandRunner,
    herdr_command: str,
) -> dict[str, CommandResult]:
    pane_ids = tuple(_required_string(pane, "pane_id") for pane in panes)

    def query(pane_id: str) -> CommandResult:
        return runner((herdr_command, *PANE_PROCESS_COMMAND, pane_id))

    with ThreadPoolExecutor(
        max_workers=min(MAX_PARALLEL_PANE_QUERIES, len(pane_ids) or 1)
    ) as executor:
        results = tuple(executor.map(query, pane_ids))
    return dict(zip(pane_ids, results, strict=True))


def _snapshot_panes(payload: dict[str, JsonValue]) -> tuple[dict[str, JsonValue], ...]:
    snapshot = _nested_object(payload, ("result", "snapshot"))
    raw_panes = snapshot.get("panes")
    if not isinstance(raw_panes, list):
        msg = "Herdr snapshot panes must be an array"
        raise DiscoveryError(msg)
    panes: list[dict[str, JsonValue]] = []
    for raw_pane in raw_panes:
        if not isinstance(raw_pane, dict):
            msg = "each Herdr snapshot pane must be an object"
            raise DiscoveryError(msg)
        panes.append(raw_pane)
    return tuple(panes)


def _json_object(raw: str, label: str) -> dict[str, JsonValue]:
    try:
        payload = cast("JsonValue", json.loads(raw))
    except json.JSONDecodeError as error:
        msg = f"{label} returned invalid JSON"
        raise DiscoveryError(msg) from error
    if not isinstance(payload, dict):
        msg = f"{label} must be a JSON object"
        raise DiscoveryError(msg)
    return payload


def _nested_object(payload: dict[str, JsonValue], path: tuple[str, ...]) -> dict[str, JsonValue]:
    current = payload
    for key in path:
        value = current.get(key)
        if not isinstance(value, dict):
            msg = f"missing object at {'.'.join(path)}"
            raise DiscoveryError(msg)
        current = value
    return current


def _required_string(payload: dict[str, JsonValue], key: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        msg = f"{key} must be a non-empty string"
        raise DiscoveryError(msg)
    return value


def _required_integer(payload: dict[str, JsonValue], key: str) -> int:
    value = payload.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        msg = f"{key} must be an integer"
        raise DiscoveryError(msg)
    return value


def _require_success(result: CommandResult, label: str) -> None:
    if result.returncode != 0:
        detail = result.stderr.strip() or "unknown error"
        msg = f"could not inspect {label}: {detail}"
        raise DiscoveryError(msg)
