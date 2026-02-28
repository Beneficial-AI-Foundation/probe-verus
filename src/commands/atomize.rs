//! Atomize command - Generate call graph atoms from SCIP indexes.

use probe_verus::{
    add_external_stubs, build_call_graph, convert_to_atoms_with_parsed_spans,
    find_duplicate_code_names, parse_scip_json,
    scip_cache::{Analyzer, ScipCache},
    AtomWithLines,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Execute the atomize command.
///
/// Generates call graph atoms with line numbers from SCIP indexes.
pub fn cmd_atomize(
    project_path: PathBuf,
    output: PathBuf,
    regenerate_scip: bool,
    with_locations: bool,
    use_rust_analyzer: bool,
    allow_duplicates: bool,
) {
    println!("═══════════════════════════════════════════════════════════");
    println!("  Probe Verus - Atomize: Generate Call Graph Data");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    // Validate project
    if let Err(msg) = validate_project(&project_path) {
        eprintln!("✗ Error: {}", msg);
        std::process::exit(1);
    }
    println!("  ✓ Valid Rust project found");

    // Get or generate SCIP JSON
    let analyzer = if use_rust_analyzer {
        Analyzer::RustAnalyzer
    } else {
        Analyzer::VerusAnalyzer
    };
    let scip_cache = ScipCache::with_analyzer(&project_path, analyzer);
    let json_path = get_scip_json(&scip_cache, regenerate_scip);

    // Parse SCIP JSON and build call graph
    println!("Parsing SCIP JSON and building call graph...");

    let scip_index = match parse_scip_json(json_path.to_str().unwrap()) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("✗ Failed to parse SCIP JSON: {}", e);
            std::process::exit(1);
        }
    };

    let (call_graph, symbol_to_display_name) = build_call_graph(&scip_index);
    println!("  ✓ Call graph built with {} functions", call_graph.len());
    println!();

    // Convert to atoms format with line numbers
    println!("Converting to atoms format with accurate line numbers...");
    println!("  Parsing source files with verus_syn for accurate function spans...");

    let atoms = convert_to_atoms_with_parsed_spans(
        &call_graph,
        &symbol_to_display_name,
        &project_path,
        with_locations,
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
            std::process::exit(1);
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

    // Write the output
    let json = serde_json::to_string_pretty(&atoms_dict).expect("Failed to serialize JSON");
    std::fs::write(&output, &json).expect("Failed to write output file");

    // Print success summary
    print_success_summary(&output, &atoms_dict);
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
fn get_scip_json(cache: &ScipCache, regenerate: bool) -> PathBuf {
    if cache.has_cached_json() && !regenerate {
        println!(
            "  ✓ Found existing SCIP JSON at {}",
            cache.json_path().display()
        );
        println!("    (use --regenerate-scip to force regeneration)");
        println!();
        return cache.json_path();
    }

    // Need to generate
    let reason = cache.generation_reason(regenerate);
    println!("Generating SCIP index {}...", reason);
    println!("  (This may take a while for large projects)");

    match cache.get_or_generate(regenerate, true) {
        Ok(path) => {
            println!();
            path
        }
        Err(e) => {
            eprintln!("✗ Error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Format a human-readable report of duplicate code_names.
fn format_duplicate_report(duplicates: &[probe_verus::DuplicateCodeName]) -> String {
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
/// Used by the `run` command.
pub fn atomize_internal(
    project_path: &PathBuf,
    output: &PathBuf,
    regenerate_scip: bool,
    verbose: bool,
    use_rust_analyzer: bool,
    allow_duplicates: bool,
) -> Result<usize, String> {
    let analyzer = if use_rust_analyzer {
        Analyzer::RustAnalyzer
    } else {
        Analyzer::VerusAnalyzer
    };
    let cache = ScipCache::with_analyzer(project_path, analyzer);

    // Get or generate SCIP JSON
    let json_path = cache
        .get_or_generate(regenerate_scip, verbose)
        .map_err(|e| e.to_string())?;

    // Parse and build call graph
    let scip_index = parse_scip_json(json_path.to_str().unwrap())
        .map_err(|e| format!("Failed to parse SCIP JSON: {}", e))?;

    let (call_graph, symbol_to_display_name) = build_call_graph(&scip_index);

    let atoms = convert_to_atoms_with_parsed_spans(
        &call_graph,
        &symbol_to_display_name,
        project_path,
        false,
    );

    // Check for duplicates
    let duplicates = find_duplicate_code_names(&atoms);
    if !duplicates.is_empty() {
        if allow_duplicates {
            eprintln!(
                "Warning: Found {} duplicate code_name(s) (continuing with --allow-duplicates)",
                duplicates.len()
            );
        } else {
            return Err(format!("Found {} duplicate code_name(s)", duplicates.len()));
        }
    }

    // Convert to dictionary (first occurrence wins)
    let mut atoms_dict: BTreeMap<String, AtomWithLines> = BTreeMap::new();
    for atom in atoms {
        atoms_dict.entry(atom.code_name.clone()).or_insert(atom);
    }

    // Add stub atoms for external function dependencies
    add_external_stubs(&mut atoms_dict);

    let count = atoms_dict.len();

    let json = serde_json::to_string_pretty(&atoms_dict)
        .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    std::fs::write(output, &json).map_err(|e| format!("Failed to write output: {}", e))?;

    Ok(count)
}
