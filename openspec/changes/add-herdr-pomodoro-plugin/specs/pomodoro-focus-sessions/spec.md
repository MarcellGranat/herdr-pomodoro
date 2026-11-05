## ADDED Requirements

### Requirement: Configurable focus and break lifecycle
The system SHALL provide persisted focus and break phases with defaults of 25 minutes for focus and five minutes for break.

#### Scenario: Start focus session
- **WHEN** the user invokes the start-focus action in a workspace
- **THEN** the system creates a focus session bound to that workspace with a persisted deadline, publishes a live countdown on that workspace's sidebar row, and does not open a timer pane

#### Scenario: Focus completes
- **WHEN** the focus deadline is reached
- **THEN** the system plays the Herdr done sound, completes the focus phase, and starts the configured break countdown

#### Scenario: Break completes
- **WHEN** the five-minute break deadline is reached
- **THEN** the system returns to idle and displays a Pomodoro break-ended popup

#### Scenario: Restart during timed phase
- **WHEN** Herdr or the plugin restarts during a focus, grace, or break phase
- **THEN** the system reconstructs the current phase from its persisted deadline without extending the timer

### Requirement: Single-session command popup
The system SHALL provide one keyboard-opened popup for the primary Pomodoro commands and SHALL NOT replace an active session with a new one.

#### Scenario: Open command popup while idle
- **WHEN** the user opens the Pomodoro popup while no timed phase is active
- **THEN** it offers Start focus, Open dashboard, Check another project, and Leave terminal without opening a normal tab

#### Scenario: Open command popup during a session
- **WHEN** the popup observes an active focus, grace, or break phase
- **THEN** Start focus is visibly unavailable while dashboard and applicable grace commands remain selectable

#### Scenario: Start is invoked despite an active session
- **WHEN** any caller requests Start focus while a timed phase is already active
- **THEN** the engine rejects the request and preserves the existing session and deadline

#### Scenario: Stop focus from the popup
- **WHEN** the popup is opened during focus and the user selects End focus session
- **THEN** the engine terminates the session, restores owned presentation, and returns directly to idle

#### Scenario: Return from either grace
- **WHEN** the popup is opened during away or workspace grace and the user selects Return to focus
- **THEN** the engine resumes focus with the original focus deadline

#### Scenario: End grace or break
- **WHEN** the user selects End focus session during either grace or End break during the five-minute break
- **THEN** the engine closes the active phase and returns directly to idle

### Requirement: Focus presentation and restoration
The system SHALL publish a compact countdown through focused-workspace metadata, filter the built-in Agent view to the focus workspace, request complete sidebar hiding when Herdr exposes that capability, and clear presentation state owned by the plugin when the session ends.

#### Scenario: Presentation controls are supported
- **WHEN** focus begins and Herdr provides the required presentation controls
- **THEN** the system publishes the countdown token, installs an Agent view filtered to the focus workspace, leaves workspace rows visible, and clears its token and owned Agent view at session end

#### Scenario: Presentation control fails
- **WHEN** one or more requested presentation changes are unsupported or fail
- **THEN** the focus timer and activity accounting continue and the system reports degraded presentation without restoring state it did not change

#### Scenario: Installed Herdr lacks sidebar visibility control
- **WHEN** the installed Herdr plugin API cannot change client-local sidebar collapse state
- **THEN** Herdr starts with the configured zero-width collapsed sidebar, the plugin leaves that state unchanged during focus and grace, and it does not inject synthetic terminal input

### Requirement: Away and automatic cross-workspace grace
The system SHALL provide selectable away grace and automatic cross-workspace grace with a default maximum of three minutes.

#### Scenario: User leaves the terminal
- **WHEN** the user starts away grace during focus
- **THEN** the system shows a three-minute grace countdown, clears its focused Agent view so all agents are visible, and permits the focus session and agents to continue

#### Scenario: User explicitly leaves for another workspace
- **WHEN** the user invokes the use-other-workspace action during focus
- **THEN** the system starts a three-minute workspace grace countdown and clears its focused Agent view so all agents are visible

#### Scenario: User temporarily works elsewhere
- **WHEN** the user provides human pane input in another workspace during focus
- **THEN** the input breaks the focused workspace's activity chain and automatically starts three-minute workspace grace without opening a timer pane

#### Scenario: User only navigates elsewhere
- **WHEN** the user switches to another workspace but provides no human pane input there
- **THEN** the system keeps the focus phase active and does not start workspace grace

#### Scenario: User returns during workspace grace
- **WHEN** the user provides human pane input in the focus workspace before workspace grace expires
- **THEN** the system resumes focus with the original focus deadline, reinstalls its focused Agent view, and updates the sidebar countdown

#### Scenario: User returns during away grace
- **WHEN** the user provides human pane input in the focus workspace before away grace expires
- **THEN** the system resumes focus with the original focus deadline, reinstalls its focused Agent view, and updates the sidebar countdown

#### Scenario: Grace expires
- **WHEN** a grace deadline passes without return to the focused workspace
- **THEN** the system terminates focus, clears its sidebar and Agent-view presentation, and records the outcome

#### Scenario: Grace reaches two minutes forty seconds remaining
- **WHEN** either three-minute grace first reaches `02:40` remaining
- **THEN** the system plays one Herdr request sound and durably suppresses duplicate delivery for that timed phase

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
