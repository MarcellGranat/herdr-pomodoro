## Context

Herdr 0.9.x plugins can declare actions, event hooks, and terminal panes, and can keep durable files in `HERDR_PLUGIN_STATE_DIR`. The current event surface identifies workspace, pane, and agent-state changes but does not expose privacy-preserving human input activity. This plugin therefore spans a small upstream Herdr event contract, a local accounting engine, a Pomodoro state machine, and a terminal dashboard.

The defining accounting rule is global: retain the most recent human input delivered to any pane. Credit the gap only when the next human pane input belongs to the same workspace and arrives within the configured maximum gap. Herdr navigation is ignored. Merely switching away and back does not break the chain, but human pane input in another workspace replaces the global anchor and therefore breaks the original workspace's chain.

Activity must remain private, work for local and remotely backed workspaces, distinguish human work from agent activity, and avoid creating a high-volume key-event database. Git and Jujutsu metadata are reporting dimensions, not sources of activity.

## Goals / Non-Goals

**Goals:**

- Record human-active intervals according to the global consecutive-input rule without recording input content.
- Preserve durable machine, project, workspace/worktree, and revision attribution.
- Run configurable focus, grace, and break timers inside Herdr and restore the previous UI state afterward.
- Show daily totals and timelines across local and remote projects while keeping human, agent-supervision, grace, focus, and break measures distinct.
- Remain efficient during continuous typing and recover safely from restarts.

**Non-Goals:**

- Keylogging, command history capture, semantic interpretation of terminal input, or general operating-system activity monitoring.
- Counting terminal output, agent/API-generated input, Herdr navigation commands, or passive time as human work.
- Synchronizing the private activity database between machines or publishing it with the plugin source.
- Modifying Git or Jujutsu history.
- Treating a focus session's wall-clock duration as proof of human-active work.

## Decisions

### Use a content-free, coalesced Herdr human-input event

Herdr will expose an event for human-originated input that is actually forwarded to a terminal pane. The event will carry timestamps, workspace/tab/pane identifiers, client and machine provenance, pane working directory, and a source classification, but no key codes, text, clipboard data, or commands. Programmatic APIs such as `pane.send_input`, agent output, and commands consumed by Herdr will not produce the event.

To avoid launching a plugin hook for each keystroke, Herdr will coalesce contiguous input for the same workspace into short batches and flush on a small cadence, workspace change, idle boundary, or shutdown. Each batch retains first and last input timestamps. A change of workspace with human pane input forces a boundary; navigation without pane input does not.

Alternatives considered:

- OS-level input monitoring was rejected because it requires elevated permissions, cannot reliably attribute remote/mosh activity, and risks observing unrelated applications.
- Workspace/pane focus events were rejected because they measure navigation rather than work.
- Raw per-key plugin events were rejected because of process and storage volume as well as privacy risk.

### Keep one global activity anchor and persist compact segments

The accounting engine maintains one last-input anchor, not one anchor per workspace. For a new human input at `current`:

1. If no anchor exists, store `current` without credit.
2. If `current.workspace` equals `anchor.workspace` and the monotonic gap is at most the configured threshold, add the gap to that workspace.
3. Otherwise, add no time.
4. Replace the anchor with `current` in all cases.

The default threshold is 120 seconds. Coalesced batches are expanded logically at their retained boundaries, and adjacent credited intervals with identical attribution are merged. This preserves the user's rule while avoiding a raw-input table. Wall-clock timestamps support reporting; monotonic durations prevent clock adjustments from creating negative or inflated work.

### Implement the plugin in typed, low-dependency Python

The plugin will use typed Python modules, dataclasses for domain state, `pathlib` for paths, the standard `sqlite3` module for persistence, and JSON only at process/event boundaries. Commands will be explicit entrypoints with `main()` functions. The dashboard will be a terminal UI hosted by a declared Herdr plugin pane; the initial version will prefer the standard library and introduce a UI dependency only if the dashboard requirements cannot be met accessibly without one.

This fits the event-hook and pane model without introducing a separate network service. Event hooks perform short transactions, while a dashboard process reads the database in read-only/query mode and refreshes periodically.

### Store private state in the Herdr plugin state directory

The primary SQLite database will be located beneath `HERDR_PLUGIN_STATE_DIR`, with schema versioning and forward migrations. Configuration will live beneath `HERDR_PLUGIN_CONFIG_DIR`. SQLite write-ahead logging and bounded transactions will allow event hooks and the dashboard to operate concurrently.

