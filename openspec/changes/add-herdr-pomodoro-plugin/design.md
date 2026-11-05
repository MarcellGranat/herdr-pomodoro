## Context

Herdr 0.9.x plugins can declare actions, event hooks, and terminal panes, and can keep durable files in `HERDR_PLUGIN_STATE_DIR`. The current event surface identifies workspace, pane, and agent-state changes but does not expose human input activity. A custom Herdr build is undesirable, and operating-system keyboard monitoring does not observe a keyboard located across SSH or mosh. The plugin therefore detects forwarded human input by correlating same-user PTY metadata already present on the machine running Herdr.

The defining accounting rule is global: retain the most recent human input delivered to any pane. Credit the gap only when the next human pane input belongs to the same workspace and arrives within the configured maximum gap. Herdr navigation is ignored. Merely switching away and back does not break the chain, but human pane input in another workspace replaces the global anchor and therefore breaks the original workspace's chain.

Activity must remain private, work for local and remotely backed workspaces, distinguish human work from agent activity, and avoid creating a high-volume key-event database. Git and Jujutsu metadata are reporting dimensions, not sources of activity.

## Goals / Non-Goals

**Goals:**

- Record human-active intervals according to the global consecutive-input rule without recording input content.
- Work when the user reaches the Herdr machine through SSH or mosh, without requiring physical-input permissions or a custom Herdr distribution.
- Preserve durable machine, project, workspace/worktree, and revision attribution.
- Run configurable focus, grace, and break timers inside Herdr and restore the previous UI state afterward.
- Show daily totals and timelines across local and remote projects while keeping human, agent-supervision, grace, focus, and break measures distinct.
- Remain efficient during continuous typing and recover safely from restarts.

**Non-Goals:**

- Keylogging, command history capture, semantic interpretation of terminal input, or general operating-system activity monitoring.
- Reading terminal input bytes from either the Herdr client PTY or pane PTYs.
- Counting terminal output, agent/API-generated input, Herdr navigation commands, or passive time as human work.
- Synchronizing the private activity database between machines or publishing it with the plugin source.
- Modifying Git or Jujutsu history.
- Treating a focus session's wall-clock duration as proof of human-active work.

## Decisions

### Correlate outer Herdr-client and inner pane PTY access timestamps

Each interactive Herdr client has a controlling outer PTY. Each local pane process has a separate inner PTY. When a person types through SSH, mosh, or a local terminal, Herdr reads the input from the outer PTY and forwards pane-directed input to the active pane, whose process reads it from the inner PTY. The operating system updates each device's nanosecond input access timestamp without exposing the bytes.

The sampler will discover Herdr client processes and their controlling TTYs, obtain pane shell process identifiers from the Herdr API, and resolve each shell PID to its controlling TTY. It will sample `st_atime_ns` for these devices on a short configurable cadence and create a human-input observation only when an outer-client change and exactly one mapped pane change occur within the configured correlation window. The initial measured implementation target is a 50 ms sampling cadence and a 150 ms correlation window; both remain configurable and must be validated under load.

The correlation classifies the important cases without reading input:

- Outer and inner timestamps change together: human input reached that pane.
- Only an outer timestamp changes: Herdr consumed navigation or modal input, so no work input is recorded.
- Only an inner timestamp changes: programmatic or API input reached the pane, so no human input is recorded.
- Only PTY modification timestamps change: terminal output occurred, so no input is recorded.

The observer supports multiple attached Herdr clients by treating each outer PTY independently and selecting the closest unambiguous outer/inner pair. It records only timestamps, resolved workspace/pane identity, and provenance. It never opens either PTY for reading and never stores key codes, bytes, commands, clipboard data, or terminal output.

SSH or mosh running inside a local Herdr pane needs no special handling because the local mosh/SSH process reads from the local pane PTY. A Herdr-native remote workspace has its pane PTY on another machine; that topology requires an optional remote sampler that sends timestamp and pane-identity metadata, never input content, to the local collector. If the topology cannot be resolved unambiguously, capture is disabled rather than approximated.

Alternatives considered:

- An upstream Herdr event would be precise but was rejected as a required dependency because it would make the plugin unusable until a new Herdr release and would complicate private deployment.
- OS-level physical-input monitoring was rejected because it requires elevated permissions, misses keyboards located across SSH/mosh, and risks observing unrelated applications.
- Workspace/pane focus and terminal-output events were rejected because they measure navigation or output rather than human pane input.
- A nested terminal-control proxy was rejected because it would change how every pane is used and would not cover existing panes transparently.

### Keep one global activity anchor and persist compact segments

The accounting engine maintains one last-input anchor, not one anchor per workspace. For a new human input at `current`:

1. If no anchor exists, store `current` without credit.
2. If `current.workspace` equals `anchor.workspace` and the monotonic gap is at most the configured threshold, add the gap to that workspace.
3. Otherwise, add no time.
4. Replace the anchor with `current` in all cases.

