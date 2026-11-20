use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::graph::TaskGraph;
use crate::model::{Status, Task, TaskId, now_epoch_secs};
use crate::store::TaskStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    Ready,
    Blocked,
    Done,
}

impl FilterMode {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Ready,
            Self::Ready => Self::Blocked,
            Self::Blocked => Self::Done,
            Self::Done => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Ready => "Ready",
            Self::Blocked => "Blocked",
            Self::Done => "Done",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    TaskList,
    Neighborhood,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborKind {
    Needs,
    Unlocks,
    Related,
}

impl NeighborKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Needs => "Needs",
            Self::Unlocks => "Unlocks",
            Self::Related => "Related",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborEntry {
    pub kind: NeighborKind,
    pub id: TaskId,
}

/// Interactive state for the 2-pane TUI workbench.
pub struct App {
    pub store: TaskStore,
    pub graph: TaskGraph,
    pub filter: FilterMode,
    pub focus: PaneFocus,
    pub searching: bool,
    pub search_query: String,
    pub visible_ids: Vec<TaskId>,
    pub list_state: ListState,
    pub neighbor_index: usize,
    pub status_message: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(store: TaskStore, graph: TaskGraph) -> Self {
        let mut app = Self {
            store,
            graph,
            filter: FilterMode::All,
            focus: PaneFocus::TaskList,
            searching: false,
            search_query: String::new(),
            visible_ids: Vec::new(),
            list_state: ListState::default(),
            neighbor_index: 0,
            status_message: None,
            should_quit: false,
        };
        app.refresh_visible(None);
        app
    }

    /// Recomputes `visible_ids` from the current filter & search query, preserving selection.
    pub fn refresh_visible(&mut self, preferred_id: Option<&TaskId>) {
        let keep_id = preferred_id.cloned().or_else(|| self.selected_id().cloned());
        let query = self.search_query.trim().to_ascii_lowercase();

        let mut tasks: Vec<&Task> = self
            .graph
            .all_tasks()
            .filter(|t| self.matches_filter(t))
            .filter(|t| {
                query.is_empty()
                    || t.title.to_ascii_lowercase().contains(&query)
                    || t.id.as_str().contains(&query)
                    || t.description.to_ascii_lowercase().contains(&query)
            })
            .collect();

        tasks.sort_by(|a, b| {
            self.sort_rank(a)
                .cmp(&self.sort_rank(b))
                .then_with(|| b.priority.cmp(&a.priority))
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });

        self.visible_ids = tasks.into_iter().map(|t| t.id.clone()).collect();

        if self.visible_ids.is_empty() {
            self.list_state.select(None);
            self.neighbor_index = 0;
            return;
        }

        let new_idx = keep_id
            .and_then(|id| self.visible_ids.iter().position(|v| v == &id))
            .unwrap_or_else(|| {
                self.list_state
                    .selected()
                    .unwrap_or(0)
                    .min(self.visible_ids.len() - 1)
            });
        self.list_state.select(Some(new_idx));
        self.clamp_neighbor_index();
    }

    fn matches_filter(&self, task: &Task) -> bool {
        match self.filter {
            FilterMode::All => true,
            FilterMode::Ready => self.graph.is_ready(&task.id),
            FilterMode::Blocked => self.graph.is_blocked(&task.id),
            FilterMode::Done => task.status == Status::Done,
        }
    }

    fn sort_rank(&self, task: &Task) -> u8 {
        if task.status == Status::Done {
            3
        } else if self.graph.is_blocked(&task.id) {
            2
        } else if task.status == Status::InProgress {
            0
        } else {
            1
        }
    }

    pub fn selected_id(&self) -> Option<&TaskId> {
        self.list_state
            .selected()
            .and_then(|idx| self.visible_ids.get(idx))
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.selected_id().and_then(|id| self.graph.get(id))
    }

    /// Collects ordered neighborhood links (`Needs`, `Unlocks`, `Related`) for the selected task.
    pub fn selected_neighbors(&self) -> Vec<NeighborEntry> {
        let Some(id) = self.selected_id() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for t in self.graph.prerequisites(id) {
            out.push(NeighborEntry {
                kind: NeighborKind::Needs,
                id: t.id.clone(),
            });
        }
        for t in self.graph.unlocks(id) {
            out.push(NeighborEntry {
                kind: NeighborKind::Unlocks,
                id: t.id.clone(),
            });
        }
        for t in self.graph.related_tasks(id) {
            out.push(NeighborEntry {
                kind: NeighborKind::Related,
                id: t.id.clone(),
            });
        }
        out
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.searching {
            self.handle_search_key(key.code);
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('/') => {
                self.searching = true;
                self.status_message = None;
            }
            KeyCode::Esc => {
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                    self.refresh_visible(None);
                }
                self.focus = PaneFocus::TaskList;
                self.status_message = None;
            }
            KeyCode::Char('f') => {
                self.filter = self.filter.next();
                self.refresh_visible(None);
                self.status_message = Some(format!("Filter: {}", self.filter.label()));
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.focus = PaneFocus::TaskList;
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if !self.selected_neighbors().is_empty() {
                    self.focus = PaneFocus::Neighborhood;
                    self.clamp_neighbor_index();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Enter => {
                if self.focus == PaneFocus::Neighborhood {
                    self.jump_to_selected_neighbor();
                }
            }
            KeyCode::Char(' ') => self.toggle_selected_done(),
            _ => {}
        }
    }

    fn handle_search_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => {
                self.searching = false;
            }
            KeyCode::Esc => {
                self.searching = false;
                self.search_query.clear();
                self.refresh_visible(None);
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.refresh_visible(None);
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.refresh_visible(None);
            }
            _ => {}
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        match self.focus {
            PaneFocus::TaskList => {
                if self.visible_ids.is_empty() {
                    return;
                }
                let len = self.visible_ids.len() as isize;
                let curr = self.list_state.selected().unwrap_or(0) as isize;
                let next = (curr + delta).clamp(0, len - 1) as usize;
                self.list_state.select(Some(next));
                self.neighbor_index = 0;
            }
            PaneFocus::Neighborhood => {
                let neighbors = self.selected_neighbors();
                if neighbors.is_empty() {
                    self.focus = PaneFocus::TaskList;
                    return;
                }
                let len = neighbors.len() as isize;
                let curr = self.neighbor_index as isize;
                self.neighbor_index = (curr + delta).clamp(0, len - 1) as usize;
            }
        }
    }

