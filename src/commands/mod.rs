//! Command implementations for probe-verus CLI.
//!
//! This module contains the implementation of each CLI subcommand:
//! - `atomize`: Generate call graph atoms from SCIP indexes
//! - `callee-crates`: Find which crates a function's callees belong to
//! - `run-verus`: Run Verus verification and analyze results
//! - `functions`: List all functions in a project
//! - `specify`: Extract function specifications to JSON
//! - `specs-data`: Generate specs_data.json for the specs browser
//! - `tracked-csv`: Generate curve25519_functions.csv for the dashboard
//! - `stubify`: Convert .md files with YAML frontmatter to JSON
//! - `extract`: Unified pipeline - atomize + specify + run-verus

mod atomize;
mod callee_crates;
mod extract;
mod functions;
mod merge_atoms;
mod run_verus;
mod setup;
mod specify;
mod specs_data;
mod stubify;
mod tracked_csv;

pub use atomize::cmd_atomize;
pub use callee_crates::cmd_callee_crates;
pub use extract::cmd_extract;
pub use functions::cmd_functions;
pub use merge_atoms::cmd_merge_atoms;
pub use run_verus::cmd_run_verus;
pub use setup::cmd_setup;
pub use specify::cmd_specify;
pub use specs_data::cmd_specs_data;
pub use stubify::cmd_stubify;
pub use tracked_csv::cmd_tracked_csv;

// Re-export types needed by main.rs
pub use functions::OutputFormat;
