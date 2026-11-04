"""Sample terminal metadata and detect access-time changes."""

import time
from pathlib import Path
from typing import Callable, Iterable, Iterator

from herdr_pomodoro.models import AccessChange, TtyEndpoint, TtySample

StatReader = Callable[[Path], tuple[int, int]]


def read_tty_times(device: Path) -> tuple[int, int]:
    """Read nanosecond access and modification timestamps for one PTY."""
    metadata = device.stat()
    return metadata.st_atime_ns, metadata.st_mtime_ns


def iter_tty_samples(
    endpoints: Iterable[TtyEndpoint],
    *,
    observed_monotonic_ns: int | None = None,
    stat_reader: StatReader = read_tty_times,
) -> Iterator[TtySample]:
    """Yield metadata samples for accessible endpoints, skipping stale devices."""
    observed_at = time.monotonic_ns() if observed_monotonic_ns is None else observed_monotonic_ns
    for endpoint in endpoints:
        try:
            access_time_ns, modification_time_ns = stat_reader(endpoint.device)
        except (FileNotFoundError, PermissionError):
            continue
        yield TtySample(
            endpoint=endpoint,
            observed_monotonic_ns=observed_at,
            access_time_ns=access_time_ns,
            modification_time_ns=modification_time_ns,
        )


class AccessChangeTracker:
    """Retain the latest PTY access time and emit forward transitions."""

    def __init__(self) -> None:
        """Initialize an empty endpoint baseline map."""
        self._access_times: dict[str, int] = {}

    def iter_changes(self, samples: Iterable[TtySample]) -> Iterator[AccessChange]:
        """Yield access changes; the first sample establishes a baseline."""
        for sample in samples:
            endpoint_id = sample.endpoint.endpoint_id
            previous = self._access_times.get(endpoint_id)
            self._access_times[endpoint_id] = sample.access_time_ns
            if previous is None or sample.access_time_ns <= previous:
                continue
            yield AccessChange(
                endpoint=sample.endpoint,
                observed_monotonic_ns=sample.observed_monotonic_ns,
                previous_access_time_ns=previous,
                access_time_ns=sample.access_time_ns,
            )

    def retain(self, endpoint_ids: set[str]) -> None:
        """Forget baselines for endpoints no longer present in the topology."""
        self._access_times = {
            endpoint_id: access_time
            for endpoint_id, access_time in self._access_times.items()
            if endpoint_id in endpoint_ids
        }
