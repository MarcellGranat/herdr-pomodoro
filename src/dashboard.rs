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
use rusqlite::{OptionalExtension, params};

use crate::database::Database;

const NANOSECONDS_PER_SECOND: i64 = 1_000_000_000;
const CATEGORY_ORDER: [&str; 4] = ["human_active", "focus_elapsed", "break", "agent_working"];
const COLORS: [Color; 4] = [Color::Green, Color::Red, Color::Blue, Color::Magenta];

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

    fn with_day(&self, day: NaiveDate) -> Result<Self> {
        let mut copy = self.clone();
        copy.set_day(day)?;
        Ok(copy)
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
    /// Passive agent-working time, kept separate from human-active time.
    pub passive_ns: i64,
}

/// Human-active and passive agent totals, per project, inside one time bucket.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BucketTotals {
    pub human: Vec<Total>,
    pub agent: Vec<Total>,
    /// Human-active spans clipped to the bucket, positioned inside it.
    pub segments: Vec<Segment>,
}

/// One human-active span of a project, as offsets from the bucket start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub project_id: String,
    pub start_ns: i64,
    pub end_ns: i64,
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
    pub workspaces: Vec<Total>,
    pub revisions: Vec<Total>,
    pub sessions: Vec<SessionSummary>,
    pub hourly: Vec<BucketTotals>,
    pub daily: Vec<(NaiveDate, BucketTotals)>,
    /// Completed Pomodoros overlapping the selected day, as `(start_ns, end_ns)`.
    pub pomodoros: Vec<(i64, i64)>,
    pub all_projects: Vec<(String, String)>,
}

pub const HOURS_PER_DAY: usize = 24;
pub const DAYS_SHOWN: usize = 14;
const NANOSECONDS_PER_HOUR: i64 = 3_600 * NANOSECONDS_PER_SECOND;
const HUMAN_CATEGORY: &str = "human_active";
const AGENT_CATEGORY: &str = "agent_working";

/// Splits timeline entries into buckets and groups each bucket by project.
/// Human time is apportioned by overlap; agent time is the union of
/// overlapping agent intervals, so parallel agents are not double counted.
pub fn bucket_totals(timeline: &[TimelineEntry], buckets: &[(i64, i64)]) -> Vec<BucketTotals> {
    buckets
        .iter()
        .map(|&(bucket_start, bucket_end)| {
            let mut human: Vec<Total> = Vec::new();
            let mut segments: Vec<Segment> = Vec::new();
            let mut intervals: std::collections::BTreeMap<String, (String, Vec<(i64, i64)>)> =
                std::collections::BTreeMap::new();
            for entry in timeline {
                let (Some(id), Some(name)) =
                    (entry.project_id.as_deref(), entry.project_name.as_deref())
                else {
                    continue;
                };
                let low = entry.start_ns.max(bucket_start);
                let high = entry.end_ns.min(bucket_end);
                if high <= low {
                    continue;
                }
                if entry.category == HUMAN_CATEGORY {
                    segments.push(Segment {
                        project_id: id.to_owned(),
                        start_ns: low - bucket_start,
                        end_ns: high - bucket_start,
                    });
                    let span = (entry.end_ns - entry.start_ns).max(1);
                    let share = (i128::from(entry.duration_ns) * i128::from(high - low)
                        / i128::from(span)) as i64;
                    match human.iter_mut().find(|total| total.id == id) {
                        Some(total) => total.duration_ns += share,
                        None => human.push(Total {
                            id: id.to_owned(),
                            label: name.to_owned(),
                            duration_ns: share,
                            passive_ns: 0,
                        }),
                    }
                } else if entry.category == AGENT_CATEGORY {
                    intervals
                        .entry(id.to_owned())
                        .or_insert_with(|| (name.to_owned(), Vec::new()))
                        .1
                        .push((low, high));
                }
            }
            human.sort_by(|left, right| left.label.cmp(&right.label));
            let agent = intervals
                .into_iter()
                .map(|(id, (label, mut spans))| {
                    spans.sort_unstable();
                    let mut covered = 0;
                    let mut reach = i64::MIN;
                    for (low, high) in spans {
                        let low = low.max(reach);
                        if high > low {
                            covered += high - low;
                        }
                        reach = reach.max(high);
                    }
                    Total {
                        id,
                        label,
                        duration_ns: covered,
                        passive_ns: 0,
                    }
                })
                .collect();
            BucketTotals {
                human,
                agent,
                segments,
            }
        })
        .collect()
}