The default threshold is 120 seconds. Correlated observations are coalesced in memory, and adjacent credited intervals with identical attribution are merged. This preserves the user's rule while avoiding a raw-input table. Wall-clock timestamps support reporting; monotonic durations prevent clock adjustments from creating negative or inflated work.

### Implement the complete plugin as one native Rust executable

The repository will contain one Rust crate and one `herdr-pomodoro` executable. `clap` subcommands expose watcher, reconciliation, actions, event hooks, status, timer output, and dashboard entrypoints. Domain state uses Rust structs and enums; `serde` owns JSON boundaries; `rusqlite` owns SQLite access; and all filesystem paths use `std::path`. The Herdr manifest invokes the compiled executable directly, so production does not depend on Python or `uv`.

The timestamp sampler remains a plugin-managed, per-user background service because capture must continue when no plugin pane is open. Herdr startup reconciliation ensures the service configuration targets the current plugin executable and that only one watcher owns a given client/pane topology. On macOS the production lifecycle uses a user LaunchAgent; other platforms require their own explicitly tested service adapter.

The Rust implementation MUST preserve the existing SQLite file path, schema, and stored history. Migrations remain forward-only and idempotent. The Python implementation is removed only after Rust compatibility tests open a database produced by the Python schema and reconcile its totals and session state.

### Render the dashboard with Ratatui

The dashboard is a full-screen terminal application hosted by the declared Herdr dashboard pane. `ratatui` and `crossterm` provide a responsive layout with summary cards, project/workspace and revision tables, session details, proportional charts, and a chronological timeline. Keyboard navigation supports view switching, focus movement, filter editing, clearing filters, manual refresh, and quit. A timed event loop refreshes read-only query snapshots without losing selection or flickering. Narrow terminals collapse panels into a usable vertical layout, and loading, empty, and database-error states remain explicit.

### Store private state in the Herdr plugin state directory

The primary SQLite database will be located beneath `HERDR_PLUGIN_STATE_DIR`, with schema versioning and forward migrations. Configuration will live beneath `HERDR_PLUGIN_CONFIG_DIR`. SQLite write-ahead logging and bounded transactions will allow event hooks and the dashboard to operate concurrently.

Core tables will represent machines, projects, workspace instances, revisions, activity segments, focus sessions, and timed session phases. Activity categories will be explicit (`human_active`, `agent_supervision`, `grace`, and `break`) rather than combined into a single work total. Repository-local SQLite files are development artifacts and will be ignored.

### Use durable hierarchical identity

Transient Herdr workspace IDs will be retained only as runtime references. Durable identity will combine machine identity, repository root, and worktree/workspace path. Remote provenance supplied by Herdr will be part of the machine/project key so identical paths on different machines do not collide. Multiple worktrees can be reported separately while rolling up to one project when a common repository identity is available.

For Git, revision attribution uses the current commit ID plus branch metadata. For Jujutsu, the stable change ID is the primary reporting key and the mutable commit ID, bookmarks, and description are captured as details. Metadata probes will be read-only and cached. If the two ends of a credited interval resolve to different revisions and no observed boundary can place the transition, the interval is marked as a revision transition instead of inventing false precision.

### Model Pomodoro behavior as a persisted state machine

The timer has `idle`, `focus`, `away_grace`, `workspace_grace`, and `break` phases. Defaults are 25 minutes for focus, three minutes for either grace mode, five minutes for break, and two minutes for the human-input gap. The plugin exposes separate leave-terminal and use-other-workspace commands through its unified popup. Either grace mode temporarily clears the plugin-owned Agent view so every agent is visible without expanding the client-local sidebar; human input in the focus workspace restores the focused Agent view and original focus deadline, while grace expiry ends the session. Deadlines are persisted so restart recovery derives the correct current phase rather than restarting a timer.

A plugin-owned popup is the primary command surface. It reads the current persisted timer state and renders only applicable commands without creating a normal tab: idle offers Start focus, focus offers both grace modes and End focus session, either grace offers Return to focus and End focus session, and break offers End break. Open dashboard remains available in every phase. The engine independently rejects a second start so CLI calls and stale popup state cannot replace an active session. Stopping through the popup returns directly to idle, while returning from grace restores the original focus deadline.

Starting focus captures the current workspace without opening a timer pane. The watcher publishes a compact `pomodoro` workspace-metadata token containing the current phase and remaining time, refreshed while the phase is active and cleared at session end. The user's Herdr sidebar layout renders this token on the workspace row. The plugin also installs an owned declarative Agent view filtered to the focus workspace, then clears only that owned view when focus ends or is cancelled. Herdr starts with `sidebar_start_collapsed = true` and renders collapse at zero width through `sidebar_collapsed_mode = "hidden"`; the plugin leaves that client-local state unchanged through focus and grace because version 0.9.5 does not expose it through the plugin API. Failure to publish metadata or alter supported presentation is non-fatal and must not damage timer or activity records.

