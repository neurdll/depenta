use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::model::{Status, Task, TaskId};
use super::app::App;
use super::views::{priority_badge, state_badge};

/// Flattened item in the layered dependency tree view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub layer: usize,
    pub task_id: TaskId,
}

/// Builds a flattened list of `(layer, TaskId)` pairs ordered by longest-path `layers()`.
pub fn build_tree_rows(app: &App) -> Result<Vec<TreeRow>, String> {
    let layers = app.graph.layers().map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for (layer_idx, ids) in layers.into_iter().enumerate() {
        for task_id in ids {
            rows.push(TreeRow {
                layer: layer_idx,
                task_id,
            });
        }
    }
    Ok(rows)
}

/// Renders the layered dependency tree view + selected node summary strip.
pub fn render_tree_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);

    render_layers_list(frame, app, panes[0]);
    render_node_context(frame, app, panes[1]);
}

fn render_layers_list(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.graph.is_empty() {
        let p = Paragraph::new("No tasks yet. Press 'a' to add one.").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Layered Dependency Tree (Tab:Workbench) "),
        );
        frame.render_widget(p, area);
        return;
    }

    let rows = match build_tree_rows(app) {
        Ok(rows) => rows,
        Err(err) => {
            let p = Paragraph::new(format!("Graph error: {err}"))
                .block(Block::default().borders(Borders::ALL).title(" Layered Tree "));
            frame.render_widget(p, area);
            return;
        }
    };

    let selected_id = app.selected_tree_id().cloned();
    let mut items: Vec<ListItem> = Vec::with_capacity(rows.len());

    for (idx, row) in rows.iter().enumerate() {
        let Some(task) = app.graph.get(&row.task_id) else {
            continue;
        };
        let is_last_in_layer = rows
            .get(idx + 1)
            .is_none_or(|next| next.layer != row.layer);
        let branch = if row.layer == 0 {
            "● "
        } else if is_last_in_layer {
            "└─ "
        } else {
            "├─ "
        };
        let indent = "  ".repeat(row.layer);
        let (badge, badge_color) = state_badge(app, task);
        let (prio_label, prio_color) = priority_badge(task.priority);

        let relation_hint = selected_id.as_ref().and_then(|sel| {
            if sel == &task.id {
                None
            } else if app
                .graph
                .get(sel)
                .is_some_and(|s| s.depends_on.contains(&task.id))
            {
                Some((" [needs]", Color::Yellow))
            } else if task.depends_on.contains(sel) {
                Some((" [unlocks]", Color::Green))
            } else if task.related.contains(sel) {
                Some((" [rel]", Color::Magenta))
            } else {
                None
            }
        });

        let mut spans = vec![
            Span::styled(
                format!("L{} ", row.layer),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!("{indent}{branch}"), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{badge} "), Style::default().fg(badge_color)),
            Span::styled(format!("{prio_label:<4} "), Style::default().fg(prio_color)),
            Span::styled(format!("{} ", task.id), Style::default().fg(Color::Cyan)),
            Span::raw(task.title.clone()),
        ];

        if let Some((tag, color)) = relation_hint {
            spans.push(Span::styled(tag, Style::default().fg(color)));
        }
        items.push(ListItem::new(Line::from(spans)));
    }

    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(app.tree_index.min(items.len() - 1)));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Layered Dependency Tree (Tab:Workbench) ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_node_context(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Layer Node Details ")
        .border_style(Style::default().fg(Color::DarkGray));

    let Some(task) = app.selected_tree_id().and_then(|id| app.graph.get(id)) else {
        frame.render_widget(Paragraph::new("No tasks in graph.").block(block), area);
        return;
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                task.id.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(task.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(format!(
            "status: {} | priority: {} | impact: {}",
            task.status,
            task.priority,
            app.graph.downstream_impact(&task.id)
        )),
    ];

    if !task.description.is_empty() {
        lines.push(Line::raw(""));
        for l in task.description.lines() {
            lines.push(Line::raw(l.to_owned()));
        }
    }

    push_link_section(&mut lines, "Needs (depends_on)", &app.graph.prerequisites(&task.id));
    push_link_section(&mut lines, "Unlocks (blocks)", &app.graph.unlocks(&task.id));
    push_link_section(&mut lines, "Related", &app.graph.related_tasks(&task.id));

    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn push_link_section(lines: &mut Vec<Line<'static>>, heading: &str, tasks: &[&Task]) {
    if tasks.is_empty() {
        return;
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!("{heading}:"),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    for t in tasks {
        let marker = if t.status == Status::Done { "[x]" } else { "[ ]" };
        lines.push(Line::raw(format!("  {marker} {} {}", t.id, t.title)));
    }
}
