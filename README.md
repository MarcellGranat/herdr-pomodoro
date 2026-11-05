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

Herdr-native remote panes (saved machines such as `m1`) are sampled over SSH. For every enabled
machine from `herdr machine list`, the watcher keeps one `ssh <target> python3 -u -` session open
and streams `remote/sampler.py` into it; nothing is installed on the remote host. The sampler needs
only `python3` (3.9+) and the remote `herdr` binary, and it reports metadata only: pane and
workspace IDs, working directories, repository root, current revision (jj or git, including its
description), and the instant a pane's terminal consumed input. On Linux it learns that instant from
`inotify` `IN_ACCESS` events on the pane's `/dev/pts/N` (the kernel's PTY `atime` has 8-second
granularity, so it is unusable there); on macOS it polls the PTY `atime`.

A remote input event counts as human only when it correlates, within the usual window, with an
access-time change on a local Herdr client PTY, exactly like local panes. This assumes the
machines' clocks are NTP-synchronized (within a few milliseconds). Remote workspaces appear as
`<machine>/<workspace>` (for example `m1/w1`), with the machine as a dashboard dimension. Set
`"remote_sampling_enabled": false` in the plugin config to turn this off. SSH must work
non-interactively (`BatchMode`) for each saved machine; a dropped session reconnects after 15
seconds.

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

Press the configured `prefix+ctrl+p` shortcut to open the Pomodoro command popup. Use arrow keys or
`j`/`k`, then Enter; number keys select directly and Escape closes it. The popup adapts to the
current phase:

- **Start focus** — starts the configured 25-minute focus phase without opening a pane.
- **End focus session** — stops focus or either grace immediately and returns to idle without
  starting the five-minute break.
- **Leave terminal (3 minutes)** — starts away grace and temporarily shows every agent.
- **Use other workspace (3 minutes)** — starts workspace grace and temporarily shows every agent.
  Human pane input in another workspace also starts this grace automatically; navigation alone
  does not.
- **Return to focus** — appears during either grace and resumes the original focus deadline.
- **End break** — appears during the five-minute break and returns to idle.
- **Open activity dashboard** — opens the live dashboard pane.

Start focus is unavailable while focus, grace, or break is active. The engine also rejects a second
start request from the CLI or a stale popup, preserving the original session and deadline.

The timer survives plugin or Herdr restarts because deadlines are stored rather than recomputed.
During focus, the built-in Agents view is filtered to the focus workspace. Both three-minute grace
buttons clear that filter so all agents are available. Returning human input to the focus workspace
or selecting **Return to focus** before either grace expires resumes the original focus deadline and
reinstalls the focused Agent view; expiry ends the session.

Set both `sidebar_start_collapsed = true` and `sidebar_collapsed_mode = "hidden"` under `[ui]` so
Herdr starts with a zero-width sidebar. The plugin never expands the client-local sidebar during
either three-minute grace, so it remains hidden when leaving the terminal. Herdr 0.9.5 does not
expose conditional collapse/expand through its plugin API, so the built-in `toggle_sidebar`
keybinding is blocked by a guard binding during focus and **Leave terminal** grace. **Check another
project** starts a three-minute workspace grace that simply allows the sidebar toggle (prefix+b);
the grace resumes focus only once you have been in another workspace and come back (or choose **Return to focus**); otherwise the session ends. The startup settings take effect after
restarting the Herdr client.

The sidebar toggle is blocked only during focus and leave-terminal grace. Keep a `[[keys.command]]`
block (all four lines tagged `# pomodoro-sidebar-lock`) in `[keys]` that binds `prefix+b` to the
no-op `sidebar-guard` action. The plugin enables it for those phases, comments it out otherwise, and
runs `herdr server reload-config`.

Herdr treats an empty key value as missing and restores its default, so disabling the toggle uses
`toggle_sidebar = "ctrl+b"` under `[keys]`. `ctrl+b` is already the prefix key; Herdr keeps prefix
mode and explicitly disables the conflicting sidebar action. `herdr config check` confirms this as
`kept keys.prefix, disabled keys.toggle_sidebar`.

