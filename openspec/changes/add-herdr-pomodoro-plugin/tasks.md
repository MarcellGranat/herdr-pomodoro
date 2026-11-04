## 1. Plugin Foundation and Protocol Contract

- [x] 1.1 Create the typed Python package, `pyproject.toml`, plugin manifest, executable entrypoints, ignored development-state patterns, and pytest layout.
- [x] 1.2 Replace the obsolete Herdr-event contract with typed client/pane PTY topology, timestamp sample, correlation, and provenance models.
- [x] 1.3 Implement same-user Herdr client and pane process discovery, controlling-TTY resolution, nanosecond access-time sampling, and stale-mapping refresh.
- [x] 1.4 Implement a configurable one-to-one correlator that emits an observation only for matched outer-client and inner-pane input timestamps and discards unmatched or ambiguous samples.
- [x] 1.5 Validate the spike live for SSH/mosh human input, idle polling, continuous output, Herdr navigation, paste, and programmatic pane input, recording measured timing bounds as test fixtures.
- [x] 1.6 Add topology capability detection and diagnostics that fail closed when local or native-remote PTYs cannot be resolved safely.
- [x] 1.7 Implement the watcher entrypoint and macOS per-user service reconciliation so capture continues without an open plugin pane and only one watcher owns a topology.

## 2. Private Persistence

- [x] 2.1 Implement plugin config and state path discovery using `HERDR_PLUGIN_CONFIG_DIR` and `HERDR_PLUGIN_STATE_DIR`, with test-only path injection.
- [x] 2.2 Implement idempotent SQLite migrations for machines, projects, workspace instances, revisions, activity anchors, activity segments, processed observations, focus sessions, and timed phases.
- [x] 2.3 Configure WAL mode, foreign keys, busy timeouts, short transactions, and schema-version checks for concurrent event-hook writes and dashboard reads.
- [x] 2.4 Add migration, concurrency, deduplication, and private-path tests using temporary databases.

## 3. Human Activity Accounting

- [x] 3.1 Implement the configurable global input-anchor state machine with the default 120-second maximum gap.
- [x] 3.2 Implement correlated-observation ingestion and compact segment merging without persisting keys, text, commands, clipboard content, or raw per-input rows.
- [x] 3.3 Test same-workspace credit, over-limit gaps, and the rule that human pane input in another workspace breaks and replaces the prior anchor.
- [x] 3.4 Test that switching away and back without pane input leaves the anchor intact and that Herdr navigation, agent/API input, and terminal output create no credit.
- [x] 3.5 Test monotonic duration calculation, wall-clock changes, observation replay, out-of-order samples, ambiguous correlations, and watcher restart behavior.

## 4. Project, Workspace, and Revision Attribution

- [x] 4.1 Implement durable machine, repository, project, workspace, and worktree identity resolution without conflating identical paths on different machines.
- [x] 4.2 Implement cached read-only Git metadata probes for repository root, commit ID, branch, and worktree provenance.
- [x] 4.3 Implement cached read-only Jujutsu metadata probes using stable change ID as the primary identity and commit ID, bookmarks, and description as details.
- [x] 4.4 Implement revision-transition attribution when interval endpoints differ without an observed boundary.
- [x] 4.5 Add fixture-based tests for local repositories, remote provenance, multiple worktrees, missing VCS metadata, Git revisions, Jujutsu revisions, and transition intervals.

## 5. Pomodoro Session Engine

- [x] 5.1 Implement the persisted `idle`, `focus`, `away_grace`, `workspace_grace`, and `break` state machine with configurable 25-, 3-, 3-, and 5-minute defaults.
- [x] 5.2 Implement plugin actions for starting focus, starting each grace mode, ending a session early, and opening or focusing the timer and dashboard panes.
- [x] 5.3 Implement the timer pane/controller with live countdown rendering, deadline-based restart recovery, phase transitions, and recorded termination reasons.
- [x] 5.4 Implement best-effort capture, reduction, and exact restoration of supported Herdr workspace/sidebar presentation state with capability-aware degradation.
- [x] 5.5 Implement agent-status protection and an optional Herdr Notify monitored-target adapter while storing protected time as agent-supervision rather than human-active time.
- [x] 5.6 Test normal completion, early termination, restart recovery, grace expiry, cross-workspace input during grace, agent protection, and failed presentation operations.

## 6. Work Activity Dashboard

- [x] 6.1 Implement typed query iterators for daily category totals, project/workspace rollups, revision rollups, sessions, and chronological segments.
- [x] 6.2 Implement the Herdr dashboard pane with a selected-day summary and explicit human-active, focus-elapsed, agent-supervision, grace, and break totals.
- [x] 6.3 Add project, workspace/worktree, Git commit, Jujutsu change, machine-provenance, session, category, and date-range filtering.
- [x] 6.4 Add a chronological timeline that distinguishes activity categories, session phases, revision transitions, and empty periods without inferring unrecorded work.
- [x] 6.5 Test total reconciliation, local/remote name collisions, filters, empty states, live refresh, and consistent reads during concurrent writes.

## 7. Integration, Quality, and Documentation

- [x] 7.1 Link the plugin locally and verify manifest actions, watcher lifecycle, timer pane ownership, dashboard pane ownership, config creation, and state creation against the installed Herdr version.
- [x] 7.2 Run an end-to-end synthetic activity sequence covering `A input -> navigate B -> navigate A -> A input` and `A input -> B input -> A input`, and verify the exact expected credits.
- [x] 7.3 Validate a focus session with agent work, away grace, workspace grace, early termination, break countdown, restart, and presentation restoration.
- [x] 7.4 Document installation, PTY topology requirements, watcher configuration, privacy guarantees, database location, backup/removal, remote behavior, and dashboard controls.
- [x] 7.5 Run `ruff format`, `ruff check --fix`, strict `pyright`, pytest, and `openspec validate`, then resolve every reported issue.

## 8. Sidebar Focus Presentation

- [ ] 8.1 Replace start-action timer-pane opening with a workspace-metadata countdown that refreshes for focus, grace, and break phases and clears when idle.
- [ ] 8.2 Install a plugin-owned Agent view filtered to the focus workspace and clear only that owned view when focus ends, while leaving unsupported workspace-row visibility unchanged.
- [ ] 8.3 Start workspace grace automatically on human pane input outside the focus workspace, ignore navigation-only switches, and resume focus on input in the focus workspace before expiry.
- [ ] 8.4 Add the `$pomodoro` workspace token to the documented Herdr sidebar configuration and update controls and limitation documentation.
- [ ] 8.5 Add unit and integration coverage for metadata refresh and cleanup, Agent-view ownership, automatic grace, return, expiry, restart recovery, and the absence of a timer pane.
- [ ] 8.6 Run formatting, lint, strict type checking, tests, strict OpenSpec validation, and live Herdr verification for the sidebar flow.