The watcher evaluates phase deadlines once per second and sends alerts through `herdr notification show`. A migration adds a `(phase_id, kind)` notification outbox with nullable delivery time. Phase transitions enqueue alerts in the same transaction as their state change; successful delivery stamps the outbox row, while failed delivery remains pending for a later tick or watcher restart. Either grace emits a request sound when its remaining duration first crosses `02:40`, normal focus completion emits the done sound while creating the five-minute break, and break completion emits a top-right popup before returning to idle.

Human input in another workspace always breaks the previous activity anchor and automatically moves an active focus phase into three-minute workspace grace. Human input in the focus workspace before the grace deadline resumes the original focus deadline; expiry terminates the focus session. Navigation without pane input does not start grace. Agent `working` status and explicit integration with monitored Herdr Notify targets can protect the focus session from inactivity handling, but such time is stored as `agent_supervision`, never `human_active`.

### Treat the dashboard as a projection over immutable-enough facts

The dashboard queries activity segments and session phases through typed Rust query models to render a daily summary, project/workspace breakdown, revision breakdown, session list, charts, and chronological timeline. Filters include date, project, workspace, machine provenance, session, activity category, and revision. Human-active totals are always directly visible and are never silently combined with focus or agent-supervision time.

## Risks / Trade-offs

- **[PTY access timestamps do not behave consistently]** → Run a validation spike across shells, agent TUIs, editors, paste, SSH, and mosh; disable the backend with a diagnostic on unsupported systems.
- **[Correlation matches unrelated simultaneous input]** → Use per-client topology, a narrow configurable window, one-to-one matching, and discard ambiguous samples rather than guessing.
- **[Process or TTY topology changes]** → Refresh discovery on Herdr workspace/pane events and periodically reconcile PIDs, devices, and machine provenance.
- **[Native remote pane has no local inner PTY]** → Run the optional metadata-only sampler remotely and transport only timestamps and pane identity; otherwise mark that workspace unsupported.
- **[Herdr cannot toggle client-local sidebar visibility]** → Start collapsed in zero-width mode, leave collapse state unchanged during grace, retain the manual toggle, and never inject synthetic terminal input.
- **[Watcher retries duplicate an alert]** → Persist a unique notification outbox entry per timed phase and alert kind and stamp successful delivery transactionally.
- **[Sampler death loses activity]** → Supervise it as a per-user service, persist health information, and never infer activity for an unobserved interval.
- **[Concurrent hooks contend on SQLite]** → Use WAL mode, short idempotent transactions, a busy timeout, and event identifiers for deduplication.
- **[Revision changes inside an interval are unknowable]** → Report a transition bucket unless an observed VCS boundary is available.
- **[Remote workspace identity is incomplete]** → Retain raw provenance fields, avoid merging ambiguous projects, and allow later identity reconciliation without rewriting source activity.
- **[Agent supervision inflates perceived work]** → Keep categories separate in storage and make human-active time the primary dashboard measure.
- **[Clock changes distort timelines]** → Calculate durations monotonically and use wall time only for placement on the calendar.
- **[Rust migration loses existing history]** → Keep the SQLite contract stable, test against a Python-produced fixture, and switch the manifest only after exact query reconciliation.
- **[Native build complicates plugin linking]** → Document `cargo build --release`, use a deterministic manifest path, and make the startup reconciler update the LaunchAgent to that exact executable.

## Migration Plan

1. Create the Rust crate and port the database contract, typed models, activity accounting, attribution, PTY correlation, Pomodoro engine, and Herdr adapters behind tests.
2. Open a copy of the existing Python-produced SQLite database with Rust and reconcile schema version, row counts, totals, active session state, and migrations.
3. Implement the Ratatui dashboard and sidebar metadata/Agent-view presentation, then verify automatic workspace grace.
4. Build the release executable, switch the Herdr manifest and LaunchAgent to it, and run live PTY and UI validation.
5. Remove all Python source, Python tests, `pyproject.toml`, and `uv.lock` after the Rust test suite covers the migrated behavior.
6. Add the optional remote sampler transport for Herdr-native remote workspaces after the local backend is verified.

Rollback consists of disabling or unlinking the plugin. The database remains in the plugin state directory for recovery or manual deletion, and no project repository or VCS history requires rollback.

## Open Questions

- Which authenticated transport should carry metadata-only observations from a native remote Herdr machine to the local collector?
- Which non-macOS PTY implementations preserve input access timestamps with sufficient precision?
- Does Herdr Notify expose monitored-target state through an API/event, or should that integration be a later optional adapter?
- Should manually ending a focus session always begin the five-minute break, or should cancel and finish-early have distinct outcomes?
