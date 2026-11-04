"""Tests for fail-closed PTY access-time correlation."""

from pathlib import Path, PurePosixPath

from herdr_pomodoro.correlation import iter_correlated_observations
from herdr_pomodoro.models import AccessChange, EndpointRole, TtyEndpoint

WINDOW_NS = 150_000_000


def _change(endpoint: TtyEndpoint, access_time_ns: int) -> AccessChange:
    return AccessChange(endpoint, access_time_ns + 1, access_time_ns - 1, access_time_ns)


def _client(name: str = "client") -> TtyEndpoint:
    return TtyEndpoint(name, EndpointRole.CLIENT, 1, Path(f"/dev/{name}"), client_id=name)


def _pane(name: str = "pane", workspace: str = "workspace") -> TtyEndpoint:
    return TtyEndpoint(
        name,
        EndpointRole.PANE,
        2,
        Path(f"/dev/{name}"),
        workspace_id=workspace,
        pane_id=name,
        cwd=PurePosixPath("/work/project"),
    )


def test_correlates_unique_outer_and_inner_access_changes() -> None:
    changes = [_change(_client(), 1_000_000_000), _change(_pane(), 1_010_000_000)]

    observations = tuple(iter_correlated_observations(changes, correlation_window_ns=WINDOW_NS))

    assert len(observations) == 1
    assert observations[0].workspace_id == "workspace"
    assert observations[0].correlation_delta_ns == 10_000_000


def test_does_not_count_navigation_or_programmatic_input() -> None:
    outer_only = tuple(iter_correlated_observations([_change(_client(), 1_000_000_000)]))
    inner_only = tuple(iter_correlated_observations([_change(_pane(), 1_000_000_000)]))

    assert outer_only == ()
    assert inner_only == ()


def test_discards_ambiguous_burst() -> None:
    changes = [
        _change(_client(), 1_000_000_000),
        _change(_pane("pane-a"), 1_010_000_000),
        _change(_pane("pane-b"), 1_020_000_000),
    ]

    assert tuple(iter_correlated_observations(changes, correlation_window_ns=WINDOW_NS)) == ()


def test_does_not_reuse_one_pane_change_for_two_clients() -> None:
    changes = [
        _change(_client("client-a"), 1_000_000_000),
        _change(_client("client-b"), 1_010_000_000),
        _change(_pane(), 1_020_000_000),
    ]

    assert tuple(iter_correlated_observations(changes, correlation_window_ns=WINDOW_NS)) == ()


def test_observation_id_is_deterministic() -> None:
    changes = [_change(_client(), 1_000_000_000), _change(_pane(), 1_010_000_000)]

    first = next(iter_correlated_observations(changes))
    second = next(iter_correlated_observations(changes))

    assert first.observation_id == second.observation_id
