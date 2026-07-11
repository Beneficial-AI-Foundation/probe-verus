//! Atomize command - Generate call graph atoms from SCIP indexes.

use crate::{
    add_external_stubs, backfill_atoms_from_parser, build_call_graph, build_module_visibility_map,
    convert_to_atoms_with_parsed_spans, find_duplicate_code_names, is_library_crate,
    metadata::{gather_metadata, get_default_output_path, wrap_in_envelope, AtomizeInternalConfig},
    parse_scip_json, public_api, resolve_package_root, resolve_workspace_root,
    scip_cache::{Analyzer, ScipCache},
    AtomWithLines,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Execute the atomize command.
///
/// Generates call graph atoms with line numbers from SCIP indexes.
#[allow(clippy::too_many_arguments)]
pub fn cmd_atomize(
    project_path: PathBuf,
    output: Option<PathBuf>,
    regenerate_scip: bool,
    with_locations: bool,
    use_rust_analyzer: bool,
    allow_duplicates: bool,
    auto_install: bool,
    with_public_api: bool,
) -> Result<(), String> {
    if auto_install {
        eprintln!(
            "Warning: --auto-install is deprecated and will be removed in a future major version."
        );
        eprintln!("  Use instead: probe-verus setup --from-project <project-path>");
        eprintln!();
    }

    println!("═══════════════════════════════════════════════════════════");
    println!("  Probe Verus - Atomize: Generate Call Graph Data");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    // Validate project
    validate_project(&project_path)?;

    let project_path = resolve_workspace_root(&project_path, None)?;
    println!("  ✓ Valid Rust project found");

    // Get or generate SCIP JSON
    let analyzer = if use_rust_analyzer {
        Analyzer::RustAnalyzer
    } else {
        Analyzer::VerusAnalyzer
    };
    let mut scip_cache =
        ScipCache::with_analyzer(&project_path, analyzer).with_auto_install(auto_install);
    let json_path = get_scip_json(&mut scip_cache, regenerate_scip)?;

    // Parse SCIP JSON and build call graph
    println!("Parsing SCIP JSON and building call graph...");

    let scip_index = parse_scip_json(json_path.to_string_lossy().as_ref())
        .map_err(|e| format!("Failed to parse SCIP JSON: {}", e))?;

    let (call_graph, symbol_to_display_name) = build_call_graph(&scip_index);
    println!("  ✓ Call graph built with {} functions", call_graph.len());
    println!();

    let pkg_root = resolve_package_root(&project_path, None);
    let file_module_pub = build_module_visibility_map(&pkg_root);
    let is_library = is_library_crate(&pkg_root);

    // Gather metadata early so we can use pkg_name for RQN derivation
    let metadata = gather_metadata(&project_path);

    let code_path_prefix = pkg_root
        .strip_prefix(&project_path)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .to_string();

    // Convert to atoms format with line numbers
    println!("Converting to atoms format with accurate line numbers...");
    println!("  Parsing source files with verus_syn for accurate function spans...");

    let atoms = convert_to_atoms_with_parsed_spans(
        &call_graph,
        &symbol_to_display_name,
        &pkg_root,
        with_locations,
        &file_module_pub,
        is_library,
        &code_path_prefix,
        &metadata.pkg_name,
    );
    println!("  ✓ Converted {} functions to atoms format", atoms.len());
    if with_locations {
        println!("    (including dependencies-with-locations)");
    }

    // Check for duplicate code_names
    let duplicates = find_duplicate_code_names(&atoms);
    if !duplicates.is_empty() {
        let report = format_duplicate_report(&duplicates);
        if allow_duplicates {
            eprintln!();
            eprintln!("{}", report);
            eprintln!(
                "    Continuing because --allow-duplicates was specified.\n    \
                 Duplicate entries will be dropped (first occurrence kept)."
            );
        } else {
            eprintln!();
            eprintln!("{}", report);
            return Err(format!("Found {} duplicate code_name(s)", duplicates.len()));
        }
    }

    // Convert atoms list to dictionary keyed by code_name (first occurrence wins)
    let mut atoms_dict: BTreeMap<String, AtomWithLines> = BTreeMap::new();
    for atom in atoms {
        atoms_dict.entry(atom.code_name.clone()).or_insert(atom);
    }

    // Add stub atoms for external function dependencies
    let stub_count = add_external_stubs(&mut atoms_dict);
    if stub_count > 0 {
        println!("  ✓ Added {} external function stub(s)", stub_count);
    }

    // Backfill atoms for functions that SCIP missed (e.g. cfg-gated verus! blocks)
    let backfill_count = backfill_atoms_from_parser(
        &pkg_root,
        &mut atoms_dict,
        &metadata.pkg_name,
        &metadata.pkg_version,
        &file_module_pub,
        is_library,
        &code_path_prefix,
    );
    if backfill_count > 0 {
        println!(
            "  ✓ Backfilled {} atom(s) from verus_parser (not found in SCIP index)",
            backfill_count
        );
    }

    if with_public_api {
        match public_api::run_cargo_public_api(&pkg_root) {
            Ok(public_rqns) => {
                let (matched, overridden) =
                    public_api::enrich_atoms_with_public_api(&mut atoms_dict, &public_rqns);
                println!(
                    "  ✓ cargo public-api: {} public RQNs, {matched} atoms matched, {overridden} overridden",
                    public_rqns.len()
                );
            }
            Err(e) => {
                eprintln!("  ⚠ cargo public-api failed: {e}");
                eprintln!("    Falling back to SCIP-walk is-public-api values");
            }
        }
    }

    let output =
        output.unwrap_or_else(|| get_default_output_path(&project_path, &metadata, "atoms"));

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
    }

    // Wrap in envelope and write
    let envelope = wrap_in_envelope("probe-verus/atoms", "atomize", &atoms_dict, &metadata);
    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    std::fs::write(&output, &json).map_err(|e| format!("Failed to write output file: {}", e))?;

    // Print success summary
    print_success_summary(&output, &atoms_dict);
    Ok(())
}