    fn clamp_neighbor_index(&mut self) {
        let len = self.selected_neighbors().len();
        if len == 0 {
            self.neighbor_index = 0;
            if self.focus == PaneFocus::Neighborhood {
                self.focus = PaneFocus::TaskList;
            }
        } else if self.neighbor_index >= len {
            self.neighbor_index = len - 1;
        }
    }

    fn jump_to_selected_neighbor(&mut self) {
        let neighbors = self.selected_neighbors();
        let Some(entry) = neighbors.get(self.neighbor_index) else {
            return;
        };
        let target_id = entry.id.clone();
        if !self.visible_ids.contains(&target_id) {
            self.filter = FilterMode::All;
            self.search_query.clear();
        }
        self.focus = PaneFocus::TaskList;
        self.neighbor_index = 0;
        self.refresh_visible(Some(&target_id));
        self.status_message = Some(format!("Jumped to {target_id}"));
    }

    fn toggle_selected_done(&mut self) {
        let Some(id) = self.selected_id().cloned() else {
            return;
        };
        let Some(current_status) = self.graph.get(&id).map(|t| t.status) else {
            return;
        };

        if current_status == Status::Done {
            if let Some(task) = self.graph.get_mut(&id) {
                task.status = Status::Todo;
                task.updated_at = now_epoch_secs();
                if let Err(err) = self.store.save_task(task) {
                    self.status_message = Some(format!("Save error: {err}"));
                    return;
                }
            }
            self.refresh_visible(Some(&id));
            self.status_message = Some(format!("Reopened {id}"));
        } else {
            match self.graph.mark_done(&id) {
                Ok(unblocked) => {
                    if let Some(task) = self.graph.get(&id)
                        && let Err(err) = self.store.save_task(task)
                    {
                        self.status_message = Some(format!("Save error: {err}"));
                        return;
                    }
                    self.refresh_visible(Some(&id));
                    if unblocked.is_empty() {
                        self.status_message = Some(format!("Completed {id}"));
                    } else {
                        let list: Vec<&str> = unblocked.iter().map(TaskId::as_str).collect();
                        self.status_message =
                            Some(format!("Completed {id}; unblocked: {}", list.join(", ")));
                    }
                }
                Err(err) => self.status_message = Some(err.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::model::{Priority, generate_id};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "depenta-tui-{name}-{}-{}",
                std::process::id(),
                generate_id(name)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn filter_and_search_update_visible_tasks() {
        let tmp = TempDir::new("filter");
        let store = TaskStore::init_at(&tmp.0).unwrap();

        let t1 = Task::new("Root setup", Priority::High);
        let mut t2 = Task::new("Blocked feature", Priority::Medium);
        t2.depends_on.insert(t1.id.clone());
        let id1 = t1.id.clone();
        let id2 = t2.id.clone();

        let graph = TaskGraph::from_tasks([t1, t2]);
        let mut app = App::new(store, graph);

        assert_eq!(app.visible_ids.len(), 2);

        app.handle_key(key(KeyCode::Char('f'))); // Ready
        assert_eq!(app.visible_ids, vec![id1.clone()]);

        app.handle_key(key(KeyCode::Char('f'))); // Blocked
        assert_eq!(app.visible_ids, vec![id2.clone()]);

        app.handle_key(key(KeyCode::Char('f'))); // Done
        assert!(app.visible_ids.is_empty());

        app.handle_key(key(KeyCode::Char('f'))); // All
        app.handle_key(key(KeyCode::Char('/')));
        for ch in "blocked".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(app.visible_ids, vec![id2]);
    }

    #[test]
    fn toggle_done_persists_and_neighbor_jump_selects_target() {
        let tmp = TempDir::new("jump");
        let store = TaskStore::init_at(&tmp.0).unwrap();

        let t1 = Task::new("Prereq", Priority::High);
        let mut t2 = Task::new("Dependent", Priority::Medium);
        t2.depends_on.insert(t1.id.clone());
        let id1 = t1.id.clone();
        let id2 = t2.id.clone();
        store.save_task(&t1).unwrap();
        store.save_task(&t2).unwrap();

        let graph = TaskGraph::from_tasks([t1, t2]);
        let mut app = App::new(store, graph);
        app.refresh_visible(Some(&id1));

        // Focus neighborhood and jump to `id2` (Unlocks).
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.focus, PaneFocus::Neighborhood);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.focus, PaneFocus::TaskList);
        assert_eq!(app.selected_id(), Some(&id2));

        // Jump back to `id1`, mark Done via Space, and verify `id2` is now ready.
        app.refresh_visible(Some(&id1));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.graph.get(&id1).unwrap().status, Status::Done);
        assert!(app.graph.is_ready(&id2));
    }
}
