//! `setup` subcommand: install and manage external tool dependencies.

use crate::tool_manager;

pub fn cmd_setup(status: bool) {
    if status {
        tool_manager::print_status();
        return;
    }

    eprintln!("Installing external tools for probe-verus...\n");

    let errors = tool_manager::install_all();

    if errors.is_empty() {
        eprintln!("\nAll tools installed successfully.");
        println!();
        tool_manager::print_status();
    } else {
        for e in &errors {
            eprintln!("Error: {e}");
        }
        eprintln!(
            "\n{} tool(s) failed to install. See errors above.",
            errors.len()
        );
        std::process::exit(1);
    }
}
