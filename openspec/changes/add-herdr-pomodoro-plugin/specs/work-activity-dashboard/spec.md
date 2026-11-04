## ADDED Requirements

### Requirement: Central daily summary
The system SHALL provide a central Herdr dashboard showing selected-day totals by activity category and project.

#### Scenario: View today's work
- **WHEN** the user opens the dashboard without selecting a date
- **THEN** the system shows today's human-active total separately from focus-elapsed, agent-supervision, grace, and break totals

#### Scenario: Activity spans projects
- **WHEN** the selected day contains activity in multiple projects
- **THEN** the system shows a project breakdown whose human-active values reconcile with the daily human-active total

### Requirement: Workspace and revision breakdown
The system SHALL break project activity down by workspace/worktree and by Git commit or stable Jujutsu change.

#### Scenario: Jujutsu project has multiple changes
- **WHEN** selected activity is attributed to multiple Jujutsu change IDs
- **THEN** the dashboard shows separate totals for each stable change and a separate transition total where applicable

#### Scenario: Project has multiple workspaces
- **WHEN** selected activity belongs to multiple workspace or worktree identities
- **THEN** the dashboard provides separate workspace totals and a reconciled project rollup

### Requirement: Chronological timeline
The system SHALL display a chronological timeline of credited activity segments and Pomodoro phases for the selected period.

#### Scenario: Focus contains work and grace
- **WHEN** a focus session contains human-active, agent-supervision, and grace periods
- **THEN** the timeline visually distinguishes each period and shows its workspace and session association

#### Scenario: No recorded activity
- **WHEN** the selected period has no segments or sessions
- **THEN** the dashboard displays an empty state rather than inferring work from repository or agent history

### Requirement: Local and remote provenance
The system SHALL expose machine provenance in dashboard filters and labels whenever activity comes from more than one machine context.

#### Scenario: Local and remote work share a project name
- **WHEN** activity exists for identically named projects on local and remote machines
- **THEN** the dashboard keeps their workspace totals distinct and labels their provenance

### Requirement: Interactive filtering
The system SHALL allow filtering by date range, project, workspace, machine, session, activity category, and revision without modifying source activity records.

#### Scenario: Filter to human work on one revision
- **WHEN** the user selects one project revision and the human-active category
- **THEN** the dashboard shows only matching segments and recalculates displayed totals from those segments

### Requirement: Concurrent read safety
The dashboard SHALL remain readable while event hooks append activity and session records.

#### Scenario: New activity arrives while dashboard is open
- **WHEN** an event hook commits a new segment during dashboard use
- **THEN** the dashboard refreshes to a consistent database snapshot without blocking activity capture beyond the configured database timeout
