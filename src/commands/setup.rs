//! `setup` subcommand: install and manage external tool dependencies.

use std::path::PathBuf;

use crate::metadata;
use crate::tool_manager;

pub fn cmd_setup(
    status: bool,
    from_project: Option<PathBuf>,
    detect_version: bool,
    detect_toolchain: bool,
    skip_toolchain: bool,
) {
    // Validate mutually exclusive flags
    if status && (from_project.is_some() || detect_version || detect_toolchain) {
        eprintln!(
            "Error: --status cannot be combined with --from-project, --detect-version, or --detect-toolchain"
        );
        std::process::exit(1);
    }
    if detect_version && from_project.is_none() {
        eprintln!("Error: --detect-version requires --from-project");
        std::process::exit(1);
    }

    if status {
        tool_manager::print_status();
        return;
    }

    // --detect-toolchain: resolve Verus version, fetch its rust-toolchain.toml, print channel
    if detect_toolchain {
        let verus_version = if let Some(ref project_path) = from_project {
            let env_version = std::env::var(tool_manager::VERUS_VERSION_ENV)
                .ok()
                .filter(|v| !v.is_empty());
            env_version
                .or_else(|| metadata::detect_verus_version(project_path))
                .unwrap_or_else(|| {
                    eprintln!("Error: no Verus version found");
                    std::process::exit(1);
                })
        } else {
            tool_manager::resolve_verus_version(None).tag
        };

        match tool_manager::fetch_verus_rust_toolchain(&verus_version) {
            Some(info) => {
                println!("{}", info.channel);
            }
            None => {
                eprintln!("Error: could not fetch rust-toolchain.toml for Verus {verus_version}");
                std::process::exit(1);
            }
        }
        return;
    }

    // When --from-project is given, detect the Verus version from the project
    if let Some(ref project_path) = from_project {
        if !project_path.exists() {
            eprintln!(
                "Error: project path does not exist: {}",
                project_path.display()
            );
            std::process::exit(1);
        }

        let cargo_toml = project_path.join("Cargo.toml");
        if !cargo_toml.exists() {
            eprintln!("Error: Cargo.toml not found at {}", cargo_toml.display());
            std::process::exit(1);
        }

        // Try env var first, then Cargo.toml detection
        let env_version = std::env::var(tool_manager::VERUS_VERSION_ENV)
            .ok()
            .filter(|v| !v.is_empty());

        let detected_version = env_version.or_else(|| metadata::detect_verus_version(project_path));

        if detect_version {
            // --detect-version: just print and exit
            match detected_version {
                Some(v) => {
                    println!("{v}");
                }
                None => {
                    eprintln!(
                        "Error: no Verus version found in {}",
                        project_path.display()
                    );
                    eprintln!("  Add [package.metadata.verus] release = \"...\" to Cargo.toml,");
                    eprintln!("  or use a vstd/verus_builtin dependency with a rev matching a Verus release,");
                    eprintln!("  or set {}", tool_manager::VERUS_VERSION_ENV);
                    std::process::exit(1);
                }
            }
            return;
        }

        // --from-project install mode: version must be detectable
        match detected_version {
            Some(v) => {
                eprintln!("Detected Verus version: {v}");
                // Set env var so resolve_verus_version picks it up
                unsafe { std::env::set_var(tool_manager::VERUS_VERSION_ENV, &v) };
            }
            None => {
                eprintln!(
                    "Error: no Verus version found in {}",
                    project_path.display()
                );
                eprintln!("  Add [package.metadata.verus] release = \"...\" to Cargo.toml,");
                eprintln!(
                    "  or use a vstd/verus_builtin dependency with a rev matching a Verus release,"
                );
                eprintln!("  or set {}", tool_manager::VERUS_VERSION_ENV);
                std::process::exit(1);
            }
        }
    }

    eprintln!("Installing external tools for probe-verus...\n");

    let errors = tool_manager::install_all();

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("Error: {e}");
        }
        eprintln!(
            "\n{} tool(s) failed to install. See errors above.",
            errors.len()
        );
        std::process::exit(1);
    }

    // After tools are installed, ensure the matching Rust toolchain is present.
    if !skip_toolchain {
        let verus_version = tool_manager::resolve_verus_version(None).tag;
        eprintln!();
        match tool_manager::ensure_rust_toolchain(&verus_version) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Warning: Rust toolchain setup failed: {e}");
                eprintln!(
                    "  Verus verification may fail if the correct toolchain is not installed."
                );
                eprintln!("  Use --skip-toolchain to suppress this step.");
            }
        }
    }

    eprintln!("\nAll tools installed successfully.");
    println!();
    tool_manager::print_status();
}
