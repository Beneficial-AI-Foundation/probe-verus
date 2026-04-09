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

// NOTE: The live extract structural check has been merged into extract_backward_compat
// (issue #23). Both golden comparison and structural validation now run in a single
// extract pass. See tests/extract_backward_compat.rs.
