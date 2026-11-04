"""Command-line entrypoint for Herdr Pomodoro actions and panes."""

import argparse
import json
import platform
import sys
import time
from dataclasses import replace
from datetime import date, datetime
from os import environ
from typing import Sequence, cast

from herdr_pomodoro.capabilities import CapabilityDetectionError, detect_installed_capabilities
from herdr_pomodoro.config import discover_paths, load_config
from herdr_pomodoro.dashboard import DashboardFilters, render_dashboard
from herdr_pomodoro.database import Database
from herdr_pomodoro.discovery import DiscoveryError
from herdr_pomodoro.herdr_runtime import (
    HerdrPresentationController,
    HerdrRuntimeError,
    focused_context,
    open_plugin_pane,
)
from herdr_pomodoro.json_types import JsonValue
from herdr_pomodoro.pomodoro import Phase, PomodoroEngine
from herdr_pomodoro.service import WatcherAlreadyRunningError, reconcile_macos_service
from herdr_pomodoro.watcher import run_persistent_watcher


def build_parser() -> argparse.ArgumentParser:
    """Build the plugin command parser."""
    parser = argparse.ArgumentParser(prog="herdr-pomodoro")
    parser.add_argument(
        "command",
        choices=(
            "agent-event",
            "away",
            "dashboard",
            "end",
            "open-dashboard",
            "reconcile",
            "start",
            "status",
            "timer",
            "watch",
            "workspace-grace",
        ),
    )
    parser.add_argument("--date", dest="selected_date", default=None)
    parser.add_argument("--project", default=None)
    parser.add_argument("--workspace", default=None)
    parser.add_argument("--machine", default=None)
    parser.add_argument("--session", default=None)
    parser.add_argument("--category", default=None)
    parser.add_argument("--revision", default=None)
    parser.add_argument("--once", action="store_true")
    parser.add_argument(
        "--duration",
        type=float,
        default=None,
        help="stop the watcher after this many seconds",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:  # noqa: C901, PLR0911
    """Run one plugin command."""
    arguments = build_parser().parse_args(argv)
    if arguments.command == "status":
        try:
            report = detect_installed_capabilities()
        except CapabilityDetectionError as error:
            sys.stdout.write(f"Herdr Pomodoro: {error}\n")
            return 1
        sys.stdout.write(f"Herdr Pomodoro: {report.diagnostic}\n")
        return 0
    if arguments.command == "reconcile":
        if platform.system() != "Darwin":
            sys.stdout.write("Herdr Pomodoro: automatic watcher service requires macOS.\n")
            return 0
        paths = discover_paths()
        plist_path = reconcile_macos_service(paths)
        sys.stdout.write(f"Herdr Pomodoro: watcher service reconciled at {plist_path}.\n")
        return 0
    if arguments.command == "watch":
        try:
            paths = discover_paths()
            return run_persistent_watcher(
                paths,
                load_config(paths),
                duration_seconds=arguments.duration,
                emit_json=arguments.duration is not None,
            )
        except (CapabilityDetectionError, DiscoveryError, WatcherAlreadyRunningError) as error:
            sys.stderr.write(f"Herdr Pomodoro: {error}\n")
            return 1
        return 0
    if arguments.command in {"start", "away", "workspace-grace", "end", "agent-event"}:
        try:
            return _run_session_action(arguments.command)
        except (HerdrRuntimeError, RuntimeError, ValueError) as error:
            sys.stderr.write(f"Herdr Pomodoro: {error}\n")
            return 1
    if arguments.command == "timer":
        return _run_timer()
    if arguments.command == "open-dashboard":
        if not open_plugin_pane("dashboard"):
            sys.stderr.write("Herdr Pomodoro: could not open dashboard pane.\n")
            return 1
        return 0
    if arguments.command == "dashboard":
        selected = (
            datetime.now().astimezone().date()
            if arguments.selected_date is None
            else date.fromisoformat(arguments.selected_date)
        )
        filters = replace(
            DashboardFilters.for_day(selected),
            project_id=arguments.project,
            workspace_id=arguments.workspace,
            machine_id=arguments.machine,
            session_id=arguments.session,
            category=arguments.category,
            revision_id=arguments.revision,
        )
        return _run_dashboard(filters, once=arguments.once)
    sys.stdout.write(f"Herdr Pomodoro: {arguments.command} is not implemented yet.\n")
    return 2


def _pomodoro_engine() -> PomodoroEngine:
    paths = discover_paths()
    config = load_config(paths)
    return PomodoroEngine(
        Database(paths.database_file),
        config,
        presentation=HerdrPresentationController(),
    )


def _run_session_action(command: str) -> int:
    engine = _pomodoro_engine()
    if command == "start":
        state = engine.start_focus(focused_context().workspace_id)
        open_plugin_pane("timer")
    elif command == "away":
        state = engine.start_grace(Phase.AWAY_GRACE)
    elif command == "workspace-grace":
        state = engine.start_grace(Phase.WORKSPACE_GRACE)
    elif command == "end":
        state = engine.end_session()
    else:
        payload = _stdin_json()
        raw_status = payload.get("agent_status", payload.get("status"))
        source = payload.get("source", "herdr-agent")
        state = engine.set_agent_protection(
            active=raw_status == "working",
            source=source if isinstance(source, str) else "herdr-agent",
        )
    sys.stdout.write(
        _timer_line(
            state.phase,
            state.remaining_seconds,
            presentation_degraded=state.presentation_degraded,
        )
    )
    return 0


def _run_timer() -> int:
    engine = _pomodoro_engine()
    while True:
        state = engine.state()
        line = _timer_line(
            state.phase,
            state.remaining_seconds,
            presentation_degraded=state.presentation_degraded,
        )
        sys.stdout.write(f"\r\x1b[2K{line.rstrip()}")
        sys.stdout.flush()
        if state.phase is Phase.IDLE:
            sys.stdout.write("\n")
            return 0
        time.sleep(1)


def _run_dashboard(filters: DashboardFilters, *, once: bool) -> int:
    paths = discover_paths()
    database = Database(paths.database_file)
    while True:
        sys.stdout.write("\x1b[2J\x1b[H")
        sys.stdout.write(render_dashboard(database, filters))
        sys.stdout.flush()
        if once:
            return 0
        time.sleep(2)


def _timer_line(
    phase: Phase,
    remaining_seconds: int,
    *,
    presentation_degraded: bool,
) -> str:
    minutes, seconds = divmod(remaining_seconds, 60)
    degraded = " (presentation limited)" if presentation_degraded else ""
    return f"Herdr Pomodoro: {phase} {minutes:02d}:{seconds:02d}{degraded}\n"


def _stdin_json() -> dict[str, JsonValue]:
    raw = sys.stdin.read() or environ.get("HERDR_PLUGIN_EVENT_JSON", "")
    if not raw.strip():
        return {}
    payload = cast("JsonValue", json.loads(raw))
    if not isinstance(payload, dict):
        msg = "agent event must be a JSON object"
        raise TypeError(msg)
    return payload


if __name__ == "__main__":
    raise SystemExit(main())
