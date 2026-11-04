"""Small typed boundary around installed Herdr runtime context and presentation."""

import json
import subprocess
from dataclasses import dataclass
from os import environ
from typing import cast

from herdr_pomodoro.discovery import CommandResult, CommandRunner
from herdr_pomodoro.json_types import JsonValue
from herdr_pomodoro.pomodoro import PresentationController

PLUGIN_ID = "chmarax.herdr-pomodoro"


class HerdrRuntimeError(RuntimeError):
    """Raised when required live Herdr context cannot be resolved."""


@dataclass(frozen=True, slots=True)
class FocusContext:
    """Currently focused runtime identities."""

    workspace_id: str
    pane_id: str


def run_herdr(arguments: tuple[str, ...]) -> CommandResult:
    """Run one Herdr control command."""
    executable = environ.get("HERDR_BIN_PATH", "herdr")
    completed = subprocess.run(  # noqa: S603
        (executable, *arguments),
        check=False,
        capture_output=True,
        text=True,
    )
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def focused_context(runner: CommandRunner = run_herdr) -> FocusContext:
    """Read focused workspace and pane from one runtime snapshot."""
    result = runner(("api", "snapshot"))
    if result.returncode != 0:
        msg = result.stderr.strip() or "could not read Herdr snapshot"
        raise HerdrRuntimeError(msg)
    payload = cast("JsonValue", json.loads(result.stdout))
    snapshot = _snapshot(payload)
    workspace_id = snapshot.get("focused_workspace_id")
    pane_id = snapshot.get("focused_pane_id")
    if not isinstance(workspace_id, str) or not isinstance(pane_id, str):
        msg = "Herdr snapshot has no focused workspace and pane"
        raise HerdrRuntimeError(msg)
    return FocusContext(workspace_id, pane_id)


def open_plugin_pane(command: str, runner: CommandRunner = run_herdr) -> bool:
    """Open a declared plugin pane so Herdr retains pane ownership."""
    launched = runner(
        (
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            command,
        )
    )
    return launched.returncode == 0


@dataclass(slots=True)
class HerdrPresentationController(PresentationController):
    """Capture and restore supported workspace-focus and pane-zoom state."""

    runner: CommandRunner = run_herdr

    def capture(self) -> str:
        """Capture focused workspace/pane and that pane's zoom state."""
        result = self.runner(("api", "snapshot"))
        if result.returncode != 0:
            msg = "could not capture Herdr presentation state"
            raise HerdrRuntimeError(msg)
        payload = cast("JsonValue", json.loads(result.stdout))
        snapshot = _snapshot(payload)
        context = focused_context(self.runner)
        zoomed = False
        layouts = snapshot.get("layouts")
        if isinstance(layouts, list):
            for layout in layouts:
                if isinstance(layout, dict) and layout.get("focused_pane_id") == context.pane_id:
                    zoomed = layout.get("zoomed") is True
                    break
        return json.dumps(
            {
                "pane_id": context.pane_id,
                "workspace_id": context.workspace_id,
                "zoomed": zoomed,
            },
            sort_keys=True,
        )

    def reduce(self, focused_workspace_id: str) -> bool:
        """Focus the selected workspace and zoom its current pane."""
        focus_result = self.runner(("workspace", "focus", focused_workspace_id))
        context = focused_context(self.runner)
        zoom_result = self.runner(("pane", "zoom", "--pane", context.pane_id, "--on"))
        return focus_result.returncode == 0 and zoom_result.returncode == 0

    def restore(self, state: str) -> None:
        """Restore exactly the captured workspace focus and pane zoom value."""
        payload = cast("JsonValue", json.loads(state))
        if not isinstance(payload, dict):
            msg = "captured presentation state is invalid"
            raise HerdrRuntimeError(msg)
        workspace_id = payload.get("workspace_id")
        pane_id = payload.get("pane_id")
        zoomed = payload.get("zoomed")
        if not isinstance(workspace_id, str) or not isinstance(pane_id, str):
            msg = "captured presentation state lacks identifiers"
            raise HerdrRuntimeError(msg)
        self.runner(("workspace", "focus", workspace_id))
        mode = "--on" if zoomed is True else "--off"
        result = self.runner(("pane", "zoom", "--pane", pane_id, mode))
        if result.returncode != 0:
            msg = "could not restore Herdr pane zoom"
            raise HerdrRuntimeError(msg)


def _snapshot(payload: JsonValue) -> dict[str, JsonValue]:
    if not isinstance(payload, dict):
        msg = "Herdr snapshot response must be an object"
        raise HerdrRuntimeError(msg)
    result = payload.get("result")
    if not isinstance(result, dict):
        msg = "Herdr snapshot response has no result"
        raise HerdrRuntimeError(msg)
    snapshot = result.get("snapshot")
    if not isinstance(snapshot, dict):
        msg = "Herdr snapshot response has no snapshot"
        raise HerdrRuntimeError(msg)
    return snapshot
