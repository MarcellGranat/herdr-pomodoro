#!/usr/bin/env python3
"""Pomodoro controls popup for remote machines (ASCII only, standard library).

Reads the phase the watcher pushed and sends the chosen action back through
the shim, which the SSH sampler relays to the machine that owns the data.
"""
import os
import subprocess
import sys
import termios
import tty

STATE = os.path.join(os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state"), "herdr-pomodoro")
SHIM = os.path.join(os.path.dirname(os.path.abspath(__file__)), "herdr-pomodoro.sh")

START = ("start", "Start focus session", "25 minutes")
STOP = ("stop", "End focus session", "return to idle")
ENTRIES = {
    "idle": [START],
    "focus": [
        STOP,
        ("workspace-grace", "Check another project", "3-minute workspace break"),
        ("away", "Leave the terminal", "3-minute away break"),
    ],
    "away_grace": [("return", "Return to focus", "resume original deadline"), STOP],
    "workspace_grace": [("return", "Return to focus", "resume original deadline"), STOP],
    "await_break": [("break", "Start break", "confirm the finished focus"), ("stop", "Skip break", "return to idle")],
    "break": [("stop", "End break", "return to idle")],
}


def read(name):
    try:
        with open(os.path.join(STATE, name)) as handle:
            return handle.read().strip()
    except OSError:
        return ""


def draw(entries, selected, label):
    out = ["\x1b[2J\x1b[H", "  Pomodoro  " + (label or "(no data from the watcher yet)") + "\r\n\r\n"]
    for index, (_, title, detail) in enumerate(entries):
        mark = ">" if index == selected else " "
        line = "%s %-26s %s" % (mark, title, detail)
        out.append(("\x1b[7m%s\x1b[0m" if index == selected else "%s") % line + "\r\n")
    out.append("\r\n  j/k move, Enter select, q close\r\n")
    sys.stdout.write("".join(out))
    sys.stdout.flush()


def choose(entries, label):
    descriptor = sys.stdin.fileno()
    saved = termios.tcgetattr(descriptor)
    selected = 0
    try:
        tty.setraw(descriptor)
        while True:
            draw(entries, selected, label)
            key = os.read(descriptor, 3)
            if key in (b"q", b"\x1b", b"\x03"):
                return None
            if key in (b"j", b"\x1b[B"):
                selected = (selected + 1) % len(entries)
            elif key in (b"k", b"\x1b[A"):
                selected = (selected - 1) % len(entries)
            elif key in (b"\r", b"\n"):
                return entries[selected][0]
    finally:
        termios.tcsetattr(descriptor, termios.TCSADRAIN, saved)
        sys.stdout.write("\x1b[2J\x1b[H")


def main():
    entries = ENTRIES.get(read("phase"), ENTRIES["idle"])
    action = choose(entries, read("bar.txt"))
    if action:
        environment = dict(os.environ, HERDR_WORKSPACE_ID=read("workspace"))
        subprocess.call([SHIM, action], env=environment)


if __name__ == "__main__":
    main()
