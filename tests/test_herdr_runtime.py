"""Tests for exact best-effort Herdr presentation restoration."""

import json

from herdr_pomodoro.discovery import CommandResult
from herdr_pomodoro.herdr_runtime import HerdrPresentationController, open_plugin_pane


def test_capture_reduce_and_restore_supported_presentation() -> None:
    calls: list[tuple[str, ...]] = []
    snapshot = json.dumps(
        {
            "result": {
                "snapshot": {
                    "focused_workspace_id": "A",
                    "focused_pane_id": "p1",
                    "layouts": [{"focused_pane_id": "p1", "zoomed": False}],
                }
            }
        }
    )

    def runner(arguments: tuple[str, ...]) -> CommandResult:
        calls.append(arguments)
        if arguments == ("api", "snapshot"):
            return CommandResult(0, snapshot, "")
        return CommandResult(0, "{}", "")

    controller = HerdrPresentationController(runner=runner)
    captured = controller.capture()

    assert controller.reduce("A")
    controller.restore(captured)
    assert ("pane", "zoom", "--pane", "p1", "--on") in calls
    assert calls[-1] == ("pane", "zoom", "--pane", "p1", "--off")


def test_opens_timer_controller_in_focused_split() -> None:
    calls: list[tuple[str, ...]] = []

    def runner(arguments: tuple[str, ...]) -> CommandResult:
        calls.append(arguments)
        return CommandResult(0, "{}", "")

    assert open_plugin_pane("timer", runner)
    assert calls[0] == (
        "plugin",
        "pane",
        "open",
        "--plugin",
        "chmarax.herdr-pomodoro",
        "--entrypoint",
        "timer",
    )
