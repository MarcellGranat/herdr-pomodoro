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

## 8. Native Rust Migration

- [x] 8.1 Create the Rust crate, typed domain models, configuration/path discovery, CLI subcommands, JSON boundaries, and deterministic release executable layout.
- [x] 8.2 Port the SQLite schema, migrations, concurrency settings, repositories, and typed queries with compatibility tests against a Python-produced database fixture.
- [x] 8.3 Port PTY discovery, sampling, one-to-one correlation, capability diagnostics, activity accounting, Git/Jujutsu attribution, and watcher single ownership.
- [x] 8.4 Port the persisted Pomodoro engine, agent-supervision handling, actions, restart recovery, event hooks, and macOS LaunchAgent reconciliation.
- [x] 8.5 Replace timer-pane opening with a workspace-metadata countdown and plugin-owned Agent view; implement automatic cross-workspace grace, return, expiry, and cleanup.
- [x] 8.6 Build a responsive Ratatui dashboard with summary cards, navigable project/workspace and revision tables, sessions, charts, timeline, interactive filters, live refresh, and explicit empty/error states.
- [x] 8.7 Switch the Herdr manifest and documentation to direct Rust-binary invocation, add the `$pomodoro` sidebar token configuration, and document build, controls, privacy, service, backup, and removal behavior.
- [x] 8.8 Add Rust unit and integration coverage for accounting, persistence, attribution, correlation, Pomodoro lifecycle, presentation ownership, dashboard queries/state, end-to-end sequences, and live-measurement fixtures.
- [x] 8.9 Remove all Python source and tests, `pyproject.toml`, and `uv.lock` after Rust parity and database-compatibility checks pass.
- [x] 8.10 Run `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, strict OpenSpec validation, plugin linking, LaunchAgent verification, and live Herdr sidebar/dashboard validation.

## 9. Grace Buttons and Agent Visibility

- [x] 9.1 Specify separate three-minute leave-terminal and use-other-workspace actions that reveal all agents during grace and restore focus presentation on return.
- [x] 9.2 Implement grace presentation transitions and restart recovery with focused unit tests.
- [x] 9.3 Update action labels and documentation, then run Rust, OpenSpec, manifest-linking, and live action validation.

## 10. Unified Pomodoro Command Popup

- [x] 10.1 Specify a popup with Start focus, Open dashboard, Check another project, and Leave terminal, including active-session behavior.
- [x] 10.2 Implement the Rust popup selector and Herdr popup pane/action entrypoints.
- [x] 10.3 Enforce single-session start in the engine and add popup/state tests.
- [x] 10.4 Replace the direct dotfile bindings with `prefix+ctrl+p`, update documentation, and run Rust, OpenSpec, config, plugin-link, and live popup validation.

## 11. Phase Stop Controls and Sidebar Collapse

- [x] 11.1 Specify phase-aware End focus, Return to focus, End grace, and End break popup behavior plus capability-aware sidebar hiding.
- [x] 11.2 Implement context-sensitive popup entries, direct-to-idle stopping, explicit grace return, and focused engine/menu tests.
- [x] 11.3 Configure zero-width collapsed sidebar rendering, document the Herdr 0.9.5 plugin-API boundary, and run Rust, OpenSpec, config, and manifest validation.

## 12. Phase Alerts and Hidden Startup

- [x] 12.1 Specify hidden-at-start sidebar behavior, grace warning sound, focus-complete sound, and break-ended popup delivery.
- [x] 12.2 Add the notification ledger migration, Herdr notification adapter, phase transition alerts, watcher threshold handling, and deduplication tests.
- [x] 12.3 Enable collapsed sidebar startup in dotfiles, update documentation, and run Rust, OpenSpec, config, and release validation.

## 13. Guarded Workspace Picker

- [x] 13.1 Specify permanent sidebar-toggle disablement and workspace-picker availability limited to workspace grace.
- [x] 13.2 Implement typed workspace listing/focus adapters, the Ratatui picker, phase-aware menu integration, and focused tests.
- [x] 13.3 Disable the sidebar keybinding in dotfiles, update documentation, and run Rust, OpenSpec, config, and release validation.
