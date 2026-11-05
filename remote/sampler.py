"""Metadata-only Herdr Pomodoro sampler, streamed over SSH (python3 -u -).

Emits JSON lines on stdout:
  {"type": "topology", "panes": [...]}   every TOPOLOGY_INTERVAL_SECONDS
  {"type": "access", "pane_id": ..., "ns": <remote wall clock ns>}  per input read

It never reads terminal contents: only pane identity, working directories,
repository metadata, and the instant a pane's terminal consumed input.
Compatible with Python 3.9+; standard library only.
"""
import ctypes
import json
import os
import shutil
import struct
import subprocess
import sys
import threading
import time

TOPOLOGY_INTERVAL_SECONDS = 5.0
POLL_INTERVAL_SECONDS = 0.05
COMMAND_TIMEOUT_SECONDS = 5
IN_ACCESS = 0x1
EXTRA_PATHS = [
    "~/.local/bin",
    "~/.cargo/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
]
JJ_TEMPLATE = (
    'change_id ++ "\\n" ++ commit_id ++ "\\n" ++ '
    'bookmarks.map(|b| b.name()).join(",") ++ "\\n" ++ description.first_line()'
)

OUTPUT_LOCK = threading.Lock()
STATE_LOCK = threading.Lock()
STATE = {"version": 0, "panes": {}}


