//! `setup` subcommand: install and manage external tool dependencies.

use std::path::{Path, PathBuf};

use crate::analyzer_selection;
use crate::metadata;
use crate::tool_manager;

pub fn cmd_setup(
    status: bool,
    from_project: Option<PathBuf>,
    package: Option<String>,
    detect_version: bool,
    detect_toolchain: bool,
    skip_toolchain: bool,
    force: bool,
) {
    // Validate mutually exclusive flags
    if status && (detect_version || detect_toolchain) {
        eprintln!("Error: --status cannot be combined with --detect-version or --detect-toolchain");
        std::process::exit(1);
    }
    if detect_version && from_project.is_none() {
        eprintln!("Error: --detect-version requires --from-project");
        std::process::exit(1);
    }
    if package.is_some() && from_project.is_none() {
        eprintln!("Error: --package requires --from-project");
        std::process::exit(1);
    }

    // Only --status and the install use the analyzer selection, so an invalid
    // pin does not break --detect-version or --detect-toolchain.
    let select_analyzer = || {
        analyzer_for_setup(from_project.as_deref(), package.as_deref()).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(1);
        })
    };

    if status {
        tool_manager::print_status(&select_analyzer());
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

    let analyzer = select_analyzer();
    eprintln!("Installing external tools for probe-verus...\n");

    let errors = tool_manager::install_all(force, &analyzer);

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
    tool_manager::print_status(&analyzer);
}

/// The verus-analyzer that `extract` and `atomize` select for `project` and
/// `package` (`-p`): the same lookup on the same package directory. Without
/// `package`, a workspace root with several members has no package, so only
/// its workspace pin applies.
fn analyzer_for_setup(
    project: Option<&Path>,
    package: Option<&str>,
) -> Result<analyzer_selection::Selection, String> {
    match project {
        Some(p) => {
            let dir = match package {
                Some(_) => crate::resolve_workspace_root(p, package)?,
                None => crate::resolve_workspace_root(p, None).unwrap_or_else(|_| p.to_path_buf()),
            };
            analyzer_selection::select(&dir)
        }
        None => analyzer_selection::select_without_project(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer_selection::{Selector, ENV_MUTEX};

    #[test]
    fn setup_selects_the_same_analyzer_as_extract() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\"]\n\
             [workspace.metadata.probe-verus]\nverus-analyzer = \"2026-10-01\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(
            root.join("a/Cargo.toml"),
            "[package]\nname = \"a\"\n\
             [package.metadata.probe-verus]\nverus-analyzer = \"2026-11-01\"\n",
        )
        .unwrap();

        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var(analyzer_selection::VERUS_ANALYZER_VERSION_ENV) };
        // extract resolves the single member, then selects.
        let package_dir = crate::resolve_workspace_root(root, None).unwrap();
        let extract = analyzer_selection::select(&package_dir).unwrap();
        assert_eq!(extract.selector, Selector::Release("2026-11-01".into()));
        assert_eq!(analyzer_for_setup(Some(root), None).unwrap(), extract);
        assert_eq!(
            analyzer_for_setup(Some(&root.join("a")), None).unwrap(),
            extract
        );
    }

    #[test]
    fn setup_package_selects_the_member_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\"]\n\
             [workspace.metadata.probe-verus]\nverus-analyzer = \"2026-10-01\"\n",
        )
        .unwrap();
        for (name, pin) in [("a", "2026-11-01"), ("b", "")] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            let pin = if pin.is_empty() {
                String::new()
            } else {
                format!("[package.metadata.probe-verus]\nverus-analyzer = \"{pin}\"\n")
            };
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\n{pin}"),
            )
            .unwrap();
        }

        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var(analyzer_selection::VERUS_ANALYZER_VERSION_ENV) };
        let release = |package| analyzer_for_setup(Some(root), package).unwrap().selector;
        assert_eq!(release(None), Selector::Release("2026-10-01".into()));
        assert_eq!(release(Some("a")), Selector::Release("2026-11-01".into()));
        assert_eq!(release(Some("b")), Selector::Release("2026-10-01".into()));
        assert!(analyzer_for_setup(Some(root), Some("missing")).is_err());
    }
}
