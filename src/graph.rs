use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use crate::model::{Priority, Status, Task, TaskId, now_epoch_secs};

/// Errors produced by `TaskGraph` lookups and mutations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    NotFound(String),
    AmbiguousId {
        query: String,
        candidates: Vec<TaskId>,
    },
    Cycle(Vec<TaskId>),
    SelfRelation(TaskId),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(query) => write!(f, "task not found: '{query}'"),
            Self::AmbiguousId { query, candidates } => {
                let list: Vec<&str> = candidates.iter().map(TaskId::as_str).collect();
                write!(
                    f,
                    "ambiguous task id '{query}'; matches: {}",
                    list.join(", ")
                )
            }
            Self::Cycle(path) => {
                let chain: Vec<&str> = path.iter().map(TaskId::as_str).collect();
                write!(f, "dependency would create a cycle: {}", chain.join(" -> "))
            }
            Self::SelfRelation(id) => {
                write!(f, "cannot relate task '{id}' to itself")
            }
        }
    }
}

impl std::error::Error for GraphError {}

/// In-memory task store with DAG (`depends_on`/`blocks`) and symmetric (`related`) indices.
#[derive(Debug, Default, Clone)]
pub struct TaskGraph {
    tasks: BTreeMap<TaskId, Task>,
    /// Reverse dependency index: `blocks[prereq]` = tasks that depend on `prereq`.
    blocks: BTreeMap<TaskId, BTreeSet<TaskId>>,
}

impl TaskGraph {
    /// Builds a `TaskGraph` from a collection of tasks and indexes reverse dependency edges.
    pub fn from_tasks(tasks: impl IntoIterator<Item = Task>) -> Self {
        let mut graph = Self::default();
        for task in tasks {
            graph.tasks.insert(task.id.clone(), task);
        }
        graph.rebuild_reverse_index();
        graph
    }

    /// Rebuilds the `blocks` reverse index from all tasks' `depends_on` sets.
    fn rebuild_reverse_index(&mut self) {
        self.blocks.clear();
        for id in self.tasks.keys() {
            self.blocks.entry(id.clone()).or_default();
        }
        for (dependent_id, task) in &self.tasks {
            for prereq_id in &task.depends_on {
                if self.tasks.contains_key(prereq_id) {
                    self.blocks
                        .entry(prereq_id.clone())
                        .or_default()
                        .insert(dependent_id.clone());
                }
            }
        }
    }

    /// Inserts or replaces a task and updates the reverse dependency index.
    pub fn insert_task(&mut self, task: Task) {
        self.tasks.insert(task.id.clone(), task);
        self.rebuild_reverse_index();
    }

