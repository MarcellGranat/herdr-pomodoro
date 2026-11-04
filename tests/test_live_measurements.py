"""Regression checks for measured SSH/mosh PTY timing evidence."""

import json
from pathlib import Path
from typing import cast

from herdr_pomodoro.json_types import JsonValue

FIXTURE_PATH = Path(__file__).parent / "fixtures" / "live_pty_measurements.json"


def test_measured_human_input_fits_correlation_window() -> None:
    payload = cast("JsonValue", json.loads(FIXTURE_PATH.read_text(encoding="utf-8")))
    assert isinstance(payload, dict)
    human_input = payload["human_input"]
    window = payload["configured_correlation_window_ns"]
    assert isinstance(human_input, dict)
    delta = human_input["absolute_delta_ns"]
    assert isinstance(delta, int)
    assert isinstance(window, int)
    assert delta < window


def test_idle_output_did_not_look_like_input() -> None:
    payload = cast("JsonValue", json.loads(FIXTURE_PATH.read_text(encoding="utf-8")))
    assert isinstance(payload, dict)

    assert payload["idle_access_time_changed"] is False
    assert payload["output_modification_time_changed"] is True
    assert payload["programmatic_pane_input_observations"] == 0
    assert payload["api_workspace_navigation_observations"] == 0
    assert payload["human_navigation_only_observations"] == 0
    paste_observations = payload["human_paste_observations"]
    segments = payload["human_active_segments_after_paste"]
    active_ns = payload["human_active_ns_after_paste"]
    assert isinstance(paste_observations, int)
    assert paste_observations > 1
    assert isinstance(segments, int)
    assert segments > 0
    assert isinstance(active_ns, int)
    assert active_ns > 0
