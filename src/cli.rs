use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use lexopt::prelude::*;

use crate::graph::TaskGraph;
use crate::model::{Priority, Status, Task, TaskId, now_epoch_secs};
use crate::store::TaskStore;

/// Top-level CLI execution result: `Ok(true)` launches TUI, `Ok(false)` exits normally.
pub fn run_from_args<I>(args: I) -> Result<bool, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut parser = lexopt::Parser::from_iter(args);
    let Some(arg) = parser.next().map_err(|e| e.to_string())? else {
        return Ok(true);
    };

    let subcmd = match arg {
        Short('h') | Long("help") => {
            print_help();
            return Ok(false);
        }
        Short('V') | Long("version") => {
            println!("depenta {}", env!("CARGO_PKG_VERSION"));
            return Ok(false);
        }
        Value(val) => val.string().map_err(|e| e.to_string())?,
        other => return Err(format!("unexpected option: {other:?}")),
    };

    match subcmd.as_str() {
        "tui" => {
            expect_no_more_args(&mut parser)?;
            Ok(true)
        }
        "help" => {
            print_help();
            Ok(false)
        }
        "init" => {
            expect_no_more_args(&mut parser)?;
            cmd_init()?;
            Ok(false)
        }
        "add" => {
            cmd_add(&mut parser)?;
            Ok(false)
        }
        "link" => {
            cmd_link(&mut parser, true)?;
            Ok(false)
        }
        "unlink" => {
            cmd_link(&mut parser, false)?;
            Ok(false)
        }
        "done" => {
            cmd_done(&mut parser)?;
            Ok(false)
        }
        "status" => {
            cmd_status(&mut parser)?;
            Ok(false)
        }
        "ready" => {
            expect_no_more_args(&mut parser)?;
            cmd_ready()?;
            Ok(false)
        }
        "list" => {
            cmd_list(&mut parser)?;
            Ok(false)
        }
        "show" => {
            cmd_show(&mut parser)?;
            Ok(false)
        }
        "edit" => {
            cmd_edit(&mut parser)?;
            Ok(false)
        }
        "rm" | "remove" => {
            cmd_rm(&mut parser)?;
            Ok(false)
        }
        other => Err(format!(
            "unknown subcommand '{other}'. Run `depenta help` for usage."
        )),
    }
}

fn expect_no_more_args(parser: &mut lexopt::Parser) -> Result<(), String> {
    if let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        return Err(format!("unexpected argument: {arg:?}"));
    }
    Ok(())
}

fn open_store_and_graph() -> Result<(TaskStore, TaskGraph), String> {
    let store = TaskStore::discover().map_err(|e| e.to_string())?;
    let graph = store.load_graph().map_err(|e| e.to_string())?;
    Ok((store, graph))
}

fn cmd_init() -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let store = TaskStore::init_at(&cwd).map_err(|e| e.to_string())?;
    println!("Initialized depenta store at {}", store.root().display());
    Ok(())
}

