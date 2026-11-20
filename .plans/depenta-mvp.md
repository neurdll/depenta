# Plan: Depenta MVP (CLI + TUI Task Dependency Manager)

## Objective
Build a fast, project-local (`.depenta/`) task dependency manager with a CLI for quick capture/linking and a `ratatui` TUI featuring a 2-pane workbench and layered dependency tree view.

## Architecture & Simplicity Check
- **DOP Model:** Plain `Task` records keyed by short FNV-1a hex hash IDs (prefix-matched); no `parent/subtask` hierarchy since `depends_on` + `related` cover blocking and semantic links.
- **Graph Engine:** Pure `std` DAG + relation functions (`depends_on` strictly acyclic; `related` symmetric and unrestricted); reverse indices kept in memory for $O(\text{deg})$ queries.
- **Storage:** One JSON file per task (`.depenta/tasks/<hash>.json`) with atomic temp-write + rename.
- **Dependencies Accepted:** `ratatui`, `crossterm`, `serde` + `serde_json`, `lexopt`.
- **Dependencies Rejected:** `petgraph`, `clap`, `uuid`, `chrono` (replaced by ~20 lines of `std` graph/hash/epoch-time code).

## Execution Graph
- [x] ISS-001: Add serde, serde_json, and lexopt dependencies and core Task model (no dependencies)
- [x] ISS-002: Implement pure in-memory DAG and relation graph engine (depends on ISS-001)
- [ ] ISS-003: Implement per-task .depenta/tasks/<hash>.json file store (depends on ISS-002)
- [ ] ISS-004: Build lexopt CLI for init, task CRUD, linking, and graph queries (depends on ISS-003)
- [ ] ISS-005: Add TUI terminal lifecycle, app state, and 2-pane workbench (depends on ISS-004)
- [ ] ISS-006: Add layered dependency tree view and interactive modals in TUI (depends on ISS-005)

## Verification
- `cargo test` passes unit tests for hash IDs, cycle detection, topological layering, and atomic file persistence.
- `cargo clippy --all-targets` passes with zero warnings.
- End-to-end smoke check: `depenta init`, `add`, `link` (including cycle rejection), `ready`, `done`, and TUI launch (`depenta`) with both views and `$EDITOR`/modal editing.
