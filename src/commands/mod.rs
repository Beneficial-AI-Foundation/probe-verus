//! Command implementations for probe-verus CLI.
//!
//! This module contains the implementation of each CLI subcommand:
//! - `atomize`: Generate call graph atoms from SCIP indexes
//! - `verify`: Run Verus verification and analyze results
//! - `functions`: List all functions in a project
//! - `specify`: Extract function specifications to JSON
//! - `run`: Run both atomize and verify (for CI/Docker)

mod atomize;
mod functions;
mod run;
mod specify;
mod verify;

pub use atomize::cmd_atomize;
pub use functions::cmd_functions;
pub use run::cmd_run;
pub use specify::cmd_specify;
pub use verify::cmd_verify;

// Re-export types needed by main.rs
pub use functions::OutputFormat;
