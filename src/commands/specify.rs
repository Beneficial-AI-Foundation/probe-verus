//! Specify command - Extract function specifications to JSON.

use probe_verus::constants::LINE_TOLERANCE;
use probe_verus::path_utils::{extract_src_suffix, paths_match_by_suffix};
use probe_verus::taxonomy;
use probe_verus::verus_parser::{self, FunctionInfo, ParsedOutput};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Atom entry from atoms.json for code-name lookup.
#[derive(Deserialize)]
struct AtomEntry {
    #[serde(rename = "display-name")]
    display_name: String,
    #[serde(rename = "code-path")]
    code_path: String,
    #[serde(rename = "code-text")]
    code_text: CodeText,
}

#[derive(Deserialize)]
struct CodeText {
    #[serde(rename = "lines-start")]
    lines_start: usize,
}

/// Output entry: FunctionInfo enriched with optional taxonomy labels.
#[derive(Serialize)]
struct SpecifyEntry {
    #[serde(flatten)]
    info: FunctionInfo,
    #[serde(rename = "spec-labels", skip_serializing_if = "Vec::is_empty")]
    spec_labels: Vec<String>,
}

/// Execute the specify command.
///
/// Extracts function specifications (requires/ensures) to JSON,
/// keyed by code-name from atoms.json.
pub fn cmd_specify(
    path: PathBuf,
    output: PathBuf,
    atoms_path: PathBuf,
    with_spec_text: bool,
    taxonomy_config_path: Option<PathBuf>,
    taxonomy_explain: bool,
) {
    // Validate inputs
    if !path.exists() {
        eprintln!("Error: Path does not exist: {}", path.display());
        std::process::exit(1);
    }

    if !atoms_path.exists() {
        eprintln!("Error: atoms.json not found at {}", atoms_path.display());
        std::process::exit(1);
    }

    // Load taxonomy config if provided
    let taxonomy_config = taxonomy_config_path.map(|tc_path| {
        if !tc_path.exists() {
            eprintln!("Error: taxonomy config not found at {}", tc_path.display());
            std::process::exit(1);
        }
        match taxonomy::load_taxonomy_config(&tc_path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    });

    // Load atoms.json to get code-name mappings
    let atoms = load_atoms(&atoms_path);

    // Parse all functions with spec info (requires/ensures)
    let parsed: ParsedOutput = verus_parser::parse_all_functions(
        &path,
        true,           // include_verus_constructs
        true,           // include_methods
        false,          // show_visibility
        false,          // show_kind
        with_spec_text, // include_spec_text
    );

    // Match functions to code-names and build output dictionary
    let (matched_map, matched_count, unmatched_count) = match_functions_to_atoms(parsed, &atoms);

    // Classify with taxonomy and build final output
    let output_map: BTreeMap<String, SpecifyEntry> = matched_map
        .into_iter()
        .map(|(code_name, func)| {
            // Print explain output if requested
            if taxonomy_explain {
                if let Some(config) = taxonomy_config.as_ref() {
                    let explanations = taxonomy::explain_function(&func, config);
                    let matched: Vec<_> = explanations.iter().filter(|e| e.matched).collect();
                    let missed: Vec<_> = explanations.iter().filter(|e| !e.matched).collect();

                    if !matched.is_empty() || func.specified {
                        eprintln!("  {}", code_name);
                        for exp in &matched {
                            eprintln!("    MATCHED {}", exp.label);
                        }
                        for exp in &missed {
                            let failed: Vec<_> = exp
                                .criteria_results
                                .iter()
                                .filter(|(_, p)| !p)
                                .map(|(name, _)| name.as_str())
                                .collect();
                            eprintln!("    missed  {} (failed: {})", exp.label, failed.join(", "));
                        }
                    }
                }
            }

            let spec_labels = taxonomy_config
                .as_ref()
                .map(|config| taxonomy::classify_function(&func, config))
                .unwrap_or_default();
            (
                code_name,
                SpecifyEntry {
                    info: func,
                    spec_labels,
                },
            )
        })
        .collect();

    // Write JSON output
    let json = serde_json::to_string_pretty(&output_map).expect("Failed to serialize JSON");
    std::fs::write(&output, &json).expect("Failed to write JSON output");

    // L3: Coverage summary
    if taxonomy_config.is_some() {
        let specified_total = output_map.values().filter(|e| e.info.specified).count();
        let specified_labeled = output_map
            .values()
            .filter(|e| e.info.specified && !e.spec_labels.is_empty())
            .count();
        let labeled_total = output_map
            .values()
            .filter(|e| !e.spec_labels.is_empty())
            .count();

        println!(
            "Wrote {} functions to {} ({} unmatched)",
            matched_count,
            output.display(),
            unmatched_count
        );
        if specified_total > 0 {
            println!(
                "Taxonomy: {}/{} specified functions classified ({:.0}%), {}/{} overall",
                specified_labeled,
                specified_total,
                100.0 * specified_labeled as f64 / specified_total as f64,
                labeled_total,
                matched_count,
            );
        } else {
            println!(
                "Taxonomy: {}/{} functions classified",
                labeled_total, matched_count
            );
        }
    } else {
        println!(
            "Wrote {} functions to {} ({} unmatched)",
            matched_count,
            output.display(),
            unmatched_count
        );
    }
}

/// Load atoms from a JSON file (BTreeMap for deterministic iteration order).
fn load_atoms(atoms_path: &PathBuf) -> BTreeMap<String, AtomEntry> {
    let atoms_content = std::fs::read_to_string(atoms_path).expect("Failed to read atoms.json");
    serde_json::from_str(&atoms_content).expect("Failed to parse atoms.json")
}

/// Match parsed functions to atoms by path and line number.
fn match_functions_to_atoms(
    parsed: ParsedOutput,
    atoms: &BTreeMap<String, AtomEntry>,
) -> (BTreeMap<String, FunctionInfo>, usize, usize) {
    let mut output_map: BTreeMap<String, FunctionInfo> = BTreeMap::new();
    let mut matched_count = 0;
    let mut unmatched_count = 0;

    for func in parsed.functions {
        if let Some(code_name) = find_matching_atom(&func, atoms) {
            output_map.insert(code_name, func);
            matched_count += 1;
        } else {
            unmatched_count += 1;
        }
    }

    (output_map, matched_count, unmatched_count)
}

/// Find the best matching atom for a function.
///
/// Matching strategy:
/// 1. Path must match (by suffix comparison)
/// 2. Name must match: either exact equality or the atom's display name
///    ends with `::func.name` (handles impl methods where SCIP enriches
///    display names to `Type::method` while verus_syn yields bare identifiers)
/// 3. SCIP line must fall within the function's span [start_line, end_line]
///    OR be within LINE_TOLERANCE of fn_line
///
/// Uses `fn_line` (the `fn` keyword line) for distance calculation since it
/// closely matches SCIP's definition line, unlike `spec_text.lines_start`
/// which includes preceding doc comments and attributes.
fn find_matching_atom(func: &FunctionInfo, atoms: &BTreeMap<String, AtomEntry>) -> Option<String> {
    let func_path = func.file.as_deref().unwrap_or("");
    let func_suffix = extract_src_suffix(func_path);

    let mut best_match: Option<&str> = None;
    let mut best_line_diff = usize::MAX;

    for (code_name, atom) in atoms {
        let atom_suffix = extract_src_suffix(&atom.code_path);

        let path_matches =
            paths_match_by_suffix(func_path, &atom.code_path) || func_suffix == atom_suffix;

        let name_matches = func.name == atom.display_name
            || atom.display_name.ends_with(&format!("::{}", func.name));

        if path_matches && name_matches {
            let atom_line = atom.code_text.lines_start;

            // Check if SCIP line falls within the function span [start_line, end_line]
            // This handles doc comments being included in verus_syn's span
            let within_span =
                atom_line >= func.spec_text.lines_start && atom_line <= func.spec_text.lines_end;

            let line_diff = (func.fn_line as isize - atom_line as isize).unsigned_abs();
            let within_tolerance = line_diff <= LINE_TOLERANCE;

            if within_span || within_tolerance {
                // Prefer matches closer to fn_line
                let effective_diff = if within_span && !within_tolerance {
                    (func.fn_line as isize - atom_line as isize).unsigned_abs()
                } else {
                    line_diff
                };

                if effective_diff < best_line_diff {
                    best_match = Some(code_name);
                    best_line_diff = effective_diff;

                    if effective_diff == 0 {
                        break;
                    }
                }
            }
        }
    }

    best_match.map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_verus::verus_parser::SpecText;
    use probe_verus::FunctionMode;

    fn make_func(
        name: &str,
        file: &str,
        fn_line: usize,
        span_start: usize,
        span_end: usize,
    ) -> FunctionInfo {
        FunctionInfo {
            name: name.to_string(),
            file: Some(file.to_string()),
            spec_text: SpecText {
                lines_start: span_start,
                lines_end: span_end,
            },
            mode: FunctionMode::Exec,
            kind: None,
            visibility: None,
            context: None,
            specified: false,
            has_requires: false,
            has_ensures: false,
            has_decreases: false,
            has_trusted_assumption: false,
            is_external_body: false,
            has_no_decreases_attr: false,
            requires_text: None,
            ensures_text: None,
            ensures_calls: vec![],
            requires_calls: vec![],
            ensures_calls_full: vec![],
            requires_calls_full: vec![],
            ensures_fn_calls: vec![],
            ensures_method_calls: vec![],
            requires_fn_calls: vec![],
            requires_method_calls: vec![],
            display_name: None,
            impl_type: None,
            doc_comment: None,
            signature_text: None,
            body_text: None,
            module_path: None,
            fn_line,
        }
    }

    fn make_atom(display_name: &str, code_path: &str, lines_start: usize) -> AtomEntry {
        AtomEntry {
            display_name: display_name.to_string(),
            code_path: code_path.to_string(),
            code_text: CodeText { lines_start },
        }
    }

    #[test]
    fn test_free_function_exact_match() {
        let func = make_func("decompress", "src/edwards.rs", 50, 48, 60);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/edwards/decompress()".to_string(),
            make_atom("decompress", "src/edwards.rs", 50),
        );
        let result = find_matching_atom(&func, &atoms);
        assert_eq!(
            result,
            Some("probe:crate/1.0/edwards/decompress()".to_string())
        );
    }

    #[test]
    fn test_inherent_impl_method_suffix_match() {
        let func = make_func("square", "src/field.rs", 100, 98, 120);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/field/FieldElement51#square()".to_string(),
            make_atom("FieldElement51::square", "src/field.rs", 100),
        );
        let result = find_matching_atom(&func, &atoms);
        assert_eq!(
            result,
            Some("probe:crate/1.0/field/FieldElement51#square()".to_string())
        );
    }

    #[test]
    fn test_trait_impl_method_suffix_match() {
        let func = make_func("add", "src/edwards.rs", 200, 198, 220);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/edwards/EdwardsPoint#Add#add()".to_string(),
            make_atom("EdwardsPoint::add", "src/edwards.rs", 200),
        );
        let result = find_matching_atom(&func, &atoms);
        assert_eq!(
            result,
            Some("probe:crate/1.0/edwards/EdwardsPoint#Add#add()".to_string())
        );
    }

    #[test]
    fn test_same_name_methods_disambiguated_by_line() {
        let func_a = make_func("add", "src/edwards.rs", 100, 98, 110);
        let func_b = make_func("add", "src/edwards.rs", 200, 198, 220);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/edwards/EdwardsPoint#Add#add()".to_string(),
            make_atom("EdwardsPoint::add", "src/edwards.rs", 100),
        );
        atoms.insert(
            "probe:crate/1.0/edwards/RistrettoPoint#Add#add()".to_string(),
            make_atom("RistrettoPoint::add", "src/edwards.rs", 200),
        );

        let result_a = find_matching_atom(&func_a, &atoms);
        assert_eq!(
            result_a,
            Some("probe:crate/1.0/edwards/EdwardsPoint#Add#add()".to_string())
        );

        let result_b = find_matching_atom(&func_b, &atoms);
        assert_eq!(
            result_b,
            Some("probe:crate/1.0/edwards/RistrettoPoint#Add#add()".to_string())
        );
    }

    #[test]
    fn test_no_match_when_path_differs() {
        let func = make_func("add", "src/ristretto.rs", 100, 98, 110);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/edwards/EdwardsPoint#Add#add()".to_string(),
            make_atom("EdwardsPoint::add", "src/edwards.rs", 100),
        );
        let result = find_matching_atom(&func, &atoms);
        assert_eq!(result, None);
    }

    #[test]
    fn test_no_match_when_line_too_far() {
        let func = make_func("add", "src/edwards.rs", 500, 498, 510);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/edwards/EdwardsPoint#Add#add()".to_string(),
            make_atom("EdwardsPoint::add", "src/edwards.rs", 100),
        );
        let result = find_matching_atom(&func, &atoms);
        assert_eq!(result, None);
    }

    #[test]
    fn test_fn_line_within_doc_comment_span() {
        // verus_syn span starts at doc comment (line 45), fn keyword at line 50
        let func = make_func("compress", "src/edwards.rs", 50, 45, 70);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:crate/1.0/edwards/EdwardsPoint#compress()".to_string(),
            make_atom("EdwardsPoint::compress", "src/edwards.rs", 50),
        );
        let result = find_matching_atom(&func, &atoms);
        assert_eq!(
            result,
            Some("probe:crate/1.0/edwards/EdwardsPoint#compress()".to_string())
        );
    }
}
