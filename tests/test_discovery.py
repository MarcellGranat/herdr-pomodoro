"""Tests for same-user Herdr PTY discovery."""

from herdr_pomodoro.discovery import CommandResult, discover_topology, iter_process_records
from herdr_pomodoro.models import EndpointRole

PROCESS_OUTPUT = """\
  100     1 ttys004 /opt/bin/herdr herdr
  101     1 ?? /opt/bin/herdr herdr server
  200   100 ttys008 /bin/zsh -zsh
  300   100 ttys009 /bin/zsh -zsh
"""


def test_iter_process_records_skips_malformed_rows() -> None:
    records = tuple(iter_process_records(f"bad row\n{PROCESS_OUTPUT}"))

    assert [record.pid for record in records] == [100, 101, 200, 300]


def test_discovers_client_and_resolved_local_panes() -> None:
    responses: dict[tuple[str, ...], CommandResult] = {
        ("ps", "-axo", "pid=,ppid=,tty=,comm=,args="): CommandResult(0, PROCESS_OUTPUT, ""),
        ("herdr", "api", "snapshot"): CommandResult(
            0,
            '{"result":{"snapshot":{"panes":['
            '{"pane_id":"p1","workspace_id":"w1","cwd":"/work/one"},'
            '{"pane_id":"p2","workspace_id":"w2","cwd":"/work/two"}'
            "]}}}",
            "",
        ),
        ("herdr", "pane", "process-info", "--pane", "p1"): CommandResult(
            0, '{"result":{"process_info":{"shell_pid":200}}}', ""
        ),
        ("herdr", "pane", "process-info", "--pane", "p2"): CommandResult(
            0, '{"result":{"process_info":{"shell_pid":300}}}', ""
        ),
    }

    topology = discover_topology(runner=responses.__getitem__)

    assert len(topology.clients) == 1
    assert topology.clients[0].role is EndpointRole.CLIENT
    assert str(topology.clients[0].device) == "/dev/ttys004"
    assert [pane.workspace_id for pane in topology.panes] == ["w1", "w2"]
    assert [str(pane.device) for pane in topology.panes] == ["/dev/ttys008", "/dev/ttys009"]


def test_skips_unresolved_native_remote_pane() -> None:
    responses: dict[tuple[str, ...], CommandResult] = {
        ("ps", "-axo", "pid=,ppid=,tty=,comm=,args="): CommandResult(0, PROCESS_OUTPUT, ""),
        ("herdr", "api", "snapshot"): CommandResult(
            0,
            '{"result":{"snapshot":{"panes":[{"pane_id":"remote","workspace_id":"w-remote"}]}}}',
            "",
        ),
        ("herdr", "pane", "process-info", "--pane", "remote"): CommandResult(
            0, '{"result":{"process_info":{"shell_pid":9999}}}', ""
        ),
    }

    topology = discover_topology(runner=responses.__getitem__)

    assert topology.panes == ()
