## Why

Herdr does not currently provide a private, workspace-aware record of how much time a person actively works versus how long agents run unattended. A Pomodoro plugin can make focus sessions convenient inside Herdr while producing an honest local activity history based only on human input delivered to terminal panes.

## What Changes

- Add a Herdr workflow plugin that records active work as the time between consecutive human pane-input events in the same workspace, up to a configurable two-minute gap.
- Require a privacy-preserving Herdr activity signal that identifies the input context and origin without exposing keys, commands, or text; Herdr navigation, workspace switching, agent/API input, and terminal output do not count.
- Attribute activity to durable machine, project, workspace/worktree, and version-control revision identities, including local accounting for remotely backed workspaces.
- Add 25-minute focus sessions with a visible countdown, reduced workspace/sidebar distractions, explicit three-minute absence and cross-workspace grace modes, early termination, and a five-minute break countdown.
- Keep human-active time distinct from agent-supervision, grace, focus-elapsed, and break time, while allowing configured agent or notification-monitoring states to protect a focus session from inactivity handling.
- Add a central dashboard showing daily and timeline totals by project, workspace, Pomodoro session, and Git commit or stable Jujutsu change.
- Store all durable activity data in a private plugin-state database under the user's home directory rather than in the public repository.

## Capabilities

### New Capabilities

- `human-activity-tracking`: Privacy-preserving human-input detection, gap accounting, workspace boundaries, durable project identity, revision attribution, and local persistence.
- `pomodoro-focus-sessions`: Focus and break timers, distraction reduction, inactivity handling, grace modes, agent-supervision protection, and session lifecycle behavior.
- `work-activity-dashboard`: Central daily and timeline reporting across projects, workspaces, revisions, activity categories, and remote workspace provenance.

### Modified Capabilities

None.

## Impact

- Introduces a new Herdr plugin manifest, executable plugin service/actions, dashboard pane, local database schema and migrations, configuration, and automated tests.
- Depends on a Herdr client/server API addition that reports human-originated pane activity without input content and distinguishes it from programmatic input and Herdr navigation.
- Depends on Herdr APIs capable of presenting the timer and temporarily reducing workspace/sidebar distractions; unsupported presentation operations must degrade without corrupting session or activity state.
- Reads Git and Jujutsu metadata from workspace repositories but does not modify repository history.
- Creates private state only in Herdr's plugin state directory; repository-local development databases remain ignored.
