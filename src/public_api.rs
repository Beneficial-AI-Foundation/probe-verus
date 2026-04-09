//! Integration with `cargo public-api` for ground-truth public API detection.
//!
//! Runs `cargo public-api` as a subprocess and parses its output to build a set
//! of Rust-qualified names (RQNs) that form the crate's public API surface.
//! These RQNs can then be matched against atoms to override `is-public-api`.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use crate::AtomWithLines;
use std::collections::BTreeMap;

const BLANKET_IMPL_TRAITS: &[&str] = &[
    "core::convert::Into",
    "core::convert::TryFrom",
    "core::convert::TryInto",
    "core::borrow::Borrow",
    "core::borrow::BorrowMut",
    "core::any::Any",
    "alloc::borrow::ToOwned",
    "core::clone::CloneInto",
    "core::convert::From",
];

/// Run `cargo public-api` on the given project directory and return a set of
/// public API RQNs.
pub fn run_cargo_public_api(project_dir: &Path) -> Result<HashSet<String>, String> {
    let output = Command::new("cargo")
        .arg("public-api")
        .current_dir(project_dir)
        .output()
        .map_err(|e| format!("Failed to run `cargo public-api`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`cargo public-api` failed (exit {}):\n{stderr}",
            output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_public_api_output(&stdout))
}

/// Parse `cargo public-api` text output into a set of RQNs.
///
/// Each line looks like:
///   `pub fn crate_name::module::Type::method(args) -> Ret`
///   `pub const fn crate_name::module::free_fn(args)`
///   `pub unsafe fn crate_name::module::unsafe_fn(args)`
///
/// We extract the qualified path before the first `(` (or `<` for generic items),
/// stripping the `pub [const|unsafe|async] fn` prefix.
pub fn parse_public_api_output(output: &str) -> HashSet<String> {
    let mut rqns = HashSet::new();
    for line in output.lines() {
        let line = line.trim();
        if let Some(rqn) = extract_rqn_from_line(line) {
            if !is_blanket_impl(&rqn) {
                rqns.insert(rqn);
            }
        }
    }
    rqns
}

/// Extract an RQN from a single `cargo public-api` output line.
///
/// Returns `None` for lines that aren't function declarations.
fn extract_rqn_from_line(line: &str) -> Option<String> {
    let rest = strip_fn_prefix(line)?;
    // The qualified path ends at the first `(` (function args) or end of string.
    // For generic functions, it may have `<...>` before `(` — strip those too.
    let path = rest.find('(').map(|i| &rest[..i]).unwrap_or(rest).trim();
    // Strip trailing generic params: `foo::bar::<T>` → `foo::bar`
    let path = strip_trailing_generics(path);
    if path.is_empty() || !path.contains("::") {
        return None;
    }
    Some(path.to_string())
}

/// Strip `pub [const] [unsafe] [async] fn ` prefix.
fn strip_fn_prefix(line: &str) -> Option<&str> {
    let mut s = line.strip_prefix("pub ")?;
    for kw in &["const ", "unsafe ", "async "] {
        if let Some(rest) = s.strip_prefix(kw) {
            s = rest;
        }
    }
    s.strip_prefix("fn ")
}

/// Strip trailing `<...>` from a path.
fn strip_trailing_generics(path: &str) -> &str {
    if let Some(i) = path.rfind('<') {
        let before = &path[..i];
        before.strip_suffix("::").unwrap_or(before)
    } else {
        path
    }
}

/// Check whether an RQN corresponds to a blanket impl trait method.
fn is_blanket_impl(rqn: &str) -> bool {
    for trait_path in BLANKET_IMPL_TRAITS {
        // Match patterns like `<Type as core::convert::Into<Target>>::into`
        if rqn.contains(trait_path) {
            return true;
        }
    }
    false
}

/// Override `is-public-api` on atoms using `cargo public-api` RQN set.
///
/// For each atom that has a `rust-qualified-name`, sets `is-public-api` based on
/// whether the RQN appears in the public API set. Atoms without an RQN keep their
/// existing SCIP-walk value unchanged.
pub fn enrich_atoms_with_public_api(
    atoms: &mut BTreeMap<String, AtomWithLines>,
    public_rqns: &HashSet<String>,
) -> (usize, usize) {
    let mut matched = 0usize;
    let mut overridden = 0usize;
    for atom in atoms.values_mut() {
        if let Some(ref rqn) = atom.rust_qualified_name {
            let in_public_api = public_rqns.contains(rqn);
            let old = atom.is_public_api;
            atom.is_public_api = Some(in_public_api);
            matched += 1;
            if old != Some(in_public_api) {
                overridden += 1;
            }
        }
    }
    (matched, overridden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rqn_simple_fn() {
        let line = "pub fn curve25519_dalek::edwards::CompressedEdwardsY::decompress(&self) -> Option<EdwardsPoint>";
        assert_eq!(
            extract_rqn_from_line(line),
            Some("curve25519_dalek::edwards::CompressedEdwardsY::decompress".to_string())
        );
    }

    #[test]
    fn test_extract_rqn_const_fn() {
        let line = "pub const fn curve25519_dalek::scalar::Scalar::ZERO() -> Self";
        assert_eq!(
            extract_rqn_from_line(line),
            Some("curve25519_dalek::scalar::Scalar::ZERO".to_string())
        );
    }

    #[test]
    fn test_extract_rqn_unsafe_fn() {
        let line =
            "pub unsafe fn curve25519_dalek::backend::serial::u64::field::FieldElement51::from_raw(&[u64; 5]) -> Self";
        assert_eq!(
            extract_rqn_from_line(line),
            Some(
                "curve25519_dalek::backend::serial::u64::field::FieldElement51::from_raw"
                    .to_string()
            )
        );
    }

    #[test]
    fn test_extract_rqn_generic_fn() {
        let line = "pub fn curve25519_dalek::traits::Identity::identity<T>() -> T";
        assert_eq!(
            extract_rqn_from_line(line),
            Some("curve25519_dalek::traits::Identity::identity".to_string())
        );
    }

    #[test]
    fn test_extract_rqn_non_fn_line() {
        assert_eq!(extract_rqn_from_line("pub struct Foo"), None);
        assert_eq!(extract_rqn_from_line("pub type Bar = Baz"), None);
        assert_eq!(extract_rqn_from_line("// comment"), None);
        assert_eq!(extract_rqn_from_line(""), None);
    }

    #[test]
    fn test_blanket_impl_filtered() {
        let output = "\
pub fn curve25519_dalek::edwards::EdwardsPoint::compress(&self) -> CompressedEdwardsY
pub fn <curve25519_dalek::edwards::EdwardsPoint as core::convert::Into<MontgomeryPoint>>::into(self) -> MontgomeryPoint
pub fn <curve25519_dalek::scalar::Scalar as core::convert::From<u8>>::from(x: u8) -> Self
";
        let rqns = parse_public_api_output(output);
        assert!(rqns.contains("curve25519_dalek::edwards::EdwardsPoint::compress"));
        assert_eq!(rqns.len(), 1, "blanket impls should be filtered out");
    }

    #[test]
    fn test_parse_full_output() {
        let output = "\
pub fn curve25519_dalek::edwards::CompressedEdwardsY::decompress(&self) -> Option<EdwardsPoint>
pub fn curve25519_dalek::ristretto::RistrettoPoint::compress(&self) -> CompressedRistretto
pub const fn curve25519_dalek::scalar::Scalar::ZERO() -> Self
pub unsafe fn curve25519_dalek::backend::serial::u64::field::FieldElement51::from_raw(&[u64; 5]) -> Self
";
        let rqns = parse_public_api_output(output);
        assert_eq!(rqns.len(), 4);
        assert!(rqns.contains("curve25519_dalek::edwards::CompressedEdwardsY::decompress"));
        assert!(rqns.contains("curve25519_dalek::ristretto::RistrettoPoint::compress"));
        assert!(rqns.contains("curve25519_dalek::scalar::Scalar::ZERO"));
        assert!(rqns
            .contains("curve25519_dalek::backend::serial::u64::field::FieldElement51::from_raw"));
    }

    #[test]
    fn test_enrich_atoms_overrides_correctly() {
        use std::collections::BTreeSet;
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:test/1.0/foo()".to_string(),
            AtomWithLines {
                display_name: "foo".to_string(),
                code_name: "probe:test/1.0/foo()".to_string(),
                dependencies: BTreeSet::new(),
                dependencies_with_locations: Vec::new(),
                code_module: String::new(),
                code_path: "test-crate/src/lib.rs".to_string(),
                code_text: crate::CodeTextInfo {
                    lines_start: 1,
                    lines_end: 5,
                },
                kind: crate::DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: Some("test_crate::foo".to_string()),
                is_public: Some(true),
                is_public_api: Some(false), // SCIP-walk said false
                has_body: Some(true),
                is_external: Some(false),
                is_cfg_gated: Some(false),
            },
        );
        atoms.insert(
            "probe:test/1.0/bar()".to_string(),
            AtomWithLines {
                display_name: "bar".to_string(),
                code_name: "probe:test/1.0/bar()".to_string(),
                dependencies: BTreeSet::new(),
                dependencies_with_locations: Vec::new(),
                code_module: String::new(),
                code_path: "test-crate/src/lib.rs".to_string(),
                code_text: crate::CodeTextInfo {
                    lines_start: 10,
                    lines_end: 15,
                },
                kind: crate::DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: Some("test_crate::bar".to_string()),
                is_public: Some(true),
                is_public_api: Some(true), // SCIP-walk said true
                has_body: Some(true),
                is_external: Some(false),
                is_cfg_gated: Some(false),
            },
        );
        atoms.insert(
            "probe:test/1.0/no_rqn()".to_string(),
            AtomWithLines {
                display_name: "no_rqn".to_string(),
                code_name: "probe:test/1.0/no_rqn()".to_string(),
                dependencies: BTreeSet::new(),
                dependencies_with_locations: Vec::new(),
                code_module: String::new(),
                code_path: "src/lib.rs".to_string(),
                code_text: crate::CodeTextInfo {
                    lines_start: 20,
                    lines_end: 25,
                },
                kind: crate::DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: None,
                is_public: Some(true),
                is_public_api: Some(true),
                has_body: Some(true),
                is_external: Some(false),
                is_cfg_gated: Some(false),
            },
        );

        let mut public_rqns = HashSet::new();
        public_rqns.insert("test_crate::foo".to_string()); // foo IS in public API

        let (matched, overridden) = enrich_atoms_with_public_api(&mut atoms, &public_rqns);
        assert_eq!(matched, 2); // foo and bar have RQNs
        assert_eq!(overridden, 2); // foo: false→true, bar: true→false

        assert_eq!(
            atoms["probe:test/1.0/foo()"].is_public_api,
            Some(true),
            "foo should now be public API"
        );
        assert_eq!(
            atoms["probe:test/1.0/bar()"].is_public_api,
            Some(false),
            "bar should now be non-public API"
        );
        assert_eq!(
            atoms["probe:test/1.0/no_rqn()"].is_public_api,
            Some(true),
            "no_rqn should keep its SCIP-walk value"
        );
    }
}
