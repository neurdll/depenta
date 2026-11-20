mod cli;
#[allow(dead_code)] // Layers used by TUI tree view in ISS-006.
mod graph;
mod model;
mod store;

fn main() {
    match cli::run_from_args(std::env::args_os().skip(1)) {
        Ok(true) => {
            eprintln!("TUI workbench will be wired in ISS-005. Run `depenta help` for CLI usage.");
        }
        Ok(false) => {}
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
