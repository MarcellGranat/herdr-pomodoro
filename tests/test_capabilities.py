"""Tests for fail-closed topology capability diagnostics."""

from pathlib import Path

from herdr_pomodoro.capabilities import capabilities_from_topology
from herdr_pomodoro.models import EndpointRole, PtyTopology, TtyEndpoint


def test_reports_supported_local_topology() -> None:
    client = TtyEndpoint("client", EndpointRole.CLIENT, 1, Path("/dev/ttys001"))
    pane = TtyEndpoint(
        "pane",
        EndpointRole.PANE,
        2,
        Path("/dev/ttys002"),
        workspace_id="workspace",
        pane_id="pane",
    )

    report = capabilities_from_topology(PtyTopology((client,), (pane,)))

    assert report.capture_supported
    assert "1 client PTY" in report.diagnostic


def test_disables_capture_without_client() -> None:
    report = capabilities_from_topology(PtyTopology((), ()))

    assert not report.capture_supported
    assert "no local Herdr client PTY" in report.diagnostic


def test_explains_native_remote_requirement_when_no_pane_is_local() -> None:
    client = TtyEndpoint("client", EndpointRole.CLIENT, 1, Path("/dev/ttys001"))

    report = capabilities_from_topology(PtyTopology((client,), ()))

    assert not report.capture_supported
    assert "remote metadata sampler" in report.diagnostic
