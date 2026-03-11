//! Integration tests for the unified extract output.
//!
//! Verifies that merging atoms + specs + proofs into a UnifiedAtom dict
//! produces consistent results, using pre-built fixture files.

use probe_verus::metadata::unwrap_envelope;
use probe_verus::{AtomWithLines, UnifiedAtom};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

const FIXTURES: &str = "tests/fixtures/unified_test";

#[derive(Deserialize)]
struct SpecsEntryMinimal {
    specified: bool,
}

#[derive(Deserialize)]
struct ProofsEntryMinimal {
    status: String,
}

fn map_verification_status(status: &str) -> &'static str {
    match status {
        "success" => "verified",
        "failure" => "failed",
        "sorries" => "unverified",
        "warning" => "verified",
        _ => "failed",
    }
}

fn load_enveloped<T: serde::de::DeserializeOwned>(path: &Path) -> BTreeMap<String, T> {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    let json: serde_json::Value = serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));
    let data = unwrap_envelope(json);
    serde_json::from_value(data)
        .unwrap_or_else(|e| panic!("Failed to deserialize {}: {}", path.display(), e))
}

/// Merge atoms + specs + proofs into unified output (mirrors the logic in extract.rs).
fn merge_fixture_files(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
) -> BTreeMap<String, UnifiedAtom> {
    let atoms: BTreeMap<String, AtomWithLines> = load_enveloped(atoms_path);
    let specs: Option<BTreeMap<String, SpecsEntryMinimal>> = specs_path.map(load_enveloped);
    let proofs: Option<BTreeMap<String, ProofsEntryMinimal>> = proofs_path.map(load_enveloped);

    let mut unified = BTreeMap::new();
    for (code_name, atom) in atoms {
        let specified = specs
            .as_ref()
            .and_then(|s| s.get(&code_name))
            .map(|e| e.specified);
        let verification_status = proofs
            .as_ref()
            .and_then(|p| p.get(&code_name))
            .map(|e| map_verification_status(&e.status).to_string());
        unified.insert(
            code_name,
            UnifiedAtom {
                atom,
                verification_status,
                specified,
            },
        );
    }
    unified
}

#[test]
fn test_unified_keys_match_atoms() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let atoms: BTreeMap<String, AtomWithLines> = load_enveloped(&atoms_path);
    let unified = merge_fixture_files(&atoms_path, None, None);

    assert_eq!(atoms.len(), unified.len());
    for key in atoms.keys() {
        assert!(
            unified.contains_key(key),
            "Atom key missing from unified output: {}",
            key
        );
    }
}

#[test]
fn test_unified_specified_matches_specs() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let specs: BTreeMap<String, SpecsEntryMinimal> = load_enveloped(&specs_path);
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), None);

    for (code_name, spec_entry) in &specs {
        let unified_entry = unified
            .get(code_name)
            .unwrap_or_else(|| panic!("Spec key missing from unified output: {}", code_name));
        assert_eq!(
            unified_entry.specified,
            Some(spec_entry.specified),
            "Mismatch for {}: unified.specified={:?}, specs.specified={}",
            code_name,
            unified_entry.specified,
            spec_entry.specified
        );
    }
}

#[test]
fn test_unified_verification_status_matches_proofs() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let proofs_path = Path::new(FIXTURES).join("proofs.json");
    let proofs: BTreeMap<String, ProofsEntryMinimal> = load_enveloped(&proofs_path);
    let unified = merge_fixture_files(&atoms_path, None, Some(&proofs_path));

    for (code_name, proof_entry) in &proofs {
        let unified_entry = unified
            .get(code_name)
            .unwrap_or_else(|| panic!("Proof key missing from unified output: {}", code_name));
        let expected = map_verification_status(&proof_entry.status);
        assert_eq!(
            unified_entry.verification_status.as_deref(),
            Some(expected),
            "Mismatch for {}: unified.verification_status={:?}, expected={}",
            code_name,
            unified_entry.verification_status,
            expected
        );
    }
}

#[test]
fn test_external_stubs_have_no_enrichment() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let proofs_path = Path::new(FIXTURES).join("proofs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), Some(&proofs_path));

    let ext = &unified["probe:external/1.0.0/lib/ext()"];
    assert!(
        ext.specified.is_none(),
        "External stub should have no 'specified' field"
    );
    assert!(
        ext.verification_status.is_none(),
        "External stub should have no 'verification-status' field"
    );
}

#[test]
fn test_full_merge_all_fields_populated() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let proofs_path = Path::new(FIXTURES).join("proofs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), Some(&proofs_path));

    assert_eq!(unified.len(), 4);

    let foo = &unified["probe:test-crate/0.1.0/module/foo()"];
    assert_eq!(foo.atom.display_name, "foo");
    assert_eq!(foo.specified, Some(true));
    assert_eq!(foo.verification_status.as_deref(), Some("verified"));

    let bar = &unified["probe:test-crate/0.1.0/module/bar()"];
    assert_eq!(bar.atom.display_name, "bar");
    assert_eq!(bar.specified, Some(false));
    assert_eq!(bar.verification_status.as_deref(), Some("failed"));

    // baz has specs (specified=true) but no proofs entry
    let baz = &unified["probe:test-crate/0.1.0/module/baz()"];
    assert_eq!(baz.atom.display_name, "baz");
    assert_eq!(baz.specified, Some(true));
    assert!(baz.verification_status.is_none());
}

#[test]
fn test_unified_json_serialization_format() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let proofs_path = Path::new(FIXTURES).join("proofs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), Some(&proofs_path));

    let json = serde_json::to_value(&unified).unwrap();

    // foo: has both verification-status and specified
    let foo_json = &json["probe:test-crate/0.1.0/module/foo()"];
    assert_eq!(foo_json["display-name"], "foo");
    assert_eq!(foo_json["verification-status"], "verified");
    assert_eq!(foo_json["specified"], true);
    assert_eq!(foo_json["kind"], "exec");
    assert_eq!(foo_json["language"], "rust");

    // ext: no verification-status or specified (skip_serializing_if)
    let ext_json = &json["probe:external/1.0.0/lib/ext()"];
    assert_eq!(ext_json["display-name"], "ext");
    assert!(
        ext_json.get("verification-status").is_none(),
        "External stub should not have verification-status in JSON"
    );
    assert!(
        ext_json.get("specified").is_none(),
        "External stub should not have specified in JSON"
    );
}
