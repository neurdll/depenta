pub mod app;
pub mod modals;
pub mod tree;
pub mod views;

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::cli::edit_with_editor;
use crate::model::{TaskId, now_epoch_secs};
use crate::store::TaskStore;
use app::App;

/// RAII terminal guard restoring raw mode and alternate screen on drop or panic.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(err) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(err) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), LeaveAlternateScreen);
                Err(err)
            }
        }
    }

    fn suspend_for_editor<F, R>(&mut self, f: F) -> io::Result<R>
    where
        F: FnOnce() -> R,
    {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;

        let outcome = f();

        enable_raw_mode()?;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        self.terminal.clear()?;
        Ok(outcome)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// Launches the interactive TUI workbench and layered dependency tree.
pub fn run() -> Result<(), String> {
    let store = TaskStore::discover().map_err(|e| e.to_string())?;
    let graph = store.load_graph().map_err(|e| e.to_string())?;
    let mut app = App::new(store, graph);

    install_panic_hook();
    let mut guard = TerminalGuard::enter().map_err(|e| format!("terminal init failed: {e}"))?;

    while !app.should_quit {
        guard
            .terminal
            .draw(|frame| views::render(frame, &mut app))
            .map_err(|e| format!("render failed: {e}"))?;

        if event::poll(Duration::from_millis(200)).map_err(|e| format!("event poll failed: {e}"))?
            && let Event::Key(key) =
                event::read().map_err(|e| format!("event read failed: {e}"))?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }

        if let Some(task_id) = app.pending_editor_task_id.take() {
            run_external_editor(&mut guard, &mut app, &task_id)?;
        }
    }

    Ok(())
}

fn run_external_editor(
    guard: &mut TerminalGuard,
    app: &mut App,
    task_id: &TaskId,
) -> Result<(), String> {
    let Some((title, description)) = app
        .graph
        .get(task_id)
        .map(|t| (t.title.clone(), t.description.clone()))
    else {
        return Ok(());
    };

    let edit_result = guard
        .suspend_for_editor(|| edit_with_editor(&title, &description))
        .map_err(|e| format!("terminal suspend/resume failed: {e}"))?;

    match edit_result {
        Ok(Some((new_title, new_desc))) => {
            if let Some(task) = app.graph.get_mut(task_id)
                && (new_title != task.title || new_desc != task.description)
            {
                task.title = new_title;
                task.description = new_desc;
                task.updated_at = now_epoch_secs();
                match app.store.save_task(task) {
                    Ok(()) => {
                        app.refresh_visible(Some(task_id));
                        app.status_message = Some(format!("Edited {task_id}"));
                    }
                    Err(err) => app.status_message = Some(format!("Save error: {err}")),
                }
            } else {
                app.status_message = Some("No changes".to_owned());
            }
        }
        Ok(None) => {}
        Err(err) => app.status_message = Some(format!("Editor error: {err}")),
    }
    Ok(())
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        prev(info);
    }));
}
