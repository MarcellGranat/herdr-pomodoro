"""One-to-one correlation of outer-client and inner-pane access changes."""

import hashlib
from dataclasses import dataclass, field
from typing import Iterable, Iterator

from herdr_pomodoro.constants import DEFAULT_CORRELATION_WINDOW_NS
from herdr_pomodoro.models import AccessChange, EndpointRole, HumanInputObservation


def _empty_changes() -> list[AccessChange]:
    return []


@dataclass(slots=True)
class CorrelationBuffer:
    """Match changes across polling boundaries after a short settling window."""

    correlation_window_ns: int = DEFAULT_CORRELATION_WINDOW_NS
    _pending: list[AccessChange] = field(default_factory=_empty_changes)

    def iter_ready(
        self,
        changes: Iterable[AccessChange],
        *,
        observed_monotonic_ns: int,
    ) -> Iterator[HumanInputObservation]:
        """Yield settled mutually-closest one-to-one client/pane matches."""
        self._pending.extend(changes)
        cutoff = observed_monotonic_ns - self.correlation_window_ns
        client_indices = tuple(
            index
            for index, change in enumerate(self._pending)
            if change.endpoint.role is EndpointRole.CLIENT
        )
        pane_indices = tuple(
            index
            for index, change in enumerate(self._pending)
            if change.endpoint.role is EndpointRole.PANE
        )
        matches: list[tuple[int, int]] = []
        for client_index in client_indices:
            pane_index = self._unique_nearest(client_index, pane_indices)
            if pane_index is None:
                continue
            nearest_client = self._unique_nearest(pane_index, client_indices)
            if nearest_client != client_index:
                continue
            client = self._pending[client_index]
            pane = self._pending[pane_index]
            if max(client.observed_monotonic_ns, pane.observed_monotonic_ns) > cutoff:
                continue
            matches.append((client_index, pane_index))
        consumed = {index for match in matches for index in match}
        for client_index, pane_index in sorted(
            matches,
            key=lambda match: max(
                self._pending[match[0]].observed_monotonic_ns,
                self._pending[match[1]].observed_monotonic_ns,
            ),
        ):
            yield _observation(self._pending[client_index], self._pending[pane_index])
        expiry = observed_monotonic_ns - (2 * self.correlation_window_ns)
        self._pending = [
            change
            for index, change in enumerate(self._pending)
            if index not in consumed and change.observed_monotonic_ns > expiry
        ]

    def retain(self, endpoint_ids: set[str]) -> None:
        """Discard pending changes belonging to stale topology endpoints."""
        self._pending = [
            change for change in self._pending if change.endpoint.endpoint_id in endpoint_ids
        ]

    def _unique_nearest(
        self,
        source_index: int,
        candidate_indices: tuple[int, ...],
    ) -> int | None:
        source = self._pending[source_index]
        candidates = tuple(
            (
                abs(source.access_time_ns - self._pending[index].access_time_ns),
                index,
            )
            for index in candidate_indices
            if abs(source.access_time_ns - self._pending[index].access_time_ns)
            <= self.correlation_window_ns
        )
        if not candidates:
            return None
        nearest_delta = min(delta for delta, _index in candidates)
        nearest = tuple(index for delta, index in candidates if delta == nearest_delta)
        return nearest[0] if len(nearest) == 1 else None


def iter_correlated_observations(
    changes: Iterable[AccessChange],
    *,
    correlation_window_ns: int = DEFAULT_CORRELATION_WINDOW_NS,
) -> Iterator[HumanInputObservation]:
    """Yield only unique client/pane timestamp matches.

    A change participates only when it has exactly one counterpart within the
    window and that counterpart also has exactly one candidate. This deliberately
    fails closed for bursts whose attribution is ambiguous.
    """
    buffered = tuple(changes)
    clients = tuple(change for change in buffered if change.endpoint.role is EndpointRole.CLIENT)
    panes = tuple(change for change in buffered if change.endpoint.role is EndpointRole.PANE)
    client_candidates = {
        index: tuple(
            pane_index
            for pane_index, pane in enumerate(panes)
            if abs(client.access_time_ns - pane.access_time_ns) <= correlation_window_ns
        )
        for index, client in enumerate(clients)
    }
    pane_candidates = {
        index: tuple(
            client_index
            for client_index, client in enumerate(clients)
            if abs(client.access_time_ns - pane.access_time_ns) <= correlation_window_ns
        )
        for index, pane in enumerate(panes)
    }
    for client_index, candidate_panes in client_candidates.items():
        if len(candidate_panes) != 1:
            continue
        pane_index = candidate_panes[0]
        if pane_candidates[pane_index] != (client_index,):
            continue
        yield _observation(clients[client_index], panes[pane_index])


def _observation(client: AccessChange, pane: AccessChange) -> HumanInputObservation:
    workspace_id = pane.endpoint.workspace_id
    pane_id = pane.endpoint.pane_id
    if workspace_id is None or pane_id is None:
        msg = "pane correlation endpoints require workspace and pane identifiers"
        raise ValueError(msg)
    identity = ":".join(
        (
            client.endpoint.endpoint_id,
            str(client.access_time_ns),
            pane.endpoint.endpoint_id,
            str(pane.access_time_ns),
        )
    )
    observation_id = hashlib.sha256(identity.encode()).hexdigest()
    return HumanInputObservation(
        observation_id=observation_id,
        observed_monotonic_ns=max(
            client.observed_monotonic_ns,
            pane.observed_monotonic_ns,
        ),
        input_time_ns=max(client.access_time_ns, pane.access_time_ns),
        correlation_delta_ns=abs(client.access_time_ns - pane.access_time_ns),
        client_endpoint_id=client.endpoint.endpoint_id,
        pane_endpoint_id=pane.endpoint.endpoint_id,
        workspace_id=workspace_id,
        pane_id=pane_id,
        cwd=pane.endpoint.cwd,
    )
