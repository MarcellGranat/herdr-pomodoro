# Herdr Pomodoro

Herdr Pomodoro is a private, local-first Herdr workflow plugin for focus sessions and
workspace-aware activity reporting.

## Installation

The plugin requires Herdr 0.9.5 or newer and a current stable Rust toolchain. Build and link this checkout for local use:

```bash
cargo build --release --locked
herdr plugin link "$PWD" --enabled
```

Herdr registers the actions, dashboard pane entrypoint, agent-status event hook, and
startup reconciliation command from `herdr-plugin.toml`. On macOS, run the startup reconciliation
once after linking if the Herdr server will not be restarted immediately:

```bash
./target/release/herdr-pomodoro reconcile
```

This installs a per-user LaunchAgent that keeps one metadata watcher running when no plugin pane is
open. Herdr normally supplies the plugin config/state directories and binary path automatically.

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
./target/release/herdr-pomodoro status
```

Run the metadata-only watcher for ten seconds:

```bash
./target/release/herdr-pomodoro watch --duration 10
```

The watcher prints content-free correlated observations as JSON Lines. Silence means that it did
not observe any unambiguous human input reaching a pane.

## Pomodoro controls

The registered Herdr actions provide:

- **Start focus** — starts the configured 25-minute focus phase without opening a pane.
- **Leave terminal** — starts three-minute away grace.
- **Use other workspaces** — starts three-minute workspace grace explicitly. Human pane input in
  another workspace also starts this grace automatically; navigation alone does not.
- **End session** — ends focus early and starts the configured five-minute break; invoking End
  during the break stops it.
- **Open activity dashboard** — opens the live dashboard pane.

The timer survives plugin or Herdr restarts because deadlines are stored rather than recomputed.
During focus, the built-in Agents view is filtered to the focus workspace. Other workspace rows
remain visible because Herdr does not currently expose a workspace-view filter. Returning human
input to the focus workspace before workspace grace expires resumes the original focus deadline;
expiry ends the session.

Add the `$pomodoro` metadata token to the Space sidebar layout in `~/.config/herdr/config.toml`:

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", "workspace"],
  ["branch", "git_status"],
  [{ token = "$pomodoro", fg = "#f38ba8", bold = true }],
]
```

Reload Herdr with `herdr server reload-config`. The focused workspace then shows a live line such
as `🍅 focus 24:37`, `⚠ workspace grace 02:41`, or `☕ break 04:58`.

The dashboard is a full-screen Ratatui application with summary cards, charts, project/workspace,
revision, machine, and session tables, and a proportional timeline. It supports date, project,
workspace/worktree, machine, session, category, and revision filters and never treats unrecorded
time as work.

Dashboard keys:

- `Tab`, `Shift+Tab`, or arrow keys change views; `j`/`k` changes selection.
- `Enter` toggles the selected project, workspace, revision, machine, or session filter.
- `1`–`5` toggle activity categories; `[`/`]` changes day; `d` returns to today.
- `c` clears filters, `r` refreshes, and `q` closes the dashboard.

## Private storage and configuration

In a linked installation Herdr currently creates:

- Config: `~/.config/herdr/plugins/config/chmarax.herdr-pomodoro/config.json`
- State: `~/.local/state/herdr/plugins/chmarax.herdr-pomodoro/activity.sqlite3`
- Service logs: the same state directory as `watcher.stdout.log` and `watcher.stderr.log`

The exact locations are supplied through `HERDR_PLUGIN_CONFIG_DIR` and
`HERDR_PLUGIN_STATE_DIR`; those variables remain authoritative if Herdr changes its directory
layout. The database uses WAL mode and is never written into this repository.

To back up activity safely, copy the database with SQLite's online backup facility while the
watcher is running, or stop the LaunchAgent before copying the database together with its `-wal`
file. Unlinking the plugin does not delete private state:

```bash
herdr plugin unlink chmarax.herdr-pomodoro
```

Delete the config, state, or LaunchAgent only when you intentionally want to remove the retained
history and service. Herdr-native remote panes remain unsupported until a metadata-only remote
sampler is installed; local panes containing SSH or mosh sessions work without one.