    /// Resolves an exact ID or unique lowercase hex prefix to a `TaskId`.
    pub fn resolve_id(&self, query: &str) -> Result<TaskId, GraphError> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Err(GraphError::NotFound(query.to_owned()));
        }

        let exact = TaskId(needle.clone());
        if self.tasks.contains_key(&exact) {
            return Ok(exact);
        }

        let matches: Vec<TaskId> = self
            .tasks
            .keys()
            .filter(|id| id.as_str().starts_with(&needle))
            .cloned()
            .collect();

        match matches.len() {
            0 => Err(GraphError::NotFound(query.to_owned())),
            1 => Ok(matches.into_iter().next().expect("single match")),
            _ => Err(GraphError::AmbiguousId {
                query: query.to_owned(),
                candidates: matches,
            }),
        }
    }

    /// Returns a reference to a task by exact ID.
    pub fn get(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Returns a mutable reference to a task by exact ID.
    pub fn get_mut(&mut self, id: &TaskId) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    /// Iterates over all tasks in ID order.
    pub fn all_tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Returns the total number of tasks in the graph.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Returns `true` if the graph contains no tasks.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Adds a directed `dependent depends_on prerequisite` edge after verifying acyclicity.
    /// Returns `Ok(true)` if the edge was newly added, or `Ok(false)` if already present.
    pub fn add_dependency(
        &mut self,
        dependent: &TaskId,
        prerequisite: &TaskId,
    ) -> Result<bool, GraphError> {
        if !self.tasks.contains_key(dependent) {
            return Err(GraphError::NotFound(dependent.to_string()));
        }
        if !self.tasks.contains_key(prerequisite) {
            return Err(GraphError::NotFound(prerequisite.to_string()));
        }
        if dependent == prerequisite {
            return Err(GraphError::Cycle(vec![
                dependent.clone(),
                dependent.clone(),
            ]));
        }
        if self.tasks[dependent].depends_on.contains(prerequisite) {
            return Ok(false);
        }

        if let Some(mut path) = self.find_dependency_path(prerequisite, dependent) {
            path.insert(0, dependent.clone());
            return Err(GraphError::Cycle(path));
        }

        let now = now_epoch_secs();
        if let Some(task) = self.tasks.get_mut(dependent) {
            task.depends_on.insert(prerequisite.clone());
            task.updated_at = now;
        }
        self.blocks
            .entry(prerequisite.clone())
            .or_default()
            .insert(dependent.clone());
        Ok(true)
    }

    /// Removes a directed `dependent depends_on prerequisite` edge.
    /// Returns `Ok(true)` if the edge existed and was removed.
    pub fn remove_dependency(
        &mut self,
        dependent: &TaskId,
        prerequisite: &TaskId,
    ) -> Result<bool, GraphError> {
        if !self.tasks.contains_key(dependent) {
            return Err(GraphError::NotFound(dependent.to_string()));
        }
        if !self.tasks.contains_key(prerequisite) {
            return Err(GraphError::NotFound(prerequisite.to_string()));
        }

        let removed = self
            .tasks
            .get_mut(dependent)
            .is_some_and(|t| t.depends_on.remove(prerequisite));
        if removed {
            if let Some(task) = self.tasks.get_mut(dependent) {
                task.updated_at = now_epoch_secs();
            }
            if let Some(set) = self.blocks.get_mut(prerequisite) {
                set.remove(dependent);
            }
        }
        Ok(removed)
    }

    /// Adds a symmetric `related` edge between two distinct tasks (cycles permitted).
    /// Returns `Ok(true)` if either side changed.
    pub fn add_related(&mut self, a: &TaskId, b: &TaskId) -> Result<bool, GraphError> {
        if !self.tasks.contains_key(a) {
            return Err(GraphError::NotFound(a.to_string()));
        }
        if !self.tasks.contains_key(b) {
            return Err(GraphError::NotFound(b.to_string()));
        }
        if a == b {
            return Err(GraphError::SelfRelation(a.clone()));
        }

        let now = now_epoch_secs();
        let mut changed = false;
        if let Some(task_a) = self.tasks.get_mut(a)
            && task_a.related.insert(b.clone())
        {
            task_a.updated_at = now;
            changed = true;
        }
        if let Some(task_b) = self.tasks.get_mut(b)
            && task_b.related.insert(a.clone())
        {
            task_b.updated_at = now;
            changed = true;
        }
        Ok(changed)
    }

    /// Removes a symmetric `related` edge between two tasks.
    /// Returns `Ok(true)` if either side changed.
    pub fn remove_related(&mut self, a: &TaskId, b: &TaskId) -> Result<bool, GraphError> {
        if !self.tasks.contains_key(a) {
            return Err(GraphError::NotFound(a.to_string()));
        }
        if !self.tasks.contains_key(b) {
            return Err(GraphError::NotFound(b.to_string()));
        }

        let now = now_epoch_secs();
        let mut changed = false;
        if let Some(task_a) = self.tasks.get_mut(a)
            && task_a.related.remove(b)
        {
            task_a.updated_at = now;
            changed = true;
        }
        if let Some(task_b) = self.tasks.get_mut(b)
            && task_b.related.remove(a)
        {
            task_b.updated_at = now;
            changed = true;
        }
        Ok(changed)
    }

    /// Removes a task and cleans dangling edges on remaining tasks.
    /// Returns the IDs of other tasks whose fields were modified.
    pub fn remove_task(&mut self, id: &TaskId) -> Result<Vec<TaskId>, GraphError> {
        if self.tasks.remove(id).is_none() {
            return Err(GraphError::NotFound(id.to_string()));
        }

        let now = now_epoch_secs();
        let mut touched = Vec::new();
        for (other_id, task) in &mut self.tasks {
            let dep_removed = task.depends_on.remove(id);
            let rel_removed = task.related.remove(id);
            if dep_removed || rel_removed {
                task.updated_at = now;
                touched.push(other_id.clone());
            }
        }
        self.rebuild_reverse_index();
        Ok(touched)
    }

    /// Marks a task `Done` and returns the IDs of direct dependents that just became ready.
    pub fn mark_done(&mut self, id: &TaskId) -> Result<Vec<TaskId>, GraphError> {
        if !self.tasks.contains_key(id) {
            return Err(GraphError::NotFound(id.to_string()));
        }

        let candidates: Vec<TaskId> = self.blocks.get(id).into_iter().flatten().cloned().collect();
        let was_ready: BTreeMap<TaskId, bool> = candidates
            .iter()
            .map(|cid| (cid.clone(), self.is_ready(cid)))
            .collect();

        if let Some(task) = self.tasks.get_mut(id) {
            task.status = Status::Done;
            task.updated_at = now_epoch_secs();
        }

        let newly_ready = candidates
            .into_iter()
            .filter(|cid| !was_ready.get(cid).copied().unwrap_or(false) && self.is_ready(cid))
            .collect();
        Ok(newly_ready)
    }

    /// Returns `true` if `id` is `Todo` and has no incomplete prerequisites.
    pub fn is_ready(&self, id: &TaskId) -> bool {
        let Some(task) = self.tasks.get(id) else {
            return false;
        };
        task.status == Status::Todo && !self.is_blocked(id)
    }

    /// Returns `true` if `id` is not `Done` and has at least one incomplete prerequisite.
    pub fn is_blocked(&self, id: &TaskId) -> bool {
        let Some(task) = self.tasks.get(id) else {
            return false;
        };
        if task.status == Status::Done {
            return false;
        }
        task.depends_on.iter().any(|prereq_id| {
            self.tasks
                .get(prereq_id)
                .is_some_and(|prereq| prereq.status != Status::Done)
        })
    }

    /// Returns all direct prerequisites (`depends_on`) of `id`.
    pub fn prerequisites(&self, id: &TaskId) -> Vec<&Task> {
        let Some(task) = self.tasks.get(id) else {
            return Vec::new();
        };
        task.depends_on
            .iter()
            .filter_map(|dep_id| self.tasks.get(dep_id))
            .collect()
    }

    /// Returns incomplete direct prerequisites blocking `id`.
    pub fn blocked_by(&self, id: &TaskId) -> Vec<&Task> {
        self.prerequisites(id)
            .into_iter()
            .filter(|t| t.status != Status::Done)
            .collect()
    }

    /// Returns direct downstream tasks that depend on `id`.
    pub fn unlocks(&self, id: &TaskId) -> Vec<&Task> {
        self.blocks
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|dep_id| self.tasks.get(dep_id))
            .collect()
    }

    /// Returns tasks related to `id`.
    pub fn related_tasks(&self, id: &TaskId) -> Vec<&Task> {
        let Some(task) = self.tasks.get(id) else {
            return Vec::new();
        };
        task.related
            .iter()
            .filter_map(|rel_id| self.tasks.get(rel_id))
            .collect()
    }

    /// Counts unique incomplete downstream tasks transitively waiting on `id`.
    pub fn downstream_impact(&self, id: &TaskId) -> usize {
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        if let Some(direct) = self.blocks.get(id) {
            for dep in direct {
                queue.push_back(dep.clone());
            }
        }

        while let Some(curr) = queue.pop_front() {
            if !visited.insert(curr.clone()) {
                continue;
            }
            if let Some(nexts) = self.blocks.get(&curr) {
                for next in nexts {
                    if !visited.contains(next) {
                        queue.push_back(next.clone());
                    }
                }
            }
        }

        visited
            .into_iter()
            .filter(|tid| self.tasks.get(tid).is_some_and(|t| t.status != Status::Done))
            .count()
    }

    /// Returns all ready tasks sorted by priority (desc), downstream impact (desc), then creation/ID.
    pub fn ready_tasks(&self) -> Vec<&Task> {
        let mut ready: Vec<&Task> = self
            .tasks
            .values()
            .filter(|t| self.is_ready(&t.id))
            .collect();

        ready.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| {
                    self.downstream_impact(&b.id)
                        .cmp(&self.downstream_impact(&a.id))
                })
                .then_with(|| a.created_at.cmp(&b.created_at))
                .then_with(|| a.id.cmp(&b.id))
        });
        ready
    }

    /// Returns tasks in topological order (prerequisites before dependents) via Kahn's algorithm.
    pub fn topo_sort(&self) -> Result<Vec<TaskId>, GraphError> {
        let mut in_degree: BTreeMap<TaskId, usize> = BTreeMap::new();
        for (id, task) in &self.tasks {
            let deg = task
                .depends_on
                .iter()
                .filter(|dep| self.tasks.contains_key(*dep))
                .count();
            in_degree.insert(id.clone(), deg);
        }

        let mut ready_queue: BTreeSet<(std::cmp::Reverse<Priority>, TaskId)> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(id, _)| (std::cmp::Reverse(self.tasks[id].priority), id.clone()))
            .collect();

        let mut order = Vec::with_capacity(self.tasks.len());
        while let Some((_, current)) = ready_queue.pop_first() {
            order.push(current.clone());
            if let Some(dependents) = self.blocks.get(&current) {
                for dep_id in dependents {
                    if let Some(deg) = in_degree.get_mut(dep_id) {
                        *deg -= 1;
                        if *deg == 0 {
                            ready_queue.insert((
                                std::cmp::Reverse(self.tasks[dep_id].priority),
                                dep_id.clone(),
                            ));
                        }
                    }
                }
            }
        }

        if order.len() != self.tasks.len() {
            // Find an arbitrary cycle among remaining nodes to report a concrete path.
            for id in in_degree.keys() {
                if in_degree[id] > 0
                    && let Some(cycle) = self.find_any_cycle_from(id)
                {
                    return Err(GraphError::Cycle(cycle));
                }
            }
        }

        Ok(order)
    }

    /// Groups tasks into longest-path dependency layers (layer 0 = roots with no prerequisites).
    pub fn layers(&self) -> Result<Vec<Vec<TaskId>>, GraphError> {
        let order = self.topo_sort()?;
        if order.is_empty() {
            return Ok(Vec::new());
        }

        let mut depth: BTreeMap<TaskId, usize> = BTreeMap::new();
        let mut max_depth = 0;

        for id in &order {
            let d = self.tasks[id]
                .depends_on
                .iter()
                .filter_map(|prereq| depth.get(prereq))
                .map(|&p_depth| p_depth + 1)
                .max()
                .unwrap_or(0);
            max_depth = max_depth.max(d);
            depth.insert(id.clone(), d);
        }

        let mut layers = vec![Vec::new(); max_depth + 1];
        for id in order {
            let d = depth[&id];
            layers[d].push(id);
        }

        for layer in &mut layers {
            layer.sort_by(|a, b| {
                self.tasks[b]
                    .priority
                    .cmp(&self.tasks[a].priority)
                    .then_with(|| a.cmp(b))
            });
        }

        Ok(layers)
    }

    /// DFS along `depends_on` edges from `start` to `target`, returning the path if reachable.
    fn find_dependency_path(&self, start: &TaskId, target: &TaskId) -> Option<Vec<TaskId>> {
        let mut visited = BTreeSet::new();
        let mut stack = Vec::new();
        self.dfs_path(start, target, &mut visited, &mut stack)
            .then_some(stack)
    }

    fn dfs_path(
        &self,
        curr: &TaskId,
        target: &TaskId,
        visited: &mut BTreeSet<TaskId>,
        path: &mut Vec<TaskId>,
    ) -> bool {
        if !visited.insert(curr.clone()) {
            return false;
        }
        path.push(curr.clone());
        if curr == target {
            return true;
        }
        if let Some(task) = self.tasks.get(curr) {
            for next in &task.depends_on {
                if self.tasks.contains_key(next) && self.dfs_path(next, target, visited, path) {
                    return true;
                }
            }
        }
        path.pop();
        false
    }

    fn find_any_cycle_from(&self, start: &TaskId) -> Option<Vec<TaskId>> {
        let mut visiting = Vec::new();
        let mut on_stack = BTreeSet::new();
        let mut visited = BTreeSet::new();
        self.dfs_cycle(start, &mut visiting, &mut on_stack, &mut visited)
    }

    fn dfs_cycle(
        &self,
        curr: &TaskId,
        stack: &mut Vec<TaskId>,
        on_stack: &mut BTreeSet<TaskId>,
        visited: &mut BTreeSet<TaskId>,
    ) -> Option<Vec<TaskId>> {
        if !visited.insert(curr.clone()) {
            return None;
        }
        stack.push(curr.clone());
        on_stack.insert(curr.clone());

        if let Some(task) = self.tasks.get(curr) {
            for next in &task.depends_on {
                if !self.tasks.contains_key(next) {
                    continue;
                }
                if on_stack.contains(next) {
                    let pos = stack.iter().position(|id| id == next).unwrap_or(0);
                    let mut cycle = stack[pos..].to_vec();
                    cycle.push(next.clone());
                    return Some(cycle);
                }
                if let Some(cycle) = self.dfs_cycle(next, stack, on_stack, visited) {
                    return Some(cycle);
                }
            }
        }

        on_stack.remove(curr);
        stack.pop();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(id: &str, title: &str, priority: Priority) -> Task {
        let mut t = Task::new(title, priority);
        t.id = TaskId::from(id);
        t
    }

    #[test]
    fn resolves_exact_and_unique_prefixes_and_rejects_ambiguous() {
        let graph = TaskGraph::from_tasks([
            make_task("a1b2c3", "First", Priority::Medium),
            make_task("a1ffff", "Second", Priority::Medium),
            make_task("deadbe", "Third", Priority::Medium),
        ]);

        assert_eq!(graph.resolve_id("d").unwrap(), TaskId::from("deadbe"));
        assert_eq!(graph.resolve_id("a1b").unwrap(), TaskId::from("a1b2c3"));
        assert_eq!(graph.resolve_id("A1B2C3").unwrap(), TaskId::from("a1b2c3"));
        assert!(matches!(
            graph.resolve_id("a1"),
            Err(GraphError::AmbiguousId { .. })
        ));
        assert!(matches!(
            graph.resolve_id("zzz"),
            Err(GraphError::NotFound(_))
        ));
    }

    #[test]
    fn rejects_dependency_cycle_and_returns_exact_path() {
        let mut graph = TaskGraph::from_tasks([
            make_task("aa0001", "A", Priority::Medium),
            make_task("bb0002", "B", Priority::Medium),
            make_task("cc0003", "C", Priority::Medium),
        ]);

        let a = TaskId::from("aa0001");
        let b = TaskId::from("bb0002");
        let c = TaskId::from("cc0003");

        assert_eq!(graph.add_dependency(&b, &a), Ok(true)); // B depends on A
        assert_eq!(graph.add_dependency(&c, &b), Ok(true)); // C depends on B

        // A depending on C closes A -> C -> B -> A.
        let err = graph.add_dependency(&a, &c).unwrap_err();
        assert_eq!(
            err,
            GraphError::Cycle(vec![a.clone(), c.clone(), b.clone(), a])
        );
    }

    #[test]
    fn related_links_are_symmetric_and_allow_cycles() {
        let mut graph = TaskGraph::from_tasks([
            make_task("111111", "One", Priority::Low),
            make_task("222222", "Two", Priority::Low),
            make_task("333333", "Three", Priority::Low),
        ]);

        let t1 = TaskId::from("111111");
        let t2 = TaskId::from("222222");
        let t3 = TaskId::from("333333");

        assert_eq!(graph.add_related(&t1, &t2), Ok(true));
        assert_eq!(graph.add_related(&t2, &t3), Ok(true));
        assert_eq!(graph.add_related(&t3, &t1), Ok(true));

        assert!(graph.get(&t1).unwrap().related.contains(&t2));
        assert!(graph.get(&t2).unwrap().related.contains(&t1));
        assert_eq!(graph.remove_related(&t1, &t2), Ok(true));
        assert!(!graph.get(&t2).unwrap().related.contains(&t1));
    }

    #[test]
    fn ready_tasks_rank_by_priority_then_impact_and_unlock_on_done() {
        let mut graph = TaskGraph::from_tasks([
            make_task("r1", "Root low impact", Priority::High),
            make_task("r2", "Root high impact", Priority::High),
            make_task("d1", "Downstream 1", Priority::Low),
            make_task("d2", "Downstream 2", Priority::Low),
            make_task("c1", "Critical root", Priority::Critical),
        ]);

        let r2 = TaskId::from("r2");
        let d1 = TaskId::from("d1");
        let d2 = TaskId::from("d2");

        graph.add_dependency(&d1, &r2).unwrap();
        graph.add_dependency(&d2, &d1).unwrap();

        let ready_ids: Vec<&str> = graph
            .ready_tasks()
            .into_iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ready_ids, vec!["c1", "r2", "r1"]);
        assert!(!graph.is_ready(&d1));
        assert!(graph.is_blocked(&d1));

        let unblocked = graph.mark_done(&r2).unwrap();
        assert_eq!(unblocked, vec![d1.clone()]);
        assert!(graph.is_ready(&d1));
        assert!(!graph.is_ready(&d2));
    }

    #[test]
    fn topo_sort_and_layers_order_longest_paths() {
        let mut graph = TaskGraph::from_tasks([
            make_task("a", "Root A", Priority::Medium),
            make_task("b", "Mid B", Priority::Medium),
            make_task("c", "Leaf C", Priority::Medium),
            make_task("d", "Root D", Priority::High),
        ]);

        let a = TaskId::from("a");
        let b = TaskId::from("b");
        let c = TaskId::from("c");
        let d = TaskId::from("d");

        // C depends on A and B; B depends on A => longest path to C is 2 (A -> B -> C).
        graph.add_dependency(&b, &a).unwrap();
        graph.add_dependency(&c, &a).unwrap();
        graph.add_dependency(&c, &b).unwrap();

        let layers = graph.layers().unwrap();
        assert_eq!(layers, vec![vec![d, a], vec![b], vec![c]]);
    }
}
