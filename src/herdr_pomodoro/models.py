"""Typed, content-free models for PTY activity correlation."""

from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path, PurePosixPath


class EndpointRole(StrEnum):
    """Role played by a terminal endpoint in input correlation."""

    CLIENT = "client"
    PANE = "pane"


@dataclass(frozen=True, slots=True)
class TtyEndpoint:
    """One terminal device whose metadata may be sampled."""

    endpoint_id: str
    role: EndpointRole
    pid: int
    device: Path
    client_id: str | None = None
    workspace_id: str | None = None
    pane_id: str | None = None
    cwd: PurePosixPath | None = None


@dataclass(frozen=True, slots=True)
class PtyTopology:
    """Locally observable Herdr clients and terminal panes."""

    clients: tuple[TtyEndpoint, ...]
    panes: tuple[TtyEndpoint, ...]

    @property
    def endpoints(self) -> tuple[TtyEndpoint, ...]:
        """Return all endpoints in deterministic role order."""
        return (*self.clients, *self.panes)


@dataclass(frozen=True, slots=True)
class TtySample:
    """One metadata-only observation of a terminal device."""

    endpoint: TtyEndpoint
    observed_monotonic_ns: int
    access_time_ns: int
    modification_time_ns: int


@dataclass(frozen=True, slots=True)
class AccessChange:
    """A terminal access-time transition without any input content."""

    endpoint: TtyEndpoint
    observed_monotonic_ns: int
    previous_access_time_ns: int
    access_time_ns: int


@dataclass(frozen=True, slots=True)
class HumanInputObservation:
    """A correlated outer-client and inner-pane human input observation."""

    observation_id: str
    observed_monotonic_ns: int
    input_time_ns: int
    correlation_delta_ns: int
    client_endpoint_id: str
    pane_endpoint_id: str
    workspace_id: str
    pane_id: str
    cwd: PurePosixPath | None
    provenance: str = "local-pty-atime-correlation-v1"
