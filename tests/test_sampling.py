"""Tests for content-free PTY timestamp sampling."""

from pathlib import Path

from herdr_pomodoro.models import EndpointRole, TtyEndpoint
from herdr_pomodoro.sampling import AccessChangeTracker, iter_tty_samples


def test_tracker_uses_first_sample_as_baseline_and_ignores_mtime_only() -> None:
    endpoint = TtyEndpoint("client", EndpointRole.CLIENT, 1, Path("/dev/ttys001"))
    times = iter(((100, 200), (100, 300), (150, 300)))
    tracker = AccessChangeTracker()

    first = tuple(
        tracker.iter_changes(
            iter_tty_samples(
                [endpoint], observed_monotonic_ns=1, stat_reader=lambda _path: next(times)
            )
        )
    )
    output_only = tuple(
        tracker.iter_changes(
            iter_tty_samples(
                [endpoint], observed_monotonic_ns=2, stat_reader=lambda _path: next(times)
            )
        )
    )
    input_change = tuple(
        tracker.iter_changes(
            iter_tty_samples(
                [endpoint], observed_monotonic_ns=3, stat_reader=lambda _path: next(times)
            )
        )
    )

    assert first == ()
    assert output_only == ()
    assert len(input_change) == 1
    assert input_change[0].access_time_ns == 150


def test_sampler_skips_stale_devices() -> None:
    endpoint = TtyEndpoint("pane", EndpointRole.PANE, 2, Path("/dev/missing"))

    def missing(_path: Path) -> tuple[int, int]:
        raise FileNotFoundError

    assert tuple(iter_tty_samples([endpoint], stat_reader=missing)) == ()