/// Validate that the project path exists and contains a Cargo.toml.
fn validate_project(project_path: &Path) -> Result<(), String> {
    if !project_path.exists() {
        return Err(format!(
            "Project path does not exist: {}",
            project_path.display()
        ));
    }

    let cargo_toml = project_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(format!(
            "Not a valid Rust project (Cargo.toml not found): {}",
            project_path.display()
        ));
    }

    Ok(())
}

/// Get the SCIP JSON path, generating if necessary.
fn get_scip_json(cache: &mut ScipCache, regenerate: bool) -> Result<PathBuf, String> {
    if cache.has_current_cached_json() && !regenerate {
        println!(
            "  ✓ Found existing SCIP JSON at {}",
            cache.json_path().display()
        );
        println!("    (use --regenerate-scip to force regeneration)");
        println!();
        return Ok(cache.json_path());
    }

    // Need to generate
    let reason = cache.generation_reason(regenerate);
    println!("Generating SCIP index {}...", reason);
    println!("  (This may take a while for large projects)");

    let path = cache
        .get_or_generate(regenerate, true)
        .map_err(|e| e.to_string())?;
    println!();
    Ok(path)
}

/// Format a human-readable report of duplicate code_names.
fn format_duplicate_report(duplicates: &[crate::DuplicateCodeName]) -> String {
    let mut msg = format!(
        "WARNING: Found {} duplicate code_name(s):\n",
        duplicates.len()
    );
    for dup in duplicates {
        msg.push_str(&format!("    - '{}'\n", dup.code_name));
        for occ in &dup.occurrences {
            msg.push_str(&format!(
                "      at {}:{} ({})\n",
                occ.code_path, occ.lines_start, occ.display_name
            ));
        }
    }
    msg.push_str("\n    Duplicate code_names cannot be used as dictionary keys.\n");
    msg.push_str("    This may indicate trait implementations that cannot be distinguished.\n");
    msg.push_str("    Use --allow-duplicates to continue anyway (first occurrence kept).");
    msg
}

