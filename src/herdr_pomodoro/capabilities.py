"""Capability diagnostics for privacy-preserving PTY correlation."""

from dataclasses import dataclass

from herdr_pomodoro.discovery import CommandRunner, DiscoveryError, discover_topology, run_command
from herdr_pomodoro.models import PtyTopology


class CapabilityDetectionError(RuntimeError):
    """Raised when the installed Herdr topology cannot be inspected."""


@dataclass(frozen=True, slots=True)
class CapabilityReport:
    """Safety and availability of local PTY correlation."""

    capture_supported: bool
    client_count: int
    pane_count: int
    diagnostic: str


def capabilities_from_topology(topology: PtyTopology) -> CapabilityReport:
    """Build a fail-closed report from a discovered topology."""
    client_count = len(topology.clients)
    pane_count = len(topology.panes)
    if client_count == 0:
        diagnostic = "capture is disabled: no local Herdr client PTY was resolved"
    elif pane_count == 0:
        diagnostic = (
            "capture is disabled: no local pane PTY was resolved; native remote panes "
            "require the optional remote metadata sampler"
        )
    else:
        diagnostic = (
            f"human input capture is supported for {client_count} client PTY(s) and "
            f"{pane_count} local pane PTY(s)"
        )
    return CapabilityReport(
        capture_supported=client_count > 0 and pane_count > 0,
        client_count=client_count,
        pane_count=pane_count,
        diagnostic=diagnostic,
    )


def detect_installed_capabilities(
    runner: CommandRunner = run_command,
) -> CapabilityReport:
    """Discover the installed topology and report whether capture is safe."""
    try:
        topology = discover_topology(runner=runner)
    except DiscoveryError as error:
        raise CapabilityDetectionError(str(error)) from error
    return capabilities_from_topology(topology)