fn cmd_add(parser: &mut lexopt::Parser) -> Result<(), String> {
    let mut title: Option<String> = None;
    let mut priority = Priority::Medium;
    let mut description = String::new();
    let mut after: Vec<String> = Vec::new();
    let mut related: Vec<String> = Vec::new();

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Short('p') | Long("priority") => {
                let raw = parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?;
                priority = raw.parse()?;
            }
            Short('d') | Long("desc") | Long("description") => {
                description = parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?;
            }
            Long("after") => {
                after.push(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Long("rel") | Long("related") => {
                related.push(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Value(val) if title.is_none() => {
                title = Some(val.string().map_err(|e| e.to_string())?);
            }
            Value(val) => {
                let extra = val.string().map_err(|e| e.to_string())?;
                return Err(format!("unexpected extra positional argument '{extra}'"));
            }
            other => return Err(format!("unexpected option for add: {other:?}")),
        }
    }

    let title = title
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "usage: depenta add <title> [--priority P] [--after ID] [--rel ID] [--desc TEXT]".to_owned())?;

    let (store, mut graph) = open_store_and_graph()?;

    let mut after_ids = Vec::with_capacity(after.len());
    for q in &after {
        after_ids.push(graph.resolve_id(q).map_err(|e| e.to_string())?);
    }
    let mut rel_ids = Vec::with_capacity(related.len());
    for q in &related {
        rel_ids.push(graph.resolve_id(q).map_err(|e| e.to_string())?);
    }

    let mut task = Task::new(title, priority);
    task.description = description;
    let new_id = task.id.clone();
    graph.insert_task(task);

    for prereq in &after_ids {
        graph.add_dependency(&new_id, prereq).map_err(|e| e.to_string())?;
    }
    for rel in &rel_ids {
        graph.add_related(&new_id, rel).map_err(|e| e.to_string())?;
    }

    let mut to_save = vec![new_id.clone()];
    to_save.extend(rel_ids);
    store.save_tasks(&graph, &to_save).map_err(|e| e.to_string())?;

    let created = graph.get(&new_id).expect("inserted task");
    println!("Added {} [{}] {}", created.id, created.priority, created.title);
    Ok(())
}

fn cmd_link(parser: &mut lexopt::Parser, add: bool) -> Result<(), String> {
    let mut target: Option<String> = None;
    let mut after: Vec<String> = Vec::new();
    let mut before: Vec<String> = Vec::new();
    let mut related: Vec<String> = Vec::new();

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("after") => {
                after.push(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Long("before") => {
                before.push(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Long("rel") | Long("related") => {
                related.push(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Value(val) if target.is_none() => {
                target = Some(val.string().map_err(|e| e.to_string())?);
            }
            other => return Err(format!("unexpected argument for link/unlink: {other:?}")),
        }
    }

    let verb = if add { "link" } else { "unlink" };
    let target_query = target.ok_or_else(|| {
        format!("usage: depenta {verb} <id> [--after ID] [--before ID] [--rel ID]")
    })?;
    if after.is_empty() && before.is_empty() && related.is_empty() {
        return Err(format!(
            "specify at least one of --after, --before, or --rel for `{verb}`"
        ));
    }

    let (store, mut graph) = open_store_and_graph()?;
    let target_id = graph.resolve_id(&target_query).map_err(|e| e.to_string())?;

    let after_ids = resolve_all(&graph, &after)?;
    let before_ids = resolve_all(&graph, &before)?;
    let rel_ids = resolve_all(&graph, &related)?;

    let mut dirty: Vec<TaskId> = Vec::new();
    for prereq in after_ids {
        let changed = if add {
            graph.add_dependency(&target_id, &prereq)
        } else {
            graph.remove_dependency(&target_id, &prereq)
        }
        .map_err(|e| e.to_string())?;
        if changed {
            dirty.push(target_id.clone());
            println!("{verb}: {target_id} depends_on {prereq}");
        }
    }

    for dependent in before_ids {
        let changed = if add {
            graph.add_dependency(&dependent, &target_id)
        } else {
            graph.remove_dependency(&dependent, &target_id)
        }
        .map_err(|e| e.to_string())?;
        if changed {
            dirty.push(dependent.clone());
            println!("{verb}: {dependent} depends_on {target_id}");
        }
    }

    for other in rel_ids {
        let changed = if add {
            graph.add_related(&target_id, &other)
        } else {
            graph.remove_related(&target_id, &other)
        }
        .map_err(|e| e.to_string())?;
        if changed {
            dirty.push(target_id.clone());
            dirty.push(other.clone());
            println!("{verb}: {target_id} <-> {other} (related)");
        }
    }

    dirty.sort();
    dirty.dedup();
    store.save_tasks(&graph, &dirty).map_err(|e| e.to_string())?;
    Ok(())
}

fn resolve_all(graph: &TaskGraph, queries: &[String]) -> Result<Vec<TaskId>, String> {
    queries
        .iter()
        .map(|q| graph.resolve_id(q).map_err(|e| e.to_string()))
        .collect()
}

fn cmd_done(parser: &mut lexopt::Parser) -> Result<(), String> {
    let query = next_positional(parser, "usage: depenta done <id>")?;
    expect_no_more_args(parser)?;

    let (store, mut graph) = open_store_and_graph()?;
    let id = graph.resolve_id(&query).map_err(|e| e.to_string())?;
    let unblocked = graph.mark_done(&id).map_err(|e| e.to_string())?;

    if let Some(task) = graph.get(&id) {
        store.save_task(task).map_err(|e| e.to_string())?;
        println!("Done: {} {}", task.id, task.title);
    }

    if !unblocked.is_empty() {
        println!("Newly unblocked:");
        for uid in unblocked {
            if let Some(t) = graph.get(&uid) {
                println!("  {} [{}] {}", t.id, t.priority, t.title);
            }
        }
    }
    Ok(())
}

fn cmd_status(parser: &mut lexopt::Parser) -> Result<(), String> {
    let query = next_positional(parser, "usage: depenta status <id> <todo|in_progress|done>")?;
    let raw_status = next_positional(parser, "usage: depenta status <id> <todo|in_progress|done>")?;
    expect_no_more_args(parser)?;

    let status: Status = raw_status.parse()?;
    let (store, mut graph) = open_store_and_graph()?;
    let id = graph.resolve_id(&query).map_err(|e| e.to_string())?;

    if status == Status::Done {
        let unblocked = graph.mark_done(&id).map_err(|e| e.to_string())?;
        if let Some(task) = graph.get(&id) {
            store.save_task(task).map_err(|e| e.to_string())?;
            println!("Updated {} -> done", task.id);
        }
        for uid in unblocked {
            if let Some(t) = graph.get(&uid) {
                println!("  unblocked: {} {}", t.id, t.title);
            }
        }
    } else if let Some(task) = graph.get_mut(&id) {
        task.status = status;
        task.updated_at = now_epoch_secs();
        store.save_task(task).map_err(|e| e.to_string())?;
        println!("Updated {} -> {}", task.id, task.status);
    }
    Ok(())
}

fn cmd_ready() -> Result<(), String> {
    let (_, graph) = open_store_and_graph()?;
    let ready = graph.ready_tasks();
    if ready.is_empty() {
        println!("No ready tasks.");
        return Ok(());
    }
    for task in ready {
        let impact = graph.downstream_impact(&task.id);
        println!(
            "{}  {:<8}  unlocks:{:<2}  {}",
            task.id,
            task.priority.to_string(),
            impact,
            task.title
        );
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListMode {
    All,
    Ready,
    Blocked,
    Done,
    Topo,
}

fn cmd_list(parser: &mut lexopt::Parser) -> Result<(), String> {
    let mut mode = ListMode::All;
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("all") => mode = ListMode::All,
            Long("ready") => mode = ListMode::Ready,
            Long("blocked") => mode = ListMode::Blocked,
            Long("done") => mode = ListMode::Done,
            Long("topo") => mode = ListMode::Topo,
            other => return Err(format!("unexpected option for list: {other:?}")),
        }
    }

    let (_, graph) = open_store_and_graph()?;
    let tasks: Vec<&Task> = match mode {
        ListMode::All => graph.all_tasks().collect(),
        ListMode::Ready => graph.ready_tasks(),
        ListMode::Blocked => graph
            .all_tasks()
            .filter(|t| graph.is_blocked(&t.id))
            .collect(),
        ListMode::Done => graph
            .all_tasks()
            .filter(|t| t.status == Status::Done)
            .collect(),
        ListMode::Topo => graph
            .topo_sort()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter_map(|id| graph.get(&id))
            .collect(),
    };

    if tasks.is_empty() {
        println!("No matching tasks.");
        return Ok(());
    }

    for task in tasks {
        let state = task_state_badge(&graph, task);
        println!(
            "{}  {:<11}  {:<8}  {}",
            task.id,
            state,
            task.priority.to_string(),
            task.title
        );
    }
    Ok(())
}

fn task_state_badge(graph: &TaskGraph, task: &Task) -> &'static str {
    if task.status == Status::Done {
        "done"
    } else if graph.is_blocked(&task.id) {
        "blocked"
    } else if task.status == Status::InProgress {
        "in_progress"
    } else {
        "ready"
    }
}

fn cmd_show(parser: &mut lexopt::Parser) -> Result<(), String> {
    let query = next_positional(parser, "usage: depenta show <id>")?;
    expect_no_more_args(parser)?;

    let (_, graph) = open_store_and_graph()?;
    let id = graph.resolve_id(&query).map_err(|e| e.to_string())?;
    let task = graph.get(&id).expect("resolved task");

    println!("{}  {}", task.id, task.title);
    println!(
        "status: {} ({}) | priority: {}",
        task.status,
        task_state_badge(&graph, task),
        task.priority
    );
    if !task.description.is_empty() {
        println!("\n{}", task.description);
    }

    let needs = graph.prerequisites(&id);
    if !needs.is_empty() {
        println!("\nNeeds (depends_on):");
        for t in needs {
            println!("  {} [{}] {}", t.id, t.status, t.title);
        }
    }
    let unlocks = graph.unlocks(&id);
    if !unlocks.is_empty() {
        println!("\nUnlocks (blocks):");
        for t in unlocks {
            println!("  {} [{}] {}", t.id, t.status, t.title);
        }
    }
    let related = graph.related_tasks(&id);
    if !related.is_empty() {
        println!("\nRelated:");
        for t in related {
            println!("  {} [{}] {}", t.id, t.status, t.title);
        }
    }
    Ok(())
}

fn cmd_edit(parser: &mut lexopt::Parser) -> Result<(), String> {
    let mut query: Option<String> = None;
    let mut new_title: Option<String> = None;
    let mut new_desc: Option<String> = None;
    let mut new_priority: Option<Priority> = None;

    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Short('t') | Long("title") => {
                new_title = Some(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Short('d') | Long("desc") | Long("description") => {
                new_desc = Some(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Short('p') | Long("priority") => {
                let raw = parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?;
                new_priority = Some(raw.parse()?);
            }
            Value(val) if query.is_none() => {
                query = Some(val.string().map_err(|e| e.to_string())?);
            }
            other => return Err(format!("unexpected argument for edit: {other:?}")),
        }
    }

    let query = query.ok_or_else(|| {
        "usage: depenta edit <id> [--title TEXT] [--desc TEXT] [--priority P]".to_owned()
    })?;

    let (store, mut graph) = open_store_and_graph()?;
    let id = graph.resolve_id(&query).map_err(|e| e.to_string())?;
    let task = graph.get_mut(&id).expect("resolved task");

    let has_inline_flags = new_title.is_some() || new_desc.is_some() || new_priority.is_some();
    if has_inline_flags {
        if let Some(title) = new_title {
            let trimmed = title.trim().to_owned();
            if trimmed.is_empty() {
                return Err("task title cannot be empty".to_owned());
            }
            task.title = trimmed;
        }
        if let Some(desc) = new_desc {
            task.description = desc;
        }
        if let Some(prio) = new_priority {
            task.priority = prio;
        }
        task.updated_at = now_epoch_secs();
        store.save_task(task).map_err(|e| e.to_string())?;
        println!("Updated {} {}", task.id, task.title);
    } else if let Some((edited_title, edited_desc)) =
        edit_with_editor(&task.title, &task.description)?
    {
        if edited_title != task.title || edited_desc != task.description {
            task.title = edited_title;
            task.description = edited_desc;
            task.updated_at = now_epoch_secs();
            store.save_task(task).map_err(|e| e.to_string())?;
            println!("Updated {} {}", task.id, task.title);
        } else {
            println!("No changes.");
        }
    }
    Ok(())
}

fn cmd_rm(parser: &mut lexopt::Parser) -> Result<(), String> {
    let query = next_positional(parser, "usage: depenta rm <id>")?;
    expect_no_more_args(parser)?;

    let (store, mut graph) = open_store_and_graph()?;
    let id = graph.resolve_id(&query).map_err(|e| e.to_string())?;
    let touched = graph.remove_task(&id).map_err(|e| e.to_string())?;

    store.save_tasks(&graph, &touched).map_err(|e| e.to_string())?;
    store.delete_task(&id).map_err(|e| e.to_string())?;
    println!("Removed {id}");
    Ok(())
}

fn next_positional(parser: &mut lexopt::Parser, usage: &str) -> Result<String, String> {
    match parser.next().map_err(|e| e.to_string())? {
        Some(Value(val)) => val.string().map_err(|e| e.to_string()),
        Some(other) => Err(format!("unexpected option {other:?}; {usage}")),
        None => Err(usage.to_owned()),
    }
}

/// Opens `$VISUAL` / `$EDITOR` (fallback `vi`, then `nano`) to edit a task's title and description.
pub fn edit_with_editor(title: &str, description: &str) -> Result<Option<(String, String)>, String> {
    let tmp_path = temp_editor_file_path();
    let initial = format_editor_buffer(title, description);
    fs::write(&tmp_path, &initial).map_err(|e| format!("failed to write temp file: {e}"))?;

    let status = spawn_editor(&tmp_path);
    let result = match status {
        Ok(exit) if exit.success() => {
            let edited = fs::read_to_string(&tmp_path)
                .map_err(|e| format!("failed to read edited file: {e}"))?;
            Ok(parse_editor_buffer(&edited)?)
        }
        Ok(exit) => Err(format!("editor exited with status {exit}")),
        Err(err) => Err(err),
    };

    let _ = fs::remove_file(&tmp_path);
    result.map(Some)
}

fn temp_editor_file_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "depenta-edit-{}-{}.md",
        std::process::id(),
        now_epoch_secs()
    ))
}

fn spawn_editor(path: &PathBuf) -> Result<std::process::ExitStatus, String> {
    if let Ok(editor_cmd) = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")) {
        let mut parts = editor_cmd.split_whitespace();
        if let Some(bin) = parts.next() {
            return Command::new(bin)
                .args(parts)
                .arg(path)
                .status()
                .map_err(|e| format!("failed to launch editor '{editor_cmd}': {e}"));
        }
    }

    Command::new("vi")
        .arg(path)
        .status()
        .or_else(|_| Command::new("nano").arg(path).status())
        .map_err(|e| format!("failed to launch fallback editor (vi/nano): {e}"))
}

fn format_editor_buffer(title: &str, description: &str) -> String {
    let mut out = String::from(
        "# First non-comment line is the task title.\n# Remaining lines form the description.\n",
    );
    out.push_str(title);
    out.push_str("\n\n");
    if !description.is_empty() {
        out.push_str(description);
        if !description.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn parse_editor_buffer(raw: &str) -> Result<(String, String), String> {
    let lines: Vec<&str> = raw
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect();

    let first_non_empty = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .ok_or_else(|| "aborted: task title cannot be empty".to_owned())?;

    let title = lines[first_non_empty].trim().to_owned();
    let desc = lines[first_non_empty + 1..]
        .join("\n")
        .trim()
        .to_owned();
    Ok((title, desc))
}

fn print_help() {
    println!(
        "\
depenta - task dependency manager

USAGE:
  depenta                        Launch interactive TUI
  depenta <COMMAND> [OPTIONS]

COMMANDS:
  init                           Initialize .depenta/ in current directory
  add <title> [OPTIONS]          Create task (-p/--priority, --after ID, --rel ID, -d/--desc)
  link <id> [OPTIONS]            Add edges (--after ID, --before ID, --rel ID)
  unlink <id> [OPTIONS]          Remove edges (--after ID, --before ID, --rel ID)
  done <id>                      Mark task done and list newly unblocked tasks
  status <id> <state>            Set status (todo | in_progress | done)
  ready                          List actionable tasks sorted by priority and impact
  list [--all|--ready|--blocked|--done|--topo]
  show <id>                      Show task details, blockers, unlocks, and related tasks
  edit <id> [OPTIONS]            Edit via $EDITOR or flags (--title, --desc, --priority)
  rm <id>                        Remove task and clean up links
  tui                            Launch interactive TUI"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_buffer_roundtrip_ignores_comments_and_parses_multiline_desc() {
        let buf = format_editor_buffer("Fix cycle bug", "Line one\nLine two");
        let (title, desc) = parse_editor_buffer(&buf).unwrap();
        assert_eq!(title, "Fix cycle bug");
        assert_eq!(desc, "Line one\nLine two");
    }

    #[test]
    fn editor_buffer_rejects_empty_title() {
        let err = parse_editor_buffer("# comment only\n   \n").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn no_args_requests_tui_launch() {
        let launch_tui = run_from_args(Vec::<OsString>::new()).unwrap();
        assert!(launch_tui);
    }

    #[test]
    fn tui_subcommand_requests_tui_launch() {
        let launch_tui = run_from_args([OsString::from("tui")]).unwrap();
        assert!(launch_tui);
    }
}
