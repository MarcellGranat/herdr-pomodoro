## ADDED Requirements

### Requirement: Privacy-preserving correlated PTY input detection
The system SHALL recognize human pane input only by correlating input-access timestamp changes on a Herdr client's outer PTY with timestamp changes on one mapped pane PTY, and it MUST NOT read or store input bytes, pressed keys, typed text, commands, clipboard contents, selected text, or terminal output.

#### Scenario: Human types through SSH or mosh
- **WHEN** the Herdr client outer PTY and one local pane PTY report input-access changes within the configured correlation window
- **THEN** the system produces one human-input observation for that pane and its workspace without input content

#### Scenario: Herdr consumes navigation
- **WHEN** the client outer PTY changes but no pane PTY changes within the correlation window
- **THEN** the system records no human-input observation and does not alter the activity anchor

#### Scenario: Programmatic pane input occurs
- **WHEN** a pane PTY changes but no Herdr client outer PTY changes within the correlation window
- **THEN** the system records no human-input observation

#### Scenario: Terminal output occurs
- **WHEN** pane or client PTY modification timestamps change without matching input-access timestamp changes
- **THEN** the system records no human-input observation

#### Scenario: Correlation is ambiguous
- **WHEN** timestamp changes cannot be matched to exactly one client and pane within the configured correlation window
- **THEN** the system discards the ambiguous sample rather than guessing an attribution

### Requirement: PTY topology discovery and capability status
The system SHALL discover same-user Herdr client PTYs and pane PTYs, map panes to workspace context, refresh stale process mappings, and clearly disable capture for topologies it cannot observe safely.

#### Scenario: Pane process is replaced
- **WHEN** a pane's shell or foreground process changes its controlling TTY
- **THEN** the system refreshes the mapping before attributing later observations

#### Scenario: Multiple Herdr clients are attached
- **WHEN** several Herdr clients have distinct controlling PTYs
- **THEN** the system correlates each outer PTY independently and prevents one client observation from being reused by another

#### Scenario: Native remote pane is supported by helper
- **WHEN** a Herdr-native remote pane has no local PTY and an authenticated remote sampler supplies timestamp and pane-identity metadata
- **THEN** the system can correlate and store the observation locally with remote provenance

#### Scenario: Required topology is unavailable
- **WHEN** the system cannot resolve a client PTY, pane PTY, or safe native-remote helper
- **THEN** it disables capture for the affected topology and reports the reason without inferring work

### Requirement: Global consecutive-input accounting
The system SHALL maintain one global last-human-input anchor and SHALL credit the gap between consecutive inputs only when both inputs belong to the same workspace and the gap does not exceed the configured maximum.

#### Scenario: Consecutive input in one workspace
- **WHEN** consecutive human pane inputs occur in workspace A within the default 120-second maximum
- **THEN** the system credits the elapsed interval to workspace A and replaces the anchor with the newer input

#### Scenario: Input changes workspace
- **WHEN** the current human pane input occurs in workspace B while the anchor belongs to workspace A
- **THEN** the system credits no interval and replaces the global anchor with the workspace B input

#### Scenario: Gap exceeds maximum
- **WHEN** consecutive human pane inputs in the same workspace are separated by more than the configured maximum
- **THEN** the system credits no interval and replaces the anchor with the newer input

### Requirement: Navigation does not affect the activity anchor
The system SHALL ignore workspace, tab, and pane navigation that is consumed by Herdr rather than forwarded as human input to a terminal pane.

#### Scenario: Switch away and return without pane input
- **WHEN** a user switches from workspace A to workspace B, provides no pane input in B, returns to A, and provides pane input in A within the maximum gap from the anchor
- **THEN** the system credits the interval to workspace A

#### Scenario: Pane input after switching
- **WHEN** a user switches from workspace A to workspace B and provides human pane input in B
- **THEN** the B input breaks A's chain and becomes the global anchor

### Requirement: Efficient segment persistence
The system SHALL coalesce high-frequency human input into compact credited activity segments without storing raw individual keys or mouse actions.

#### Scenario: Continuous typing
- **WHEN** a user produces many inputs in one workspace without exceeding the maximum gap
- **THEN** the database contains merged activity segments rather than one durable row per input

#### Scenario: Duplicate observation
- **WHEN** the same correlated PTY timestamp pair is processed again
- **THEN** the system performs no duplicate credit

### Requirement: Durable workspace and project attribution
The system SHALL attribute each segment to durable machine, project, workspace/worktree, and runtime provenance so local and remote paths do not collide.

#### Scenario: Same path exists on two machines
- **WHEN** two workspaces have identical repository paths but different machine identities
- **THEN** the system stores them as distinct workspace instances

#### Scenario: Multiple worktrees share a project
- **WHEN** distinct worktree paths resolve to the same repository identity
- **THEN** the system can report them separately and roll them up to the same project

### Requirement: Read-only revision attribution
The system SHALL attach read-only Git or Jujutsu revision metadata to credited activity without modifying repository history.

#### Scenario: Activity occurs in a Jujutsu workspace
- **WHEN** a credited interval resolves to a Jujutsu repository
- **THEN** the system records the stable change ID as the primary revision identity and records available commit ID, bookmarks, and description as details

#### Scenario: Activity occurs in a Git workspace
- **WHEN** a credited interval resolves to a Git repository
- **THEN** the system records the current commit ID and available branch metadata

#### Scenario: Revision differs across an interval
- **WHEN** the endpoints of a credited interval resolve to different revisions and no boundary time is known
- **THEN** the system attributes the interval to an explicit revision-transition category

### Requirement: Private local storage
The system SHALL store durable activity data under the Herdr plugin state directory and SHALL NOT place the production database in the plugin source repository.

#### Scenario: First plugin use
- **WHEN** the plugin receives its first activity or session event
- **THEN** it creates or migrates the private database in `HERDR_PLUGIN_STATE_DIR`

#### Scenario: Plugin is unlinked
- **WHEN** the user disables or unlinks the plugin
- **THEN** the private database remains available for recovery unless the user explicitly deletes it