Core tables will represent machines, projects, workspace instances, revisions, activity segments, focus sessions, and timed session phases. Activity categories will be explicit (`human_active`, `agent_supervision`, `grace`, and `break`) rather than combined into a single work total. Repository-local SQLite files are development artifacts and will be ignored.

### Use durable hierarchical identity

Transient Herdr workspace IDs will be retained only as runtime references. Durable identity will combine machine identity, repository root, and worktree/workspace path. Remote provenance supplied by Herdr will be part of the machine/project key so identical paths on different machines do not collide. Multiple worktrees can be reported separately while rolling up to one project when a common repository identity is available.

For Git, revision attribution uses the current commit ID plus branch metadata. For Jujutsu, the stable change ID is the primary reporting key and the mutable commit ID, bookmarks, and description are captured as details. Metadata probes will be read-only and cached. If the two ends of a credited interval resolve to different revisions and no observed boundary can place the transition, the interval is marked as a revision transition instead of inventing false precision.

### Model Pomodoro behavior as a persisted state machine

The timer has `idle`, `focus`, `away_grace`, `workspace_grace`, and `break` phases. Defaults are 25 minutes for focus, three minutes for either grace mode, five minutes for break, and two minutes for the human-input gap. Deadlines are persisted so restart recovery derives the correct current phase rather than restarting a timer.

Starting focus captures the current workspace and relevant presentation state. Herdr presentation APIs will hide or collapse other workspaces and the agent sidebar when supported, while a compact countdown remains visible. The plugin restores the captured presentation state when focus ends or is cancelled. Failure to alter presentation is non-fatal and must not damage timer or activity records.

Human input in another workspace always breaks the previous activity anchor. During an explicit workspace-grace phase, it does not additionally invalidate the focus session. Agent `working` status and explicit integration with monitored Herdr Notify targets can protect the focus session from inactivity handling, but such time is stored as `agent_supervision`, never `human_active`.

### Treat the dashboard as a projection over immutable-enough facts

The dashboard queries activity segments and session phases to render a daily summary, project/workspace breakdown, revision breakdown, and chronological timeline. Filters include date, project, workspace, machine provenance, session, activity category, and revision. Human-active totals are always directly visible and are never silently combined with focus or agent-supervision time.

## Risks / Trade-offs

- **[Upstream Herdr event is unavailable]** → Gate activity recording on protocol capability detection, show a clear diagnostic, and implement/test against a documented fixture contract before integration.
- **[Herdr cannot hide all requested UI regions]** → Use capability detection, provide a timer pane with best-effort distraction reduction, and always restore only state the plugin successfully changed.
- **[Event batching loses the last few seconds on abrupt process death]** → Flush on boundaries and shutdown, keep the cadence short, and accept a bounded loss rather than persist every key event.
- **[Concurrent hooks contend on SQLite]** → Use WAL mode, short idempotent transactions, a busy timeout, and event identifiers for deduplication.
- **[Revision changes inside an interval are unknowable]** → Report a transition bucket unless an observed VCS boundary is available.
- **[Remote workspace identity is incomplete]** → Retain raw provenance fields, avoid merging ambiguous projects, and allow later identity reconciliation without rewriting source activity.
- **[Agent supervision inflates perceived work]** → Keep categories separate in storage and make human-active time the primary dashboard measure.
- **[Clock changes distort timelines]** → Calculate durations monotonically and use wall time only for placement on the calendar.

## Migration Plan

1. Define and land the privacy-preserving Herdr event schema and protocol capability flag.
2. Link the plugin locally with activity capture disabled when the capability is absent.
3. Create the private database on first use and apply idempotent schema migrations.
4. Enable capture for a limited validation period and compare recorded boundaries with synthetic event fixtures.
5. Enable focus controls and dashboard actions after activity accounting is verified.

Rollback consists of disabling or unlinking the plugin. The database remains in the plugin state directory for recovery or manual deletion, and no project repository or VCS history requires rollback.

## Open Questions

- Which exact Herdr presentation APIs will own hiding/restoring other workspaces and the agent sidebar?
- What stable machine and remote-workspace provenance fields will the new activity event guarantee?
- Does Herdr Notify expose monitored-target state through an API/event, or should that integration be a later optional adapter?
- Should manually ending a focus session always begin the five-minute break, or should cancel and finish-early have distinct outcomes?
