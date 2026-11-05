## Why

Herdr does not currently provide a private, workspace-aware record of how much time a person actively works versus how long agents run unattended. A Pomodoro plugin can make focus sessions convenient inside Herdr while producing an honest local activity history based only on human input delivered to terminal panes.

## What Changes

- Add a Herdr workflow plugin that records active work as the time between consecutive human pane-input events in the same workspace, up to a configurable two-minute gap.
- Detect human pane input by correlating input-access timestamp changes on a Herdr client's outer PTY with timestamp changes on the destination pane PTY, without reading or storing keys, commands, or text; unmatched navigation, agent/API input, and terminal output do not count.
- Attribute activity to durable machine, project, workspace/worktree, and version-control revision identities, including local accounting for remotely backed workspaces.
- Add 25-minute focus sessions with a countdown rendered as metadata on the focused workspace's sidebar row, a focused-workspace Agent view, explicit three-minute leave-terminal and use-other-workspace buttons that temporarily reveal all agents, automatic three-minute cross-workspace grace, early termination, and a five-minute break countdown.
- Add one compact Pomodoro command popup for starting focus, opening the dashboard, or selecting either three-minute grace mode, and refuse to replace an already active session.
- Keep human-active time distinct from agent-supervision, grace, focus-elapsed, and break time, while allowing configured agent or notification-monitoring states to protect a focus session from inactivity handling.
- Add a central dashboard showing daily and timeline totals by project, workspace, Pomodoro session, and Git commit or stable Jujutsu change.
- Provide the dashboard as a polished, full-screen interactive terminal application with keyboard navigation, filters, summary cards, tables, charts, a chronological timeline, and live refresh.
- Store all durable activity data in a private plugin-state database under the user's home directory rather than in the public repository.
- **BREAKING** Replace the Python package and `uv` runtime with one native Rust executable while preserving the existing private SQLite history.

## Capabilities

### New Capabilities

- `human-activity-tracking`: Privacy-preserving human-input detection, gap accounting, workspace boundaries, durable project identity, revision attribution, and local persistence.
- `pomodoro-focus-sessions`: Focus and break timers, distraction reduction, inactivity handling, grace modes, agent-supervision protection, and session lifecycle behavior.
- `work-activity-dashboard`: Central daily and timeline reporting across projects, workspaces, revisions, activity categories, and remote workspace provenance.

### Modified Capabilities

None.

## Impact

- Introduces a Rust Herdr plugin executable, manifest actions and pane entrypoints, a Ratatui dashboard, local database schema and migrations, configuration, and automated tests.
- Uses same-user process and PTY metadata already available on macOS, with no custom Herdr distribution or operating-system keyboard monitoring permission.
- Requires a metadata-only helper on a remote machine only when using Herdr-native remote panes whose destination PTY is not local; SSH or mosh running inside a local pane needs no helper.
- Uses Herdr workspace metadata for the timer and the declarative Agent view for distraction reduction; other workspace rows remain visible because Herdr does not expose a workspace-view filter.
- Reads Git and Jujutsu metadata from workspace repositories but does not modify repository history.
- Creates private state only in Herdr's plugin state directory; repository-local development databases remain ignored.
- Removes the Python source tree, Python tests, `pyproject.toml`, and `uv.lock`; Herdr invokes the compiled Rust executable directly.