impl DashboardData {
    pub fn load(database: &Database, filters: &DashboardFilters) -> Result<Self> {
        let mut timeline = activity_entries(database, filters)?;
        let credited = unconfirmed_focus_entries(database, filters, &timeline)?;
        timeline.extend(credited);
        timeline.extend(phase_entries(database, filters)?);
        timeline.extend(agent_entries(database, filters)?);
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
                passive_ns: 0,
            })
            .collect();
        let hour_buckets: Vec<(i64, i64)> = (0..HOURS_PER_DAY as i64)
            .map(|hour| {
                let start = filters.start_ns + hour * NANOSECONDS_PER_HOUR;
                (start, start + NANOSECONDS_PER_HOUR)
            })
            .collect();
        Ok(Self {
            workspaces: grouped_totals(&timeline, |entry| {
                entry
                    .workspace_id
                    .clone()
                    .zip(entry.workspace_path.as_deref().map(|path| {
                        format!(
                            "{}  {path}",
                            short_machine(entry.machine_label.as_deref().unwrap_or("?"))
                        )
                    }))
            }),
            revisions: grouped_totals(&timeline, |entry| {
                entry
                    .revision_id
                    .clone()
                    .zip(entry.revision_label.as_deref().map(|label| {
                        format!(
                            "{}  ·  {label}",
                            entry.project_name.as_deref().unwrap_or("?")
                        )
                    }))
            }),
            sessions: sessions(database, filters)?,
            hourly: bucket_totals(&timeline, &hour_buckets),
            daily: daily_totals(database, filters)?,
            pomodoros: completed_pomodoros(database, filters)?,
            all_projects: all_projects(database)?,
            categories,
            timeline,
        })
    }
}

fn short_machine(label: &str) -> &str {
    label.strip_suffix(".local").unwrap_or(label)
}

