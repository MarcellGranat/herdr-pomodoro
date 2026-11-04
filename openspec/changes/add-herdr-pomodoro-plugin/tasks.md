## 1. Plugin Foundation and Protocol Contract

- [ ] 1.1 Create the typed Python package, `pyproject.toml`, plugin manifest, executable entrypoints, ignored development-state patterns, and pytest layout.
- [ ] 1.2 Define versioned dataclasses and JSON fixtures for the content-free, coalesced Herdr human-input event, including local and remote provenance and unique event identifiers.
- [ ] 1.3 Add protocol capability detection that disables capture with an actionable diagnostic when the installed Herdr version does not expose the required input event.
- [ ] 1.4 Add or coordinate the upstream Herdr event implementation so only human input forwarded to panes is coalesced and emitted, while navigation, programmatic input, and output are excluded.

## 2. Private Persistence

- [ ] 2.1 Implement plugin config and state path discovery using `HERDR_PLUGIN_CONFIG_DIR` and `HERDR_PLUGIN_STATE_DIR`, with test-only path injection.
- [ ] 2.2 Implement idempotent SQLite migrations for machines, projects, workspace instances, revisions, activity anchors, activity segments, processed events, focus sessions, and timed phases.
- [ ] 2.3 Configure WAL mode, foreign keys, busy timeouts, short transactions, and schema-version checks for concurrent event-hook writes and dashboard reads.
- [ ] 2.4 Add migration, concurrency, deduplication, and private-path tests using temporary databases.

## 3. Human Activity Accounting

- [ ] 3.1 Implement the configurable global input-anchor state machine with the default 120-second maximum gap.
- [ ] 3.2 Implement coalesced-event ingestion and compact segment merging without persisting keys, text, commands, clipboard content, or raw per-input rows.
- [ ] 3.3 Test same-workspace credit, over-limit gaps, and the rule that human pane input in another workspace breaks and replaces the prior anchor.
- [ ] 3.4 Test that switching away and back without pane input leaves the anchor intact and that Herdr navigation, agent/API input, and terminal output create no credit.
- [ ] 3.5 Test monotonic duration calculation, wall-clock changes, event replay, out-of-order events, and bounded flush-loss behavior.

## 4. Project, Workspace, and Revision Attribution

- [ ] 4.1 Implement durable machine, repository, project, workspace, and worktree identity resolution without conflating identical paths on different machines.
- [ ] 4.2 Implement cached read-only Git metadata probes for repository root, commit ID, branch, and worktree provenance.
- [ ] 4.3 Implement cached read-only Jujutsu metadata probes using stable change ID as the primary identity and commit ID, bookmarks, and description as details.
- [ ] 4.4 Implement revision-transition attribution when interval endpoints differ without an observed boundary.
- [ ] 4.5 Add fixture-based tests for local repositories, remote provenance, multiple worktrees, missing VCS metadata, Git revisions, Jujutsu revisions, and transition intervals.

## 5. Pomodoro Session Engine

- [ ] 5.1 Implement the persisted `idle`, `focus`, `away_grace`, `workspace_grace`, and `break` state machine with configurable 25-, 3-, 3-, and 5-minute defaults.
- [ ] 5.2 Implement plugin actions for starting focus, starting each grace mode, ending a session early, and opening or focusing the timer and dashboard panes.
- [ ] 5.3 Implement the timer pane/controller with live countdown rendering, deadline-based restart recovery, phase transitions, and recorded termination reasons.
- [ ] 5.4 Implement best-effort capture, reduction, and exact restoration of supported Herdr workspace/sidebar presentation state with capability-aware degradation.
- [ ] 5.5 Implement agent-status protection and an optional Herdr Notify monitored-target adapter while storing protected time as agent-supervision rather than human-active time.
- [ ] 5.6 Test normal completion, early termination, restart recovery, grace expiry, cross-workspace input during grace, agent protection, and failed presentation operations.

## 6. Work Activity Dashboard

- [ ] 6.1 Implement typed query iterators for daily category totals, project/workspace rollups, revision rollups, sessions, and chronological segments.
- [ ] 6.2 Implement the Herdr dashboard pane with a selected-day summary and explicit human-active, focus-elapsed, agent-supervision, grace, and break totals.
- [ ] 6.3 Add project, workspace/worktree, Git commit, Jujutsu change, machine-provenance, session, category, and date-range filtering.
- [ ] 6.4 Add a chronological timeline that distinguishes activity categories, session phases, revision transitions, and empty periods without inferring unrecorded work.
- [ ] 6.5 Test total reconciliation, local/remote name collisions, filters, empty states, live refresh, and consistent reads during concurrent writes.

## 7. Integration, Quality, and Documentation

- [ ] 7.1 Link the plugin locally and verify manifest actions, event hooks, timer pane ownership, dashboard pane ownership, config creation, and state creation against the installed Herdr version.
- [ ] 7.2 Run an end-to-end synthetic activity sequence covering `A input -> navigate B -> navigate A -> A input` and `A input -> B input -> A input`, and verify the exact expected credits.
- [ ] 7.3 Validate a focus session with agent work, away grace, workspace grace, early termination, break countdown, restart, and presentation restoration.
- [ ] 7.4 Document installation, required Herdr protocol capability, configuration, privacy guarantees, database location, backup/removal, remote behavior, and dashboard controls.
- [ ] 7.5 Run `ruff format`, `ruff check --fix`, strict `pyright`, pytest, and `openspec validate`, then resolve every reported issue.
