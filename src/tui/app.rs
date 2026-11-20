use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::graph::TaskGraph;
use crate::model::{Status, Task, TaskId, now_epoch_secs};
use crate::store::TaskStore;
use super::modals::{
    AddField, AddModal, LinkModal, LinkRelation, ModalState, cycle_priority,
};
use super::tree::build_tree_rows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Workbench,
    Tree,
}

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

/// Interactive state for the TUI workbench, layered tree view, and modals.
pub struct App {
    pub store: TaskStore,
    pub graph: TaskGraph,
    pub view_mode: ViewMode,
    pub filter: FilterMode,
    pub focus: PaneFocus,
    pub searching: bool,
    pub search_query: String,
    pub visible_ids: Vec<TaskId>,
    pub list_state: ListState,
    pub neighbor_index: usize,
    pub tree_index: usize,
    pub modal: Option<ModalState>,
    pub pending_editor_task_id: Option<TaskId>,
    pub status_message: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(store: TaskStore, graph: TaskGraph) -> Self {
        let mut app = Self {
            store,
            graph,
            view_mode: ViewMode::Workbench,
            filter: FilterMode::All,
            focus: PaneFocus::TaskList,
            searching: false,
            search_query: String::new(),
            visible_ids: Vec::new(),
            list_state: ListState::default(),
            neighbor_index: 0,
            tree_index: 0,
            modal: None,
            pending_editor_task_id: None,
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
        } else {
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

        self.clamp_tree_index();
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

    pub fn selected_tree_id(&self) -> Option<&TaskId> {
        let rows = build_tree_rows(self).ok()?;
        let row = rows.get(self.tree_index)?;
        self.graph.get(&row.task_id).map(|t| &t.id)
    }

    pub fn active_selected_id(&self) -> Option<TaskId> {
        match self.view_mode {
            ViewMode::Workbench => self.selected_id().cloned(),
            ViewMode::Tree => self.selected_tree_id().cloned(),
        }
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

    pub fn link_candidates(&self, modal: &LinkModal) -> Vec<TaskId> {
        let needle = modal.filter.trim().to_ascii_lowercase();
        let source = self.graph.get(&modal.source_id);

        let mut ids: Vec<TaskId> = self
            .graph
            .all_tasks()
            .filter(|t| t.id != modal.source_id)
            .filter(|t| {
                if modal.adding {
                    true
                } else {
                    match modal.relation {
                        LinkRelation::DependsOn => {
                            source.is_some_and(|s| s.depends_on.contains(&t.id))
                        }
                        LinkRelation::Blocks => t.depends_on.contains(&modal.source_id),
                        LinkRelation::Related => {
                            source.is_some_and(|s| s.related.contains(&t.id))
                        }
                    }
                }
            })
            .filter(|t| {
                needle.is_empty()
                    || t.title.to_ascii_lowercase().contains(&needle)
                    || t.id.as_str().contains(&needle)
            })
            .map(|t| t.id.clone())
            .collect();

        ids.sort();
        ids
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }

        if self.searching {
            self.handle_search_key(key.code);
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Tab => self.toggle_view_mode(),
            KeyCode::Char('a') => {
                self.modal = Some(ModalState::Add(AddModal::default()));
                self.status_message = None;
            }
            KeyCode::Char('l') => self.open_link_modal(true),
            KeyCode::Char('u') => self.open_link_modal(false),
            KeyCode::Char('e') => {
                if let Some(id) = self.active_selected_id() {
                    self.pending_editor_task_id = Some(id);
                }
            }
            KeyCode::Char('/') if self.view_mode == ViewMode::Workbench => {
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
            KeyCode::Char('f') if self.view_mode == ViewMode::Workbench => {
                self.filter = self.filter.next();
                self.refresh_visible(None);
                self.status_message = Some(format!("Filter: {}", self.filter.label()));
            }
            KeyCode::Char('h') | KeyCode::Left if self.view_mode == ViewMode::Workbench => {
                self.focus = PaneFocus::TaskList;
            }
            KeyCode::Right if self.view_mode == ViewMode::Workbench => {
                if !self.selected_neighbors().is_empty() {
                    self.focus = PaneFocus::Neighborhood;
                    self.clamp_neighbor_index();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Enter => {
                if self.view_mode == ViewMode::Workbench && self.focus == PaneFocus::Neighborhood {
                    self.jump_to_selected_neighbor();
                }
            }
            KeyCode::Char(' ') => self.toggle_selected_done(),
            _ => {}
        }
    }

    fn toggle_view_mode(&mut self) {
        match self.view_mode {
            ViewMode::Workbench => {
                if let Some(id) = self.selected_id().cloned()
                    && let Ok(rows) = build_tree_rows(self)
                    && let Some(pos) = rows.iter().position(|r| r.task_id == id)
                {
                    self.tree_index = pos;
                }
                self.view_mode = ViewMode::Tree;
                self.clamp_tree_index();
            }
            ViewMode::Tree => {
                if let Some(id) = self.selected_tree_id().cloned() {
                    if !self.visible_ids.contains(&id) {
                        self.filter = FilterMode::All;
                        self.search_query.clear();
                    }
                    self.refresh_visible(Some(&id));
                }
                self.view_mode = ViewMode::Workbench;
            }
        }
    }

    fn open_link_modal(&mut self, adding: bool) {
        if let Some(id) = self.active_selected_id() {
            self.modal = Some(ModalState::Link(LinkModal::new(adding, id)));
            self.status_message = None;
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        let keep_open = match &mut modal {
            ModalState::Add(add_modal) => self.handle_add_modal_key(add_modal, key),
            ModalState::Link(link_modal) => self.handle_link_modal_key(link_modal, key),
        };
        if keep_open {
            self.modal = Some(modal);
        }
    }

    fn handle_add_modal_key(&mut self, modal: &mut AddModal, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => return false,
            KeyCode::Tab => modal.active_field = modal.active_field.next(),
            KeyCode::BackTab => modal.active_field = modal.active_field.prev(),
            KeyCode::Enter => {
                let title = modal.title.trim().to_owned();
                if title.is_empty() {
                    modal.error = Some("title cannot be empty".to_owned());
                    return true;
                }
                let mut task = Task::new(title, modal.priority);
                task.description = modal.description.trim().to_owned();
                let new_id = task.id.clone();
                if let Err(err) = self.store.save_task(&task) {
                    modal.error = Some(err.to_string());
                    return true;
                }
                self.graph.insert_task(task);
                self.filter = FilterMode::All;
                self.search_query.clear();
                self.refresh_visible(Some(&new_id));
                self.status_message = Some(format!("Added {new_id}"));
                return false;
            }
            KeyCode::Left if modal.active_field == AddField::Priority => {
                modal.priority = cycle_priority(modal.priority, false);
            }
            KeyCode::Right if modal.active_field == AddField::Priority => {
                modal.priority = cycle_priority(modal.priority, true);
            }
            KeyCode::Backspace => match modal.active_field {
                AddField::Title => {
                    modal.title.pop();
                }
                AddField::Description => {
                    modal.description.pop();
                }
                AddField::Priority => {}
            },
            KeyCode::Char(c) => {
                modal.error = None;
                match modal.active_field {
                    AddField::Title => modal.title.push(c),
                    AddField::Description => modal.description.push(c),
                    AddField::Priority => {
                        if c == ' ' || c == 'l' {
                            modal.priority = cycle_priority(modal.priority, true);
                        } else if c == 'h' {
                            modal.priority = cycle_priority(modal.priority, false);
                        }
                    }
                }
            }
            _ => {}
        }
        true
    }

    fn handle_link_modal_key(&mut self, modal: &mut LinkModal, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => return false,
            KeyCode::Tab => {
                modal.relation = modal.relation.next();
                modal.selected = 0;
                modal.error = None;
            }
            KeyCode::Up => {
                modal.selected = modal.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let count = self.link_candidates(modal).len();
                if count > 0 {
                    modal.selected = (modal.selected + 1).min(count - 1);
                }
            }
            KeyCode::Backspace => {
                modal.filter.pop();
                modal.selected = 0;
                modal.error = None;
            }
            KeyCode::Enter => {
                let candidates = self.link_candidates(modal);
                let Some(target_id) = candidates.get(modal.selected).cloned() else {
                    modal.error = Some("no matching task selected".to_owned());
                    return true;
                };
                return self.apply_link_modal(modal, &target_id);
            }
            KeyCode::Char(c) => {
                modal.filter.push(c);
                modal.selected = 0;
                modal.error = None;
            }
            _ => {}
        }
        true
    }

    fn apply_link_modal(&mut self, modal: &mut LinkModal, target_id: &TaskId) -> bool {
        let src = modal.source_id.clone();
        let outcome = match (modal.adding, modal.relation) {
            (true, LinkRelation::DependsOn) => self.graph.add_dependency(&src, target_id),
            (false, LinkRelation::DependsOn) => self.graph.remove_dependency(&src, target_id),
            (true, LinkRelation::Blocks) => self.graph.add_dependency(target_id, &src),
            (false, LinkRelation::Blocks) => self.graph.remove_dependency(target_id, &src),
            (true, LinkRelation::Related) => self.graph.add_related(&src, target_id),
            (false, LinkRelation::Related) => self.graph.remove_related(&src, target_id),
        };

        match outcome {
            Ok(changed) => {
                if changed {
                    let to_save: Vec<TaskId> = match modal.relation {
                        LinkRelation::DependsOn => vec![src.clone()],
                        LinkRelation::Blocks => vec![target_id.clone()],
                        LinkRelation::Related => vec![src.clone(), target_id.clone()],
                    };
                    if let Err(err) = self.store.save_tasks(&self.graph, &to_save) {
                        modal.error = Some(err.to_string());
                        return true;
                    }
                    self.refresh_visible(Some(&src));
                    let verb = if modal.adding { "Linked" } else { "Unlinked" };
                    self.status_message = Some(format!("{verb} {src} / {target_id}"));
                } else {
                    self.status_message = Some("No link changes".to_owned());
                }
                false
            }
            Err(err) => {
                modal.error = Some(err.to_string());
                true
            }
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
        if self.view_mode == ViewMode::Tree {
            let len = build_tree_rows(self).map(|r| r.len()).unwrap_or(0) as isize;
            if len == 0 {
                self.tree_index = 0;
                return;
            }
            let curr = self.tree_index as isize;
            self.tree_index = (curr + delta).clamp(0, len - 1) as usize;
            return;
        }

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

    fn clamp_tree_index(&mut self) {
        let len = build_tree_rows(self).map(|r| r.len()).unwrap_or(0);
        if len == 0 {
            self.tree_index = 0;
        } else if self.tree_index >= len {
            self.tree_index = len - 1;
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
        let Some(id) = self.active_selected_id() else {
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
        app.handle_key(key(KeyCode::Right));
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

    #[test]
    fn tab_toggles_layered_tree_view_and_add_modal_creates_task() {
        let tmp = TempDir::new("tree-add");
        let store = TaskStore::init_at(&tmp.0).unwrap();

        let t1 = Task::new("Layer zero", Priority::High);
        let mut t2 = Task::new("Layer one", Priority::Medium);
        t2.depends_on.insert(t1.id.clone());
        store.save_task(&t1).unwrap();
        store.save_task(&t2).unwrap();

        let graph = TaskGraph::from_tasks([t1, t2]);
        let mut app = App::new(store, graph);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.view_mode, ViewMode::Tree);
        let rows = build_tree_rows(&app).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].layer, 0);
        assert_eq!(rows[1].layer, 1);

        // Quick-add modal
        app.handle_key(key(KeyCode::Char('a')));
        assert!(matches!(app.modal, Some(ModalState::Add(_))));
        for ch in "New task".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert_eq!(app.graph.len(), 3);
    }

    #[test]
    fn link_modal_rejects_cycles_with_live_error_and_unlinks() {
        let tmp = TempDir::new("link-modal");
        let store = TaskStore::init_at(&tmp.0).unwrap();

        let t1 = Task::new("Alpha", Priority::High);
        let mut t2 = Task::new("Beta", Priority::Medium);
        t2.depends_on.insert(t1.id.clone());
        let id1 = t1.id.clone();
        let id2 = t2.id.clone();
        store.save_task(&t1).unwrap();
        store.save_task(&t2).unwrap();

        let graph = TaskGraph::from_tasks([t1, t2]);
        let mut app = App::new(store, graph);
        app.refresh_visible(Some(&id1));

        // Try linking `id1 depends_on id2`, which creates a cycle.
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Enter));
        match &app.modal {
            Some(ModalState::Link(m)) => {
                assert!(m.error.as_ref().is_some_and(|e| e.contains("cycle")));
            }
            other => panic!("expected Link modal staying open on cycle, got {other:?}"),
        }
        app.handle_key(key(KeyCode::Esc));

        // Unlink `id2 depends_on id1`.
        app.refresh_visible(Some(&id2));
        app.handle_key(key(KeyCode::Char('u')));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert!(!app.graph.get(&id2).unwrap().depends_on.contains(&id1));
    }
}
