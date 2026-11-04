"""One-to-one correlation of outer-client and inner-pane access changes."""

import hashlib
from typing import Iterable, Iterator

from herdr_pomodoro.constants import DEFAULT_CORRELATION_WINDOW_NS
from herdr_pomodoro.models import AccessChange, EndpointRole, HumanInputObservation


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
