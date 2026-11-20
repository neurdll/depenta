mod cli;
#[allow(dead_code)] // Layers used by TUI tree view in ISS-006.
mod graph;
mod model;
mod store;
mod tui;

fn main() {
    let result = match cli::run_from_args(std::env::args_os().skip(1)) {
        Ok(true) => tui::run(),
        Ok(false) => Ok(()),
        Err(err) => Err(err),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
