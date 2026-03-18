//! Integration tests that validate probe-verus extract output using probe-extract-check.

use probe_extract_check::{check_all, load_extract_json};
use std::path::Path;

/// Validate the existing unified_test atoms fixture structurally.
///
/// This loads the test fixture atoms.json and runs structural checks
/// (envelope fields, line ranges, referential integrity).
#[test]
fn fixture_atoms_structural_check() {
    let json_path = Path::new("tests/fixtures/unified_test/atoms.json");
    let envelope = load_extract_json(json_path)
        .unwrap_or_else(|e| panic!("failed to load fixture atoms: {e}"));

    let report = check_all(&envelope, None);

    for d in report.errors() {
        eprintln!("{d}");
    }
    assert!(
        report.is_ok(),
        "structural check found {} error(s)",
        report.error_count()
    );
}

/// Validate that fixture atoms have well-formed keys.
#[test]
fn fixture_atoms_keys_have_probe_prefix() {
    let json_path = Path::new("tests/fixtures/unified_test/atoms.json");
    let envelope = load_extract_json(json_path).unwrap();

    let non_probe_keys: Vec<_> = envelope
        .data
        .keys()
        .filter(|k| !k.starts_with("probe:"))
        .collect();
    assert!(
        non_probe_keys.is_empty(),
        "found atom keys without 'probe:' prefix: {:?}",
        non_probe_keys
    );
}

/// Validate that fixture atoms have valid Verus-specific kinds.
#[test]
fn fixture_atoms_have_valid_kinds() {
    let json_path = Path::new("tests/fixtures/unified_test/atoms.json");
    let envelope = load_extract_json(json_path).unwrap();

    let valid_kinds = ["exec", "proof", "spec"];
    for (key, atom) in &envelope.data {
        if atom.is_stub() {
            continue;
        }
        assert!(
            valid_kinds.contains(&atom.kind.as_str()),
            "atom {key} has unexpected kind '{}', expected one of {valid_kinds:?}",
            atom.kind
        );
    }
}

/// Run extraction via the library API and validate the output.
///
/// Requires `verus-analyzer` (or `rust-analyzer`), `scip`, and `verus` to be installed.
#[test]
#[ignore]
fn live_extract_structural_check() {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().to_path_buf();

    let fixture = Path::new("../probe/probe-extract-check/tests/fixtures/verus_micro");
    if !fixture.exists() {
        panic!("verus_micro fixture not found at {}", fixture.display());
    }

    probe_verus::commands::cmd_extract(
        fixture.to_path_buf(),
        output_dir.clone(),
        false,
        false,
        false,
        None,
        false,
        false,
        false,
        false,
        true,
        None,
        false,
        None,
        vec![],
        false,
    )
    .expect("probe-verus extract failed");

    // Find the unified output file in the output directory.
    let unified = std::fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("verus_") && n.ends_with(".json"))
        })
        .unwrap_or_else(|| panic!("no unified output found in {}", output_dir.display()));

    let envelope = load_extract_json(&unified.path()).unwrap();
    let report = check_all(&envelope, Some(fixture));

    report.print_summary();
    assert!(
        report.is_ok(),
        "live extract check found {} error(s)",
        report.error_count()
    );
}
