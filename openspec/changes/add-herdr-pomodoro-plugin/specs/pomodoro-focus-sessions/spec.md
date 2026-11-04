## ADDED Requirements

### Requirement: Configurable focus and break lifecycle
The system SHALL provide persisted focus and break phases with defaults of 25 minutes for focus and five minutes for break.

#### Scenario: Start focus session
- **WHEN** the user invokes the start-focus action in a workspace
- **THEN** the system creates a focus session bound to that workspace with a persisted deadline, publishes a live countdown on that workspace's sidebar row, and does not open a timer pane

#### Scenario: Focus completes
- **WHEN** the focus deadline is reached
- **THEN** the system completes the focus phase and starts the configured break countdown

#### Scenario: Restart during timed phase
- **WHEN** Herdr or the plugin restarts during a focus, grace, or break phase
- **THEN** the system reconstructs the current phase from its persisted deadline without extending the timer

### Requirement: Focus presentation and restoration
The system SHALL publish a compact countdown through focused-workspace metadata, filter the built-in Agent view to the focus workspace, and clear presentation state owned by the plugin when the session ends.

#### Scenario: Presentation controls are supported
- **WHEN** focus begins and Herdr provides the required presentation controls
- **THEN** the system publishes the countdown token, installs an Agent view filtered to the focus workspace, leaves workspace rows visible, and clears its token and owned Agent view at session end

#### Scenario: Presentation control fails
- **WHEN** one or more requested presentation changes are unsupported or fail
- **THEN** the focus timer and activity accounting continue and the system reports degraded presentation without restoring state it did not change

### Requirement: Away and automatic cross-workspace grace
The system SHALL provide selectable away grace and automatic cross-workspace grace with a default maximum of three minutes.

#### Scenario: User leaves the terminal
- **WHEN** the user starts away grace during focus
- **THEN** the system shows a three-minute grace countdown while permitting the focus session and agents to continue

#### Scenario: User temporarily works elsewhere
- **WHEN** the user provides human pane input in another workspace during focus
- **THEN** the input breaks the focused workspace's activity chain and automatically starts three-minute workspace grace without opening a timer pane

#### Scenario: User only navigates elsewhere
- **WHEN** the user switches to another workspace but provides no human pane input there
- **THEN** the system keeps the focus phase active and does not start workspace grace

#### Scenario: User returns during workspace grace
- **WHEN** the user provides human pane input in the focus workspace before workspace grace expires
- **THEN** the system resumes focus with the original focus deadline and updates the sidebar countdown

#### Scenario: Grace expires
- **WHEN** a grace deadline passes without return to the focused workspace
- **THEN** the system terminates focus, clears its sidebar and Agent-view presentation, and records the outcome

### Requirement: Activity categories remain distinct
The system SHALL record human-active, agent-supervision, grace, focus-elapsed, and break durations as distinct measures.

#### Scenario: Agent works without human input
- **WHEN** an agent is working during a focus session and no qualifying human input interval occurs
- **THEN** the system records no human-active time for that period

#### Scenario: Human resumes input
- **WHEN** qualifying consecutive human inputs occur during focus
- **THEN** the system credits human-active time independently of the focus wall clock

### Requirement: Agent supervision can protect focus
The system SHALL allow supported agent-working or explicitly monitored notification states to protect a focus session from inactivity handling without converting that time to human-active work.

#### Scenario: Focused agent is working
- **WHEN** the configured focused-workspace agent enters the working state during otherwise inactive focus time
- **THEN** the system keeps the focus session eligible and classifies protected time as agent-supervision

#### Scenario: Monitored target is active
- **WHEN** an integrated Herdr Notify target is explicitly monitored and meets the configured protection condition
- **THEN** the system protects focus and records the protection source without human-active credit

### Requirement: User-controlled termination
The system SHALL provide an action to end a running focus or grace phase early and SHALL record its actual duration and termination reason.

#### Scenario: End focus early
- **WHEN** the user invokes the end-session action before the focus deadline
- **THEN** the system stops the active phase, restores presentation state, records an early termination, and applies the configured post-session break behavior