def emit(message):
    with OUTPUT_LOCK:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def run(arguments, cwd=None):
    try:
        result = subprocess.run(
            arguments,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.decode("utf-8", "replace")


def herdr_binary():
    candidate = os.path.expanduser("~/.local/bin/herdr")
    if os.path.exists(candidate):
        return candidate
    return shutil.which("herdr") or "herdr"


def revision_metadata(root):
    output = run(
        ["jj", "log", "--ignore-working-copy", "-r", "@", "--no-graph", "-T", JJ_TEMPLATE],
        cwd=root,
    )
    if output:
        lines = output.rstrip("\n").split("\n")
        if lines and lines[0]:
            return {
                "kind": "jj",
                "stable_id": lines[0],
                "commit_id": lines[1] if len(lines) > 1 and lines[1] else None,
                "branch": None,
                "bookmarks": lines[2] if len(lines) > 2 and lines[2] else None,
                "description": lines[3] if len(lines) > 3 and lines[3] else None,
            }
    head = run(["git", "rev-parse", "HEAD"], cwd=root)
    if head:
        branch = run(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=root)
        subject = run(["git", "log", "-1", "--format=%s"], cwd=root)
        commit = head.strip()
        return {
            "kind": "git",
            "stable_id": commit,
            "commit_id": commit,
            "branch": branch.strip() if branch else None,
            "bookmarks": None,
            "description": subject.strip() if subject and subject.strip() else None,
        }
    return {
        "kind": "none",
        "stable_id": "unversioned",
        "commit_id": None,
        "branch": None,
        "bookmarks": None,
        "description": None,
    }


def repository(cwd):
    root = run(["jj", "root"], cwd=cwd)
    if not root:
        root = run(["git", "rev-parse", "--show-toplevel"], cwd=cwd)
    if not root:
        return cwd, cwd, revision_metadata(cwd)
    root = root.strip()
    common = run(["git", "rev-parse", "--git-common-dir"], cwd=root)
    identity = root
    if common:
        common = common.strip()
        identity = common if os.path.isabs(common) else os.path.join(root, common)
    return root, os.path.normpath(identity), revision_metadata(root)


def terminal_device(shell_pid):
    tty = run(["ps", "-o", "tty=", "-p", str(shell_pid)])
    if not tty:
        return None
    name = tty.strip()
    if not name or name in ("?", "??", "-"):
        return None
    return "/dev/" + name


def discover():
    herdr = herdr_binary()
    raw = run([herdr, "api", "snapshot"])
    if not raw:
        return None
    try:
        panes = json.loads(raw)["result"]["snapshot"]["panes"]
    except (ValueError, KeyError):
        return None
    cache = {}
    found = {}
    for pane in panes:
        pane_id = pane.get("pane_id")
        cwd = pane.get("cwd") or ""
        info = run([herdr, "pane", "process-info", "--pane", pane_id])
        if not pane_id or not info or not cwd:
            continue
        try:
            shell_pid = json.loads(info)["result"]["process_info"]["shell_pid"]
        except (ValueError, KeyError, TypeError):
            continue
        device = terminal_device(shell_pid)
        if not device:
            continue
        if cwd not in cache:
            cache[cwd] = repository(cwd)
        root, identity, revision = cache[cwd]
        found[pane_id] = {
            "pane_id": pane_id,
            "workspace_id": pane.get("workspace_id"),
            "cwd": cwd,
            "device": device,
            "repo_root": root,
            "repo_identity": identity,
            "revision": revision,
        }
    return found


def refresher():
    while True:
        found = discover()
        if found is not None:
            with STATE_LOCK:
                changed = {k: v["device"] for k, v in found.items()} != {
                    k: v["device"] for k, v in STATE["panes"].items()
                }
                STATE["panes"] = found
                if changed:
                    STATE["version"] += 1
            emit({"type": "topology", "panes": list(found.values())})
        time.sleep(TOPOLOGY_INTERVAL_SECONDS)


def snapshot_state():
    with STATE_LOCK:
        return STATE["version"], dict(STATE["panes"])


class InotifyWatcher:
    def __init__(self):
        self.libc = ctypes.CDLL("libc.so.6", use_errno=True)
        self.fd = self.libc.inotify_init1(os.O_NONBLOCK)
        self.watches = {}  # wd -> pane_id
        self.paths = {}  # pane_id -> (device, wd)

    def sync(self, panes):
        for pane_id, (device, wd) in list(self.paths.items()):
            current = panes.get(pane_id)
            if current is None or current["device"] != device:
                self.libc.inotify_rm_watch(self.fd, wd)
                self.watches.pop(wd, None)
                del self.paths[pane_id]
        for pane_id, pane in panes.items():
            if pane_id in self.paths:
                continue
            wd = self.libc.inotify_add_watch(self.fd, pane["device"].encode(), IN_ACCESS)
            if wd >= 0:
                self.watches[wd] = pane_id
                self.paths[pane_id] = (pane["device"], wd)

    def wait(self, timeout):
        import select

        ready, _, _ = select.select([self.fd], [], [], timeout)
        if not ready:
            return []
        now = time.time_ns()
        try:
            data = os.read(self.fd, 65536)
        except BlockingIOError:
            return []
        events = []
        offset = 0
        while offset < len(data):
            wd, mask, _, length = struct.unpack_from("iIII", data, offset)
            offset += 16 + length
            pane_id = self.watches.get(wd)
            if pane_id and mask & IN_ACCESS:
                events.append((pane_id, now))
        return events


class PollingWatcher:
    def __init__(self):
        self.last = {}
        self.devices = {}

    def sync(self, panes):
        self.devices = {pane_id: pane["device"] for pane_id, pane in panes.items()}
        self.last = {k: v for k, v in self.last.items() if k in self.devices}

    def wait(self, timeout):
        time.sleep(min(timeout, POLL_INTERVAL_SECONDS))
        events = []
        for pane_id, device in self.devices.items():
            try:
                atime = os.stat(device).st_atime_ns
            except OSError:
                continue
            previous = self.last.get(pane_id)
            self.last[pane_id] = atime
            if previous is not None and atime > previous:
                events.append((pane_id, atime))
        return events


def main():
    os.environ["PATH"] = os.pathsep.join(
        [os.path.expanduser(path) for path in EXTRA_PATHS] + [os.environ.get("PATH", "")]
    )
    threading.Thread(target=refresher, daemon=True).start()
    watcher = InotifyWatcher() if sys.platform.startswith("linux") else PollingWatcher()
    seen_version = -1
    while True:
        version, panes = snapshot_state()
        if version != seen_version:
            watcher.sync(panes)
            seen_version = version
        for pane_id, nanoseconds in watcher.wait(0.5):
            emit({"type": "access", "pane_id": pane_id, "ns": nanoseconds})


if __name__ == "__main__":
    main()
