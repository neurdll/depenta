use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::model::{Priority, Status, Task};
use super::app::{App, FilterMode, NeighborKind, PaneFocus};

/// Renders the 2-pane TUI workbench.
pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, app, chunks[0]);
    render_workbench(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.graph.len();
    let ready = app.graph.ready_tasks().len();
    let blocked = app
        .graph
        .all_tasks()
        .filter(|t| app.graph.is_blocked(&t.id))
        .count();
    let done = app
        .graph
        .all_tasks()
        .filter(|t| t.status == Status::Done)
        .count();

    let mut spans = vec![
        Span::styled(
            " depenta ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];

    for mode in [
        FilterMode::All,
        FilterMode::Ready,
        FilterMode::Blocked,
        FilterMode::Done,
    ] {
        let style = if app.filter == mode {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("[{}] ", mode.label()), style));
    }

    spans.push(Span::styled(
        format!(" total:{total} ready:{ready} blocked:{blocked} done:{done}"),
        Style::default().fg(Color::Gray),
    ));

    if !app.search_query.is_empty() {
        spans.push(Span::styled(
            format!("  /{}", app.search_query),
            Style::default().fg(Color::Magenta),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_workbench(frame: &mut Frame, app: &mut App, area: Rect) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(area);

    render_task_list(frame, app, panes[0]);
    render_detail_pane(frame, app, panes[1]);
}

fn render_task_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let border_style = if app.focus == PaneFocus::TaskList {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = app
        .visible_ids
        .iter()
        .filter_map(|id| app.graph.get(id))
        .map(|task| format_task_row(app, task))
        .collect();

    let title = format!(" Tasks ({}) ", items.len());
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn format_task_row<'a>(app: &App, task: &Task) -> ListItem<'a> {
    let (badge, badge_color) = state_badge(app, task);
    let (prio_label, prio_color) = priority_badge(task.priority);

    let line = Line::from(vec![
        Span::styled(format!("{badge} "), Style::default().fg(badge_color)),
        Span::styled(format!("{prio_label:<4} "), Style::default().fg(prio_color)),
        Span::styled(
            format!("{} ", task.id),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(task.title.clone()),
    ]);
    ListItem::new(line)
}

fn render_detail_pane(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == PaneFocus::Neighborhood {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Task & Neighborhood ")
        .border_style(border_style);

    let Some(task) = app.selected_task() else {
        let empty = Paragraph::new("No task selected.").block(block);
        frame.render_widget(empty, area);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let (badge, badge_color) = state_badge(app, task);
    let (prio_label, prio_color) = priority_badge(task.priority);

    lines.push(Line::from(vec![
        Span::styled(
            task.id.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(task.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{badge} "), Style::default().fg(badge_color)),
        Span::raw(format!("status:{}  ", task.status)),
        Span::styled(
            format!("priority:{prio_label}"),
            Style::default().fg(prio_color),
        ),
    ]));

    if !task.description.is_empty() {
        lines.push(Line::raw(""));
        for desc_line in task.description.lines() {
            lines.push(Line::raw(desc_line.to_owned()));
        }
    }

    lines.push(Line::raw(""));
    let neighbors = app.selected_neighbors();
    if neighbors.is_empty() {
        lines.push(Line::styled(
            "No linked tasks.",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        let mut current_kind: Option<NeighborKind> = None;
        for (idx, entry) in neighbors.iter().enumerate() {
            if current_kind != Some(entry.kind) {
                current_kind = Some(entry.kind);
                lines.push(Line::styled(
                    format!("{}:", entry.kind.label()),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(target) = app.graph.get(&entry.id) {
                let is_selected =
                    app.focus == PaneFocus::Neighborhood && app.neighbor_index == idx;
                let (n_badge, n_color) = state_badge(app, target);
                let prefix = if is_selected { " ▶ " } else { "   " };
                let row_style = if is_selected {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix, row_style),
                    Span::styled(format!("{n_badge} "), row_style.fg(n_color)),
                    Span::styled(format!("{} ", target.id), row_style.fg(Color::Cyan)),
                    Span::styled(target.title.clone(), row_style),
                ]));
            }
        }
    }

    let detail = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let text = if app.searching {
        Line::from(vec![
            Span::styled(" Search: ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("{}█  (Enter:apply  Esc:clear)", app.search_query)),
        ])
    } else if let Some(msg) = &app.status_message {
        Line::from(vec![
            Span::styled(format!(" {msg} "), Style::default().fg(Color::Green)),
            Span::styled(
                " | j/k:move  h/l:pane  Enter:jump  Space:done  f:filter  /:search  q:quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::styled(
            " j/k:move  h/l:pane  Enter:jump  Space:done  f:filter  /:search  q:quit",
            Style::default().fg(Color::DarkGray),
        )
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn state_badge(app: &App, task: &Task) -> (&'static str, Color) {
    if task.status == Status::Done {
        ("[x]", Color::Green)
    } else if app.graph.is_blocked(&task.id) {
        ("[B]", Color::Red)
    } else if task.status == Status::InProgress {
        ("[~]", Color::Yellow)
    } else {
        ("[R]", Color::Cyan)
    }
}

fn priority_badge(priority: Priority) -> (&'static str, Color) {
    match priority {
        Priority::Critical => ("CRIT", Color::Red),
        Priority::High => ("HIGH", Color::LightRed),
        Priority::Medium => ("MED", Color::Yellow),
        Priority::Low => ("LOW", Color::DarkGray),
    }
}
