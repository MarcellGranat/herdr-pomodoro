# Herdr Pomodoro

Herdr Pomodoro is a private, local-first Herdr workflow plugin for focus sessions and
workspace-aware activity reporting.

## Human-input activity spike

The activity spike works with the stock Herdr distribution; it does not require a custom build.
It discovers the same-user Herdr client and local pane PTYs, then samples only their filesystem
access and modification timestamps.

Human input is counted only when an access-time change on one outer Herdr-client PTY uniquely
matches an access-time change on one inner workspace PTY. This gives the following classification:

| Observation | Classification |
| --- | --- |
| Outer and inner access times change together | Human input reached that workspace |
| Only the outer access time changes | Herdr navigation or modal input; not work |
| Only the inner access time changes | Programmatic or agent input; not human work |
| Only modification time changes | Terminal output; not input |
| More than one possible match | Ambiguous; discarded |

The watcher never reads PTY contents and never records keystrokes, commands, clipboard data, or
terminal output. It stores only timestamps, resolved workspace and pane identity, and provenance.

## SSH, mosh, and remote workspaces

The approach works when the Mac running Herdr is reached through SSH or mosh. Human input is still
forwarded through both the outer Herdr-client PTY and the selected inner pane PTY on that Mac, so
physical keyboard monitoring is unnecessary.

Herdr-native remote panes are different: their inner PTY exists on the remote machine. Supporting
those panes will require a small metadata-only sampler on the remote host. Until that exists,
capture fails closed for any pane whose topology cannot be resolved safely.

## Measured results

The live spike against Herdr 0.9.5 resolved three client PTYs and five local pane PTYs. Measured
SSH/mosh human input changed the outer and inner access timestamps 10.276 ms apart, comfortably
inside the configured 150 ms correlation window. An idle three-second watcher run emitted no false
activity, and continuous terminal output changed modification time without changing access time.

Pane topology queries run concurrently. This reduced live topology discovery from roughly 26
seconds to about one second. The watcher samples timestamps every 50 ms and refreshes topology once
per minute.

## Development commands

Inspect whether the current Herdr topology supports activity capture:

```bash
uv run herdr-pomodoro status
```

Run the metadata-only watcher for ten seconds:

```bash
uv run herdr-pomodoro watch --duration 10
```

The watcher prints content-free correlated observations as JSON Lines. Silence means that it did
not observe any unambiguous human input reaching a pane.

