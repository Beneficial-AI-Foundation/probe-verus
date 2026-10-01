//! Trait impls that older verus-analyzer releases collapsed onto one SCIP symbol
//! (e.g. `impl Mul<&Scalar> for &MontgomeryPoint` and `impl Mul<&MontgomeryPoint> for &Scalar`
//! both became `montgomery/Mul#mul().`) must get distinct code_names.
//!
//! Requires `data/curve_top.json`: a SCIP JSON index of dalek-lite produced by
//! verus-analyzer 2026-08-22 or later (see the integration-test job in CI).

use probe_verus::{
    build_call_graph, convert_to_atoms_with_lines, find_duplicate_code_names, parse_scip_json,
    uses_legacy_symbol_format, AtomWithLines,
};
use std::collections::HashSet;

const PREFIX: &str = "probe:curve25519-dalek/4.1.3/";

fn atoms() -> Vec<AtomWithLines> {
    let scip_data = parse_scip_json("data/curve_top.json").expect("Failed to parse SCIP JSON");
    convert_to_atoms_with_lines(&build_call_graph(&scip_data))
}

fn code_names(atoms: &[AtomWithLines]) -> HashSet<&str> {
    atoms.iter().map(|a| a.code_name.as_str()).collect()
}

fn assert_has_all(names: &HashSet<&str>, expected: &[&str]) {
    for suffix in expected {
        let full = format!("{PREFIX}{suffix}");
        assert!(names.contains(full.as_str()), "missing code_name {full}");
    }
}

#[test]
fn test_index_uses_current_symbol_format() {
    let scip_data = parse_scip_json("data/curve_top.json").expect("Failed to parse SCIP JSON");
    assert!(
        !uses_legacy_symbol_format(&scip_data),
        "data/curve_top.json was produced by a verus-analyzer older than 2026-08-22"
    );
}

#[test]
fn test_mul_implementations_with_swapped_operands() {
    let atoms = atoms();
    assert_has_all(
        &code_names(&atoms),
        &[
            "montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul()",
            "montgomery/impl#[`&Scalar`][`Mul<&MontgomeryPoint>`]mul()",
        ],
    );
}

/// `impl Mul<&Scalar> for &RistrettoPoint` and `impl Mul<&Scalar> for &RistrettoBasepointTable`
/// share the method signature; only the Self type tells them apart. Lifetimes in the
/// SCIP symbol (`&'a RistrettoPoint`) are dropped from the code_name.
#[test]
fn test_same_signature_different_self_types() {
    let atoms = atoms();
    assert_has_all(
        &code_names(&atoms),
        &[
            "ristretto/impl#[`&RistrettoPoint`][`Mul<&Scalar>`]mul()",
            "ristretto/impl#[`&RistrettoBasepointTable`][`Mul<&Scalar>`]mul()",
            "ristretto/impl#[`&Scalar`][`Mul<&RistrettoPoint>`]mul()",
            "ristretto/impl#[`&Scalar`][`Mul<&RistrettoBasepointTable>`]mul()",
        ],
    );
}

#[test]
fn test_neg_implementations_for_owned_and_borrowed_self() {
    let atoms = atoms();
    assert_has_all(
        &code_names(&atoms),
        &[
            "scalar/impl#[`&Scalar`][Neg]neg()",
            "scalar/impl#[Scalar][Neg]neg()",
            "ristretto/impl#[`&RistrettoPoint`][Neg]neg()",
            "ristretto/impl#[RistrettoPoint][Neg]neg()",
        ],
    );
}

/// `impl From<&EdwardsPoint> for NafLookupTable5<ProjectiveNielsPoint>` and
/// `... for NafLookupTable5<AffineNielsPoint>` differ only in the Self type's
/// generic argument.
#[test]
fn test_from_implementations_for_generic_self_types() {
    let atoms = atoms();
    assert_has_all(
        &code_names(&atoms),
        &[
            "window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&EdwardsPoint>`]from()",
            "window/impl#[`NafLookupTable5<AffineNielsPoint>`][`From<&EdwardsPoint>`]from()",
        ],
    );
}

#[test]
fn test_display_names_include_self_type() {
    let atoms = atoms();
    let mul = atoms
        .iter()
        .find(|a| {
            a.code_name
                == format!("{PREFIX}montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul()")
        })
        .expect("MontgomeryPoint * Scalar atom");
    assert_eq!(mul.display_name, "MontgomeryPoint::mul");
}

#[test]
fn test_no_duplicate_code_names() {
    let duplicates = find_duplicate_code_names(&atoms());
    assert!(
        duplicates.is_empty(),
        "duplicate code_names: {:?}",
        duplicates.iter().map(|d| &d.code_name).collect::<Vec<_>>()
    );
}