/// Print the success summary.
fn print_success_summary(output: &Path, atoms_dict: &BTreeMap<String, AtomWithLines>) {
    println!();
    println!("═══════════════════════════════════════════════════════════");
    println!("  ✓ SUCCESS");
    println!("═══════════════════════════════════════════════════════════");
    println!();
    println!("Output written to: {}", output.display());
    println!();
    println!("Summary:");
    println!("  - Total functions: {}", atoms_dict.len());
    println!(
        "  - Total dependencies: {}",
        atoms_dict
            .values()
            .map(|a| a.dependencies.len())
            .sum::<usize>()
    );
    println!("  - Output format: dictionary keyed by code_name");
    println!();
}

/// Internal atomize implementation that returns Result for better error handling.
/// Used by the `run` command (which pre-gathers metadata to share a timestamp).
pub fn atomize_internal(config: &AtomizeInternalConfig) -> Result<usize, String> {
    let project_path = resolve_workspace_root(config.project_path, config.package)?;

    let analyzer = if config.use_rust_analyzer {
        Analyzer::RustAnalyzer
    } else {
        Analyzer::VerusAnalyzer
    };
    let mut cache =
        ScipCache::with_analyzer(&project_path, analyzer).with_auto_install(config.auto_install);

    let json_path = cache
        .get_or_generate(config.regenerate_scip, config.verbose)
        .map_err(|e| e.to_string())?;

    let scip_index = parse_scip_json(json_path.to_string_lossy().as_ref())
        .map_err(|e| format!("Failed to parse SCIP JSON: {}", e))?;

    let (call_graph, symbol_to_display_name) = build_call_graph(&scip_index);

    let pkg_root = resolve_package_root(&project_path, config.package);
    let file_module_pub = build_module_visibility_map(&pkg_root);
    let is_library = is_library_crate(&pkg_root);

    let code_path_prefix = pkg_root
        .strip_prefix(&project_path)
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .to_string();

    let atoms = convert_to_atoms_with_parsed_spans(
        &call_graph,
        &symbol_to_display_name,
        &pkg_root,
        config.with_locations,
        &file_module_pub,
        is_library,
        &code_path_prefix,
        &config.metadata.pkg_name,
    );

    let duplicates = find_duplicate_code_names(&atoms);
    if !duplicates.is_empty() {
        if config.allow_duplicates {
            eprintln!(
                "Warning: Found {} duplicate code_name(s) (continuing with --allow-duplicates)",
                duplicates.len()
            );
        } else {
            return Err(format!("Found {} duplicate code_name(s)", duplicates.len()));
        }
    }

    let mut atoms_dict: BTreeMap<String, AtomWithLines> = BTreeMap::new();
    for atom in atoms {
        atoms_dict.entry(atom.code_name.clone()).or_insert(atom);
    }

    add_external_stubs(&mut atoms_dict);

    backfill_atoms_from_parser(
        &pkg_root,
        &mut atoms_dict,
        &config.metadata.pkg_name,
        &config.metadata.pkg_version,
        &file_module_pub,
        is_library,
        &code_path_prefix,
    );

    if config.with_public_api {
        match public_api::run_cargo_public_api(&pkg_root) {
            Ok(public_rqns) => {
                let (matched, overridden) =
                    public_api::enrich_atoms_with_public_api(&mut atoms_dict, &public_rqns);
                eprintln!(
                    "  ✓ cargo public-api: {} public RQNs, {matched} atoms matched, {overridden} overridden",
                    public_rqns.len()
                );
            }
            Err(e) => {
                eprintln!("  ⚠ cargo public-api failed: {e}");
                eprintln!("    Falling back to SCIP-walk is-public-api values");
            }
        }
    }

    let count = atoms_dict.len();

    let envelope = wrap_in_envelope("probe-verus/atoms", "atomize", &atoms_dict, config.metadata);

    if let Some(parent) = config.output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
    }

    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    std::fs::write(config.output, &json).map_err(|e| format!("Failed to write output: {}", e))?;

    Ok(count)
}
