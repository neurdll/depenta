use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::graph::TaskGraph;
use crate::model::{Task, TaskId};

pub const DEPENTA_DIR: &str = ".depenta";
pub const TASKS_DIR: &str = "tasks";

/// Errors returned by `.depenta/` store discovery and file persistence.
#[derive(Debug)]
pub enum StoreError {
    NotInitialized(PathBuf),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized(start) => write!(
                f,
                "no '{DEPENTA_DIR}' directory found in '{}' or any parent (run `depenta init`)",
                start.display()
            ),
            Self::Io { path, source } => {
                write!(f, "I/O error at '{}': {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "corrupted task file '{}': {source}", path.display())
            }
        }
    }
}

impl std::error::Error for StoreError {}

/// Per-task filesystem store backed by `.depenta/tasks/<hash>.json`.
#[derive(Debug, Clone)]
pub struct TaskStore {
    root: PathBuf,
}

impl TaskStore {
    /// Discovers `.depenta/` starting from the current working directory.
    pub fn discover() -> Result<Self, StoreError> {
        let cwd = std::env::current_dir().map_err(|source| StoreError::Io {
            path: PathBuf::from("."),
            source,
        })?;
        Self::discover_from(&cwd)
    }

    /// Walks upward from `start` to locate an existing `.depenta/` directory.
    pub fn discover_from(start: &Path) -> Result<Self, StoreError> {
        let mut current = Some(start);
        while let Some(dir) = current {
            let candidate = dir.join(DEPENTA_DIR);
            if candidate.is_dir() {
                return Ok(Self { root: candidate });
            }
            current = dir.parent();
        }
        Err(StoreError::NotInitialized(start.to_path_buf()))
    }

    /// Initializes `.depenta/tasks/` inside `project_dir` (idempotent).
    pub fn init_at(project_dir: &Path) -> Result<Self, StoreError> {
        let root = project_dir.join(DEPENTA_DIR);
        let tasks_dir = root.join(TASKS_DIR);
        fs::create_dir_all(&tasks_dir).map_err(|source| StoreError::Io {
            path: tasks_dir,
            source,
        })?;
        Ok(Self { root })
    }

    /// Returns the `.depenta/` directory path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the `.depenta/tasks/` directory path.
    pub fn tasks_dir(&self) -> PathBuf {
        self.root.join(TASKS_DIR)
    }

    /// Returns the JSON file path for a specific task ID.
    pub fn task_path(&self, id: &TaskId) -> PathBuf {
        self.tasks_dir().join(format!("{}.json", id.as_str()))
    }

    /// Loads all `*.json` files in `.depenta/tasks/` into a `TaskGraph`.
    pub fn load_graph(&self) -> Result<TaskGraph, StoreError> {
        let tasks_dir = self.tasks_dir();
        if !tasks_dir.exists() {
            return Ok(TaskGraph::default());
        }

        let entries = fs::read_dir(&tasks_dir).map_err(|source| StoreError::Io {
            path: tasks_dir.clone(),
            source,
        })?;

        let mut tasks = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| StoreError::Io {
                path: tasks_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if !is_task_json_file(&path) {
                continue;
            }
            let content = fs::read_to_string(&path).map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
            let task: Task =
                serde_json::from_str(&content).map_err(|source| StoreError::Parse { path, source })?;
            tasks.push(task);
        }

        Ok(TaskGraph::from_tasks(tasks))
    }

    /// Atomically writes `<id>.json` using a temporary file in `.depenta/tasks/` + rename.
    pub fn save_task(&self, task: &Task) -> Result<(), StoreError> {
        let tasks_dir = self.tasks_dir();
        fs::create_dir_all(&tasks_dir).map_err(|source| StoreError::Io {
            path: tasks_dir.clone(),
            source,
        })?;

        let target_path = self.task_path(&task.id);
        let tmp_path = tasks_dir.join(format!(".{}.json.tmp.{}", task.id, std::process::id()));

        let mut payload = serde_json::to_string_pretty(task).map_err(|source| StoreError::Parse {
            path: target_path.clone(),
            source,
        })?;
        payload.push('\n');

        let write_result = (|| -> io::Result<()> {
            let mut file = File::create(&tmp_path)?;
            file.write_all(payload.as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp_path, &target_path)?;
            Ok(())
        })();

        if let Err(source) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(StoreError::Io {
                path: target_path,
                source,
            });
        }

        Ok(())
    }

    /// Saves multiple tasks from `graph` by ID.
    pub fn save_tasks<'a>(
        &self,
        graph: &TaskGraph,
        ids: impl IntoIterator<Item = &'a TaskId>,
    ) -> Result<(), StoreError> {
        for id in ids {
            if let Some(task) = graph.get(id) {
                self.save_task(task)?;
            }
        }
        Ok(())
    }

    /// Removes `.depenta/tasks/<id>.json` if present.
    pub fn delete_task(&self, id: &TaskId) -> Result<(), StoreError> {
        let path = self.task_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }
}

fn is_task_json_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let is_hidden = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with('.'));
    !is_hidden && path.extension().and_then(|ext| ext.to_str()) == Some("json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Priority, Status, generate_id};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = generate_id(label);
            let path = std::env::temp_dir().join(format!(
                "depenta-test-{label}-{}-{}",
                std::process::id(),
                unique
            ));
            fs::create_dir_all(&path).expect("create temp test dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn init_and_discover_from_nested_subdirectory() {
        let tmp = TempDir::new("discover");
        let nested = tmp.path.join("crates/core/src");
        fs::create_dir_all(&nested).unwrap();

        assert!(matches!(
            TaskStore::discover_from(&nested),
            Err(StoreError::NotInitialized(_))
        ));

        let initialized = TaskStore::init_at(&tmp.path).unwrap();
        // Idempotent second init succeeds.
        TaskStore::init_at(&tmp.path).unwrap();

        let discovered = TaskStore::discover_from(&nested).unwrap();
        assert_eq!(discovered.root(), initialized.root());
        assert!(discovered.tasks_dir().is_dir());
    }

    #[test]
    fn save_load_and_delete_task_files() {
        let tmp = TempDir::new("crud");
        let store = TaskStore::init_at(&tmp.path).unwrap();

        let mut t1 = Task::new("Build store", Priority::High);
        t1.status = Status::Done;
        let mut t2 = Task::new("Wire CLI", Priority::Critical);
        t2.depends_on.insert(t1.id.clone());

        store.save_task(&t1).unwrap();
        store.save_task(&t2).unwrap();

        assert!(store.task_path(&t1.id).is_file());
        assert!(store.task_path(&t2.id).is_file());

        let graph = store.load_graph().unwrap();
        assert_eq!(graph.len(), 2);
        assert!(graph.is_ready(&t2.id));

        store.delete_task(&t1.id).unwrap();
        assert!(!store.task_path(&t1.id).exists());
        // Deleting missing task is a no-op.
        store.delete_task(&t1.id).unwrap();
    }

    #[test]
    fn load_graph_reports_corrupted_json_with_path_context() {
        let tmp = TempDir::new("corrupt");
        let store = TaskStore::init_at(&tmp.path).unwrap();
        let bad_file = store.tasks_dir().join("bad123.json");
        fs::write(&bad_file, "{ not valid json").unwrap();

        let err = store.load_graph().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad123.json"), "unexpected message: {msg}");
    }
}
