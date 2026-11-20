pub mod app;
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
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// Launches the interactive 2-pane TUI workbench.
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
