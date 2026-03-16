//! Integration tests for the unified extract output.
//!
//! Verifies that merging atoms + specs + proofs into a UnifiedAtom dict
//! produces consistent results, using pre-built fixture files.

use probe_verus::metadata::unwrap_envelope;
use probe_verus::{
    split_clauses, AtomWithLines, CallLocation, SpecCondition, SpecConditionKind, UnifiedAtom,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

const FIXTURES: &str = "tests/fixtures/unified_test";

#[derive(Deserialize)]
struct SpecsEntry {
    #[serde(default)]
    has_requires: bool,
    #[serde(default)]
    has_ensures: bool,
    #[serde(default)]
    requires_text: Option<String>,
    #[serde(default)]
    ensures_text: Option<String>,
    #[serde(rename = "requires-calls", default)]
    requires_calls: Vec<String>,
    #[serde(rename = "ensures-calls", default)]
    ensures_calls: Vec<String>,
    #[serde(rename = "requires-calls-full", default)]
    requires_calls_full: Vec<String>,
    #[serde(rename = "ensures-calls-full", default)]
    ensures_calls_full: Vec<String>,
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

fn build_spec_conditions(entry: &SpecsEntry) -> Vec<SpecCondition> {
    let mut conditions = Vec::new();

    if entry.has_requires {
        conditions.push(SpecCondition {
            kind: SpecConditionKind::Precondition,
            text: entry.requires_text.clone(),
            clauses: entry
                .requires_text
                .as_deref()
                .map(split_clauses)
                .unwrap_or_default(),
            calls: entry.requires_calls.clone(),
            calls_full: entry.requires_calls_full.clone(),
        });
    }

    if entry.has_ensures {
        conditions.push(SpecCondition {
            kind: SpecConditionKind::Postcondition,
            text: entry.ensures_text.clone(),
            clauses: entry
                .ensures_text
                .as_deref()
                .map(split_clauses)
                .unwrap_or_default(),
            calls: entry.ensures_calls.clone(),
            calls_full: entry.ensures_calls_full.clone(),
        });
    }

    conditions
}

/// Merge atoms + specs + proofs into unified output (mirrors the logic in extract.rs).
fn merge_fixture_files(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
) -> BTreeMap<String, UnifiedAtom> {
    let atoms: BTreeMap<String, AtomWithLines> = load_enveloped(atoms_path);
    let specs: Option<BTreeMap<String, SpecsEntry>> = specs_path.map(load_enveloped);
    let proofs: Option<BTreeMap<String, ProofsEntryMinimal>> = proofs_path.map(load_enveloped);

    let mut unified = BTreeMap::new();
    for (code_name, mut atom) in atoms {
        let spec_conditions: Option<Vec<SpecCondition>> = specs
            .as_ref()
            .and_then(|s| s.get(&code_name))
            .map(build_spec_conditions);

        if spec_conditions.is_some() && !atom.dependencies_with_locations.is_empty() {
            let inner_code_names: std::collections::BTreeSet<String> = atom
                .dependencies_with_locations
                .iter()
                .filter(|d| d.location == CallLocation::Inner)
                .map(|d| d.code_name.clone())
                .collect();
            atom.dependencies = inner_code_names;
            atom.dependencies_with_locations
                .retain(|d| d.location == CallLocation::Inner);
        }

        let verification_status = proofs
            .as_ref()
            .and_then(|p| p.get(&code_name))
            .map(|e| map_verification_status(&e.status).to_string());
        unified.insert(
            code_name,
            UnifiedAtom {
                atom,
                verification_status,
                specs: spec_conditions,
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
fn test_unified_specs_matches_specs_file() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let specs: BTreeMap<String, SpecsEntry> = load_enveloped(&specs_path);
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), None);

    for (code_name, spec_entry) in &specs {
        let unified_entry = unified
            .get(code_name)
            .unwrap_or_else(|| panic!("Spec key missing from unified output: {}", code_name));
        let spec_conditions = unified_entry
            .specs
            .as_ref()
            .expect("specs field should be present");
        let has_specs = !spec_conditions.is_empty();
        let expected = spec_entry.has_requires || spec_entry.has_ensures;
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
        ext.specs.is_none(),
        "External stub should have no 'specs' field"
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
    let foo_specs = foo.specs.as_ref().unwrap();
    assert_eq!(foo_specs.len(), 2);
    assert_eq!(foo_specs[0].kind, SpecConditionKind::Precondition);
    assert_eq!(foo_specs[1].kind, SpecConditionKind::Postcondition);
    assert_eq!(foo.verification_status.as_deref(), Some("verified"));

    let bar = &unified["probe:test-crate/0.1.0/module/bar()"];
    assert_eq!(bar.atom.display_name, "bar");
    assert!(bar.specs.as_ref().unwrap().is_empty());
    assert_eq!(bar.verification_status.as_deref(), Some("failed"));

    // baz has specs (ensures only) but no proofs entry
    let baz = &unified["probe:test-crate/0.1.0/module/baz()"];
    assert_eq!(baz.atom.display_name, "baz");
    let baz_specs = baz.specs.as_ref().unwrap();
    assert_eq!(baz_specs.len(), 1);
    assert_eq!(baz_specs[0].kind, SpecConditionKind::Postcondition);
    assert!(baz.verification_status.is_none());
}

#[test]
fn test_unified_json_serialization_format() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let proofs_path = Path::new(FIXTURES).join("proofs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), Some(&proofs_path));

    let json = serde_json::to_value(&unified).unwrap();

    // foo: has both verification-status and specs
    let foo_json = &json["probe:test-crate/0.1.0/module/foo()"];
    assert_eq!(foo_json["display-name"], "foo");
    assert_eq!(foo_json["verification-status"], "verified");
    assert!(foo_json["specs"].is_array());
    assert_eq!(foo_json["specs"].as_array().unwrap().len(), 2);
    assert_eq!(foo_json["specs"][0]["kind"], "precondition");
    assert_eq!(foo_json["specs"][1]["kind"], "postcondition");
    assert_eq!(foo_json["kind"], "exec");
    assert_eq!(foo_json["language"], "rust");

    // bar: analyzed but no specs -> empty array
    let bar_json = &json["probe:test-crate/0.1.0/module/bar()"];
    assert!(bar_json["specs"].is_array());
    assert_eq!(bar_json["specs"].as_array().unwrap().len(), 0);

    // ext: no verification-status or specs (skip_serializing_if)
    let ext_json = &json["probe:external/1.0.0/lib/ext()"];
    assert_eq!(ext_json["display-name"], "ext");
    assert!(
        ext_json.get("verification-status").is_none(),
        "External stub should not have verification-status in JSON"
    );
    assert!(
        ext_json.get("specs").is_none(),
        "External stub should not have specs in JSON"
    );
}

#[test]
fn test_specs_preconditions_postconditions_content() {
    let atoms_path = Path::new(FIXTURES).join("atoms.json");
    let specs_path = Path::new(FIXTURES).join("specs.json");
    let unified = merge_fixture_files(&atoms_path, Some(&specs_path), None);

    let foo = &unified["probe:test-crate/0.1.0/module/foo()"];
    let specs = foo.specs.as_ref().unwrap();

    let pre = &specs[0];
    assert_eq!(pre.kind, SpecConditionKind::Precondition);
    assert_eq!(pre.clauses, vec!["x > 0", "y < 100"]);
    assert_eq!(pre.calls, vec!["is_valid"]);

    let post = &specs[1];
    assert_eq!(post.kind, SpecConditionKind::Postcondition);
    assert_eq!(post.clauses, vec!["result > x"]);
    assert_eq!(post.calls, vec!["helper_spec"]);

    // baz has only postconditions
    let baz = &unified["probe:test-crate/0.1.0/module/baz()"];
    let baz_specs = baz.specs.as_ref().unwrap();
    assert_eq!(baz_specs.len(), 1);
    assert_eq!(baz_specs[0].kind, SpecConditionKind::Postcondition);
    assert_eq!(baz_specs[0].clauses, vec!["result == x * 2"]);
    assert_eq!(baz_specs[0].calls, vec!["spec_helper"]);
}
