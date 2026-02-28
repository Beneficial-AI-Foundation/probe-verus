//! Integration tests for the merge-atoms command.
//!
//! Uses pre-generated fixture files that simulate independently-indexed atoms:
//! - atoms_a.json: crate-a with stubs for crate-b functions
//! - atoms_b.json: crate-b with real function entries
//! - atoms_combined.json: expected result after merging

use probe_verus::AtomWithLines;
use std::collections::BTreeMap;
use std::process::Command;

const FIXTURES: &str = "tests/fixtures/merge_test";

fn load_atoms(path: &str) -> BTreeMap<String, AtomWithLines> {
    let content =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));
    let mut atoms: BTreeMap<String, AtomWithLines> = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path, e));
    for (key, atom) in atoms.iter_mut() {
        atom.code_name = key.clone();
    }
    atoms
}

#[test]
fn test_merge_fixtures_match_expected() {
    let binary = env!("CARGO_BIN_EXE_probe-verus");
    let output_path = std::env::temp_dir().join("merge_test_output.json");

    let status = Command::new(binary)
        .args([
            "merge-atoms",
            &format!("{}/atoms_a.json", FIXTURES),
            &format!("{}/atoms_b.json", FIXTURES),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run probe-verus");

    assert!(status.success(), "merge-atoms command failed");

    let merged = load_atoms(output_path.to_str().unwrap());
    let expected = load_atoms(&format!("{}/atoms_combined.json", FIXTURES));

    assert_eq!(
        merged.len(),
        expected.len(),
        "Different number of atoms: merged={}, expected={}",
        merged.len(),
        expected.len()
    );

    for (key, expected_atom) in &expected {
        let merged_atom = merged
            .get(key)
            .unwrap_or_else(|| panic!("Missing key in merged output: {}", key));

        assert_eq!(
            merged_atom.display_name, expected_atom.display_name,
            "display-name mismatch for {}",
            key
        );
        assert_eq!(
            merged_atom.code_path, expected_atom.code_path,
            "code-path mismatch for {}",
            key
        );
        assert_eq!(
            merged_atom.code_text.lines_start, expected_atom.code_text.lines_start,
            "lines-start mismatch for {}",
            key
        );
        assert_eq!(
            merged_atom.code_text.lines_end, expected_atom.code_text.lines_end,
            "lines-end mismatch for {}",
            key
        );
        assert_eq!(
            merged_atom.mode, expected_atom.mode,
            "mode mismatch for {}",
            key
        );
        assert_eq!(
            merged_atom.dependencies, expected_atom.dependencies,
            "dependencies mismatch for {}",
            key
        );
        assert_eq!(
            merged_atom.code_module, expected_atom.code_module,
            "code-module mismatch for {}",
            key
        );
    }

    std::fs::remove_file(&output_path).ok();
}

#[test]
fn test_merge_stubs_replaced_with_real_atoms() {
    let atoms_a = load_atoms(&format!("{}/atoms_a.json", FIXTURES));
    let atoms_b = load_atoms(&format!("{}/atoms_b.json", FIXTURES));

    let compute_a = atoms_a.get("probe:crate-b/1.0/helpers/compute()").unwrap();
    assert!(
        compute_a.code_path.is_empty(),
        "compute in atoms_a should be a stub"
    );

    let compute_b = atoms_b.get("probe:crate-b/1.0/helpers/compute()").unwrap();
    assert!(
        !compute_b.code_path.is_empty(),
        "compute in atoms_b should be real"
    );

    let binary = env!("CARGO_BIN_EXE_probe-verus");
    let output_path = std::env::temp_dir().join("merge_test_stubs.json");

    Command::new(binary)
        .args([
            "merge-atoms",
            &format!("{}/atoms_a.json", FIXTURES),
            &format!("{}/atoms_b.json", FIXTURES),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run probe-verus");

    let merged = load_atoms(output_path.to_str().unwrap());
    let compute_merged = merged.get("probe:crate-b/1.0/helpers/compute()").unwrap();
    assert_eq!(compute_merged.code_path, "crate-b/src/helpers.rs");
    assert_eq!(compute_merged.mode, probe_verus::FunctionMode::Spec);

    std::fs::remove_file(&output_path).ok();
}

#[test]
fn test_merge_cross_project_edges_preserved() {
    let binary = env!("CARGO_BIN_EXE_probe-verus");
    let output_path = std::env::temp_dir().join("merge_test_edges.json");

    Command::new(binary)
        .args([
            "merge-atoms",
            &format!("{}/atoms_a.json", FIXTURES),
            &format!("{}/atoms_b.json", FIXTURES),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run probe-verus");

    let merged = load_atoms(output_path.to_str().unwrap());

    let main_fn = merged.get("probe:crate-a/1.0/lib/main()").unwrap();
    assert!(
        main_fn
            .dependencies
            .contains("probe:crate-b/1.0/helpers/compute()"),
        "main() should depend on compute()"
    );

    let process_fn = merged.get("probe:crate-a/1.0/lib/process()").unwrap();
    assert!(
        process_fn
            .dependencies
            .contains("probe:crate-b/1.0/helpers/validate()"),
        "process() should depend on validate()"
    );

    std::fs::remove_file(&output_path).ok();
}