fn all_projects(database: &Database) -> Result<Vec<(String, String)>> {
    let connection = database.connect_readonly()?;
    let mut statement = connection.prepare("SELECT id, name FROM projects ORDER BY name, id")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Completed Pomodoros (focus ran to its original deadline) overlapping the day.
fn completed_pomodoros(database: &Database, filters: &DashboardFilters) -> Result<Vec<(i64, i64)>> {
    let connection = database.connect_readonly()?;
    let mut statement = connection.prepare("SELECT p.started_at, p.deadline FROM timed_phases p WHERE p.phase='focus' AND p.category='focus_elapsed' AND p.id=(SELECT MIN(q.id) FROM timed_phases q WHERE q.session_id=p.session_id AND q.phase='focus') AND EXISTS (SELECT 1 FROM timed_phases r WHERE r.session_id=p.session_id AND r.phase='focus' AND r.termination_reason='completed')")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut spans = Vec::new();
    for row in rows {
        let (started, deadline) = row?;
        let (Ok(start), Ok(end)) = (parse_timestamp_ns(&started), parse_timestamp_ns(&deadline))
        else {
            continue;
        };
        if end > filters.start_ns && start < filters.end_ns {
            spans.push((start, end));
        }
    }
    Ok(spans)
}

const UNCONFIRMED_PROVENANCE: &str = "unconfirmed focus";

/// Parts of `span` not covered by the (sorted, possibly overlapping) `covered` spans.
fn uncovered(span: (i64, i64), covered: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut gaps = Vec::new();
    let mut cursor = span.0;
    for &(low, high) in covered {
        if high <= cursor {
            continue;
        }
        if low >= span.1 {
            break;
        }
        if low > cursor {
            gaps.push((cursor, low));
        }
        cursor = cursor.max(high);
    }
    if cursor < span.1 {
        gaps.push((cursor, span.1));
    }
    gaps
}

/// Focus sessions whose break was never confirmed do not count as Pomodoros,
/// but the time was still spent working: credit the part of it with no
/// recorded input as human-active time, attributed to the session's workspace.
fn unconfirmed_focus_entries(
    database: &Database,
    filters: &DashboardFilters,
    recorded: &[TimelineEntry],
) -> Result<Vec<TimelineEntry>> {
    let connection = database.connect_readonly()?;
    let mut phases = connection.prepare("SELECT s.id, s.workspace_id, p.started_at, MIN(COALESCE(p.ended_at, p.deadline), p.deadline) FROM timed_phases p JOIN focus_sessions s ON s.id=p.session_id WHERE p.phase='focus' AND p.termination_reason='unconfirmed'")?;
    let mut instance = connection.prepare("SELECT w.id, w.project_id, pr.name, w.path, w.machine_id, m.label FROM workspace_instances w JOIN projects pr ON pr.id=w.project_id JOIN machines m ON m.id=w.machine_id WHERE w.runtime_workspace_id=?1 AND ((?2 IS NULL AND m.kind='local') OR m.label=?2) ORDER BY (SELECT MAX(a.end_wall_ns) FROM activity_segments a WHERE a.workspace_instance_id=w.id) DESC LIMIT 1")?;
    let mut covered: Vec<(i64, i64)> = recorded
        .iter()
        .filter(|entry| entry.category == HUMAN_CATEGORY)
        .map(|entry| (entry.start_ns, entry.end_ns))
        .collect();
    covered.sort_unstable();
    let rows = phases.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let (session, workspace, started, ended) = row?;
        let (Ok(start), Ok(end)) = (parse_timestamp_ns(&started), parse_timestamp_ns(&ended))
        else {
            continue;
        };
        let span = (start.max(filters.start_ns), end.min(filters.end_ns));
        if span.1 <= span.0 {
            continue;
        }
        let (machine, runtime) = match workspace.split_once('/') {
            Some((machine, runtime)) => (Some(machine.to_owned()), runtime.to_owned()),
            None => (None, workspace.clone()),
        };
        let found = instance
            .query_row(params![runtime, machine], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .optional()?;
        let Some((instance_id, project_id, project_name, path, machine_id, machine_label)) = found
        else {
            continue;
        };
        for (low, high) in uncovered(span, &covered) {
            let entry = TimelineEntry {
                category: HUMAN_CATEGORY.to_owned(),
                start_ns: low,
                end_ns: high,
                duration_ns: high - low,
                project_id: Some(project_id.clone()),
                project_name: Some(project_name.clone()),
                workspace_id: Some(instance_id.clone()),
                workspace_path: Some(path.clone()),
                machine_id: Some(machine_id.clone()),
                machine_label: Some(machine_label.clone()),
                revision_id: None,
                revision_label: None,
                session_id: Some(session.clone()),
                provenance: UNCONFIRMED_PROVENANCE.to_owned(),
            };
            if matches_filters(&entry, filters) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

/// Per-day totals for the `DAYS_SHOWN` days ending at the selected day.
fn daily_totals(
    database: &Database,
    filters: &DashboardFilters,
) -> Result<Vec<(NaiveDate, BucketTotals)>> {
    let mut days = Vec::new();
    for offset in (0..DAYS_SHOWN as u64).rev() {
        if let Some(day) = filters.day.checked_sub_days(Days::new(offset)) {
            days.push(DashboardFilters::for_day(day)?);
        }
    }
    let Some((first, last)) = days.first().zip(days.last()) else {
        return Ok(Vec::new());
    };
    let mut wide = filters.clone();
    wide.start_ns = first.start_ns;
    wide.end_ns = last.end_ns;
    let mut entries = activity_entries(database, &wide)?;
    let credited = unconfirmed_focus_entries(database, &wide, &entries)?;
    entries.extend(credited);
    let buckets: Vec<(i64, i64)> = days.iter().map(|day| (day.start_ns, day.end_ns)).collect();
    Ok(days
        .iter()
        .map(|day| day.day)
        .zip(bucket_totals(&entries, &buckets))
        .collect())
}

fn agent_entries(database: &Database, filters: &DashboardFilters) -> Result<Vec<TimelineEntry>> {
    let connection = database.connect_readonly()?;
    let mut statement = connection.prepare("SELECT a.start_wall_ns, a.end_wall_ns, a.project_id, p.name, a.workspace_instance_id, w.path, a.machine_id, m.label, a.revision_id, r.kind, r.stable_id, r.description FROM agent_segments a LEFT JOIN projects p ON p.id=a.project_id LEFT JOIN workspace_instances w ON w.id=a.workspace_instance_id LEFT JOIN machines m ON m.id=a.machine_id LEFT JOIN revisions r ON r.id=a.revision_id WHERE a.end_wall_ns>=?1 AND a.start_wall_ns<=?2 AND (?3 IS NULL OR a.project_id=?3) AND (?4 IS NULL OR a.workspace_instance_id=?4) AND (?5 IS NULL OR a.machine_id=?5) AND (?6 IS NULL OR a.revision_id=?6)")?;
    let rows = statement.query_map(
        params![
            filters.start_ns,
            filters.end_ns,
            filters.project_id,
            filters.workspace_id,
            filters.machine_id,
            filters.revision_id
        ],
        |row| {
            let start = row.get::<_, i64>(0)?.max(filters.start_ns);
            let end = row.get::<_, i64>(1)?.min(filters.end_ns);
            let kind: Option<String> = row.get(9)?;
            let stable: Option<String> = row.get(10)?;
            let description: Option<String> = row.get(11)?;
            Ok(TimelineEntry {
                category: AGENT_CATEGORY.to_owned(),
                start_ns: start,
                end_ns: end,
                duration_ns: (end - start).max(0),
                project_id: row.get(2)?,
                project_name: row.get(3)?,
                workspace_id: row.get(4)?,
                workspace_path: row.get(5)?,
                machine_id: row.get(6)?,
                machine_label: row.get(7)?,
                revision_id: row.get(8)?,
                revision_label: kind
                    .zip(stable)
                    .map(|(kind, stable)| revision_label(&kind, &stable, description.as_deref())),
                session_id: None,
                provenance: "agent-status-poll-v1".to_owned(),
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn grouped_totals(
    entries: &[TimelineEntry],
    key: impl Fn(&TimelineEntry) -> Option<(String, String)>,
) -> Vec<Total> {
    let mut totals = std::collections::BTreeMap::<(String, String), (i64, i64)>::new();
    for entry in entries {
        let human = entry.category == HUMAN_CATEGORY;
        if !human && entry.category != AGENT_CATEGORY {
            continue;
        }
        if let Some(group) = key(entry) {
            let slot = totals.entry(group).or_default();
            if human {
                slot.0 += entry.duration_ns;
            } else {
                slot.1 += entry.duration_ns;
            }
        }
    }
    let mut values: Vec<Total> = totals
        .into_iter()
        .map(|((id, label), (duration_ns, passive_ns))| Total {
            id,
            label,
            duration_ns,
            passive_ns,
        })
        .collect();
    values.sort_by_key(|total| std::cmp::Reverse(total.duration_ns + total.passive_ns));
    values
}

fn activity_entries(database: &Database, filters: &DashboardFilters) -> Result<Vec<TimelineEntry>> {
    let connection = database.connect_readonly()?;
    let mut statement = connection.prepare("SELECT s.category, s.start_wall_ns, s.end_wall_ns, s.duration_ns, s.project_id, p.name, s.workspace_instance_id, w.path, s.machine_id, m.label, s.revision_id, r.kind, r.stable_id, s.session_id, s.provenance, r.description FROM activity_segments s LEFT JOIN projects p ON p.id=s.project_id LEFT JOIN workspace_instances w ON w.id=s.workspace_instance_id LEFT JOIN machines m ON m.id=s.machine_id LEFT JOIN revisions r ON r.id=s.revision_id WHERE s.end_wall_ns>=?1 AND s.start_wall_ns<=?2 AND (?3 IS NULL OR s.project_id=?3) AND (?4 IS NULL OR s.workspace_instance_id=?4) AND (?5 IS NULL OR s.machine_id=?5) AND (?6 IS NULL OR s.session_id=?6) AND (?7 IS NULL OR s.category=?7) AND (?8 IS NULL OR s.revision_id=?8)")?;
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
            let duration = (i128::from(raw_duration) * i128::from((end - start).max(0))
                / i128::from(original)) as i64;
            let kind: Option<String> = row.get(11)?;
            let stable: Option<String> = row.get(12)?;
            let description: Option<String> = row.get(15)?;
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
                    .map(|(kind, stable)| revision_label(&kind, &stable, description.as_deref())),
                session_id: row.get(13)?,
                provenance: row.get(14)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

const REVISION_LABEL_FALLBACK_LENGTH: usize = 8;

fn revision_label(kind: &str, stable: &str, description: Option<&str>) -> String {
    let first_line = description
        .and_then(|value| value.lines().map(str::trim).find(|line| !line.is_empty()))
        .filter(|_| kind != "transition");
    match first_line {
        Some(line) => line.to_owned(),
        None if kind == "transition" => "(revision changed mid-interval)".to_owned(),
        None => format!(
            "({kind} {} · no description)",
            stable
                .chars()
                .take(REVISION_LABEL_FALLBACK_LENGTH)
                .collect::<String>()
        ),
    }
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
    Hours,
    Days,
    Workspaces,
    Revisions,
    Sessions,
    Timeline,
}

impl View {
    const ALL: [Self; 7] = [
        Self::Overview,
        Self::Hours,
        Self::Days,
        Self::Workspaces,
        Self::Revisions,
        Self::Sessions,
        Self::Timeline,
    ];
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Hours => "Hours",
            Self::Days => "Days",
            Self::Workspaces => "Workspaces",
            Self::Revisions => "Revisions",
            Self::Sessions => "Sessions",
            Self::Timeline => "Timeline",
        }
    }
}

struct DashboardApp {
    database: Database,
    filters: DashboardFilters,
    data: DashboardData,
    /// Data of the day before the selected one, loaded while comparing.
    previous: Option<DashboardData>,
    compare: bool,
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
            previous: None,
            compare: false,
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
        self.previous = None;
        if self.compare {
            let loaded = self
                .filters
                .day
                .checked_sub_days(Days::new(1))
                .and_then(|day| self.filters.with_day(day).ok())
                .and_then(|filters| DashboardData::load(&self.database, &filters).ok());
            self.previous = loaded;
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
            View::Hours | View::Days => 0,
            View::Workspaces => self.data.workspaces.len(),
            View::Revisions => self.data.revisions.len(),
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
            (KeyCode::Char('y'), _) => {
                self.compare = !self.compare;
                self.refresh();
            }
            (KeyCode::Char('d'), _) => {
                self.filters.set_day(Local::now().date_naive())?;
                self.refresh();
            }
            (KeyCode::Char('p'), _) => {
                self.cycle_project();
                self.refresh();
            }
            (KeyCode::Char('r'), _) => self.refresh(),
            (KeyCode::Char('c'), _) | (KeyCode::Esc, _) => {
                self.filters.clear_dimensions();
                self.refresh();
            }
            (KeyCode::Char(value @ '1'..='4'), _) => {
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

    /// Steps the project filter through every known project, then back to all.
    fn cycle_project(&mut self) {
        let ids: Vec<&String> = self.data.all_projects.iter().map(|(id, _)| id).collect();
        self.filters.project_id = match self.filters.project_id.as_ref() {
            None => ids.first().map(|id| (*id).clone()),
            Some(current) => ids
                .iter()
                .position(|id| *id == current)
                .and_then(|index| ids.get(index + 1))
                .map(|id| (*id).clone()),
        };
        self.selected = 0;
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
    let footer = Paragraph::new(Line::from(vec![Span::styled(app.filters.description(), Style::default().fg(Color::Yellow)), Span::raw("   │   Tab/←→ view  j/k select  Enter filter  p project  [/] day  y prev. day  1–4 category  c clear  r refresh  q quit")])).block(panel(" Filters & keys ", Color::DarkGray)).wrap(Wrap { trim: true });
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
        View::Hours => render_hours(frame, area, app),
        View::Days => render_days(frame, area, app),
        View::Workspaces => render_totals_table(
            frame,
            area,
            " Workspaces · machine  folder · Enter toggles filter ",
            &app.data.workspaces,
            app.selected,
        ),
        View::Revisions => render_totals_table(
            frame,
            area,
            " Revisions · project  ·  description · p cycles project · Enter toggles filter ",
            &app.data.revisions,
            app.selected,
        ),
        View::Sessions => render_sessions(frame, area, app),
        View::Timeline => render_timeline(frame, area, app),
    }
}

const PROJECT_COLORS: [Color; 7] = [
    Color::Green,
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Blue,
    Color::Red,
    Color::LightGreen,
];
const OTHER_COLOR: Color = Color::DarkGray;
const FRAME_COLOR: Color = Color::Yellow;
const BAR_EIGHTHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const Y_AXIS_WIDTH: u16 = 5;
const MIN_COLUMN_WIDTH: u16 = 3;
const MAX_COLUMN_WIDTH: u16 = 6;

/// A completed Pomodoro inside one column, as fractions of the column's scale.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    low: f64,
    high: f64,
    open_below: bool,
    open_above: bool,
}

struct ChartSpec<'a> {
    title: &'a str,
    buckets: &'a [BucketTotals],
    labels: Vec<String>,
    capacity_ns: i64,
    axis_labels: [String; 3],
    frames: Vec<Vec<Frame>>,
}

fn hour_frames(data: &DashboardData, filters: &DashboardFilters) -> Vec<Vec<Frame>> {
    (0..HOURS_PER_DAY)
        .map(|hour| {
            let start = filters.start_ns + hour as i64 * NANOSECONDS_PER_HOUR;
            let end = start + NANOSECONDS_PER_HOUR;
            data.pomodoros
                .iter()
                .filter(|(from, to)| *to > start && *from < end)
                .map(|(from, to)| Frame {
                    low: (*from).max(start).saturating_sub(start) as f64
                        / NANOSECONDS_PER_HOUR as f64,
                    high: (*to).min(end).saturating_sub(start) as f64 / NANOSECONDS_PER_HOUR as f64,
                    open_below: *from < start,
                    open_above: *to > end,
                })
                .collect()
        })
        .collect()
}

fn render_hours(frame: &mut ratatui::Frame<'_>, area: Rect, app: &DashboardApp) {
    match (&app.previous, app.compare) {
        (Some(previous), true) => {
            let halves = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            let earlier = app
                .filters
                .day
                .checked_sub_days(Days::new(1))
                .and_then(|day| app.filters.with_day(day).ok());
            if let Some(earlier) = earlier {
                render_hours_chart(frame, halves[0], previous, &earlier);
            }
            render_hours_chart(frame, halves[1], &app.data, &app.filters);
        }
        _ => render_hours_chart(frame, area, &app.data, &app.filters),
    }
}

fn render_hours_chart(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    data: &DashboardData,
    filters: &DashboardFilters,
) {
    let title = format!(
        " Hours · {} · human time by project · frame = completed Pomodoro ",
        filters.day.format("%a %m-%d")
    );
    let spec = ChartSpec {
        title: &title,
        buckets: &data.hourly,
        labels: (0..HOURS_PER_DAY)
            .map(|hour| format!("{hour:02}"))
            .collect(),
        capacity_ns: NANOSECONDS_PER_HOUR,
        axis_labels: ["60m".to_owned(), "30m".to_owned(), "0m".to_owned()],
        frames: hour_frames(data, filters),
    };
    render_stacked_columns(frame, area, &spec);
}

fn render_days(frame: &mut ratatui::Frame<'_>, area: Rect, app: &DashboardApp) {
    let buckets: Vec<BucketTotals> = app
        .data
        .daily
        .iter()
        .map(|(_, bucket)| bucket.clone())
        .collect();
    let spec = ChartSpec {
        title: " Days · human time placed by time of day, by project ",
        labels: app
            .data
            .daily
            .iter()
            .map(|(day, _)| day.format("%m-%d").to_string())
            .collect(),
        capacity_ns: HOURS_PER_DAY as i64 * NANOSECONDS_PER_HOUR,
        axis_labels: ["24h".to_owned(), "12h".to_owned(), "0h".to_owned()],
        frames: vec![Vec::new(); buckets.len()],
        buckets: &buckets,
    };
    render_stacked_columns(frame, area, &spec);
}

fn bucket_total(totals: &[Total]) -> i64 {
    totals.iter().map(|total| total.duration_ns).sum()
}

/// Glyph and colour of one row: the human time that falls inside the row's
/// slice of the bucket, so a column floats where the work actually happened.
fn column_cell(
    segments: &[Segment],
    capacity_ns: i64,
    row: usize,
    rows: usize,
    color_of: &dyn Fn(&str) -> Color,
) -> (char, Color) {
    let row_start = capacity_ns as f64 * row as f64 / rows as f64;
    let row_end = capacity_ns as f64 * (row + 1) as f64 / rows as f64;
    let mut per_project: Vec<(&str, f64)> = Vec::new();
    let mut covered = 0.0;
    let mut touches_bottom = false;
    let mut touches_top = false;
    for segment in segments {
        let low = (segment.start_ns as f64).max(row_start);
        let high = (segment.end_ns as f64).min(row_end);
        if high <= low {
            continue;
        }
        covered += high - low;
        touches_bottom |= low <= row_start;
        touches_top |= high >= row_end;
        match per_project
            .iter_mut()
            .find(|(id, _)| *id == segment.project_id)
        {
            Some((_, amount)) => *amount += high - low,
            None => per_project.push((segment.project_id.as_str(), high - low)),
        }
    }
    let fraction = (covered / (row_end - row_start)).min(1.0);
    let eighths = (fraction * 8.0).round() as usize;
    if eighths == 0 {
        return (' ', Color::Reset);
    }
    let owner = per_project
        .iter()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map_or(OTHER_COLOR, |(id, _)| color_of(id));
    // A span that starts inside the row hangs from the top; one that ends
    // inside it (or sits in the middle) grows from the bottom.
    let glyph = if touches_top && !touches_bottom && eighths < 8 {
        if eighths >= 4 { '▀' } else { '▔' }
    } else {
        BAR_EIGHTHS[eighths - 1]
    };
    (glyph, owner)
}

fn frame_glyphs(frames: &[Frame], row: usize, rows: usize) -> (char, char, bool) {
    // Returns (left edge, right edge, horizontal cap row).
    for value in frames {
        let first = ((value.low * rows as f64).floor() as usize).min(rows - 1);
        let last = (((value.high * rows as f64).ceil() as usize).max(1) - 1).min(rows - 1);
        if row < first || row > last {
            continue;
        }
        let top = row == last && !value.open_above;
        let bottom = row == first && !value.open_below;
        return match (top, bottom) {
            (true, _) => ('╭', '╮', true),
            (_, true) => ('╰', '╯', true),
            _ => ('│', '│', false),
        };
    }
    (' ', ' ', false)
}

fn render_stacked_columns(frame: &mut ratatui::Frame<'_>, area: Rect, spec: &ChartSpec<'_>) {
    let block = panel(spec.title, Color::Green);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let count = spec.buckets.len().max(1) as u16;
    if inner.height < 8 || inner.width < Y_AXIS_WIDTH + count * MIN_COLUMN_WIDTH {
        render_empty(frame, inner);
        return;
    }
    let has_data = spec
        .buckets
        .iter()
        .any(|bucket| bucket_total(&bucket.human) > 0);
    if !has_data {
        render_empty(frame, inner);
        return;
    }
    // Project colours follow total human time across the chart.
    let mut project_totals: Vec<Total> = Vec::new();
    for bucket in spec.buckets {
        for total in &bucket.human {
            match project_totals.iter_mut().find(|value| value.id == total.id) {
                Some(value) => value.duration_ns += total.duration_ns,
                None => project_totals.push(total.clone()),
            }
        }
    }
    project_totals.sort_by_key(|total| std::cmp::Reverse(total.duration_ns));
    let color_of = |id: &str| -> Color {
        project_totals
            .iter()
            .position(|total| total.id == id)
            .and_then(|index| PROJECT_COLORS.get(index).copied())
            .unwrap_or(OTHER_COLOR)
    };

    let legend_rows =
        u16::try_from(project_totals.len().min(PROJECT_COLORS.len() + 1)).unwrap_or(1) + 1;
    let rows = usize::from(inner.height.saturating_sub(legend_rows + 2).max(3));
    let column_width =
        ((inner.width - Y_AXIS_WIDTH) / count).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
    let human_cells = usize::from(column_width) - 2;

    let mut lines: Vec<Line> = Vec::new();
    for row in (0..rows).rev() {
        let axis = if row == rows - 1 {
            spec.axis_labels[0].clone()
        } else if row == rows / 2 {
            spec.axis_labels[1].clone()
        } else if row == 0 {
            spec.axis_labels[2].clone()
        } else {
            String::new()
        };
        let mut spans = vec![Span::styled(
            format!("{axis:>width$} ", width = usize::from(Y_AXIS_WIDTH) - 1),
            Style::default().fg(Color::DarkGray),
        )];
        for (index, bucket) in spec.buckets.iter().enumerate() {
            let (left, right, cap) = frame_glyphs(&spec.frames[index], row, rows);
            let frame_style = Style::default()
                .fg(FRAME_COLOR)
                .add_modifier(Modifier::BOLD);
            spans.push(Span::styled(left.to_string(), frame_style));
            let (glyph, color) =
                column_cell(&bucket.segments, spec.capacity_ns, row, rows, &color_of);
            let human = if glyph == ' ' && cap {
                Span::styled("─".repeat(human_cells), frame_style)
            } else {
                Span::styled(
                    glyph.to_string().repeat(human_cells),
                    Style::default().fg(color),
                )
            };
            spans.push(human);
            spans.push(Span::styled(right.to_string(), frame_style));
        }
        lines.push(Line::from(spans));
    }
    let width = usize::from(column_width);
    let mut label_spans = vec![Span::raw(" ".repeat(usize::from(Y_AXIS_WIDTH)))];
    let mut human_spans = vec![Span::styled(
        format!("{:>5}", "human"),
        Style::default().fg(Color::DarkGray),
    )];
    for (index, bucket) in spec.buckets.iter().enumerate() {
        label_spans.push(Span::styled(
            format!("{:<width$}", spec.labels[index]),
            Style::default().fg(Color::White),
        ));
        human_spans.push(Span::styled(
            format!("{:<width$}", compact_minutes(bucket_total(&bucket.human))),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(label_spans));
    lines.push(Line::from(human_spans));
    lines.push(Line::from(Span::styled(
        "  bars sit at the time the work happened · human active time only",
        Style::default().fg(Color::DarkGray),
    )));
    for (index, total) in project_totals
        .iter()
        .take(PROJECT_COLORS.len() + 1)
        .enumerate()
    {
        let color = PROJECT_COLORS.get(index).copied().unwrap_or(OTHER_COLOR);
        lines.push(Line::from(vec![
            Span::styled("  ■ ", Style::default().fg(color)),
            Span::raw(format!("{}   {}", total.label, duration(total.duration_ns))),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Minutes (or h:mm beyond two hours) in at most `MAX_COLUMN_WIDTH - 1` characters.
fn compact_minutes(ns: i64) -> String {
    let minutes = (ns + 30 * NANOSECONDS_PER_SECOND) / (60 * NANOSECONDS_PER_SECOND);
    match minutes {
        0 => String::new(),
        1..=119 => minutes.to_string(),
        _ => format!("{}h{:02}", minutes / 60, minutes % 60),
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
            Cell::from(duration(value.passive_ns)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(64),
            Constraint::Length(14),
            Constraint::Length(16),
        ],
    )
    .header(
        Row::new(["Name", "Human active", "Agent (passive)"]).style(
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
    fn empty_database_has_four_explicit_categories() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let filters = DashboardFilters::for_day(Local::now().date_naive()).unwrap();
        let data = DashboardData::load(&database, &filters).unwrap();
        assert_eq!(data.categories.len(), 4);
        assert!(data.categories.iter().all(|value| value.duration_ns == 0));
    }

    #[test]
    fn hourly_totals_split_entries_across_hour_brackets() {
        let hour = NANOSECONDS_PER_HOUR;
        let entry = TimelineEntry {
            category: "human_active".to_owned(),
            start_ns: hour + hour / 2,
            end_ns: 2 * hour + hour / 2,
            duration_ns: hour,
            project_id: Some("p".to_owned()),
            project_name: Some("proj".to_owned()),
            workspace_id: None,
            workspace_path: None,
            machine_id: None,
            machine_label: None,
            revision_id: None,
            revision_label: None,
            session_id: None,
            provenance: String::new(),
        };
        let hours: Vec<(i64, i64)> = (0..HOURS_PER_DAY as i64)
            .map(|h| (h * hour, (h + 1) * hour))
            .collect();
        let buckets = bucket_totals(&[entry], &hours);
        assert_eq!(buckets[1].human[0].duration_ns, hour / 2);
        assert_eq!(buckets[2].human[0].duration_ns, hour / 2);
        assert!(buckets[0].human.is_empty() && buckets[3].human.is_empty());
    }

    #[test]
    fn hours_view_renders_stacked_columns() {
        let hour = NANOSECONDS_PER_HOUR;
        let mut app_data = DashboardData::default();
        let make = |id: &str, mins: i64| Total {
            id: id.to_owned(),
            label: id.to_owned(),
            duration_ns: mins * 60 * NANOSECONDS_PER_SECOND,
            passive_ns: 0,
        };
        app_data.hourly = vec![BucketTotals::default(); HOURS_PER_DAY];
        let minute = 60 * NANOSECONDS_PER_SECOND;
        let segment = |id: &str, from: i64, to: i64| Segment {
            project_id: id.to_owned(),
            start_ns: from * minute,
            end_ns: to * minute,
        };
        app_data.hourly[9].human = vec![make("alpha", 40), make("beta", 15)];
        app_data.hourly[9].segments = vec![segment("alpha", 5, 45), segment("beta", 45, 60)];
        app_data.hourly[10].human = vec![make("alpha", 20)];
        app_data.hourly[10].segments = vec![segment("alpha", 0, 20)];
        app_data.hourly[14].human = vec![make("beta", 5)];
        app_data.hourly[14].segments = vec![segment("beta", 30, 35)];
        app_data.timeline.push(TimelineEntry {
            category: "human_active".to_owned(),
            start_ns: 0,
            end_ns: hour,
            duration_ns: hour,
            project_id: None,
            project_name: None,
            workspace_id: None,
            workspace_path: None,
            machine_id: None,
            machine_label: None,
            revision_id: None,
            revision_label: None,
            session_id: None,
            provenance: String::new(),
        });
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("db.sqlite3")).unwrap();
        let filters =
            DashboardFilters::for_day(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
                .unwrap();
        let mut app = DashboardApp::new(database, filters);
        app.data = app_data;
        let day_start = app.filters.start_ns;
        app.data.pomodoros = vec![(
            day_start + 9 * NANOSECONDS_PER_HOUR + NANOSECONDS_PER_HOUR / 2,
            day_start + 10 * NANOSECONDS_PER_HOUR + 5 * 60 * NANOSECONDS_PER_SECOND,
        )];
        let backend = ratatui::backend::TestBackend::new(110, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_hours(frame, frame.area(), &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        for y in 0..24 {
            let line: String = (0..110)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect();
            println!("{line}");
        }
        let text: String = (0..24)
            .flat_map(|y| (0..110).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol().to_owned())
            .collect();
        assert!(text.contains("60m") && text.contains("23") && text.contains('█'));
        assert!(text.contains('╭') && text.contains('╰'));
    }

    #[test]
    fn uncovered_returns_the_parts_without_recorded_input() {
        let covered = [(10, 20), (15, 30), (50, 60)];
        assert_eq!(
            uncovered((0, 100), &covered),
            vec![(0, 10), (30, 50), (60, 100)]
        );
        assert_eq!(uncovered((12, 25), &covered), Vec::<(i64, i64)>::new());
        assert_eq!(uncovered((40, 45), &covered), vec![(40, 45)]);
    }

    #[test]
    fn columns_float_at_the_time_of_the_work() {
        let minute = 60 * NANOSECONDS_PER_SECOND;
        let segments = vec![Segment {
            project_id: "p".to_owned(),
            start_ns: 30 * minute,
            end_ns: 35 * minute,
        }];
        let color_of = |_: &str| Color::Green;
        let cell = |row| column_cell(&segments, NANOSECONDS_PER_HOUR, row, 60, &color_of).0;
        assert_eq!(cell(0), ' ');
        assert_eq!(cell(29), ' ');
        assert_eq!(cell(30), '█');
        assert_eq!(cell(34), '█');
        assert_eq!(cell(35), ' ');
        assert_eq!(cell(59), ' ');
        // A span starting mid-row hangs from the row's top edge.
        let late = vec![Segment {
            project_id: "p".to_owned(),
            start_ns: 30 * minute + 30 * NANOSECONDS_PER_SECOND,
            end_ns: 40 * minute,
        }];
        assert_eq!(
            column_cell(&late, NANOSECONDS_PER_HOUR, 30, 60, &color_of).0,
            '▀'
        );
    }

    #[test]
    fn bucket_totals_record_positioned_segments() {
        let hour = NANOSECONDS_PER_HOUR;
        let entry = TimelineEntry {
            category: "human_active".to_owned(),
            start_ns: hour + hour / 2,
            end_ns: 2 * hour + hour / 4,
            duration_ns: hour * 3 / 4,
            project_id: Some("p".to_owned()),
            project_name: Some("proj".to_owned()),
            workspace_id: None,
            workspace_path: None,
            machine_id: None,
            machine_label: None,
            revision_id: None,
            revision_label: None,
            session_id: None,
            provenance: String::new(),
        };
        let hours: Vec<(i64, i64)> = (0..4).map(|h| (h * hour, (h + 1) * hour)).collect();
        let buckets = bucket_totals(&[entry], &hours);
        assert_eq!(buckets[1].segments[0].start_ns, hour / 2);
        assert_eq!(buckets[1].segments[0].end_ns, hour);
        assert_eq!(buckets[2].segments[0].start_ns, 0);
        assert_eq!(buckets[2].segments[0].end_ns, hour / 4);
    }

    #[test]
    fn agent_time_is_the_union_of_overlapping_agents() {
        let hour = NANOSECONDS_PER_HOUR;
        let agent = |start: i64, end: i64| TimelineEntry {
            category: AGENT_CATEGORY.to_owned(),
            start_ns: start,
            end_ns: end,
            duration_ns: end - start,
            project_id: Some("p".to_owned()),
            project_name: Some("proj".to_owned()),
            workspace_id: None,
            workspace_path: None,
            machine_id: None,
            machine_label: None,
            revision_id: None,
            revision_label: None,
            session_id: None,
            provenance: String::new(),
        };
        let entries = [agent(0, hour / 2), agent(hour / 4, hour)];
        let buckets = bucket_totals(&entries, &[(0, hour)]);
        assert_eq!(buckets[0].agent[0].duration_ns, hour);
        assert!(buckets[0].human.is_empty());
    }

    #[test]
    fn frame_spans_from_the_middle_upward_and_opens_at_the_hour_edge() {
        let frames = [Frame {
            low: 0.5,
            high: 1.0,
            open_below: false,
            open_above: true,
        }];
        assert_eq!(frame_glyphs(&frames, 0, 10), (' ', ' ', false));
        assert_eq!(frame_glyphs(&frames, 5, 10), ('╰', '╯', true));
        assert_eq!(frame_glyphs(&frames, 9, 10), ('│', '│', false));
    }

    #[test]
    fn revision_label_prefers_description() {
        assert_eq!(
            revision_label("jj", "abcdef123456", Some("Fix bug\nbody")),
            "Fix bug"
        );
        assert_eq!(
            revision_label("jj", "abcdef123456", None),
            "(jj abcdef12 · no description)"
        );
        assert_eq!(
            revision_label("transition", "x", Some("boundary")),
            "(revision changed mid-interval)"
        );
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
        assert_eq!(data.workspaces[0].duration_ns, 1_000_000_000);
        assert_eq!(data.workspaces[0].label, "Mac  /project");
        assert_eq!(data.categories[0].duration_ns, 1_000_000_000);
    }
}
