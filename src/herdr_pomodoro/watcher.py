"""Long-running metadata-only PTY observation watcher."""

import json
import time
from dataclasses import dataclass, field
from typing import Callable, Iterator

from herdr_pomodoro.constants import DEFAULT_SAMPLE_INTERVAL_MILLISECONDS
from herdr_pomodoro.correlation import iter_correlated_observations
from herdr_pomodoro.discovery import discover_topology
from herdr_pomodoro.models import HumanInputObservation, PtyTopology
from herdr_pomodoro.sampling import AccessChangeTracker, iter_tty_samples

NANOSECONDS_PER_SECOND = 1_000_000_000
MILLISECONDS_PER_SECOND = 1_000
DEFAULT_TOPOLOGY_REFRESH_SECONDS = 60.0

TopologyDiscoverer = Callable[[], PtyTopology]


@dataclass(slots=True)
class ObservationWatcher:
    """Reconcile topology and emit correlations while retaining timestamp baselines."""

    discover: TopologyDiscoverer = discover_topology
    refresh_seconds: float = DEFAULT_TOPOLOGY_REFRESH_SECONDS
    tracker: AccessChangeTracker = field(default_factory=AccessChangeTracker)
    _topology: PtyTopology | None = None
    _next_refresh_ns: int = 0

    def topology_at(self, observed_monotonic_ns: int) -> PtyTopology:
        """Refresh stale topology mappings and discard removed endpoint baselines."""
        if self._topology is None or observed_monotonic_ns >= self._next_refresh_ns:
            self._topology = self.discover()
            endpoint_ids = {endpoint.endpoint_id for endpoint in self._topology.endpoints}
            self.tracker.retain(endpoint_ids)
            self._next_refresh_ns = observed_monotonic_ns + int(
                self.refresh_seconds * NANOSECONDS_PER_SECOND
            )
        return self._topology

    def iter_poll(self, observed_monotonic_ns: int) -> Iterator[HumanInputObservation]:
        """Sample all endpoints once and yield unique same-poll correlations."""
        topology = self.topology_at(observed_monotonic_ns)
        samples = iter_tty_samples(
            topology.endpoints,
            observed_monotonic_ns=observed_monotonic_ns,
        )
        changes = tuple(self.tracker.iter_changes(samples))
        yield from iter_correlated_observations(changes)


def iter_live_observations(
    *,
    duration_seconds: float | None = None,
    sample_interval_milliseconds: int = DEFAULT_SAMPLE_INTERVAL_MILLISECONDS,
    watcher: ObservationWatcher | None = None,
) -> Iterator[HumanInputObservation]:
    """Yield live human-input observations until the optional duration expires."""
    active_watcher = ObservationWatcher() if watcher is None else watcher
    started_ns = time.monotonic_ns()
    duration_ns = (
        None if duration_seconds is None else int(duration_seconds * NANOSECONDS_PER_SECOND)
    )
    sleep_seconds = sample_interval_milliseconds / MILLISECONDS_PER_SECOND
    while duration_ns is None or time.monotonic_ns() - started_ns < duration_ns:
        observed_ns = time.monotonic_ns()
        yield from active_watcher.iter_poll(observed_ns)
        time.sleep(sleep_seconds)


def observation_to_json(observation: HumanInputObservation) -> str:
    """Serialize content-free observation metadata as one JSON line."""
    payload = {
        "client_endpoint_id": observation.client_endpoint_id,
        "correlation_delta_ns": observation.correlation_delta_ns,
        "cwd": None if observation.cwd is None else str(observation.cwd),
        "input_time_ns": observation.input_time_ns,
        "observation_id": observation.observation_id,
        "observed_monotonic_ns": observation.observed_monotonic_ns,
        "pane_endpoint_id": observation.pane_endpoint_id,
        "pane_id": observation.pane_id,
        "provenance": observation.provenance,
        "workspace_id": observation.workspace_id,
    }
    return json.dumps(payload, separators=(",", ":"), sort_keys=True)
