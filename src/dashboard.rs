use std::io::{self, Stdout};
use std::time::{Duration as StdDuration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Bar, BarChart, BarGroup, Block, BorderType, Borders, Cell, Clear, List, ListItem, Paragraph,
    Row, Table, TableState, Tabs, Wrap,
};
use rusqlite::params;

use crate::database::Database;

const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;
const CATEGORY_ORDER: [&str; 5] = [
    "human_active",
    "focus_elapsed",
    "agent_supervision",
    "grace",
    "break",
];
const COLORS: [Color; 5] = [
    Color::Green,
    Color::Red,
    Color::Cyan,
    Color::Yellow,
    Color::Blue,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DashboardFilters {
    pub day: NaiveDate,
    pub start_ns: i64,
    pub end_ns: i64,
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub machine_id: Option<String>,
    pub session_id: Option<String>,
    pub category: Option<String>,
    pub revision_id: Option<String>,
}

impl DashboardFilters {
    pub fn for_day(day: NaiveDate) -> Result<Self> {
        let start_local = Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).context("invalid day")?)
            .single()
            .context("ambiguous local day start")?;
        let next = day
            .checked_add_days(Days::new(1))
            .context("date overflow")?;
        let end_local = Local
            .from_local_datetime(&next.and_hms_opt(0, 0, 0).context("invalid next day")?)
            .single()
            .context("ambiguous local day end")?;
        Ok(Self {
            day,
            start_ns: timestamp_ns(start_local.with_timezone(&Utc)),
            end_ns: timestamp_ns(end_local.with_timezone(&Utc)).saturating_sub(1),
            project_id: None,
            workspace_id: None,
            machine_id: None,
            session_id: None,
            category: None,
            revision_id: None,
        })
    }

    fn set_day(&mut self, day: NaiveDate) -> Result<()> {
        let replacement = Self::for_day(day)?;
        self.day = replacement.day;
        self.start_ns = replacement.start_ns;
        self.end_ns = replacement.end_ns;
        Ok(())
    }

    fn clear_dimensions(&mut self) {
        self.project_id = None;
        self.workspace_id = None;
        self.machine_id = None;
        self.session_id = None;
        self.category = None;
        self.revision_id = None;
    }

    fn description(&self) -> String {
        let mut values = vec![self.day.to_string()];
        for (name, value) in [
            ("project", &self.project_id),
            ("workspace", &self.workspace_id),
            ("machine", &self.machine_id),
            ("session", &self.session_id),
            ("category", &self.category),
            ("revision", &self.revision_id),
        ] {
            if let Some(value) = value {
                values.push(format!("{name}={}", shorten(value, 12)));
            }
        }
        values.join("  ·  ")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineEntry {
    pub category: String,
    pub start_ns: i64,
    pub end_ns: i64,
    pub duration_ns: i64,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_path: Option<String>,
    pub machine_id: Option<String>,
    pub machine_label: Option<String>,
    pub revision_id: Option<String>,
    pub revision_label: Option<String>,
    pub session_id: Option<String>,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Total {
    pub id: String,
    pub label: String,
    pub duration_ns: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub workspace_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    pub termination_reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DashboardData {
    pub timeline: Vec<TimelineEntry>,
    pub categories: Vec<Total>,
    pub projects: Vec<Total>,
    pub workspaces: Vec<Total>,
    pub revisions: Vec<Total>,
    pub machines: Vec<Total>,
    pub sessions: Vec<SessionSummary>,
}

impl DashboardData {
    pub fn load(database: &Database, filters: &DashboardFilters) -> Result<Self> {
        let mut timeline = activity_entries(database, filters)?;
        timeline.extend(phase_entries(database, filters)?);
        timeline.sort_by_key(|entry| (entry.start_ns, entry.end_ns));
        let categories = CATEGORY_ORDER
            .iter()
            .map(|category| Total {
                id: (*category).to_owned(),
                label: category.replace('_', " "),
                duration_ns: timeline
                    .iter()
                    .filter(|entry| entry.category == *category)
                    .map(|entry| entry.duration_ns)
                    .sum(),
            })
            .collect();
        Ok(Self {
            projects: grouped_totals(
                &timeline,
                |entry| {
                    entry
                        .project_id
                        .as_deref()
                        .zip(entry.project_name.as_deref())
                },
                true,
            ),
            workspaces: grouped_totals(
                &timeline,
                |entry| {
                    entry
                        .workspace_id
                        .as_deref()
                        .zip(entry.workspace_path.as_deref())
                },
                true,
            ),
            revisions: grouped_totals(
                &timeline,
                |entry| {
                    entry
                        .revision_id
                        .as_deref()
                        .zip(entry.revision_label.as_deref())
                },
                true,
            ),
            machines: grouped_totals(
                &timeline,
                |entry| {
                    entry
                        .machine_id
                        .as_deref()
                        .zip(entry.machine_label.as_deref())
                },
                true,
            ),
            sessions: sessions(database, filters)?,
            categories,
            timeline,
        })
    }
}

fn grouped_totals<'a>(
    entries: &'a [TimelineEntry],
    key: impl Fn(&'a TimelineEntry) -> Option<(&'a str, &'a str)>,
    human_only: bool,
) -> Vec<Total> {
    let mut totals = std::collections::BTreeMap::<(String, String), i64>::new();
    for entry in entries {
        if human_only && entry.category != "human_active" {
            continue;
        }
        if let Some((id, label)) = key(entry) {
            *totals.entry((id.to_owned(), label.to_owned())).or_default() += entry.duration_ns;
        }
    }
    totals
        .into_iter()
        .map(|((id, label), duration_ns)| Total {
            id,
            label,
            duration_ns,
        })
        .collect()
}

fn activity_entries(database: &Database, filters: &DashboardFilters) -> Result<Vec<TimelineEntry>> {
    let connection = database.connect_readonly()?;
    let mut statement = connection.prepare("SELECT s.category, s.start_wall_ns, s.end_wall_ns, s.duration_ns, s.project_id, p.name, s.workspace_instance_id, w.path, s.machine_id, m.label, s.revision_id, r.kind, r.stable_id, s.session_id, s.provenance FROM activity_segments s LEFT JOIN projects p ON p.id=s.project_id LEFT JOIN workspace_instances w ON w.id=s.workspace_instance_id LEFT JOIN machines m ON m.id=s.machine_id LEFT JOIN revisions r ON r.id=s.revision_id WHERE s.end_wall_ns>=?1 AND s.start_wall_ns<=?2 AND (?3 IS NULL OR s.project_id=?3) AND (?4 IS NULL OR s.workspace_instance_id=?4) AND (?5 IS NULL OR s.machine_id=?5) AND (?6 IS NULL OR s.session_id=?6) AND (?7 IS NULL OR s.category=?7) AND (?8 IS NULL OR s.revision_id=?8)")?;
    let rows = statement.query_map(
        params![
            filters.start_ns,
            filters.end_ns,
            filters.project_id,
            filters.workspace_id,
            filters.machine_id,
            filters.session_id,
            filters.category,
            filters.revision_id
        ],
        |row| {
            let raw_start: i64 = row.get(1)?;
            let raw_end: i64 = row.get(2)?;
            let raw_duration: i64 = row.get(3)?;
            let start = raw_start.max(filters.start_ns);
            let end = raw_end.min(filters.end_ns);
            let original = (raw_end - raw_start).max(1);
            let duration = raw_duration.saturating_mul((end - start).max(0)) / original;
            let kind: Option<String> = row.get(11)?;
            let stable: Option<String> = row.get(12)?;
            Ok(TimelineEntry {
                category: row.get(0)?,
                start_ns: start,
                end_ns: end,
                duration_ns: duration,
                project_id: row.get(4)?,
                project_name: row.get(5)?,
                workspace_id: row.get(6)?,
                workspace_path: row.get(7)?,
                machine_id: row.get(8)?,
                machine_label: row.get(9)?,
                revision_id: row.get(10)?,
                revision_label: kind
                    .zip(stable)
                    .map(|(kind, stable)| format!("{kind}:{stable}")),
                session_id: row.get(13)?,
                provenance: row.get(14)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn phase_entries(database: &Database, filters: &DashboardFilters) -> Result<Vec<TimelineEntry>> {
    let connection = database.connect_readonly()?;
    let mut statement = connection.prepare("SELECT p.category, p.started_at, p.deadline, p.ended_at, p.session_id, s.workspace_instance_id, w.path, w.project_id, w.machine_id, pr.name, m.label, p.protection_source FROM timed_phases p JOIN focus_sessions s ON s.id=p.session_id LEFT JOIN workspace_instances w ON w.id=s.workspace_instance_id LEFT JOIN projects pr ON pr.id=w.project_id LEFT JOIN machines m ON m.id=w.machine_id WHERE p.category!='agent_supervision'")?;
    let now = timestamp_ns(Utc::now());
    let rows = statement.query_map([], |row| {
        let started: String = row.get(1)?;
        let deadline: String = row.get(2)?;
        let ended: Option<String> = row.get(3)?;
        let start = parse_timestamp_ns(&started)
            .unwrap_or(0)
            .max(filters.start_ns);
        let end = ended
            .as_deref()
            .and_then(|value| parse_timestamp_ns(value).ok())
            .unwrap_or_else(|| parse_timestamp_ns(&deadline).unwrap_or(now))
            .min(parse_timestamp_ns(&deadline).unwrap_or(now))
            .min(now)
            .min(filters.end_ns);
        Ok(TimelineEntry {
            category: row.get(0)?,
            start_ns: start,
            end_ns: end,
            duration_ns: (end - start).max(0),
            project_id: row.get(7)?,
            project_name: row.get(9)?,
            workspace_id: row.get(5)?,
            workspace_path: row.get(6)?,
            machine_id: row.get(8)?,
            machine_label: row.get(10)?,
            revision_id: None,
            revision_label: None,
            session_id: row.get(4)?,
            provenance: row
                .get::<_, Option<String>>(11)?
                .unwrap_or_else(|| "pomodoro".to_owned()),
        })
    })?;
    let mut values = Vec::new();
    for row in rows {
        let entry = row?;
        if entry.duration_ns <= 0 || !matches_filters(&entry, filters) {
            continue;
        }
        values.push(entry);
    }
    Ok(values)
}

fn matches_filters(entry: &TimelineEntry, filters: &DashboardFilters) -> bool {
    [
        (entry.project_id.as_ref(), filters.project_id.as_ref()),
        (entry.workspace_id.as_ref(), filters.workspace_id.as_ref()),
        (entry.machine_id.as_ref(), filters.machine_id.as_ref()),
        (entry.session_id.as_ref(), filters.session_id.as_ref()),
        (Some(&entry.category), filters.category.as_ref()),
        (entry.revision_id.as_ref(), filters.revision_id.as_ref()),
    ]
    .iter()
    .all(|(actual, expected)| expected.is_none() || *actual == *expected)
}

fn sessions(database: &Database, filters: &DashboardFilters) -> Result<Vec<SessionSummary>> {
    let connection = database.connect_readonly()?;
    let start = datetime_from_ns(filters.start_ns).to_rfc3339();
    let end = datetime_from_ns(filters.end_ns).to_rfc3339();
    let mut statement = connection.prepare("SELECT id, workspace_id, started_at, ended_at, status, termination_reason FROM focus_sessions WHERE started_at<=?1 AND COALESCE(ended_at, ?1)>=?2 AND (?3 IS NULL OR id=?3) ORDER BY started_at")?;
    Ok(statement
        .query_map(params![end, start, filters.session_id], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                status: row.get(4)?,
                termination_reason: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Overview,
    Projects,
    Workspaces,
    Revisions,
    Machines,
    Sessions,
    Timeline,
}

impl View {
    const ALL: [Self; 7] = [
        Self::Overview,
        Self::Projects,
        Self::Workspaces,
        Self::Revisions,
        Self::Machines,
        Self::Sessions,
        Self::Timeline,
    ];
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Projects => "Projects",
            Self::Workspaces => "Workspaces",
            Self::Revisions => "Revisions",
            Self::Machines => "Machines",
            Self::Sessions => "Sessions",
            Self::Timeline => "Timeline",
        }
    }
}

struct DashboardApp {
    database: Database,
    filters: DashboardFilters,
    data: DashboardData,
    view: usize,
    selected: usize,
    last_refresh: Instant,
    error: Option<String>,
}

impl DashboardApp {
    fn new(database: Database, filters: DashboardFilters) -> Self {
        let mut app = Self {
            database,
            filters,
            data: DashboardData::default(),
            view: 0,
            selected: 0,
            last_refresh: Instant::now(),
            error: None,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        match DashboardData::load(&self.database, &self.filters) {
            Ok(data) => {
                self.data = data;
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        self.last_refresh = Instant::now();
        self.selected = self.selected.min(self.current_len().saturating_sub(1));
    }

    fn current_view(&self) -> View {
        View::ALL[self.view]
    }
    fn current_len(&self) -> usize {
        match self.current_view() {
            View::Overview => self.data.categories.len(),
            View::Projects => self.data.projects.len(),
            View::Workspaces => self.data.workspaces.len(),
            View::Revisions => self.data.revisions.len(),
            View::Machines => self.data.machines.len(),
            View::Sessions => self.data.sessions.len(),
            View::Timeline => self.data.timeline.len(),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                return Ok(false);
            }
            (KeyCode::Tab | KeyCode::Right, _) => {
                self.view = (self.view + 1) % View::ALL.len();
                self.selected = 0;
            }
            (KeyCode::BackTab | KeyCode::Left, _) => {
                self.view = (self.view + View::ALL.len() - 1) % View::ALL.len();
                self.selected = 0;
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                self.selected = (self.selected + 1).min(self.current_len().saturating_sub(1))
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                self.selected = self.selected.saturating_sub(1)
            }
            (KeyCode::Char('['), _) => {
                if let Some(day) = self.filters.day.checked_sub_days(Days::new(1)) {
                    self.filters.set_day(day)?;
                    self.refresh();
                }
            }
            (KeyCode::Char(']'), _) => {
                if let Some(day) = self.filters.day.checked_add_days(Days::new(1)) {
                    self.filters.set_day(day)?;
                    self.refresh();
                }
            }
            (KeyCode::Char('d'), _) => {
                self.filters.set_day(Local::now().date_naive())?;
                self.refresh();
            }
            (KeyCode::Char('r'), _) => self.refresh(),
            (KeyCode::Char('c'), _) | (KeyCode::Esc, _) => {
                self.filters.clear_dimensions();
                self.refresh();
            }
            (KeyCode::Char(value @ '1'..='5'), _) => {
                let index = value.to_digit(10).unwrap_or(1) as usize - 1;
                let category = CATEGORY_ORDER[index].to_owned();
                self.filters.category =
                    (self.filters.category.as_deref() != Some(&category)).then_some(category);
                self.refresh();
            }
            (KeyCode::Enter, _) => {
                self.toggle_selected_filter();
                self.refresh();
            }
            _ => {}
        }
        Ok(true)
    }

    fn toggle_selected_filter(&mut self) {
        fn toggle(slot: &mut Option<String>, value: Option<String>) {
            if let Some(value) = value {
                if slot.as_deref() == Some(&value) {
                    *slot = None
                } else {
                    *slot = Some(value)
                }
            }
        }
        match self.current_view() {
            View::Projects => toggle(
                &mut self.filters.project_id,
                self.data
                    .projects
                    .get(self.selected)
                    .map(|value| value.id.clone()),
            ),
            View::Workspaces => toggle(
                &mut self.filters.workspace_id,
                self.data
                    .workspaces
                    .get(self.selected)
                    .map(|value| value.id.clone()),
            ),
            View::Revisions => toggle(
                &mut self.filters.revision_id,
                self.data
                    .revisions
                    .get(self.selected)
                    .map(|value| value.id.clone()),
            ),
            View::Machines => toggle(
                &mut self.filters.machine_id,
                self.data
                    .machines
                    .get(self.selected)
                    .map(|value| value.id.clone()),
            ),
            View::Sessions => toggle(
                &mut self.filters.session_id,
                self.data
                    .sessions
                    .get(self.selected)
                    .map(|value| value.id.clone()),
            ),
            _ => {}
        }
    }
}

pub fn run_dashboard(database: Database, filters: DashboardFilters) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_event_loop(&mut terminal, DashboardApp::new(database, filters));
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: DashboardApp,
) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, &app))?;
        if event::poll(StdDuration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && !app.handle_key(key)?
        {
            return Ok(());
        }
        if app.last_refresh.elapsed() >= StdDuration::from_secs(2) {
            app.refresh();
        }
    }
}

fn render(frame: &mut ratatui::Frame<'_>, app: &DashboardApp) {
    let area = frame.area();
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(if area.width >= 100 { 5 } else { 9 }),
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(area);
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " HERDR ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " POMODORO ACTIVITY",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(panel("", Color::DarkGray))
    .alignment(Alignment::Left);
    frame.render_widget(title, layout[0]);
    render_summary(frame, layout[1], &app.data.categories);
    let tabs = Tabs::new(
        View::ALL
            .iter()
            .map(|view| Line::from(view.title()))
            .collect::<Vec<_>>(),
    )
    .select(app.view)
    .highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .divider("  ")
    .block(panel(" Views ", Color::DarkGray));
    frame.render_widget(tabs, layout[2]);
    if let Some(error) = &app.error {
        frame.render_widget(Clear, layout[3]);
        frame.render_widget(
            Paragraph::new(format!("Could not read activity database\n\n{error}"))
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .block(panel(" Error ", Color::Red)),
            layout[3],
        );
    } else {
        render_body(frame, layout[3], app);
    }
    let footer = Paragraph::new(Line::from(vec![Span::styled(app.filters.description(), Style::default().fg(Color::Yellow)), Span::raw("   │   Tab/←→ view  j/k select  Enter filter  [/] day  1–5 category  c clear  r refresh  q quit")])).block(panel(" Filters & keys ", Color::DarkGray)).wrap(Wrap { trim: true });
    frame.render_widget(footer, layout[4]);
}

fn render_summary(frame: &mut ratatui::Frame<'_>, area: Rect, values: &[Total]) {
    let rows: usize = if area.width >= 100 { 1 } else { 2 };
    let outer = Layout::vertical(
        (0..rows)
            .map(|_| Constraint::Ratio(1, u32::try_from(rows).unwrap_or(1)))
            .collect::<Vec<_>>(),
    )
    .split(area);
    for (row_index, row_area) in outer.iter().enumerate() {
        let start = row_index * values.len().div_ceil(rows);
        let end = ((row_index + 1) * values.len().div_ceil(rows)).min(values.len());
        if start >= end {
            continue;
        }
        let cells = Layout::horizontal(
            (start..end)
                .map(|_| Constraint::Ratio(1, u32::try_from(end - start).unwrap_or(1)))
                .collect::<Vec<_>>(),
        )
        .split(*row_area);
        for (offset, value) in values[start..end].iter().enumerate() {
            let color = COLORS.get(start + offset).copied().unwrap_or(Color::White);
            let widget = Paragraph::new(vec![
                Line::from(Span::styled(
                    value.label.to_uppercase(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    duration(value.duration_ns),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
            ])
            .alignment(Alignment::Center)
            .block(panel("", color));
            frame.render_widget(widget, cells[offset]);
        }
    }
}

fn render_body(frame: &mut ratatui::Frame<'_>, area: Rect, app: &DashboardApp) {
    match app.current_view() {
        View::Overview => render_overview(frame, area, app),
        View::Projects => render_totals_table(
            frame,
            area,
            " Projects · Enter toggles filter ",
            &app.data.projects,
            app.selected,
        ),
        View::Workspaces => render_totals_table(
            frame,
            area,
            " Workspaces · Enter toggles filter ",
            &app.data.workspaces,
            app.selected,
        ),
        View::Revisions => render_totals_table(
            frame,
            area,
            " Revisions · Enter toggles filter ",
            &app.data.revisions,
            app.selected,
        ),
        View::Machines => render_totals_table(
            frame,
            area,
            " Machines · Enter toggles filter ",
            &app.data.machines,
            app.selected,
        ),
        View::Sessions => render_sessions(frame, area, app),
        View::Timeline => render_timeline(frame, area, app),
    }
}

fn render_overview(frame: &mut ratatui::Frame<'_>, area: Rect, app: &DashboardApp) {
    if app.data.timeline.is_empty() {
        render_empty(frame, area);
        return;
    }
    let split = if area.width >= 90 {
        Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)]).split(area)
    } else {
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area)
    };
    let bars: Vec<Bar> = app
        .data
        .categories
        .iter()
        .enumerate()
        .map(|(index, value)| {
            Bar::default()
                .value(u64::try_from(value.duration_ns / NANOSECONDS_PER_SECOND).unwrap_or(0))
                .label(Line::from(shorten(&value.label, 9)))
                .style(Style::default().fg(COLORS[index]))
        })
        .collect();
    frame.render_widget(
        BarChart::default()
            .block(panel(" Category seconds ", Color::Cyan))
            .bar_width(7)
            .bar_gap(2)
            .data(BarGroup::default().bars(&bars))
            .value_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        split[0],
    );
    let items: Vec<ListItem> = app
        .data
        .timeline
        .iter()
        .rev()
        .take(split[1].height.saturating_sub(2) as usize)
        .map(timeline_item)
        .collect();
    frame.render_widget(
        List::new(items).block(panel(" Recent timeline ", Color::Magenta)),
        split[1],
    );
}

fn render_totals_table(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    values: &[Total],
    selected: usize,
) {
    if values.is_empty() {
        render_empty(frame, area);
        return;
    }
    let rows = values.iter().map(|value| {
        Row::new(vec![
            Cell::from(value.label.clone()),
            Cell::from(duration(value.duration_ns)),
            Cell::from(shorten(&value.id, 16)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(58),
            Constraint::Length(12),
            Constraint::Percentage(25),
        ],
    )
    .header(
        Row::new(["Name", "Human active", "Identity"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
    .highlight_symbol(" ▶ ")
    .block(panel(title, Color::Cyan));
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_sessions(frame: &mut ratatui::Frame<'_>, area: Rect, app: &DashboardApp) {
    if app.data.sessions.is_empty() {
        render_empty(frame, area);
        return;
    }
    let rows = app.data.sessions.iter().map(|value| {
        Row::new(vec![
            shorten(&value.id, 12),
            value.workspace_id.clone(),
            local_clock(&value.started_at),
            value.status.clone(),
            value.termination_reason.clone().unwrap_or_default(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(["Session", "Workspace", "Started", "Status", "Reason"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
    .highlight_symbol(" ▶ ")
    .block(panel(" Sessions · Enter toggles filter ", Color::Cyan));
    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_timeline(frame: &mut ratatui::Frame<'_>, area: Rect, app: &DashboardApp) {
    if app.data.timeline.is_empty() {
        render_empty(frame, area);
        return;
    }
    let max = app
        .data
        .timeline
        .iter()
        .map(|value| value.duration_ns)
        .max()
        .unwrap_or(1)
        .max(1);
    let items: Vec<ListItem> = app
        .data
        .timeline
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let width = ((value.duration_ns as f64 / max as f64) * 20.0).round() as usize;
            let color = CATEGORY_ORDER
                .iter()
                .position(|category| *category == value.category)
                .and_then(|position| COLORS.get(position))
                .copied()
                .unwrap_or(Color::White);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", local_time(value.start_ns)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<18}", value.category.replace('_', " ")),
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!("{:>9} ", duration(value.duration_ns)),
                    Style::default().fg(Color::White),
                ),
                Span::styled("█".repeat(width.max(1)), Style::default().fg(color)),
                Span::raw(format!(
                    "  {}",
                    value.workspace_path.as_deref().unwrap_or("session")
                )),
            ]))
            .style(if index == app.selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            })
        })
        .collect();
    frame.render_widget(
        List::new(items).block(panel(" Chronological activity ", Color::Magenta)),
        area,
    );
}

fn render_empty(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("No recorded activity matches these filters.\n\nPress c to clear filters or [ / ] to change day.").alignment(Alignment::Center).style(Style::default().fg(Color::DarkGray)).block(panel(" Empty ", Color::DarkGray)).wrap(Wrap { trim: true }), area);
}

fn timeline_item(value: &TimelineEntry) -> ListItem<'static> {
    ListItem::new(format!(
        "{}  {:<18} {:>9}  {}",
        local_time(value.start_ns),
        value.category.replace('_', " "),
        duration(value.duration_ns),
        value.workspace_path.as_deref().unwrap_or("session")
    ))
}

fn panel<'a>(title: &'a str, color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
}
fn duration(ns: i64) -> String {
    let seconds = ns.max(0) / NANOSECONDS_PER_SECOND;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}
fn timestamp_ns(value: DateTime<Utc>) -> i64 {
    value.timestamp_nanos_opt().unwrap_or(0)
}
fn datetime_from_ns(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(
        value / NANOSECONDS_PER_SECOND,
        u32::try_from(value.rem_euclid(NANOSECONDS_PER_SECOND)).unwrap_or(0),
    )
    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}
fn parse_timestamp_ns(value: &str) -> Result<i64> {
    Ok(timestamp_ns(
        DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc),
    ))
}
fn local_time(value: i64) -> String {
    datetime_from_ns(value)
        .with_timezone(&Local)
        .format("%H:%M:%S")
        .to_string()
}
fn local_clock(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| "?".into())
}
fn shorten(value: &str, length: usize) -> String {
    if value.chars().count() <= length {
        value.to_owned()
    } else {
        format!(
            "{}…",
            value
                .chars()
                .take(length.saturating_sub(1))
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_database_has_five_explicit_categories() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let filters = DashboardFilters::for_day(Local::now().date_naive()).unwrap();
        let data = DashboardData::load(&database, &filters).unwrap();
        assert_eq!(data.categories.len(), 5);
        assert!(data.categories.iter().all(|value| value.duration_ns == 0));
    }

    #[test]
    fn duration_format_is_stable() {
        assert_eq!(duration(3_661_000_000_000), "01:01:01");
    }

    #[test]
    fn activity_rows_reconcile_into_category_and_project_totals() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let connection = database.connect().unwrap();
        connection
            .execute(
                "INSERT INTO machines(id, kind, label) VALUES('m', 'local', 'Mac')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO projects(id, machine_id, repository_identity, name, root_path) VALUES('p', 'm', 'r', 'Project', '/project')", []).unwrap();
        connection.execute("INSERT INTO workspace_instances(id, machine_id, project_id, runtime_workspace_id, path, worktree_path) VALUES('w', 'm', 'p', 'A', '/project', '/project')", []).unwrap();
        let filters = DashboardFilters::for_day(Local::now().date_naive()).unwrap();
        connection.execute("INSERT INTO activity_segments(category, machine_id, project_id, workspace_instance_id, start_wall_ns, end_wall_ns, duration_ns, provenance) VALUES('human_active', 'm', 'p', 'w', ?1, ?2, 1000000000, 'test')", params![filters.start_ns + 1, filters.start_ns + 1_000_000_001]).unwrap();
        drop(connection);
        let data = DashboardData::load(&database, &filters).unwrap();
        assert_eq!(data.projects[0].duration_ns, 1_000_000_000);
        assert_eq!(data.categories[0].duration_ns, 1_000_000_000);
    }
}
