use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::model::{Priority, TaskId};
use super::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddField {
    Title,
    Priority,
    Description,
}

impl AddField {
    pub fn next(self) -> Self {
        match self {
            Self::Title => Self::Priority,
            Self::Priority => Self::Description,
            Self::Description => Self::Title,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Title => Self::Description,
            Self::Priority => Self::Title,
            Self::Description => Self::Priority,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AddModal {
    pub title: String,
    pub priority: Priority,
    pub description: String,
    pub active_field: AddField,
    pub error: Option<String>,
}

impl Default for AddModal {
    fn default() -> Self {
        Self {
            title: String::new(),
            priority: Priority::Medium,
            description: String::new(),
            active_field: AddField::Title,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRelation {
    DependsOn,
    Blocks,
    Related,
}

impl LinkRelation {
    pub fn next(self) -> Self {
        match self {
            Self::DependsOn => Self::Blocks,
            Self::Blocks => Self::Related,
            Self::Related => Self::DependsOn,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on (source needs target)",
            Self::Blocks => "blocks (target needs source)",
            Self::Related => "related (symmetric)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinkModal {
    pub adding: bool,
    pub source_id: TaskId,
    pub relation: LinkRelation,
    pub filter: String,
    pub selected: usize,
    pub error: Option<String>,
}

impl LinkModal {
    pub fn new(adding: bool, source_id: TaskId) -> Self {
        Self {
            adding,
            source_id,
            relation: LinkRelation::DependsOn,
            filter: String::new(),
            selected: 0,
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ModalState {
    Add(AddModal),
    Link(LinkModal),
}

pub fn cycle_priority(priority: Priority, forward: bool) -> Priority {
    match (priority, forward) {
        (Priority::Low, true) => Priority::Medium,
        (Priority::Medium, true) => Priority::High,
        (Priority::High, true) => Priority::Critical,
        (Priority::Critical, true) => Priority::Low,
        (Priority::Low, false) => Priority::Critical,
        (Priority::Medium, false) => Priority::Low,
        (Priority::High, false) => Priority::Medium,
        (Priority::Critical, false) => Priority::High,
    }
}

pub fn render_modal(frame: &mut Frame, app: &App, modal: &ModalState) {
    match modal {
        ModalState::Add(state) => render_add_modal(frame, state),
        ModalState::Link(state) => render_link_modal(frame, app, state),
    }
}

fn render_add_modal(frame: &mut Frame, modal: &AddModal) {
    let area = centered_rect(64, 52, frame.area());
    frame.render_widget(Clear, area);

    let title_style = field_style(modal.active_field == AddField::Title);
    let prio_style = field_style(modal.active_field == AddField::Priority);
    let desc_style = field_style(modal.active_field == AddField::Description);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Title:       ", title_style),
            Span::raw(if modal.active_field == AddField::Title {
                format!("{}█", modal.title)
            } else {
                modal.title.clone()
            }),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Priority:    ", prio_style),
            Span::styled(
                format!("< {} >", modal.priority),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("  (Left/Right)", Style::default().fg(Color::DarkGray)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Description: ", desc_style),
            Span::raw(if modal.active_field == AddField::Description {
                format!("{}█", modal.description)
            } else {
                modal.description.clone()
            }),
        ]),
    ];

    if let Some(err) = &modal.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ));
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Tab:next field  Enter:create  Esc:cancel",
        Style::default().fg(Color::DarkGray),
    ));

    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Quick Add Task ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(widget, area);
}

fn render_link_modal(frame: &mut Frame, app: &App, modal: &LinkModal) {
    let area = centered_rect(72, 68, frame.area());
    frame.render_widget(Clear, area);

    let action = if modal.adding { "Link" } else { "Unlink" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {action} Task ({}) ", modal.source_id))
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

    let header_lines = vec![
        Line::from(vec![
            Span::styled("Relation: ", Style::default().fg(Color::Yellow)),
            Span::raw(modal.relation.label()),
            Span::styled("  (Tab to switch)", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled("Filter:   ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("{}█", modal.filter)),
        ]),
    ];
    frame.render_widget(Paragraph::new(header_lines), chunks[0]);

    let candidates = app.link_candidates(modal);
    let items: Vec<ListItem> = candidates
        .iter()
        .filter_map(|id| app.graph.get(id))
        .map(|t| ListItem::new(format!("{}  [{}]  {}", t.id, t.status, t.title)))
        .collect();

    let mut list_state = ListState::default();
    if !items.is_empty() {
        list_state.select(Some(modal.selected.min(items.len() - 1)));
    }
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, chunks[1], &mut list_state);

    let footer = if let Some(err) = &modal.error {
        Line::styled(err.clone(), Style::default().fg(Color::Red))
    } else {
        Line::styled(
            "Up/Down:select  Tab:relation  Enter:apply  Esc:close",
            Style::default().fg(Color::DarkGray),
        )
    };
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), chunks[2]);
}

fn field_style(active: bool) -> Style {
    if active {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
