use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};

use crate::pomodoro::Phase;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    Start,
    Dashboard,
    WorkspaceGrace,
    AwayGrace,
    ReturnToFocus,
    StopSession,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MenuEntry {
    action: MenuAction,
    title: &'static str,
    detail: &'static str,
    enabled: bool,
}

fn entries(phase: Phase) -> Vec<MenuEntry> {
    let mut items = Vec::with_capacity(5);
    match phase {
        Phase::Idle => items.push(MenuEntry {
            action: MenuAction::Start,
            title: "Start focus session",
            detail: "25 minutes",
            enabled: true,
        }),
        Phase::Focus => {
            items.push(MenuEntry {
                action: MenuAction::StopSession,
                title: "End focus session",
                detail: "return to idle",
                enabled: true,
            });
            items.push(MenuEntry {
                action: MenuAction::WorkspaceGrace,
                title: "Check another project",
                detail: "3-minute workspace break",
                enabled: true,
            });
            items.push(MenuEntry {
                action: MenuAction::AwayGrace,
                title: "Leave the terminal",
                detail: "3-minute away break",
                enabled: true,
            });
        }
        Phase::AwayGrace | Phase::WorkspaceGrace => {
            items.push(MenuEntry {
                action: MenuAction::ReturnToFocus,
                title: "Return to focus",
                detail: "resume original deadline",
                enabled: true,
            });
            items.push(MenuEntry {
                action: MenuAction::StopSession,
                title: "End focus session",
                detail: "return to idle",
                enabled: true,
            });
        }
        Phase::Break => items.push(MenuEntry {
            action: MenuAction::StopSession,
            title: "End break",
            detail: "return to idle",
            enabled: true,
        }),
    }
    items.push(MenuEntry {
        action: MenuAction::Dashboard,
        title: "Open activity dashboard",
        detail: "daily work and timeline",
        enabled: true,
    });
    items
}

pub fn run_menu(phase: Phase) -> Result<Option<MenuAction>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_event_loop(&mut terminal, phase);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    phase: Phase,
) -> Result<Option<MenuAction>> {
    let items = entries(phase);
    let mut selected = 0;
    loop {
        terminal.draw(|frame| render(frame, &items, selected, phase))?;
        if let Event::Key(key) = event::read()? {
            match handle_key(key, &items, &mut selected) {
                MenuEvent::Continue => {}
                MenuEvent::Close => return Ok(None),
                MenuEvent::Select(action) => return Ok(Some(action)),
            }
        }
    }
}

enum MenuEvent {
    Continue,
    Close,
    Select(MenuAction),
}

fn handle_key(key: KeyEvent, items: &[MenuEntry], selected: &mut usize) -> MenuEvent {
    match (key.code, key.modifiers) {
        (KeyCode::Esc | KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            MenuEvent::Close
        }
        (KeyCode::Down | KeyCode::Char('j'), _) => {
            *selected = (*selected + 1) % items.len();
            MenuEvent::Continue
        }
        (KeyCode::Up | KeyCode::Char('k'), _) => {
            *selected = (*selected + items.len() - 1) % items.len();
            MenuEvent::Continue
        }
        (KeyCode::Char(value @ '1'..='5'), _) => {
            let index = value.to_digit(10).unwrap_or(1) as usize - 1;
            items
                .get(index)
                .copied()
                .map_or(MenuEvent::Continue, select)
        }
        (KeyCode::Enter, _) => select(items[*selected]),
        _ => MenuEvent::Continue,
    }
}

fn select(entry: MenuEntry) -> MenuEvent {
    if entry.enabled {
        MenuEvent::Select(entry.action)
    } else {
        MenuEvent::Continue
    }
}

fn render(frame: &mut ratatui::Frame<'_>, items: &[MenuEntry], selected: usize, phase: Phase) {
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(frame.area());
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " POMODORO ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", phase.as_str().replace('_', " ")),
            Style::default().fg(Color::Gray),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    frame.render_widget(title, layout[0]);

    let rows = items.iter().enumerate().map(|(index, entry)| {
        let marker = if entry.enabled { "" } else { "  unavailable" };
        let style = if entry.enabled {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{}  {:<28}", index + 1, entry.title), style),
            Span::styled(entry.detail, style.add_modifier(Modifier::DIM)),
            Span::styled(marker, Style::default().fg(Color::Red)),
        ]))
    });
    let list = List::new(rows)
        .block(
            Block::default()
                .title(" Choose ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" › ");
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, layout[1], &mut state);

    let help = Paragraph::new("↑/↓ or j/k select  ·  Enter run  ·  number direct  ·  Esc close")
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
    frame.render_widget(help, layout[2]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_menu_starts_focus_and_opens_dashboard() {
        let idle = entries(Phase::Idle);
        assert_eq!(idle.len(), 2);
        assert_eq!(idle[0].action, MenuAction::Start);
        assert_eq!(idle[1].action, MenuAction::Dashboard);
    }

    #[test]
    fn focus_menu_can_stop_or_start_either_grace() {
        let focus = entries(Phase::Focus);
        assert_eq!(focus.len(), 4);
        assert_eq!(focus[0].action, MenuAction::StopSession);
        assert_eq!(focus[1].action, MenuAction::WorkspaceGrace);
        assert_eq!(focus[2].action, MenuAction::AwayGrace);
    }

    #[test]
    fn grace_menu_can_return_or_end_session() {
        for phase in [Phase::AwayGrace, Phase::WorkspaceGrace] {
            let grace = entries(phase);
            assert_eq!(grace[0].action, MenuAction::ReturnToFocus);
            assert_eq!(grace[1].action, MenuAction::StopSession);
        }
    }

    #[test]
    fn break_menu_can_end_break() {
        assert_eq!(entries(Phase::Break)[0].action, MenuAction::StopSession);
    }
}
