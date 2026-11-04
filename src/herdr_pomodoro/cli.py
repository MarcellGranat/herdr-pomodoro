"""Command-line entrypoint for Herdr Pomodoro actions and panes."""

import argparse
import sys
from typing import Sequence

from herdr_pomodoro.capabilities import CapabilityDetectionError, detect_installed_capabilities
from herdr_pomodoro.discovery import DiscoveryError
from herdr_pomodoro.watcher import iter_live_observations, observation_to_json


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
    parser.add_argument(
        "--duration",
        type=float,
        default=None,
        help="stop the watcher after this many seconds",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run one plugin command."""
    arguments = build_parser().parse_args(argv)
    if arguments.command in {"status", "reconcile"}:
        try:
            report = detect_installed_capabilities()
        except CapabilityDetectionError as error:
            sys.stdout.write(f"Herdr Pomodoro: {error}\n")
            return 1
        sys.stdout.write(f"Herdr Pomodoro: {report.diagnostic}\n")
        return 0
    if arguments.command == "watch":
        try:
            for observation in iter_live_observations(duration_seconds=arguments.duration):
                sys.stdout.write(f"{observation_to_json(observation)}\n")
                sys.stdout.flush()
        except (CapabilityDetectionError, DiscoveryError) as error:
            sys.stderr.write(f"Herdr Pomodoro: {error}\n")
            return 1
        return 0
    sys.stdout.write(f"Herdr Pomodoro: {arguments.command} is not implemented yet.\n")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
