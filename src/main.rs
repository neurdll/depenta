mod cli;
mod graph;
mod model;
mod store;
mod tui;

fn main() {
    let result = match cli::run_from_args(std::env::args_os()) {
        Ok(true) => tui::run(),
        Ok(false) => Ok(()),
        Err(err) => Err(err),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