On macOS the watcher also opens native dialogs (via `osascript`) on status changes:

- **Focus complete** — an **OK** dialog announces break time. The five-minute break starts only
  when you press OK (or choose **Start break** in the menu). With no answer within
  `break_confirmation_seconds` (default 180) the session is discarded as `unconfirmed` and does not
  count as a completed focus. If a dialog cannot be shown, the break starts automatically.
- **Grace about to expire** — at `00:20` remaining (2:40 elapsed) in either three-minute grace, a
  **Resume session** / **Let it end** dialog appears next to the warning sound. Choosing it resumes the
  original focus deadline. Ending a session by hand never shows this dialog.
- **Break over** — a **Start new session** / **OK** dialog appears (plus the top-right toast); the
  new session starts in the focused workspace.

Dialogs close themselves if the phase changes first. Alert delivery is persisted per timed phase, so
watcher polling and restarts do not repeat a toast that was already delivered.

Add the `$pomodoro` metadata token to the Space sidebar layout in `~/.config/herdr/config.toml`:

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", "workspace"],
  ["branch", "git_status"],
  [{ token = "$pomodoro", fg = "#f38ba8", bold = true }],
]
```

To keep the counter visible while the sidebar is hidden, show it in the tab bar instead (set
`tab_bar_position = "bottom"` for a bottom status bar):

```toml
[ui]
tab_bar_right = [
  { type = "command", command = "/path/to/herdr-pomodoro/target/release/herdr-pomodoro bar", interval_seconds = 1, timeout_seconds = 2 },
]
```

`herdr-pomodoro bar` prints the live label (`🦦` when idle).

Reload Herdr with `herdr server reload-config`. The focused workspace then shows a live line such
as `🍅 focus 24:37`, `⏳ workspace grace 02:41`, or `☕ break 04:58`.

The dashboard is a full-screen Ratatui application with summary cards and these views: **Overview**
(category totals and recent timeline), **Hours**, **Days**, **Workspaces**, **Revisions**,
**Sessions**, and **Timeline**. It supports date, workspace/worktree, session, category, revision,
and project filters and never treats unrecorded time as work. Opening it from the Pomodoro popup or
`herdr-pomodoro open-dashboard` always launches it in a new Herdr tab.

- **Hours** is a column chart with one column per local hour (00–23), scaled to 60 minutes and
  stacked by project. A yellow frame marks each *completed* Pomodoro at the minutes it actually
  covered, so a session started at 09:30 is framed from the middle of the 09 column upward and
  continues (open-ended) into the 10 column.
- **Days** is the same chart per day for the 14 days ending at the selected day, scaled in hours.
- **Workspaces** are labelled `machine  folder`; **Revisions** are labelled `project · description`
  and can be narrowed to one project with `p`.
- **Passive agent time:** whenever an agent's status is `working` (polled every second, local and
  on sampled remote machines) its project's time is recorded separately from your human-active time.
  Overlapping agents on one project are merged, not double counted. Charts show it as the dimmer
  right-hand bar of each column, tables show it in an *Agent (passive)* column, and it is the sixth
  category (`6`).

Dashboard keys:

- `Tab`, `Shift+Tab`, or arrow keys change views; `j`/`k` changes selection.
- `Enter` toggles the selected workspace, revision, or session filter; `p` cycles the project filter.
- `1`–`6` toggle activity categories; `[`/`]` changes day; `d` returns to today.
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
history and service. Herdr-native remote panes are sampled over SSH (see above); local panes
containing SSH or mosh sessions work without one.


When reconciling the watcher by hand, set `HERDR_PLUGIN_STATE_DIR` and `HERDR_PLUGIN_CONFIG_DIR` to Herdr's plugin directories (`~/.local/state/herdr/plugins/chmarax.herdr-pomodoro`, `~/.config/herdr/plugins/config/chmarax.herdr-pomodoro`); otherwise the watcher reads a different database from the popup.
