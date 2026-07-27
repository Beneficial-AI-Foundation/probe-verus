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
struct SpecsEntry {
    #[serde(default)]
    requires_text: Option<String>,
    #[serde(default)]
    ensures_text: Option<String>,
}

#[derive(Deserialize)]
struct ProofsEntryMinimal {
    status: String,
}

fn map_verification_status(status: &str) -> &'static str {
    match status {
        "success" => "verified",
        "failure" => "failed",
        "sorries" | "warning" => "unverified",
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

/// Merge atoms + specs + proofs into unified output by delegating to the real
/// pipeline logic in `extract.rs`. Using the production function (rather than a
/// hand-rolled copy) keeps these tests honest about scope/status/spec semantics
/// (KB P16/P24/P25): `untracked` is derived from out-of-scope conditions, and a
/// spec-less in-scope function is backlog (no `verification-status`).
fn merge_fixture_files(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
) -> BTreeMap<String, UnifiedAtom> {
    probe_verus::commands::merge_into_unified(atoms_path, specs_path, proofs_path)
        .expect("merge_into_unified should succeed on the fixtures")
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
fn test_unified_specs_matches_specs_file() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let specs: BTreeMap<String, SpecsEntry> = load_enveloped(&specs_path);
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), None);

    for (code_name, spec_entry) in &specs {
        let unified_entry = unified
            .get(code_name)
            .unwrap_or_else(|| panic!("Spec key missing from unified output: {}", code_name));
        let spec_text = unified_entry
            .primary_spec
            .as_ref()
            .expect("primary_spec field should be present");
        let has_specs = !spec_text.is_empty();
        let expected = spec_entry.requires_text.is_some() || spec_entry.ensures_text.is_some();
        assert_eq!(
            has_specs, expected,
            "Mismatch for {}: unified has_specs={}, expected={}",
            code_name, has_specs, expected
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
        ext.primary_spec.is_none(),
        "External stub should have no 'primary-spec' field"
    );
    assert_eq!(
        ext.untracked,
        Some(true),
        "External-crate stub is out of verification scope (KB P25): untracked = true"
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
    assert!(!foo.primary_spec.as_ref().unwrap().is_empty());
    assert_eq!(foo.untracked, Some(false));
    assert_eq!(foo.verification_status.as_deref(), Some("verified"));
    assert_eq!(foo.spec_labels, vec!["safety-critical", "arithmetic"]);

    let bar = &unified["probe:test-crate/0.1.0/module/bar()"];
    assert_eq!(bar.atom.display_name, "bar");
    assert_eq!(bar.primary_spec.as_deref(), Some(""));
    // bar is spec-less but in scope: backlog (KB P24) — untracked=false and no
    // verification-status, even though a proofs entry exists (a spec-less function
    // earns no status: there is no spec to have verified against).
    assert_eq!(bar.untracked, Some(false));
    assert!(bar.verification_status.is_none());
    assert!(bar.spec_labels.is_empty());

    // baz has specs (ensures only) but no proofs entry
    let baz = &unified["probe:test-crate/0.1.0/module/baz()"];
    assert_eq!(baz.atom.display_name, "baz");
    assert!(!baz.primary_spec.as_ref().unwrap().is_empty());
    assert_eq!(baz.untracked, Some(false));
    assert!(baz.verification_status.is_none());
    assert!(baz.spec_labels.is_empty());
}

#[test]
fn test_unified_json_serialization_format() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let proofs_path = Path::new(FIXTURES).join("proofs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), Some(&proofs_path));

    let json = serde_json::to_value(&unified).unwrap();

    // foo: has specs, verification-status, untracked=false, spec-labels
    let foo_json = &json["probe:test-crate/0.1.0/module/foo()"];
    assert_eq!(foo_json["display-name"], "foo");
    assert_eq!(foo_json["verification-status"], "verified");
    assert!(foo_json["primary-spec"].is_string());
    assert!(!foo_json["primary-spec"].as_str().unwrap().is_empty());
    assert_eq!(foo_json["untracked"], false);
    assert_eq!(foo_json["kind"], "exec");
    assert_eq!(foo_json["language"], "rust");
    let labels = foo_json["spec-labels"]
        .as_array()
        .expect("spec-labels should be an array");
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0], "safety-critical");
    assert_eq!(labels[1], "arithmetic");

    // bar: spec-less in-scope backlog -> empty primary-spec, untracked=false, no
    // verification-status, no spec-labels
    let bar_json = &json["probe:test-crate/0.1.0/module/bar()"];
    assert_eq!(bar_json["primary-spec"], "");
    assert_eq!(bar_json["untracked"], false);
    assert!(
        bar_json.get("spec-labels").is_none(),
        "Empty spec-labels should be omitted from JSON"
    );

    // ext: out-of-scope external stub (KB P25) -> untracked=true is serialized; no
    // verification-status, primary-spec, or spec-labels (skip_serializing_if)
    let ext_json = &json["probe:external/1.0.0/lib/ext()"];
    assert_eq!(ext_json["display-name"], "ext");
    assert!(
        ext_json.get("verification-status").is_none(),
        "External stub should not have verification-status in JSON"
    );
    assert!(
        ext_json.get("primary-spec").is_none(),
        "External stub should not have primary-spec in JSON"
    );
    assert_eq!(
        ext_json["untracked"], true,
        "External stub is out of scope (KB P25): untracked = true"
    );
    assert!(
        ext_json.get("spec-labels").is_none(),
        "External stub should not have spec-labels in JSON"
    );
}

#[test]
fn test_specs_text_content() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), None);

    let foo = &unified["probe:test-crate/0.1.0/module/foo()"];
    assert_eq!(
        foo.primary_spec.as_deref(),
        Some("requires\n    x > 0,\n    y < 100\nensures\n    result > x")
    );
    assert_eq!(foo.untracked, Some(false));

    // baz has only ensures
    let baz = &unified["probe:test-crate/0.1.0/module/baz()"];
    assert_eq!(
        baz.primary_spec.as_deref(),
        Some("ensures\n    result == x * 2")
    );
    assert_eq!(baz.untracked, Some(false));

    // bar is spec-less but in scope: backlog (KB P24) — untracked=false
    let bar = &unified["probe:test-crate/0.1.0/module/bar()"];
    assert_eq!(bar.primary_spec.as_deref(), Some(""));
    assert_eq!(bar.untracked, Some(false));
}
