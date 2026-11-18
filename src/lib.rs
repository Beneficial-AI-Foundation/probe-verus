use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub mod cfg_eval;
pub mod commands;
pub mod constants;
pub mod error;
pub mod metadata;
pub mod path_utils;
pub mod public_api;
pub mod scip_cache;
pub mod taxonomy;
pub mod tool_manager;
pub mod verification;
pub mod verus_parser;

pub use error::{ProbeError, ProbeResult};

use constants::{
    is_definition, is_external_function_symbol, is_function_like_kind, PROBE_URI_PREFIX,
    SCIP_SYMBOL_PREFIX,
};
use path_utils::{extract_src_suffix, paths_match_by_suffix};

// =============================================================================
// Declaration Kind Enum
// =============================================================================

/// Declaration kind - indicates what kind of verification is performed.
///
/// - `Exec`: Executable code, compiled to native code and verified
/// - `Proof`: Proof code, verified but not compiled (erased at runtime)
/// - `Spec`: Specification code, defines logical properties (erased at runtime)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeclKind {
    #[default]
    Exec,
    Proof,
    Spec,
}

impl DeclKind {
    /// Parse a function mode from a string.
    ///
    /// Accepts: "exec", "proof", "spec" (case-insensitive)
    /// Returns `Exec` for unrecognized values (the default mode).
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "proof" => DeclKind::Proof,
            "spec" => DeclKind::Spec,
            _ => DeclKind::Exec,
        }
    }

    /// Convert to a string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            DeclKind::Exec => "exec",
            DeclKind::Proof => "proof",
            DeclKind::Spec => "spec",
        }
    }
}

impl fmt::Display for DeclKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// SCIP data structures
#[derive(Debug, Serialize, Deserialize)]
pub struct ScipIndex {
    pub metadata: Metadata,
    pub documents: Vec<Document>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub tool_info: ScipToolInfo,
    pub project_root: String,
    pub text_document_encoding: i32,
}

/// Producer of a SCIP index. Both fields can be missing; they are then empty.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScipToolInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Document {
    pub language: String,
    pub relative_path: String,
    pub occurrences: Vec<Occurrence>,
    #[serde(default)]
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Occurrence {
    pub range: Vec<i32>,
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_roles: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub symbol: String,
    pub kind: i32,
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Vec<String>>,
    pub signature_documentation: SignatureDocumentation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing_symbol: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignatureDocumentation {
    pub language: String,
    pub text: String,
}

/// A call from one function to another
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CalleeInfo {
    /// The raw SCIP symbol of the callee
    pub symbol: String,
    /// Line number where the call occurs (0-based from SCIP)
    pub line: i32,
}

/// Location where a function call occurs
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallLocation {
    /// Call in requires clause (precondition)
    Precondition,
    /// Call in ensures clause (postcondition)
    Postcondition,
    /// Call in function body
    Inner,
}

/// A dependency with its call location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyWithLocation {
    #[serde(rename = "code-name")]
    pub code_name: String,
    pub location: CallLocation,
    pub line: usize,
}

/// Function node in the call graph
#[derive(Debug, Clone)]
pub struct FunctionNode {
    pub symbol: String,
    pub display_name: String,
    pub signature_text: String,
    pub relative_path: String,
    pub callees: HashSet<CalleeInfo>,
    pub range: Vec<i32>,
}

fn default_language() -> String {
    "rust".to_string()
}

/// Check whether a SCIP signature represents an unrestricted `pub` item.
///
/// Returns `true` for `pub fn`, `pub unsafe fn`, `pub async fn`, etc.
/// Returns `false` for `fn`, `pub(crate) fn`, `pub(super) fn`, and similar.
#[must_use]
pub fn is_signature_public(sig: &str) -> bool {
    let trimmed = sig.trim_start();
    if let Some(rest) = trimmed.strip_prefix("pub") {
        !rest.starts_with('(')
    } else {
        false
    }
}

/// Check whether a probe `code_name` represents a trait impl method.
///
/// SCIP encodes impl methods as `impl#[SelfType]method()` (inherent) and
/// `impl#[SelfType][Trait]method()` (trait impl), so a trait impl is an
/// `impl#` segment with two bracket groups.
///
/// **Known limitation:** treats ALL trait impl methods as public, including
/// impls of `pub(crate)` or private traits. SCIP symbols do not encode trait
/// visibility. In practice the affected traits are public `core`/`std` traits.
#[must_use]
pub fn is_trait_impl_code_name(code_name: &str) -> bool {
    parse_impl_segment(code_name).is_some_and(|seg| seg.trait_type.is_some())
}

/// The bracket groups of an `impl#[SelfType][Trait]method()` symbol segment.
#[derive(Debug, PartialEq, Eq)]
struct ImplSegment<'a> {
    self_type: &'a str,
    trait_type: Option<&'a str>,
}

/// Parse the `impl#[...]` / `impl#[...][...]` part of a SCIP symbol or code_name.
///
/// Bracket contents may be wrapped in backticks when they contain special
/// characters (e.g. `` [`&Scalar`] `` or `` [`[u8; 32]`] ``); the backticks are
/// stripped from the returned slices.
fn parse_impl_segment(s: &str) -> Option<ImplSegment<'_>> {
    let start = s.rfind("impl#[")? + "impl#".len();
    let (self_type, rest) = take_bracket_group(&s[start..])?;
    let trait_type = take_bracket_group(rest).map(|(t, _)| t);
    Some(ImplSegment {
        self_type,
        trait_type,
    })
}

/// Split a leading `[...]` group off `s`, honouring backtick quoting.
/// Returns the group contents (without brackets or backticks) and the remainder.
fn take_bracket_group(s: &str) -> Option<(&str, &str)> {
    let inner = s.strip_prefix('[')?;
    if let Some(quoted) = inner.strip_prefix('`') {
        let close = quoted.find("`]")?;
        Some((&quoted[..close], &quoted[close + 2..]))
    } else {
        let close = inner.find(']')?;
        Some((&inner[..close], &inner[close + 1..]))
    }
}

/// Reduce an impl Self type to the bare type name used in display names:
/// strips references, `mut`, lifetimes, generic arguments and path qualifiers.
/// `&'a NafLookupTable5<T>` -> `NafLookupTable5`,
/// `crate::lizard::lizard_constants::FieldElement51` -> `FieldElement51`.
fn bare_type_name(ty: &str) -> &str {
    let ty = ty.trim_start_matches('&');
    let ty = match ty.strip_prefix('\'') {
        Some(rest) => rest.split_once(' ').map_or(rest, |(_, t)| t),
        None => ty,
    };
    // Code-names turn spaces into `/`, so `mut` may be followed by either.
    let ty = ty
        .strip_prefix("mut ")
        .or_else(|| ty.strip_prefix("mut/"))
        .unwrap_or(ty);
    let ty = ty.split('<').next().unwrap_or(ty);
    ty.rsplit("::").next().unwrap_or(ty)
}

/// Remove lifetime parameters from a SCIP symbol, so code_names do not depend
/// on lifetime names: `` [`From<&'a EdwardsPoint>`] `` -> `` [`From<&EdwardsPoint>`] ``.
/// Character literals in const generic arguments (`` `Tag<'a'>` ``) are kept.
pub(crate) fn strip_lifetimes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let starts_lifetime = c == '\''
            && chars
                .get(i + 1)
                .is_some_and(|n| n.is_alphabetic() || *n == '_');
        if !starts_lifetime {
            out.push(c);
            i += 1;
            continue;
        }
        if chars.get(i + 2) == Some(&'\'') {
            out.extend(&chars[i..i + 3]);
            i += 3;
            continue;
        }
        i += 1;
        while chars
            .get(i)
            .is_some_and(|n| n.is_alphanumeric() || *n == '_')
        {
            i += 1;
        }
        // Drop the separator that followed the lifetime (`'a T`, `'a, T`).
        if chars.get(i) == Some(&',') {
            i += 1;
        }
        if chars.get(i) == Some(&' ') {
            i += 1;
        }
    }
    out.replace(", >", ">").replace("<>", "")
}

/// Output format: Atom with line numbers
#[derive(Debug, Serialize, Deserialize)]
pub struct AtomWithLines {
    #[serde(rename = "display-name")]
    pub display_name: String,
    #[serde(skip_serializing, default)]
    pub code_name: String,
    /// Sorted set of dependency code_names (BTreeSet for deterministic JSON output)
    pub dependencies: BTreeSet<String>,
    /// Dependencies with call location information (only included with --with-locations flag)
    #[serde(
        rename = "dependencies-with-locations",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub dependencies_with_locations: Vec<DependencyWithLocation>,
    #[serde(rename = "code-module")]
    pub code_module: String,
    #[serde(rename = "code-path")]
    pub code_path: String,
    #[serde(rename = "code-text")]
    pub code_text: CodeTextInfo,
    /// Declaration kind: exec, proof, or spec
    pub kind: DeclKind,
    /// Source language of the atom (for cross-language merge compatibility)
    #[serde(default = "default_language")]
    pub language: String,
    /// Rust-style qualified name derived from file path and display name.
    /// Enables cross-language matching with Aeneas-generated Lean code.
    /// Format: `crate_name::module::path::Type::method`
    #[serde(
        rename = "rust-qualified-name",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub rust_qualified_name: Option<String>,
    /// Whether the function signature starts with unrestricted `pub`.
    #[serde(rename = "is-public", skip_serializing_if = "Option::is_none", default)]
    pub is_public: Option<bool>,
    /// Whether the function is part of the crate's public API:
    /// `pub fn` + all ancestor modules `pub` + exec kind + library crate.
    /// `spec fn` and `proof fn` always get `false` (erased at runtime).
    /// External stubs get `None`.
    #[serde(
        rename = "is-public-api",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub is_public_api: Option<bool>,
    /// Whether the function has a body.
    /// `false` for bodiless trait method declarations; `true` otherwise.
    #[serde(rename = "has-body", skip_serializing_if = "Option::is_none", default)]
    pub has_body: Option<bool>,
    /// Whether `#[verifier::external]` (direct or via `cfg_attr`) is present.
    #[serde(
        rename = "is-external",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub is_external: Option<bool>,
    /// Whether the function or an enclosing item (impl, mod, cfg_if branch,
    /// or the module's `mod` declaration) has `#[cfg(...)]`.
    #[serde(
        rename = "is-cfg-gated",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub is_cfg_gated: Option<bool>,
}

/// Unified atom: all `AtomWithLines` fields plus optional verification, specification,
/// and categorized dependency fields.
///
/// Produced by the `extract` pipeline to match the `probe-lean/verify` output structure.
/// When a step is skipped, the corresponding field is absent (serialized as missing key).
#[derive(Debug, Serialize, Deserialize)]
pub struct UnifiedAtom {
    #[serde(flatten)]
    pub atom: AtomWithLines,
    #[serde(
        rename = "requires-dependencies",
        skip_serializing_if = "BTreeSet::is_empty",
        default
    )]
    pub requires_dependencies: BTreeSet<String>,
    #[serde(
        rename = "ensures-dependencies",
        skip_serializing_if = "BTreeSet::is_empty",
        default
    )]
    pub ensures_dependencies: BTreeSet<String>,
    #[serde(
        rename = "body-dependencies",
        skip_serializing_if = "BTreeSet::is_empty",
        default
    )]
    pub body_dependencies: BTreeSet<String>,
    /// Full spec text (requires + ensures). Empty string = analyzed, no spec. Absent = not analyzed.
    #[serde(rename = "primary-spec", skip_serializing_if = "Option::is_none")]
    pub primary_spec: Option<String>,
    /// `true` = out of verification scope (KB P25): `#[verifier::external]`,
    /// cfg-inactive in the verification build, an external-crate stub, a bodiless
    /// declaration (a trait-method signature with no body), or a non-library target
    /// (`build.rs`, `tests/`, `examples/`, `benches/`). Such atoms carry no
    /// `verification-status`. `false` = in scope: a specified function, a trusted
    /// axiom, an atom carrying any `verification-status`, or the spec-less backlog.
    /// Absent = scope not analyzed (neither specs nor proofs loaded for this atom).
    /// `has-verification-status ⟹ ¬untracked` (KB P24).
    #[serde(rename = "untracked", skip_serializing_if = "Option::is_none")]
    pub untracked: Option<bool>,
    /// Verification outcome for an in-scope atom. Values: `"verified"`,
    /// `"transitively-verified"`, `"failed"`, `"unverified"`, or `"trusted"` (in the
    /// trust base). Absent for the backlog and for out-of-scope atoms (`untracked: true`).
    #[serde(
        rename = "verification-status",
        skip_serializing_if = "Option::is_none"
    )]
    pub verification_status: Option<String>,
    /// Why this atom is trusted. Present only when `verification-status` is `"trusted"`.
    /// Values: `"admit"`, `"external-body"`, `"assume-specification"`.
    #[serde(rename = "trusted-reason", skip_serializing_if = "Option::is_none")]
    pub trusted_reason: Option<String>,
    /// The combined item-gating `#[cfg(...)]` predicate governing this atom, if any.
    #[serde(rename = "cfg", skip_serializing_if = "Option::is_none")]
    pub cfg_predicate: Option<String>,
    /// Taxonomy classification labels from the `specify` step (omitted when empty).
    #[serde(rename = "spec-labels", skip_serializing_if = "Vec::is_empty", default)]
    pub spec_labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeTextInfo {
    #[serde(rename = "lines-start")]
    pub lines_start: usize,
    #[serde(rename = "lines-end")]
    pub lines_end: usize,
}

/// Parse a SCIP JSON file
pub fn parse_scip_json(file_path: &str) -> Result<ScipIndex, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(file_path)?;
    let index: ScipIndex = serde_json::from_str(&contents)?;
    Ok(index)
}

/// Check if a symbol kind represents a function-like entity
fn is_function_like(kind: i32) -> bool {
    is_function_like_kind(kind)
}

/// Call-graph key of a function definition: its symbol plus the document and
/// full range of the defining occurrence.
///
/// SCIP symbols are unique per definition except for a few analyzer bugs
/// (e.g. spec-only trait impls whose trait is dropped from the symbol), so the
/// symbol alone cannot be the key.
fn make_definition_key(symbol: &str, relative_path: &str, range: &[i32]) -> String {
    format!("{symbol}@{relative_path}:{range:?}")
}

/// Derive a Rust-style qualified name from the code-path (file) and SCIP symbol.
///
/// The qualified name uses `::` separators and underscore-style crate names to match
/// the format produced by tools like Aeneas. This enables cross-language matching
/// between probe-verus atoms and probe-lean atoms via a translations file.
///
/// Examples:
/// - `("curve25519-dalek/src/backend/serial/u64/field.rs", "FieldElement51::reduce")`
///   → `"curve25519_dalek::backend::serial::u64::field::FieldElement51::reduce"`
/// - `("curve25519-dalek/src/backend/mod.rs", "variable_base_mul")`
///   → `"curve25519_dalek::backend::variable_base_mul"`
pub fn derive_rust_qualified_name(code_path: &str, display_name: &str) -> Option<String> {
    if code_path.is_empty() {
        return None;
    }

    // Strip crate directory prefix: "crate-name/src/..." → "..."
    let parts: Vec<&str> = code_path.splitn(2, "/src/").collect();
    if parts.len() != 2 {
        return None;
    }

    let crate_name = parts[0]
        .rsplit('/')
        .next()
        .unwrap_or(parts[0])
        .replace('-', "_");

    // Convert file path to module path: "backend/serial/u64/field.rs" → "backend::serial::u64::field"
    let file_path = parts[1];
    let module_path = file_path
        .trim_end_matches(".rs")
        .trim_end_matches("/mod")
        .replace('/', "::");

    if module_path.is_empty() || module_path == "lib" {
        Some(format!("{}::{}", crate_name, display_name))
    } else {
        Some(format!("{}::{}::{}", crate_name, module_path, display_name))
    }
}

/// For impl methods, prepend the Self type to produce "Type::method" display names.
/// Free functions are returned unchanged.
///
///   `path/impl#[Type][Trait]method().`   ->  `Type::method`
///   `path/impl#[`&Type`]method().`       ->  `Type::method`
///   `path/Trait#method().` (trait decl)  ->  `Trait::method`
///   `path/function().`                   ->  `function` (unchanged)
fn enrich_display_name(scip_symbol: &str, base_display_name: &str) -> String {
    if let Some(seg) = parse_impl_segment(scip_symbol) {
        let self_type = bare_type_name(seg.self_type);
        if !self_type.is_empty() {
            return format!("{}::{}", self_type, base_display_name);
        }
        return base_display_name.to_string();
    }
    let s = scip_symbol
        .strip_prefix(SCIP_SYMBOL_PREFIX)
        .unwrap_or(scip_symbol);
    // After stripping the prefix, the remaining format is "crate version path/..."
    let parts: Vec<&str> = s.splitn(3, ' ').collect();
    if parts.len() < 3 {
        return base_display_name.to_string();
    }
    let path_part = parts[2].trim_end_matches('.');
    let last_segment = path_part.rsplit('/').next().unwrap_or(path_part);
    if let Some((owner, _)) = last_segment.split_once('#') {
        if !owner.is_empty() {
            return format!("{}::{}", owner, base_display_name);
        }
    }
    base_display_name.to_string()
}

/// `rust-analyzer 0.3.N` patch version reported by the first verus-analyzer
/// release with the rust-analyzer symbol format (2026-08-22). 2026-06-17
/// reports `0.3.264`.
pub(crate) const FIRST_CURRENT_FORMAT_ANALYZER_PATCH: u32 = 266;

/// The `N` of a `0.3.N[-suffix]` SCIP tool version, if it has that shape.
pub(crate) fn analyzer_patch_version(version: &str) -> Option<u32> {
    let rest = version.strip_prefix("0.3.")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Why a SCIP index uses the pre-2026-08-22 verus-analyzer symbol format
/// (`module/Type#Trait#method().`, Self type sometimes missing) instead of the
/// rust-analyzer format (`module/impl#[Type][Trait]method().`), or `None` if
/// nothing points to it.
///
/// probe-verus code_names are derived directly from the symbol, so a legacy
/// index yields ambiguous and inconsistent code_names. Evidence, any of which
/// rejects the index (a mixed index is rejected too):
/// - the producer reports a `0.3.N` version older than 2026-08-22;
/// - a function symbol of the form `Type#Trait#method()`;
/// - a function symbol `Owner#method()` whose owner is defined in the index
///   and is not a trait. In the current format `Owner#method()` only names a
///   trait method declaration, so an owner without a symbol record is no
///   evidence either way.
///
/// An index with no `0.3.N` producer version and no such symbols (e.g. only free
/// functions, from an unknown producer) is accepted: the syntax cannot tell the
/// formats apart, and rejecting unknown producers would also reject upstream
/// rust-analyzer builds.
#[must_use]
pub fn legacy_symbol_format_reason(scip_data: &ScipIndex) -> Option<String> {
    let tool = &scip_data.metadata.tool_info;
    if analyzer_patch_version(&tool.version)
        .is_some_and(|patch| patch < FIRST_CURRENT_FORMAT_ANALYZER_PATCH)
    {
        return Some(format!(
            "producer {} {} is older than 0.3.{}",
            tool.name, tool.version, FIRST_CURRENT_FORMAT_ANALYZER_PATCH
        ));
    }

    let kinds: HashMap<&str, i32> = scip_data
        .documents
        .iter()
        .flat_map(|d| &d.symbols)
        .map(|s| (s.symbol.as_str(), s.kind))
        .collect();
    for symbol in scip_data.documents.iter().flat_map(|d| &d.symbols) {
        if !is_function_like(symbol.kind) {
            continue;
        }
        let last_segment = last_symbol_segment(&symbol.symbol);
        if last_segment.starts_with("impl#") {
            continue;
        }
        let hashes = unquoted_positions(last_segment, '#');
        match hashes.len() {
            0 => {}
            1 => {
                let owner_end = symbol.symbol.len() - last_segment.len() + hashes[0] + 1;
                let owner = &symbol.symbol[..owner_end];
                if kinds
                    .get(owner)
                    .is_some_and(|kind| *kind != constants::SCIP_KIND_TRAIT)
                {
                    return Some(format!(
                        "`{}` names a method by its non-trait owner",
                        symbol.symbol
                    ));
                }
            }
            _ => {
                return Some(format!(
                    "`{}` names a trait impl method as `Type#Trait#method()`",
                    symbol.symbol
                ));
            }
        }
    }
    None
}

/// Byte positions of `needle` in `s` outside backtick-quoted descriptors.
fn unquoted_positions(s: &str, needle: char) -> Vec<usize> {
    let mut quoted = false;
    let mut positions = Vec::new();
    for (i, c) in s.char_indices() {
        if c == '`' {
            quoted = !quoted;
        } else if c == needle && !quoted {
            positions.push(i);
        }
    }
    positions
}

/// The part of a SCIP symbol after its last `/`, or after the package fields
/// (`rust-analyzer cargo <package> <version> `) for a crate-root item. `/` and
/// spaces inside backtick-quoted descriptors do not count (``Tag<'/'>``,
/// ``&'a Scalar``).
fn last_symbol_segment(symbol: &str) -> &str {
    let slash = unquoted_positions(symbol, '/').last().copied();
    let space = unquoted_positions(symbol, ' ').last().copied();
    match slash.max(space) {
        Some(i) => &symbol[i + 1..],
        None => symbol,
    }
}

/// Whether [`legacy_symbol_format_reason`] finds evidence of the legacy format.
#[must_use]
pub fn uses_legacy_symbol_format(scip_data: &ScipIndex) -> bool {
    legacy_symbol_format_reason(scip_data).is_some()
}

/// Crates whose absence from an index shows that the analyzer did not load the
/// project: the Verus standard library and builtins.
const VERUS_DEPENDENCY_CRATES: [&str; 3] = ["vstd", "verus_builtin", "builtin"];

/// The direct, non-dev dependencies of a package manifest that are Verus crates
/// ([`VERUS_DEPENDENCY_CRATES`]), by package name (a `package = "vstd"` rename
/// counts as `vstd`). Returns an empty list if the manifest cannot be parsed.
pub fn verus_dependencies(manifest: &str) -> Vec<String> {
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut tables: Vec<&toml::Table> = Vec::new();
    if let Some(deps) = doc.get("dependencies").and_then(|d| d.as_table()) {
        tables.push(deps);
    }
    if let Some(targets) = doc.get("target").and_then(|t| t.as_table()) {
        for target in targets.values() {
            if let Some(deps) = target.get("dependencies").and_then(|d| d.as_table()) {
                tables.push(deps);
            }
        }
    }
    let mut names: Vec<String> = tables
        .iter()
        .flat_map(|t| t.iter())
        .map(|(key, value)| {
            value
                .get("package")
                .and_then(|p| p.as_str())
                .unwrap_or(key)
                .to_string()
        })
        .filter(|name| VERUS_DEPENDENCY_CRATES.contains(&name.as_str()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Why [`validate_index`] rejects an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexError {
    /// The index uses the symbol format of verus-analyzer before 2026-08-22.
    Legacy(String),
    /// The index misses a crate that the package depends on.
    Incomplete(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IndexError::Legacy(m) | IndexError::Incomplete(m) => f.write_str(m),
        }
    }
}

/// Check a SCIP index before converting it. Returns warnings, or an error that
/// must stop the run:
/// - the index uses the legacy symbol format ([`legacy_symbol_format_reason`]);
/// - a Verus crate in `verus_deps` (see [`verus_dependencies`]) is never
///   referenced. The analyzer then did not load the project, and the index
///   misses most calls. This is a heuristic: it does not prove completeness.
///
/// The warning case is an index with no evidence of either format: no known
/// producer version and no `impl#[` method symbol.
pub fn validate_index(
    scip_data: &ScipIndex,
    verus_deps: &[String],
) -> Result<Vec<String>, IndexError> {
    if let Some(reason) = legacy_symbol_format_reason(scip_data) {
        return Err(IndexError::Legacy(format!(
            "this index comes from a verus-analyzer older than 2026-08-22 ({reason}). \
             Use verus-analyzer 2026-08-22 or later, or probe-verus 8.x."
        )));
    }

    let referenced: HashSet<&str> = scip_data
        .documents
        .iter()
        .flat_map(|d| &d.occurrences)
        .filter_map(|o| o.symbol.strip_prefix(SCIP_SYMBOL_PREFIX))
        .filter_map(|rest| rest.split(' ').next())
        .collect();
    let missing: Vec<&str> = verus_deps
        .iter()
        .map(String::as_str)
        .filter(|dep| !referenced.contains(dep))
        .collect();
    if !missing.is_empty() {
        return Err(IndexError::Incomplete(format!(
            "the index has no references to {}, a direct dependency of the package. \
             verus-analyzer probably failed to load the project, so the index is incomplete.",
            missing.join(", ")
        )));
    }

    let tool = &scip_data.metadata.tool_info;
    let known_version = analyzer_patch_version(&tool.version).is_some();
    let has_impl_symbol = scip_data
        .documents
        .iter()
        .flat_map(|d| &d.symbols)
        .any(|s| is_function_like(s.kind) && last_symbol_segment(&s.symbol).starts_with("impl#"));
    let mut warnings = Vec::new();
    if !known_version && !has_impl_symbol {
        warnings.push(format!(
            "cannot tell which verus-analyzer produced this index (producer `{} {}`, \
             no impl method symbols). probe-verus assumes 2026-08-22 or later.",
            tool.name, tool.version
        ));
    }
    Ok(warnings)
}

/// Build a call graph from SCIP data, with one node per definition occurrence of
/// a function symbol, keyed by [`make_definition_key`].
pub fn build_call_graph(scip_data: &ScipIndex) -> HashMap<String, FunctionNode> {
    let mut call_graph: HashMap<String, FunctionNode> = HashMap::new();

    // All function symbols with a symbol record, used to recognise calls.
    let mut all_function_symbols: HashSet<String> = scip_data
        .documents
        .iter()
        .flat_map(|d| &d.symbols)
        .filter(|s| is_function_like(s.kind))
        .map(|s| s.symbol.clone())
        .collect();

    // First pass: a node per definition occurrence of a function symbol that has
    // a `symbols[]` record in the same document. A symbol normally has one
    // definition and one record; analyzer bugs can yield several of each, in
    // which case the nth record (in document order) is assumed to describe the
    // nth definition (by range). SCIP does not guarantee that order, so metadata
    // (signature, visibility) of duplicated symbols is approximate.
    for doc in &scip_data.documents {
        let rel_path = doc.relative_path.trim_start_matches('/');
        let mut records: HashMap<&str, Vec<&Symbol>> = HashMap::new();
        for symbol in doc.symbols.iter().filter(|s| is_function_like(s.kind)) {
            records.entry(&symbol.symbol).or_default().push(symbol);
        }
        let mut definitions: BTreeMap<&str, Vec<&Occurrence>> = BTreeMap::new();
        for occurrence in &doc.occurrences {
            if is_definition(occurrence.symbol_roles)
                && occurrence.range.len() >= 2
                && records.contains_key(occurrence.symbol.as_str())
            {
                definitions
                    .entry(&occurrence.symbol)
                    .or_default()
                    .push(occurrence);
            }
        }
        for (symbol, mut defs) in definitions {
            defs.sort_by(|a, b| a.range.cmp(&b.range));
            let recs = &records[symbol];
            let paired = recs.len() == defs.len();
            if !paired && recs.len() > 1 {
                eprintln!(
                    "Warning: {symbol} has {} definitions but {} symbol records in {rel_path}; \
                     using the first record for all definitions",
                    defs.len(),
                    recs.len()
                );
            }
            for (i, occurrence) in defs.into_iter().enumerate() {
                let record = if paired { recs[i] } else { recs[0] };
                let key = make_definition_key(symbol, rel_path, &occurrence.range);
                if call_graph.contains_key(&key) {
                    eprintln!("Warning: duplicate definition occurrence {key}, ignored");
                    continue;
                }
                let base_display_name = record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                call_graph.insert(
                    key,
                    FunctionNode {
                        symbol: symbol.to_string(),
                        display_name: enrich_display_name(symbol, &base_display_name),
                        signature_text: record.signature_documentation.text.clone(),
                        relative_path: rel_path.to_string(),
                        callees: HashSet::new(),
                        range: occurrence.range.clone(),
                    },
                );
            }
        }
    }

    // Second pass: attribute every call to the enclosing project function.
    for doc in &scip_data.documents {
        let rel_path = doc.relative_path.trim_start_matches('/');
        let mut current_function_key: Option<String> = None;

        let mut ordered_occurrences = doc.occurrences.clone();
        ordered_occurrences.retain(|o| o.range.len() >= 2);
        ordered_occurrences.sort_by(|a, b| {
            let a_start = (a.range[0], a.range[1]);
            let b_start = (b.range[0], b.range[1]);
            a_start.cmp(&b_start)
        });

        for occurrence in &ordered_occurrences {
            // Track when we enter a project function definition
            if is_definition(occurrence.symbol_roles) {
                let key = make_definition_key(&occurrence.symbol, rel_path, &occurrence.range);
                if call_graph.contains_key(&key) {
                    current_function_key = Some(key);
                }
                continue;
            }

            // Track ALL function calls (including to external functions)
            if !(all_function_symbols.contains(&occurrence.symbol)
                || is_external_function_symbol(&occurrence.symbol, &all_function_symbols))
            {
                continue;
            }
            all_function_symbols.insert(occurrence.symbol.clone());
            // A reference to the caller's own symbol is kept: it may name another
            // definition that shares the symbol. A reference to a duplicated
            // symbol cannot be resolved to one definition, so it fans out to all
            // of them except the caller (self-edges are dropped when the symbol is
            // resolved to code_names).
            if let Some(caller_node) = current_function_key
                .as_ref()
                .and_then(|key| call_graph.get_mut(key))
            {
                caller_node.callees.insert(CalleeInfo {
                    symbol: occurrence.symbol.clone(),
                    line: occurrence.range[0],
                });
            }
        }
    }

    call_graph
}

/// Extract the module path from a probe_name.
///
/// Given a probe_name like "probe:curve25519-dalek/4.1.3/montgomery/MontgomeryPoint#ct_eq()",
/// extracts the module path (everything between version and the type name).
///
/// Example: "probe:curve25519-dalek/4.1.3/montgomery/MontgomeryPoint#ct_eq()" -> "montgomery"
/// Example: "probe:crate/0.1.0/foo/bar/Baz#method()" -> "foo/bar"
/// Example: "probe:crate/0.1.0/TopLevel#method()" -> "" (no module path)
fn extract_code_module(probe_name: &str) -> String {
    // Strip "probe:" prefix
    let s = probe_name
        .strip_prefix(PROBE_URI_PREFIX)
        .unwrap_or(probe_name);

    // Find the position of "#" which marks the type/method boundary
    let hash_pos = s.find('#').unwrap_or(s.len());
    let before_hash = &s[..hash_pos];

    // Find positions of "/" to skip crate and version
    let slashes: Vec<usize> = before_hash.match_indices('/').map(|(i, _)| i).collect();

    // Need at least 2 slashes (after crate, after version)
    // If there's a 3rd slash, there's a module path
    if slashes.len() < 3 {
        return String::new();
    }

    // Module path is between second slash (after version) and last slash (before type)
    let start = slashes[1] + 1;
    let end = slashes[slashes.len() - 1];

    if start < end {
        before_hash[start..end].to_string()
    } else {
        String::new()
    }
}

/// Convert a SCIP symbol to a probe code_name.
///
/// The SCIP symbol already identifies the definition uniquely (Self type and
/// trait included), so the conversion is purely syntactic: strip the
/// `rust-analyzer cargo ` prefix and the trailing `.`, drop lifetimes, and turn
/// spaces into `/`.
///
/// `rust-analyzer cargo curve25519-dalek 4.1.3 montgomery/impl#[`&MontgomeryPoint`][`Mul<&'a Scalar>`]mul().`
/// becomes ``probe:curve25519-dalek/4.1.3/montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul()``.
///
/// `line_number` is appended as `@line` to separate definitions that share a symbol.
fn symbol_to_code_name(symbol: &str, line_number: Option<usize>) -> String {
    let s = symbol.strip_prefix(SCIP_SYMBOL_PREFIX).unwrap_or_else(|| {
        eprintln!(
            "Warning: Symbol does not start with '{}': {}",
            SCIP_SYMBOL_PREFIX, symbol
        );
        symbol
    });
    let s = s.strip_suffix('.').unwrap_or(s);
    let mut result = strip_lifetimes(s).replace(' ', "/");
    if let Some(line) = line_number {
        result = format!("{}@{}", result, line);
    }
    format!("{}{}", PROBE_URI_PREFIX, result)
}

/// Convert call graph to atoms with line numbers format.
///
/// This version uses only SCIP data, which only provides the function NAME location,
/// so lines_start and lines_end will be the same (or close for multi-line spans).
/// For accurate function body spans, use `convert_to_atoms_with_parsed_spans` instead.
pub fn convert_to_atoms_with_lines(
    call_graph: &HashMap<String, FunctionNode>,
) -> Vec<AtomWithLines> {
    let empty_map = HashMap::new();
    convert_to_atoms_with_lines_internal(call_graph, None, false, &empty_map, false, "", "")
}

/// Convert call graph to atoms with accurate line numbers by parsing source files.
///
/// This version uses verus_syn to parse source files and get accurate function body spans.
/// `code_path_prefix` is prepended to the SCIP `relative_path` when building the atom's
/// `code_path` field (e.g., `"curve25519-dalek"` for workspace members). Internal lookups
/// (span matching, module visibility) still use the raw SCIP path.
#[allow(clippy::too_many_arguments)]
pub fn convert_to_atoms_with_parsed_spans(
    call_graph: &HashMap<String, FunctionNode>,
    project_root: &Path,
    with_locations: bool,
    file_module_pub: &HashMap<String, ModuleInfo>,
    is_library: bool,
    code_path_prefix: &str,
    pkg_name: &str,
) -> Vec<AtomWithLines> {
    // Collect all unique relative paths (sorted for deterministic file traversal per P14)
    let mut relative_paths: Vec<String> = call_graph
        .values()
        .map(|node| node.relative_path.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    relative_paths.sort();

    // Build the span map by parsing all source files
    let span_map = verus_parser::build_function_span_map(project_root, &relative_paths);

    convert_to_atoms_with_lines_internal(
        call_graph,
        Some(&span_map),
        with_locations,
        file_module_pub,
        is_library,
        code_path_prefix,
        pkg_name,
    )
}

/// Internal function that does the actual conversion.
/// Uses a multi-pass approach:
/// 1. Compute final code_names for all atoms (with line numbers for duplicates)
/// 2. Build a map: raw_symbol → list of final_code_names
/// 3. Resolve dependencies using the map (include all matches for ambiguous refs)
#[allow(clippy::too_many_arguments)]
fn convert_to_atoms_with_lines_internal(
    call_graph: &HashMap<String, FunctionNode>,
    span_map: Option<&HashMap<(String, String, usize), verus_parser::SpanAndMode>>,
    with_locations: bool,
    file_module_pub: &HashMap<String, ModuleInfo>,
    is_library: bool,
    code_path_prefix: &str,
    pkg_name: &str,
) -> Vec<AtomWithLines> {
    // === Phase 1: Compute line ranges and base code_names for all nodes ===
    struct NodeData<'a> {
        node: &'a FunctionNode,
        lines_start: usize,
        lines_end: usize,
        base_code_name: String,
        kind: DeclKind,
        /// "verus" if found in verus_syn span map, "rust" otherwise
        language: String,
        /// Line range of requires clause, if present
        requires_range: Option<(usize, usize)>,
        /// Line range of ensures clause, if present
        ensures_range: Option<(usize, usize)>,
        has_body: bool,
        is_external: bool,
        is_cfg: bool,
    }

    let node_data: Vec<NodeData> = call_graph
        .values()
        .map(|node| {
            let lines_start = if !node.range.is_empty() {
                node.range[0] as usize + 1
            } else {
                0
            };

            let sam = span_map.and_then(|map| {
                verus_parser::get_span_and_mode(
                    map,
                    &node.relative_path,
                    &node.display_name,
                    lines_start,
                )
            });

            let lines_end = sam
                .map(|s| s.end_line)
                .unwrap_or_else(|| match node.range.len() {
                    4 => node.range[2] as usize + 1,
                    _ => lines_start,
                });

            let (kind, language) = sam
                .map(|s| {
                    let lang = if s.kind == DeclKind::Exec {
                        "rust"
                    } else {
                        "verus"
                    };
                    (s.kind, lang.to_string())
                })
                .unwrap_or((DeclKind::Exec, "rust".to_string()));

            let (requires_range, ensures_range) = sam
                .map(|s| (s.requires_range, s.ensures_range))
                .unwrap_or((None, None));

            let has_body = sam.map(|s| s.has_body).unwrap_or(true);
            let is_external = sam.map(|s| s.is_external).unwrap_or(false);
            let is_cfg = sam.map(|s| s.is_cfg).unwrap_or(false);

            let base_code_name = symbol_to_code_name(&node.symbol, None);

            NodeData {
                node,
                lines_start,
                lines_end,
                base_code_name,
                kind,
                language,
                requires_range,
                ensures_range,
                has_body,
                is_external,
                is_cfg,
            }
        })
        .collect();

    // === Phase 2: Detect duplicates and compute final code_names ===
    // Symbols are unique per definition except for rare analyzer bugs; such
    // duplicates get an `@line` suffix, or `@path:line:column` when that still
    // collides (same line in different files or columns).
    let mut code_name_count: HashMap<&str, usize> = HashMap::new();
    for data in &node_data {
        *code_name_count.entry(&data.base_code_name).or_insert(0) += 1;
    }
    let line_code_names: Vec<String> = node_data
        .iter()
        .map(|data| {
            let is_duplicate = code_name_count[data.base_code_name.as_str()] > 1;
            if is_duplicate && data.lines_start > 0 {
                format!("{}@{}", data.base_code_name, data.lines_start)
            } else {
                data.base_code_name.clone()
            }
        })
        .collect();
    let mut line_code_name_count: HashMap<&str, usize> = HashMap::new();
    for name in &line_code_names {
        *line_code_name_count.entry(name).or_insert(0) += 1;
    }
    let final_code_names: Vec<String> = node_data
        .iter()
        .zip(&line_code_names)
        .map(|(data, name)| {
            if line_code_name_count[name.as_str()] > 1 && data.node.range.len() >= 2 {
                format!(
                    "{}@{}:{}:{}",
                    data.base_code_name,
                    data.node.relative_path,
                    data.lines_start,
                    data.node.range[1] + 1
                )
            } else {
                name.clone()
            }
        })
        .collect();

    // === Phase 3: Build map from raw symbol → list of code_names ===
    let mut raw_symbol_to_code_names: HashMap<String, Vec<String>> = HashMap::new();
    for (data, final_name) in node_data.iter().zip(final_code_names.iter()) {
        raw_symbol_to_code_names
            .entry(data.node.symbol.clone())
            .or_default()
            .push(final_name.clone());
    }

    // Helper to classify call location based on line number and spec ranges
    fn classify_call_location(
        call_line: i32,
        requires_range: Option<(usize, usize)>,
        ensures_range: Option<(usize, usize)>,
    ) -> CallLocation {
        // SCIP uses 0-based lines, verus_syn uses 1-based - convert
        let call_line_1based = (call_line + 1) as usize;

        if let Some((start, end)) = requires_range {
            if call_line_1based >= start && call_line_1based <= end {
                return CallLocation::Precondition;
            }
        }

        if let Some((start, end)) = ensures_range {
            if call_line_1based >= start && call_line_1based <= end {
                return CallLocation::Postcondition;
            }
        }

        CallLocation::Inner
    }

    // === Phase 4: Build final atoms with resolved dependencies ===
    node_data
        .into_iter()
        .zip(final_code_names)
        .map(|(data, code_name)| {
            // Resolve dependencies: map raw symbols to their full code_names
            let mut dependencies = BTreeSet::new();
            let mut dependencies_with_locations: Vec<DependencyWithLocation> = Vec::new();

            for callee in &data.node.callees {
                // A project symbol maps to its code_name(s); a symbol shared by several
                // definitions (analyzer bug) resolves to all of them. Anything else is
                // an external function.
                let dep_code_names: Vec<String> = match raw_symbol_to_code_names.get(&callee.symbol)
                {
                    // Recursion (the function's own code_name) is not a dependency.
                    Some(names) => names.iter().filter(|n| **n != code_name).cloned().collect(),
                    None => vec![symbol_to_code_name(&callee.symbol, None)],
                };
                for dep_code_name in dep_code_names {
                    if with_locations {
                        dependencies_with_locations.push(DependencyWithLocation {
                            code_name: dep_code_name.clone(),
                            location: classify_call_location(
                                callee.line,
                                data.requires_range,
                                data.ensures_range,
                            ),
                            line: (callee.line + 1) as usize,
                        });
                    }
                    dependencies.insert(dep_code_name);
                }
            }

            let code_module = extract_code_module(&code_name);
            let output_code_path = if code_path_prefix.is_empty() {
                data.node.relative_path.clone()
            } else {
                format!("{}/{}", code_path_prefix, data.node.relative_path)
            };
            // For RQN, ensure path has "crate-name/src/..." format so
            // derive_rust_qualified_name can split on "/src/".
            let rqn_path = if output_code_path.contains("/src/") {
                output_code_path.clone()
            } else if !pkg_name.is_empty() && output_code_path.starts_with("src/") {
                format!("{}/{}", pkg_name, output_code_path)
            } else {
                output_code_path.clone()
            };
            let rqn = derive_rust_qualified_name(&rqn_path, &data.node.display_name);
            dependencies_with_locations.sort_by(|a, b| {
                a.line
                    .cmp(&b.line)
                    .then_with(|| a.code_name.cmp(&b.code_name))
            });
            let sig_public = is_signature_public(&data.node.signature_text);
            let module_cfg = file_module_pub
                .get(&data.node.relative_path)
                .map(|mi| mi.is_cfg)
                .unwrap_or(false);
            AtomWithLines {
                display_name: data.node.display_name.clone(),
                code_name: code_name.clone(),
                dependencies,
                dependencies_with_locations,
                code_module,
                code_path: output_code_path,
                code_text: CodeTextInfo {
                    lines_start: data.lines_start,
                    lines_end: data.lines_end,
                },
                kind: data.kind,
                language: data.language,
                rust_qualified_name: rqn,
                is_public: Some(sig_public),
                is_public_api: classify_public_api(
                    sig_public,
                    &code_name,
                    &data.node.relative_path,
                    data.kind,
                    file_module_pub,
                    is_library,
                ),
                has_body: Some(data.has_body),
                is_external: Some(data.is_external),
                is_cfg_gated: Some(data.is_cfg || module_cfg),
            }
        })
        .collect()
}

/// Information about a duplicate code_name
#[derive(Debug, Clone)]
pub struct DuplicateCodeName {
    pub code_name: String,
    pub occurrences: Vec<DuplicateOccurrence>,
}

#[derive(Debug, Clone)]
pub struct DuplicateOccurrence {
    pub display_name: String,
    pub code_path: String,
    pub lines_start: usize,
}

/// Check for duplicate code_names in the atoms output.
/// Returns a list of code_names that appear more than once.
///
/// This is useful for detecting cases where the disambiguation logic fails,
/// such as trait implementations that can't be distinguished by signature alone.
pub fn find_duplicate_code_names(atoms: &[AtomWithLines]) -> Vec<DuplicateCodeName> {
    let mut code_name_to_atoms: HashMap<String, Vec<&AtomWithLines>> = HashMap::new();

    for atom in atoms {
        code_name_to_atoms
            .entry(atom.code_name.clone())
            .or_default()
            .push(atom);
    }

    code_name_to_atoms
        .into_iter()
        .filter(|(_, atoms)| atoms.len() > 1)
        .map(|(code_name, atoms)| DuplicateCodeName {
            code_name,
            occurrences: atoms
                .into_iter()
                .map(|a| DuplicateOccurrence {
                    display_name: a.display_name.clone(),
                    code_path: a.code_path.clone(),
                    lines_start: a.code_text.lines_start,
                })
                .collect(),
        })
        .collect()
}

/// Extract a display name from a probe-style code_name.
///
/// Given `probe:x25519-dalek/2.0.1/x25519/impl#[StaticSecret]diffie_hellman()`,
/// returns `"diffie_hellman"`.
fn extract_display_name_from_code_name(code_name: &str) -> String {
    let s = code_name
        .strip_prefix(PROBE_URI_PREFIX)
        .unwrap_or(code_name);
    // Strip trailing `().` or `()` (SCIP symbols use `().`, probe code_names use `()`)
    let without_parens = s
        .strip_suffix("().")
        .or_else(|| s.strip_suffix("()"))
        .unwrap_or(s);
    // Take the part after the last delimiter
    let name = without_parens
        .rsplit_once(']')
        .map(|(_, n)| n)
        .or_else(|| without_parens.rsplit_once('#').map(|(_, n)| n))
        .or_else(|| without_parens.rsplit_once('/').map(|(_, n)| n))
        .unwrap_or(without_parens);
    name.to_string()
}

/// The owner of a method, as written in a Verus path such as an
/// `assume_specification` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodOwner<'a> {
    /// `Owner::method`: `Owner` is the impl Self type, or the trait of a trait
    /// method declaration. Compared by bare name.
    Path(&'a str),
    /// `<SelfType as Trait>::method` (or `<SelfType>::method` with no trait):
    /// the impl must have exactly this Self type and trait.
    Qualified {
        self_type: &'a str,
        trait_type: Option<&'a str>,
    },
}

/// Normalize a type for comparison across Verus paths, SCIP symbols and
/// code_names: drop lifetimes and path qualifiers, and keep whitespace (which
/// code_names write as `/`) only as one space between two words. Generic
/// arguments and references are kept.
/// `&'a crate::Foo<core::Bar>` -> `&Foo<Bar>`, `[T;/N]` -> `[T;N]`,
/// `&mut/T` -> `&mut T`.
fn normalize_type(ty: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let ty = strip_lifetimes(&ty.replace('/', " "));
    let mut out = String::with_capacity(ty.len());
    let mut pending_space = false;
    let mut rest = ty.as_str();
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("::") {
            while out.chars().last().is_some_and(is_word) {
                out.pop();
            }
            pending_space = false;
            rest = after;
            continue;
        }
        if c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && is_word(c) && out.chars().last().is_some_and(is_word) {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Whether the impl trait `candidate` (from a code_name) is the trait `wanted`
/// (from a Verus path). Without generic arguments in `wanted`, bare names are
/// compared; with them, the full normalized types.
fn trait_matches(candidate: &str, wanted: &str) -> bool {
    let wanted = normalize_type(wanted);
    if wanted.contains('<') {
        normalize_type(candidate) == wanted
    } else {
        bare_type_name(candidate) == wanted
    }
}

/// Whether `code_name` is the method `method` of `owner`.
///
/// - [`MethodOwner::Path`] matches an impl whose Self type has that bare name
///   (`impl#[Owner<..>][..]method()`) or a trait method declaration
///   (`Owner#method()`), not an impl *of* a trait named `Owner`.
/// - [`MethodOwner::Qualified`] matches only an impl with that Self type and
///   trait (an inherent impl when there is no trait); a trait declaration
///   never matches.
#[must_use]
pub fn code_name_is_method(code_name: &str, owner: MethodOwner<'_>, method: &str) -> bool {
    if extract_display_name_from_code_name(code_name) != method {
        return false;
    }
    let seg = parse_impl_segment(code_name);
    match owner {
        MethodOwner::Path(owner) => match seg {
            Some(seg) => bare_type_name(seg.self_type) == owner,
            None => code_name
                .rsplit('/')
                .next()
                .and_then(|last| last.split_once('#'))
                .is_some_and(|(o, _)| o == owner),
        },
        MethodOwner::Qualified {
            self_type,
            trait_type,
        } => seg.is_some_and(|seg| {
            normalize_type(seg.self_type) == normalize_type(self_type)
                && match (seg.trait_type, trait_type) {
                    (Some(have), Some(want)) => trait_matches(have, want),
                    (None, None) => true,
                    _ => false,
                }
        }),
    }
}

/// Normalize a code_name by stripping a trailing dot if present.
///
/// SCIP external function symbols end with `().` but probe code_names use `()`.
/// This function ensures consistent code_names for merging atoms from different sources.
pub fn normalize_code_name(code_name: &str) -> String {
    code_name.strip_suffix('.').unwrap_or(code_name).to_string()
}

// =============================================================================
// Workspace / package resolution
// =============================================================================

/// Redirect a workspace-only root to the correct member package directory.
///
/// Must be called **before** any work (SCIP generation, metadata, span maps) so
/// that every downstream path is relative to the package, not the workspace root.
///
/// Behavior by `Cargo.toml` shape:
/// - `[package]` present (with or without `[workspace]`): return `project_path` as-is.
/// - `[workspace]` only, `package` arg matches a member: return that member dir.
/// - `[workspace]` only, single member, no `package` arg: auto-resolve to the member.
/// - `[workspace]` only, multiple members, no `package` arg: return `Err` listing members.
/// - No `[package]` and no `[workspace]`: return `project_path` as-is (fallback).
pub fn resolve_workspace_root(
    project_path: &Path,
    package: Option<&str>,
) -> Result<PathBuf, String> {
    let cargo_toml = project_path.join("Cargo.toml");
    let contents = match std::fs::read_to_string(&cargo_toml) {
        Ok(c) => c,
        Err(_) => return Ok(project_path.to_path_buf()),
    };
    let table: toml::Table = match contents.parse() {
        Ok(t) => t,
        Err(_) => return Ok(project_path.to_path_buf()),
    };

    if table.contains_key("package") {
        return Ok(project_path.to_path_buf());
    }

    let members = match table
        .get("workspace")
        .and_then(|w| w.as_table())
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        Some(m) => m,
        None => return Ok(project_path.to_path_buf()),
    };

    let member_strings: Vec<&str> = members.iter().filter_map(|m| m.as_str()).collect();

    if let Some(pkg) = package {
        for &member_path in &member_strings {
            let dir = project_path.join(member_path);
            let member_toml = dir.join("Cargo.toml");
            if let Ok(mc) = std::fs::read_to_string(&member_toml) {
                if let Ok(mt) = mc.parse::<toml::Table>() {
                    let name = mt
                        .get("package")
                        .and_then(|p| p.as_table())
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str());
                    if name == Some(pkg) {
                        eprintln!(
                            "  Note: workspace root detected, resolving to member '{}'",
                            member_path
                        );
                        return Ok(dir);
                    }
                }
            }
        }
        return Err(format!(
            "'{}' is a workspace root, but no member matches --package '{}'.\n\n\
             Workspace members:\n{}\n",
            project_path.display(),
            pkg,
            member_strings
                .iter()
                .map(|m| format!("  - {m}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }

    if member_strings.len() == 1 {
        let dir = project_path.join(member_strings[0]);
        if dir.exists() {
            eprintln!(
                "  Note: workspace root detected, auto-resolving to member '{}'",
                member_strings[0]
            );
            return Ok(dir);
        }
        return Err(format!(
            "'{}' is a workspace root with member '{}', \
             but the member directory does not exist.\n",
            project_path.display(),
            member_strings[0],
        ));
    }

    let hint = member_strings
        .iter()
        .map(|m| format!("  probe-verus extract {}/{m}", project_path.display()))
        .collect::<Vec<_>>()
        .join("\n");

    Err(format!(
        "'{}' is a workspace root with multiple members. \
         Please specify which package to analyze.\n\n\
         Workspace members:\n{}\n\n\
         Run one of:\n{hint}\n\n\
         Or use --package <NAME>:\n  \
         probe-verus extract {} --package <NAME>\n",
        project_path.display(),
        member_strings
            .iter()
            .map(|m| format!("  - {m}"))
            .collect::<Vec<_>>()
            .join("\n"),
        project_path.display(),
    ))
}

// =============================================================================
// Package root resolution (source root within a project)
// =============================================================================

/// Resolve the source root for a package within a workspace.
///
/// For workspace-only `Cargo.toml` files (containing `[workspace]` but no `[package]`),
/// finds the member directory whose `Cargo.toml` `[package].name` matches `package`,
/// or falls back to a single-member workspace. Returns `project_path` unchanged if
/// it already contains a `[package]` section or no workspace is detected.
#[must_use]
pub fn resolve_package_root(project_path: &Path, package: Option<&str>) -> PathBuf {
    let cargo_toml = project_path.join("Cargo.toml");
    let contents = match std::fs::read_to_string(&cargo_toml) {
        Ok(c) => c,
        Err(_) => return project_path.to_path_buf(),
    };
    let table: toml::Table = match contents.parse() {
        Ok(t) => t,
        Err(_) => return project_path.to_path_buf(),
    };

    if table.contains_key("package") {
        return project_path.to_path_buf();
    }

    let members = match table
        .get("workspace")
        .and_then(|w| w.as_table())
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        Some(m) => m,
        None => return project_path.to_path_buf(),
    };

    if let Some(pkg) = package {
        for m in members {
            if let Some(member_path) = m.as_str() {
                let dir = project_path.join(member_path);
                let member_toml = dir.join("Cargo.toml");
                if let Ok(mc) = std::fs::read_to_string(&member_toml) {
                    if let Ok(mt) = mc.parse::<toml::Table>() {
                        let name = mt
                            .get("package")
                            .and_then(|p| p.as_table())
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str());
                        if name == Some(pkg) {
                            return dir;
                        }
                    }
                }
            }
        }
    }

    if members.len() == 1 {
        if let Some(member) = members[0].as_str() {
            let dir = project_path.join(member);
            if dir.exists() {
                return dir;
            }
        }
    }

    project_path.to_path_buf()
}

/// Check whether a Rust project is a library crate.
///
/// Returns `true` if `Cargo.toml` contains a `[lib]` section or `src/lib.rs` exists.
#[must_use]
pub fn is_library_crate(project_path: &Path) -> bool {
    let cargo_toml = project_path.join("Cargo.toml");
    if let Ok(contents) = std::fs::read_to_string(&cargo_toml) {
        if let Ok(parsed) = contents.parse::<toml::Table>() {
            if parsed.contains_key("lib") {
                return true;
            }
        }
    }
    project_path.join("src/lib.rs").exists()
}

/// Module-level metadata: visibility chain and cfg status.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleInfo {
    /// Whether every ancestor module up to the crate root is unrestricted `pub`.
    pub is_pub_chain: bool,
    /// Whether the module's `mod` declaration (or any ancestor's) has `#[cfg(...)]`.
    pub is_cfg: bool,
    /// Whether `is_pub_chain` was set exclusively from cfg-gated `mod` declarations.
    /// When a non-cfg-gated declaration exists it is authoritative (compiles in normal
    /// builds), so a cfg-gated `pub mod` (e.g. `#[cfg(docsrs)] pub mod backend`)
    /// should not override a non-cfg-gated `pub(crate) mod backend`.
    pub from_cfg_only: bool,
}

/// Build a map from relative file path to module-level metadata.
///
/// Walks `mod` declarations starting from `src/lib.rs` (or `src/main.rs`),
/// recording for each file whether all ancestor modules are `pub` and whether
/// any ancestor has `#[cfg(...)]`.
///
/// The map keys are relative file paths (e.g., `"src/scalar.rs"`) matching
/// the `code_path` field on atoms.
///
/// For duplicate `mod` declarations (e.g., `#[cfg(docsrs)] pub mod backend`
/// and `pub(crate) mod backend`), a non-cfg-gated declaration is authoritative
/// since it is what compiles in normal builds.
#[must_use]
pub fn build_module_visibility_map(project_path: &Path) -> HashMap<String, ModuleInfo> {
    let mut map = HashMap::new();

    let src_dir = project_path.join("src");
    let lib_rs = src_dir.join("lib.rs");
    let main_rs = src_dir.join("main.rs");

    let entry = if lib_rs.exists() {
        lib_rs
    } else if main_rs.exists() {
        main_rs
    } else {
        return map;
    };

    if let Ok(rel) = entry.strip_prefix(project_path) {
        map.insert(
            rel.to_string_lossy().to_string(),
            ModuleInfo {
                is_pub_chain: true,
                is_cfg: false,
                from_cfg_only: false,
            },
        );
    }

    walk_mod_declarations(project_path, &entry, true, false, &mut map);
    map
}

/// Recursively walk `mod` declarations in a Rust source file.
///
/// `parent_chain_pub` indicates whether every ancestor module up to and
/// including this file's own module is unrestricted `pub`.
/// `parent_chain_cfg` indicates whether any ancestor has `#[cfg(...)]`.
fn walk_mod_declarations(
    project_path: &Path,
    file_path: &Path,
    parent_chain_pub: bool,
    parent_chain_cfg: bool,
    map: &mut HashMap<String, ModuleInfo>,
) {
    let content = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let syntax = match verus_syn::parse_file(&content) {
        Ok(f) => f,
        Err(_) => return,
    };

    let file_dir = file_path.parent().unwrap_or(Path::new(""));

    for item in &syntax.items {
        if let verus_syn::Item::Mod(item_mod) = item {
            let mod_name = item_mod.ident.to_string();
            let is_pub_unrestricted = matches!(item_mod.vis, verus_syn::Visibility::Public(_));
            let mod_has_cfg = verus_parser::has_any_cfg_attr_pub(&item_mod.attrs);

            let chain_pub = parent_chain_pub && is_pub_unrestricted;
            let chain_cfg = parent_chain_cfg || mod_has_cfg;

            if item_mod.content.is_some() {
                continue;
            }

            let mod_file = file_dir.join(format!("{mod_name}.rs"));
            let mod_dir_file = file_dir.join(&mod_name).join("mod.rs");

            let resolved = if mod_file.exists() {
                Some(mod_file)
            } else if mod_dir_file.exists() {
                Some(mod_dir_file)
            } else {
                None
            };

            if let Some(ref path) = resolved {
                if let Ok(rel) = path.strip_prefix(project_path) {
                    let key = rel.to_string_lossy().to_string();
                    let new_info = ModuleInfo {
                        is_pub_chain: chain_pub,
                        is_cfg: chain_cfg,
                        from_cfg_only: mod_has_cfg,
                    };
                    let merged = match map.get(&key).copied() {
                        None => new_info,
                        Some(existing) => {
                            if !mod_has_cfg {
                                // Non-cfg-gated declaration is authoritative.
                                ModuleInfo {
                                    is_pub_chain: chain_pub,
                                    is_cfg: existing.is_cfg || chain_cfg,
                                    from_cfg_only: false,
                                }
                            } else if !existing.from_cfg_only {
                                // Existing came from a non-cfg-gated declaration;
                                // keep its pub chain, just merge cfg flag.
                                ModuleInfo {
                                    is_pub_chain: existing.is_pub_chain,
                                    is_cfg: existing.is_cfg || chain_cfg,
                                    from_cfg_only: false,
                                }
                            } else {
                                // Both cfg-gated: conservative AND for pub chain.
                                ModuleInfo {
                                    is_pub_chain: existing.is_pub_chain && chain_pub,
                                    is_cfg: existing.is_cfg || chain_cfg,
                                    from_cfg_only: true,
                                }
                            }
                        }
                    };
                    map.insert(key, merged);
                }
                walk_mod_declarations(project_path, path, chain_pub, chain_cfg, map);
            }
        }
    }
}

/// Determine `is-public-api` for a function.
///
/// Rules:
/// - External stubs (empty `code_path`) → `None`
/// - Binary-only crate → `Some(false)`
/// - spec/proof functions → `Some(false)` (erased at runtime)
/// - pub exec function with all-pub module chain → `Some(true)`
/// - Trait impl method (detected via `code_name`) with all-pub module chain → `Some(true)`
/// - Otherwise → `Some(false)`
#[must_use]
pub fn classify_public_api(
    is_public: bool,
    code_name: &str,
    code_path: &str,
    kind: DeclKind,
    file_module_pub: &HashMap<String, ModuleInfo>,
    is_library: bool,
) -> Option<bool> {
    if code_path.is_empty() {
        return None;
    }
    if !is_library {
        return Some(false);
    }
    if kind != DeclKind::Exec {
        return Some(false);
    }
    let module_pub = file_module_pub
        .get(code_path)
        .map(|mi| mi.is_pub_chain)
        .unwrap_or(false);
    if is_public && module_pub {
        return Some(true);
    }
    if is_trait_impl_code_name(code_name) && module_pub {
        return Some(true);
    }
    Some(false)
}

/// Result of [`backfill_atoms_from_parser`].
#[derive(Debug, Default)]
pub struct Backfill {
    /// Number of atoms added.
    pub added: usize,
    /// Functions (`code-name at path:line`) that got no atom because their
    /// code-name, `@line` and `@path:line` keys were all taken.
    pub collisions: Vec<String>,
}

/// Backfill atoms from `verus_parser` for functions that SCIP missed.
///
/// Runs the verus source parser over the project, identifies functions that are not yet
/// present in `atoms_dict`, and inserts minimal atoms for them.  This closes the gap
/// for code inside `#[cfg(verus_keep_ghost)] verus! { … }` blocks that verus-analyzer's
/// SCIP mode cannot expand.
///
/// Returns the number of atoms added and the functions that got no atom
/// because every candidate code-name was taken.
pub fn backfill_atoms_from_parser(
    project_path: &Path,
    atoms_dict: &mut BTreeMap<String, AtomWithLines>,
    pkg_name: &str,
    pkg_version: &str,
    file_module_pub: &HashMap<String, ModuleInfo>,
    is_library: bool,
    code_path_prefix: &str,
) -> Backfill {
    let src_dir = project_path.join("src");
    let parsed_from_src = src_dir.is_dir();
    let parse_root: &Path = if parsed_from_src {
        &src_dir
    } else {
        project_path
    };

    let parsed = verus_parser::parse_all_functions(
        parse_root, true,  // include_verus_constructs
        true,  // include_methods
        true,  // show_visibility
        true,  // show_kind
        false, // include_spec_text
    );

    struct PendingAtom {
        atom: AtomWithLines,
        has_spec: bool,
        /// Path as the parser and `file_module_pub` see it (no package prefix).
        parsed_path: String,
    }
    let mut pending: Vec<PendingAtom> = Vec::new();

    for fi in &parsed.functions {
        let raw_path = match &fi.file {
            Some(p) => p.clone(),
            None => continue,
        };

        // When the parser is rooted at src/, its paths are relative to src/
        // (e.g. "lemmas/foo.rs"). Prepend "src/" so code-path matches the
        // SCIP-derived format ("crate/src/lemmas/foo.rs").
        let code_path = if parsed_from_src && !raw_path.starts_with("src/") {
            format!("src/{}", raw_path)
        } else {
            raw_path
        };

        let output_code_path = if code_path_prefix.is_empty() {
            code_path.clone()
        } else {
            format!("{}/{}", code_path_prefix, code_path)
        };

        let module_path = derive_module_path_from_code_path(&code_path);

        // Methods are named after their impl or trait as in SCIP-derived code_names
        // (`impl#[Self][Trait]name()`, `Trait#name()`), rendered from the source.
        // The rendering is best effort, so the code_name may differ from the one
        // the analyzer would assign once it indexes the function.
        let owner = fi.scip_owner.as_deref().unwrap_or("").replace(' ', "/");
        let module_segment = if module_path.is_empty() {
            String::new()
        } else {
            format!("{module_path}/")
        };
        let code_name = format!(
            "{}{}{}/{}{}{}()",
            PROBE_URI_PREFIX,
            pkg_name,
            pkg_version_segment(pkg_version),
            module_segment,
            owner,
            fi.name
        );
        // `Type::method` like SCIP-derived display names (see `enrich_display_name`).
        let display_name = match parse_impl_segment(&code_name) {
            Some(seg) => format!("{}::{}", bare_type_name(seg.self_type), fi.name),
            None => match fi.scip_owner.as_deref().and_then(|o| o.strip_suffix('#')) {
                Some(trait_name) => format!("{}::{}", trait_name, fi.name),
                None => fi.name.clone(),
            },
        };

        // The function is already an atom if one for the same method sits at its
        // location: the atom's (SCIP name) line lies inside the parsed span. Names
        // alone cannot decide it: the analyzer resolves type aliases
        // (`FieldElement` -> `FieldElement51`), the parser does not, and a nearby
        // method of another impl on the same Self type has the same display name.
        let already_present = atoms_dict.values().any(|atom| {
            let same_method = atom.display_name == fi.name
                || atom.display_name.ends_with(&format!("::{}", fi.name));
            same_method
                && (paths_match_by_suffix(&code_path, &atom.code_path)
                    || extract_src_suffix(&code_path) == extract_src_suffix(&atom.code_path))
                && atom.code_text.lines_start >= fi.spec_text.lines_start
                && atom.code_text.lines_start <= fi.spec_text.lines_end
        });
        if already_present {
            continue;
        }

        let code_module = if module_path.is_empty() {
            String::new()
        } else {
            module_path.replace('/', "::")
        };

        let vis_public = fi
            .visibility
            .as_deref()
            .map(|v| v == "pub")
            .unwrap_or(false);
        // For RQN, ensure path has "crate-name/src/..." format.
        let rqn_path = if output_code_path.contains("/src/") {
            output_code_path.clone()
        } else if output_code_path.starts_with("src/") {
            format!("{}/{}", pkg_name, output_code_path)
        } else {
            // Backfill paths from verus_parser are relative to src/
            format!("{}/src/{}", pkg_name, output_code_path)
        };
        let rqn = derive_rust_qualified_name(&rqn_path, &display_name);
        pending.push(PendingAtom {
            has_spec: fi.has_requires || fi.has_ensures,
            atom: AtomWithLines {
                display_name,
                code_name,
                dependencies: BTreeSet::new(),
                dependencies_with_locations: Vec::new(),
                code_module,
                code_path: output_code_path.clone(),
                code_text: CodeTextInfo {
                    lines_start: fi.spec_text.lines_start,
                    lines_end: fi.spec_text.lines_end,
                },
                kind: fi.kind,
                language: if fi.kind == DeclKind::Exec {
                    "rust"
                } else {
                    "verus"
                }
                .to_string(),
                rust_qualified_name: rqn,
                is_public: Some(vis_public),
                is_public_api: None,
                has_body: Some(fi.has_body),
                is_external: Some(fi.is_external),
                is_cfg_gated: Some(
                    fi.is_cfg
                        || file_module_pub
                            .get(&code_path)
                            .map(|mi| mi.is_cfg)
                            .unwrap_or(false),
                ),
            },
            parsed_path: code_path,
        });
    }

    // Functions that share a code-name are cfg alternatives (or functions whose
    // owner the parser renders like another's). Each one is an atom. The
    // spec-bearing one, then the first by line, takes the plain code-name unless
    // an analyzer atom holds it. The others get `@line`, or `@path:line` if that
    // collides too. An external stub with the plain code-name is replaced: it
    // stands for this function, which the analyzer did not index.
    pending.sort_by(|a, b| {
        a.atom
            .code_name
            .cmp(&b.atom.code_name)
            .then(b.has_spec.cmp(&a.has_spec))
            .then(a.atom.code_path.cmp(&b.atom.code_path))
            .then(
                a.atom
                    .code_text
                    .lines_start
                    .cmp(&b.atom.code_text.lines_start),
            )
    });
    let mut added = 0usize;
    let mut collisions = Vec::new();
    for PendingAtom {
        mut atom,
        parsed_path,
        ..
    } in pending
    {
        let base = atom.code_name.clone();
        let plain_free = atoms_dict
            .get(&base)
            .is_none_or(|existing| existing.code_path.is_empty());
        let key = if plain_free {
            base
        } else {
            let with_line = format!("{base}@{}", atom.code_text.lines_start);
            if !atoms_dict.contains_key(&with_line) {
                with_line
            } else {
                let with_path = format!("{base}@{}:{}", atom.code_path, atom.code_text.lines_start);
                if atoms_dict.contains_key(&with_path) {
                    collisions.push(format!(
                        "{base} at {}:{}",
                        atom.code_path, atom.code_text.lines_start
                    ));
                    continue;
                }
                with_path
            }
        };
        atom.is_public_api = classify_public_api(
            atom.is_public.unwrap_or(false),
            &key,
            &parsed_path,
            atom.kind,
            file_module_pub,
            is_library,
        );
        atom.code_name = key.clone();
        atoms_dict.insert(key, atom);
        added += 1;
    }

    Backfill { added, collisions }
}

fn derive_module_path_from_code_path(code_path: &str) -> String {
    let after_src = code_path
        .find("/src/")
        .map(|pos| &code_path[pos + 5..])
        .or_else(|| code_path.strip_prefix("src/"))
        .unwrap_or(code_path);
    let module = after_src.trim_end_matches(".rs");
    // As in SCIP symbols: `foo/mod.rs` is module `foo`, the crate root has no path.
    if module == "lib" || module == "main" || module == "mod" {
        return String::new();
    }
    module.strip_suffix("/mod").unwrap_or(module).to_string()
}

fn pkg_version_segment(v: &str) -> String {
    if v.is_empty() {
        String::new()
    } else {
        format!("/{}", v)
    }
}

/// Add stub atoms for external function dependencies that don't have their own atom entry.
///
/// After building the atoms dict, some dependencies point to external (non-workspace) functions
/// that have no atom. This function creates lightweight stub entries so they appear in the graph.
pub fn add_external_stubs(atoms_dict: &mut BTreeMap<String, AtomWithLines>) -> usize {
    let external_deps: Vec<String> = atoms_dict
        .values()
        .flat_map(|atom| atom.dependencies.iter().cloned())
        .filter(|dep| !atoms_dict.contains_key(dep))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let count = external_deps.len();
    for dep_code_name in external_deps {
        let display_name = extract_display_name_from_code_name(&dep_code_name);
        let code_module = extract_code_module(&dep_code_name);
        atoms_dict.insert(
            dep_code_name.clone(),
            AtomWithLines {
                display_name,
                code_name: dep_code_name,
                dependencies: BTreeSet::new(),
                dependencies_with_locations: Vec::new(),
                code_module,
                code_path: String::new(),
                code_text: CodeTextInfo {
                    lines_start: 0,
                    lines_end: 0,
                },
                kind: DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: None,
                is_public: None,
                is_public_api: None,
                has_body: None,
                is_external: None,
                is_cfg_gated: None,
            },
        );
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // enrich_display_name tests
    // =========================================================================

    #[test]
    fn test_enrich_trait_impl() {
        let symbol = "rust-analyzer cargo curve25519-dalek 4.1.3 edwards/impl#[CompressedEdwardsY][ConstantTimeEq]ct_eq().";
        assert_eq!(
            enrich_display_name(symbol, "ct_eq"),
            "CompressedEdwardsY::ct_eq"
        );
    }

    #[test]
    fn test_enrich_borrowed_self_trait_impl() {
        let symbol = "rust-analyzer cargo curve25519-dalek 4.1.3 montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul().";
        assert_eq!(enrich_display_name(symbol, "mul"), "MontgomeryPoint::mul");
    }

    #[test]
    fn test_enrich_inherent_impl() {
        let symbol = "rust-analyzer cargo curve25519-dalek 4.1.3 backend/serial/u64/field/impl#[FieldElement51]square().";
        assert_eq!(
            enrich_display_name(symbol, "square"),
            "FieldElement51::square"
        );
    }

    #[test]
    fn test_enrich_generic_and_path_qualified_self() {
        let generic = "rust-analyzer cargo curve25519-dalek 4.1.3 window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&'a EdwardsPoint>`]from().";
        assert_eq!(
            enrich_display_name(generic, "from"),
            "NafLookupTable5::from"
        );
        let alias = "rust-analyzer cargo curve25519-dalek 4.1.3 field/impl#[`crate::lizard::lizard_constants::FieldElement51`]is_zero().";
        assert_eq!(
            enrich_display_name(alias, "is_zero"),
            "FieldElement51::is_zero"
        );
    }

    #[test]
    fn test_enrich_trait_method_declaration() {
        let symbol = "rust-analyzer cargo core https://github.com/rust-lang/rust/library/core cmp/PartialEq#eq().";
        assert_eq!(enrich_display_name(symbol, "eq"), "PartialEq::eq");
    }

    #[test]
    fn test_enrich_free_function_unchanged() {
        let symbol = "rust-analyzer cargo curve25519-dalek 4.1.3 lemmas/field_lemmas/lemma_foo().";
        assert_eq!(enrich_display_name(symbol, "lemma_foo"), "lemma_foo");
    }

    #[test]
    fn test_enrich_short_symbol_unchanged() {
        assert_eq!(enrich_display_name("short", "foo"), "foo");
    }

    // =========================================================================
    // symbol_to_code_name / parse_impl_segment / strip_lifetimes tests
    // =========================================================================

    #[test]
    fn test_symbol_to_code_name_is_syntactic() {
        assert_eq!(
            symbol_to_code_name(
                "rust-analyzer cargo curve25519-dalek 4.1.3 montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul().",
                None
            ),
            "probe:curve25519-dalek/4.1.3/montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul()"
        );
        assert_eq!(
            symbol_to_code_name(
                "rust-analyzer cargo curve25519-dalek 4.1.3 lemmas/common_lemmas/mul_lemmas/lemma_mul_distributive_3_terms().",
                None
            ),
            "probe:curve25519-dalek/4.1.3/lemmas/common_lemmas/mul_lemmas/lemma_mul_distributive_3_terms()"
        );
    }

    #[test]
    fn test_symbol_to_code_name_strips_lifetimes_and_adds_line() {
        assert_eq!(
            symbol_to_code_name(
                "rust-analyzer cargo curve25519-dalek 4.1.3 window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&'a EdwardsPoint>`]from().",
                Some(798)
            ),
            "probe:curve25519-dalek/4.1.3/window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&EdwardsPoint>`]from()@798"
        );
    }

    #[test]
    fn test_symbol_to_code_name_external() {
        assert_eq!(
            symbol_to_code_name(
                "rust-analyzer cargo core https://github.com/rust-lang/rust/library/core array/impl#[`[T; N]`][Clone]clone().",
                None
            ),
            "probe:core/https://github.com/rust-lang/rust/library/core/array/impl#[`[T;/N]`][Clone]clone()"
        );
    }

    #[test]
    fn test_strip_lifetimes() {
        assert_eq!(
            strip_lifetimes("`From<&'a EdwardsPoint>`"),
            "`From<&EdwardsPoint>`"
        );
        assert_eq!(strip_lifetimes("`Foo<'a, T>`"), "`Foo<T>`");
        assert_eq!(strip_lifetimes("`Foo<T, 'a>`"), "`Foo<T>`");
        assert_eq!(strip_lifetimes("`Foo<'a>`"), "`Foo`");
        assert_eq!(strip_lifetimes("`&'b mut Bar`"), "`&mut Bar`");
        assert_eq!(strip_lifetimes("module/free_fn()"), "module/free_fn()");
        // Character literals in const generic arguments are not lifetimes.
        assert_eq!(strip_lifetimes("`Tag<'a'>`"), "`Tag<'a'>`");
        assert_ne!(
            strip_lifetimes("impl#[`Tag<'a'>`]read()."),
            strip_lifetimes("impl#[`Tag<'b'>`]read().")
        );
        assert_eq!(strip_lifetimes("`Foo<'a, 'x'>`"), "`Foo<'x'>`");
        assert_eq!(strip_lifetimes("`&'_ T`"), "`&T`");
    }

    #[test]
    fn test_symbol_to_code_name_mut_reference() {
        let sym = "rust-analyzer cargo c 1.0 m/impl#[`&'a mut Table<T>`]get().";
        let code_name = symbol_to_code_name(sym, None);
        assert_eq!(code_name, "probe:c/1.0/m/impl#[`&mut/Table<T>`]get()");
        let seg = parse_impl_segment(&code_name).unwrap();
        assert_eq!(bare_type_name(seg.self_type), "Table");
    }

    #[test]
    fn test_parse_impl_segment() {
        assert_eq!(
            parse_impl_segment("montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul()"),
            Some(ImplSegment {
                self_type: "&MontgomeryPoint",
                trait_type: Some("Mul<&Scalar>"),
            })
        );
        assert_eq!(
            parse_impl_segment("field/impl#[FieldElement51]square()"),
            Some(ImplSegment {
                self_type: "FieldElement51",
                trait_type: None,
            })
        );
        assert_eq!(
            parse_impl_segment("core/array/impl#[`[T; 32]`][Default]default()"),
            Some(ImplSegment {
                self_type: "[T; 32]",
                trait_type: Some("Default"),
            })
        );
        assert_eq!(parse_impl_segment("scalar/free_fn()"), None);
    }

    #[test]
    fn test_bare_type_name() {
        assert_eq!(
            bare_type_name("&'a mut NafLookupTable5<T>"),
            "NafLookupTable5"
        );
        assert_eq!(bare_type_name("&Scalar"), "Scalar");
        // Code-name form: spaces became `/`, lifetimes are already gone.
        assert_eq!(bare_type_name("&mut/NafLookupTable5<T>"), "NafLookupTable5");
        assert_eq!(
            bare_type_name("crate::lizard::lizard_constants::FieldElement51"),
            "FieldElement51"
        );
    }

    #[test]
    fn test_code_name_is_method_path_owner() {
        let imp = "probe:subtle/2.6.1/impl#[Choice][`From<u8>`]from()";
        assert!(code_name_is_method(
            imp,
            MethodOwner::Path("Choice"),
            "from"
        ));
        // `From::from` names the trait method, not every impl of `From`.
        assert!(!code_name_is_method(imp, MethodOwner::Path("From"), "from"));
        assert!(!code_name_is_method(
            imp,
            MethodOwner::Path("Choice"),
            "into"
        ));
        let decl = "probe:subtle/2.6.1/ConditionallySelectable#conditional_swap()";
        assert!(code_name_is_method(
            decl,
            MethodOwner::Path("ConditionallySelectable"),
            "conditional_swap"
        ));
        assert!(!code_name_is_method(
            decl,
            MethodOwner::Path("u64"),
            "conditional_swap"
        ));
    }

    #[test]
    fn test_code_name_is_method_qualified_owner() {
        let u64_swap = MethodOwner::Qualified {
            self_type: "u64",
            trait_type: Some("ConditionallySelectable"),
        };
        assert!(code_name_is_method(
            "probe:subtle/2.6.1/impl#[u64][ConditionallySelectable]conditional_swap()",
            u64_swap,
            "conditional_swap"
        ));
        // A wrong Self type never matches, even as the only candidate.
        assert!(!code_name_is_method(
            "probe:subtle/2.6.1/impl#[u32][ConditionallySelectable]conditional_swap()",
            u64_swap,
            "conditional_swap"
        ));
        // Neither does the trait's own declaration.
        assert!(!code_name_is_method(
            "probe:subtle/2.6.1/ConditionallySelectable#conditional_swap()",
            u64_swap,
            "conditional_swap"
        ));
        // Nor an impl of a different trait for the same Self type.
        assert!(!code_name_is_method(
            "probe:x/1.0/impl#[u64][Other]conditional_swap()",
            u64_swap,
            "conditional_swap"
        ));
        // Nor an inherent method when a trait is named.
        assert!(!code_name_is_method(
            "probe:x/1.0/impl#[u64]conditional_swap()",
            u64_swap,
            "conditional_swap"
        ));
    }

    #[test]
    fn test_code_name_is_method_generic_types() {
        // Self types compare with generic arguments; the Verus path spaces them
        // differently and qualifies the trait.
        let array_hash = MethodOwner::Qualified {
            self_type: "[T ; N]",
            trait_type: Some("core::hash::Hash"),
        };
        let hash = "probe:core/https://github.com/rust-lang/rust/library/core/array/impl#[`[T;/N]`][Hash]hash()";
        assert!(code_name_is_method(hash, array_hash, "hash"));
        let other_self = MethodOwner::Qualified {
            self_type: "[u8 ; 32]",
            trait_type: Some("Hash"),
        };
        assert!(!code_name_is_method(hash, other_self, "hash"));
        // Trait generic arguments separate impls of the same trait.
        let from_u8 = "probe:subtle/2.6.1/impl#[Choice][`From<u8>`]from()";
        let want_u8 = MethodOwner::Qualified {
            self_type: "Choice",
            trait_type: Some("From<u8>"),
        };
        let want_bool = MethodOwner::Qualified {
            self_type: "Choice",
            trait_type: Some("From<bool>"),
        };
        let want_any = MethodOwner::Qualified {
            self_type: "Choice",
            trait_type: Some("From"),
        };
        assert!(code_name_is_method(from_u8, want_u8, "from"));
        assert!(!code_name_is_method(from_u8, want_bool, "from"));
        assert!(code_name_is_method(from_u8, want_any, "from"));
        // `<T>::m` names an inherent method.
        let inherent = MethodOwner::Qualified {
            self_type: "Choice",
            trait_type: None,
        };
        assert!(code_name_is_method(
            "probe:subtle/2.6.1/impl#[Choice]unwrap_u8()",
            inherent,
            "unwrap_u8"
        ));
        assert!(!code_name_is_method(from_u8, inherent, "from"));
    }

    #[test]
    fn test_normalize_type() {
        assert_eq!(normalize_type("&'a crate::Foo<core::Bar>"), "&Foo<Bar>");
        assert_eq!(normalize_type("[T;/N]"), "[T;N]");
        assert_eq!(normalize_type("&mut/T"), "&mut T");
        assert_eq!(normalize_type("& 'a mut T"), "&mut T");
        assert_eq!(normalize_type("Tag<'a'>"), "Tag<'a'>");
    }

    /// An index with one document holding `symbols` as `(symbol, kind)` records
    /// and, for each, a definition occurrence on its own line.
    fn index_with_kinds(version: &str, symbols: &[(&str, i32)]) -> ScipIndex {
        ScipIndex {
            metadata: Metadata {
                tool_info: ScipToolInfo {
                    name: "rust-analyzer".to_string(),
                    version: version.to_string(),
                },
                project_root: String::new(),
                text_document_encoding: 0,
            },
            documents: vec![Document {
                language: "rust".to_string(),
                relative_path: "src/lib.rs".to_string(),
                occurrences: symbols
                    .iter()
                    .enumerate()
                    .map(|(i, (s, _))| Occurrence {
                        range: vec![i as i32, 0, 1],
                        symbol: s.to_string(),
                        symbol_roles: Some(1),
                    })
                    .collect(),
                symbols: symbols
                    .iter()
                    .map(|(s, kind)| Symbol {
                        symbol: s.to_string(),
                        kind: *kind,
                        display_name: None,
                        documentation: None,
                        signature_documentation: SignatureDocumentation {
                            language: "rust".to_string(),
                            text: String::new(),
                        },
                        enclosing_symbol: None,
                    })
                    .collect(),
            }],
        }
    }

    fn index_with_symbols(symbols: &[&str]) -> ScipIndex {
        let with_kinds: Vec<(&str, i32)> = symbols
            .iter()
            .map(|s| (*s, constants::SCIP_KIND_METHOD))
            .collect();
        index_with_kinds("0", &with_kinds)
    }

    const STRUCT_KIND: i32 = 49;

    #[test]
    fn test_legacy_format_trait_impl_symbol() {
        let legacy = index_with_symbols(&[
            "rust-analyzer cargo c 1.0 field/FieldElement51#Clone#clone().",
            "rust-analyzer cargo c 1.0 lemmas/lemma_foo().",
        ]);
        assert!(uses_legacy_symbol_format(&legacy));
    }

    #[test]
    fn test_legacy_format_inherent_method_on_struct() {
        let legacy = index_with_kinds(
            "0",
            &[
                (
                    "rust-analyzer cargo c 1.0 montgomery/MontgomeryPoint#",
                    STRUCT_KIND,
                ),
                (
                    "rust-analyzer cargo c 1.0 montgomery/MontgomeryPoint#to_bytes().",
                    constants::SCIP_KIND_METHOD,
                ),
            ],
        );
        assert!(uses_legacy_symbol_format(&legacy));
    }

    #[test]
    fn test_current_format_trait_declaration_only() {
        // A trait-only crate: `Trait#method()` is still the current syntax for
        // trait method declarations.
        let trait_only = index_with_kinds(
            "0",
            &[
                (
                    "rust-analyzer cargo c 1.0 Trait#",
                    constants::SCIP_KIND_TRAIT,
                ),
                (
                    "rust-analyzer cargo c 1.0 Trait#method().",
                    constants::SCIP_KIND_TRAIT_METHOD,
                ),
            ],
        );
        assert!(!uses_legacy_symbol_format(&trait_only));
        // Without the trait's record the owner is unknown: no evidence either way.
        let unknown_owner = index_with_symbols(&["rust-analyzer cargo c 1.0 Trait#method()."]);
        assert!(!uses_legacy_symbol_format(&unknown_owner));
    }

    #[test]
    fn test_current_format_impl_symbols() {
        let current = index_with_symbols(&[
            "rust-analyzer cargo c 1.0 montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul().",
            "rust-analyzer cargo c 1.0 lemmas/lemma_foo().",
        ]);
        assert!(!uses_legacy_symbol_format(&current));
    }

    #[test]
    fn test_legacy_format_free_functions_only_by_version() {
        let free_fn = ["rust-analyzer cargo c 1.0 lemmas/lemma_foo()."];
        // Syntax alone cannot tell; the producer version can.
        assert!(!uses_legacy_symbol_format(&index_with_symbols(&free_fn)));
        let with = |v| index_with_kinds(v, &[(free_fn[0], constants::SCIP_KIND_FUNCTION)]);
        assert!(uses_legacy_symbol_format(&with("0.3.264-standalone")));
        assert!(uses_legacy_symbol_format(&with("0.3.259")));
        assert!(!uses_legacy_symbol_format(&with("0.3.266-standalone")));
        assert!(!uses_legacy_symbol_format(&with("0.3.269-standalone")));
        // Upstream rust-analyzer numbering.
        assert!(!uses_legacy_symbol_format(&with("0.3.2743-standalone")));
    }

    /// `/` and `#` inside a quoted descriptor are not structure.
    #[test]
    fn test_current_format_quoted_descriptor() {
        let quoted =
            index_with_symbols(&["rust-analyzer cargo c 1.0 impl#[`Tag<'/', '#', '#'>`]read()."]);
        assert!(!uses_legacy_symbol_format(&quoted));
        assert_eq!(
            last_symbol_segment("c 1.0 m/impl#[`Tag<'/'>`]read()."),
            "impl#[`Tag<'/'>`]read()."
        );
        assert_eq!(
            last_symbol_segment("rust-analyzer cargo c 1.0 impl#[`&'a S`][T]f()."),
            "impl#[`&'a S`][T]f()."
        );
    }

    #[test]
    fn test_verus_dependencies() {
        let manifest = r#"
[dependencies]
vstd = { workspace = true }
serde = "1"
vb = { package = "verus_builtin", version = "0.1" }

[target.'cfg(unix)'.dependencies]
builtin = { path = "../builtin" }

[dev-dependencies]
verus_builtin_macros = "0.1"
"#;
        assert_eq!(
            verus_dependencies(manifest),
            vec!["builtin", "verus_builtin", "vstd"]
        );
        assert!(verus_dependencies("not toml [").is_empty());
    }

    /// An index without references to a Verus dependency of the package is
    /// rejected: the analyzer did not load the project.
    #[test]
    fn test_validate_index_requires_verus_dependency_references() {
        let free_fn = "rust-analyzer cargo c 1.0 lemmas/lemma_foo().";
        let mut index = index_with_kinds(
            "0.3.269-standalone",
            &[(free_fn, constants::SCIP_KIND_FUNCTION)],
        );
        let deps = vec!["vstd".to_string()];
        let err = validate_index(&index, &deps).unwrap_err().to_string();
        assert!(err.contains("no references to vstd"), "{err}");

        index.documents[0].occurrences.push(Occurrence {
            range: vec![5, 0, 3],
            symbol: "rust-analyzer cargo vstd 0.0.0-2026-01-11-0057 seq/Seq#len().".to_string(),
            symbol_roles: None,
        });
        assert_eq!(validate_index(&index, &deps), Ok(vec![]));
    }

    #[test]
    fn test_validate_index_rejects_legacy_index() {
        let legacy = index_with_kinds(
            "0.3.264-standalone",
            &[(
                "rust-analyzer cargo c 1.0 f().",
                constants::SCIP_KIND_FUNCTION,
            )],
        );
        let err = validate_index(&legacy, &[]).unwrap_err();
        assert!(matches!(err, IndexError::Legacy(_)));
        let err = err.to_string();
        assert!(err.contains("older than 2026-08-22"), "{err}");
        assert!(err.contains("probe-verus 8.x"), "{err}");
    }

    /// Without a known producer version and without `impl#[` method symbols,
    /// nothing shows the format: a warning, not an error.
    #[test]
    fn test_validate_index_warns_without_format_evidence() {
        let free_fn = [(
            "rust-analyzer cargo c 1.0 f().",
            constants::SCIP_KIND_FUNCTION,
        )];
        let unknown = index_with_kinds("", &free_fn);
        let warnings = validate_index(&unknown, &[]).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("cannot tell"), "{}", warnings[0]);

        let known = index_with_kinds("0.3.269-standalone", &free_fn);
        assert_eq!(validate_index(&known, &[]), Ok(vec![]));
        let with_impl = index_with_kinds(
            "",
            &[(
                "rust-analyzer cargo c 1.0 impl#[S]f().",
                constants::SCIP_KIND_METHOD,
            )],
        );
        assert_eq!(validate_index(&with_impl, &[]), Ok(vec![]));
    }

    /// The producer version may be missing from the metadata.
    #[test]
    fn test_parse_index_without_tool_version() {
        let json = r#"{"metadata": {"tool_info": {"name": "x"}, "project_root": "", "text_document_encoding": 0}, "documents": []}"#;
        let index: ScipIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.metadata.tool_info.version, "");
    }

    #[test]
    fn test_legacy_format_mixed_index() {
        // One `impl#[` symbol does not vouch for the rest.
        let mixed = index_with_symbols(&[
            "rust-analyzer cargo c 1.0 montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul().",
            "rust-analyzer cargo c 1.0 field/FieldElement51#Clone#clone().",
        ]);
        assert!(uses_legacy_symbol_format(&mixed));
    }

    // =========================================================================
    // backfill_atoms_from_parser
    // =========================================================================

    /// A crate `c` 1.0 whose `src/lib.rs` is `source`.
    fn crate_with_lib(source: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), source).unwrap();
        dir
    }

    fn backfill(dir: &Path, atoms: &mut BTreeMap<String, AtomWithLines>) -> usize {
        let result = backfill_atoms_from_parser(dir, atoms, "c", "1.0", &HashMap::new(), true, "");
        assert!(result.collisions.is_empty(), "{:?}", result.collisions);
        result.added
    }

    fn analyzer_atom(display_name: &str, line: usize, deps: &[&str]) -> AtomWithLines {
        serde_json::from_value(serde_json::json!({
            "display-name": display_name,
            "dependencies": deps,
            "code-module": "",
            "code-path": "src/lib.rs",
            "code-text": {"lines-start": line, "lines-end": line + 2},
            "kind": "exec",
            "language": "rust"
        }))
        .unwrap()
    }

    /// Same-named methods of different impls next to each other are distinct atoms.
    #[test]
    fn test_backfill_keeps_adjacent_methods_of_different_impls() {
        let dir =
            crate_with_lib("struct A;\nstruct B;\nimpl A { fn f() {} }\nimpl B { fn f() {} }\n");
        let mut atoms = BTreeMap::new();
        assert_eq!(backfill(dir.path(), &mut atoms), 2);
        let names: Vec<&String> = atoms.keys().collect();
        assert_eq!(
            names,
            vec!["probe:c/1.0/impl#[A]f()", "probe:c/1.0/impl#[B]f()"]
        );
        assert_eq!(atoms["probe:c/1.0/impl#[B]f()"].display_name, "B::f");
    }

    /// A method of another trait impl on the same Self type, next to an analyzer
    /// atom, is a distinct function (Codex PR #50 review, round 2, finding 3).
    #[test]
    fn test_backfill_keeps_same_self_type_method_of_other_trait_impl() {
        let dir = crate_with_lib(
            "struct R;\ntrait A { fn f(); }\ntrait B { fn f(); }\nimpl A for R {\n    fn f() {}\n}\nimpl B for R {\n    fn f() {}\n}\n",
        );
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:c/1.0/impl#[R][A]f()".to_string(),
            analyzer_atom("R::f", 5, &[]),
        );
        backfill(dir.path(), &mut atoms);
        let atom = &atoms["probe:c/1.0/impl#[R][B]f()"];
        assert_eq!(atom.code_text.lines_start, 8);
        assert!(atoms.keys().all(|k| !k.contains('@')), "{:?}", atoms.keys());
        assert_eq!(atoms["probe:c/1.0/impl#[R][A]f()"].code_text.lines_start, 5);
    }

    /// A cfg alternative that the analyzer did not index is an atom of its own,
    /// keyed `@line`, so that every declaration has an atom (KB P25 classifies
    /// it as out of scope when its cfg is inactive).
    #[test]
    fn test_backfill_keeps_cfg_alternative_of_analyzer_atom() {
        let dir = crate_with_lib(
            "verus! {\n#[cfg(feature = \"a\")]\nfn h() {}\n#[cfg(not(feature = \"a\"))]\nfn h()\n    ensures true,\n{}\n}\n",
        );
        let mut atoms = BTreeMap::new();
        atoms.insert("probe:c/1.0/h()".to_string(), analyzer_atom("h", 3, &[]));
        assert_eq!(backfill(dir.path(), &mut atoms), 1);
        assert_eq!(atoms["probe:c/1.0/h()"].code_text.lines_start, 3);
        assert_eq!(atoms["probe:c/1.0/h()@4"].code_text.lines_start, 4);
    }

    /// An analyzer atom inside the parsed span is the same function, even when
    /// the analyzer named the aliased type.
    #[test]
    fn test_backfill_recognises_analyzer_atom_through_type_alias() {
        let dir = crate_with_lib("type Alias = Real;\nimpl Alias {\n    fn f() {\n    }\n}\n");
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:c/1.0/impl#[Real]f()".to_string(),
            analyzer_atom("Real::f", 3, &[]),
        );
        assert_eq!(backfill(dir.path(), &mut atoms), 0);
    }

    /// An analyzer atom is not replaced by a parser-found cfg alternative with
    /// the same code-name, even one with a spec.
    #[test]
    fn test_backfill_does_not_replace_analyzer_atom() {
        let mut source = String::from(
            "struct S;\nverus! {\nimpl S {\n    #[cfg(feature = \"a\")]\n    fn f() { g() }\n",
        );
        source.push_str(&"\n".repeat(20));
        source.push_str(
            "    #[cfg(not(feature = \"a\"))]\n    fn f()\n        ensures true,\n    {}\n}\n}\n",
        );
        let dir = crate_with_lib(&source);
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:c/1.0/impl#[S]f()".to_string(),
            analyzer_atom("S::f", 5, &["probe:c/1.0/g()"]),
        );
        backfill(dir.path(), &mut atoms);
        let atom = &atoms["probe:c/1.0/impl#[S]f()"];
        assert_eq!(atom.code_text.lines_start, 5);
        assert_eq!(atom.dependencies.len(), 1);
    }

    /// Two parser-found cfg alternatives are both atoms. The spec-bearing one
    /// takes the plain code-name.
    #[test]
    fn test_backfill_prefers_spec_bearing_cfg_alternative() {
        let source = "verus! {\n#[cfg(feature = \"a\")]\nfn h() {}\n\n\n\n\n\n\n\n#[cfg(not(feature = \"a\"))]\nfn h()\n    ensures true,\n{}\n}\n";
        let dir = crate_with_lib(source);
        let mut atoms = BTreeMap::new();
        assert_eq!(backfill(dir.path(), &mut atoms), 2);
        assert_eq!(atoms["probe:c/1.0/h()"].code_text.lines_start, 11);
        assert_eq!(atoms["probe:c/1.0/h()@2"].code_text.lines_start, 2);
    }

    /// Functions without a free code-name are reported, not dropped silently
    /// and never overwrite an atom.
    #[test]
    fn test_backfill_reports_code_name_collisions() {
        let dir = crate_with_lib(
            "struct S;\nimpl S { fn f() {} } impl S { fn f() {} } impl S { fn f() {} } impl S { fn f() {} }\n",
        );
        let mut atoms = BTreeMap::new();
        let result = backfill_atoms_from_parser(
            dir.path(),
            &mut atoms,
            "c",
            "1.0",
            &HashMap::new(),
            true,
            "",
        );
        assert_eq!(result.added, 3);
        assert_eq!(
            result.collisions,
            vec!["probe:c/1.0/impl#[S]f() at src/lib.rs:2"]
        );
        let mut keys: Vec<&String> = atoms.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "probe:c/1.0/impl#[S]f()",
                "probe:c/1.0/impl#[S]f()@2",
                "probe:c/1.0/impl#[S]f()@src/lib.rs:2",
            ]
        );
    }

    /// A backfilled function replaces the external stub that stands for it.
    #[test]
    fn test_backfill_replaces_stub_with_same_code_name() {
        let dir = crate_with_lib("fn h() {}\n");
        let mut atoms = BTreeMap::new();
        let mut stub = analyzer_atom("h", 0, &[]);
        stub.code_path = String::new();
        atoms.insert("probe:c/1.0/h()".to_string(), stub);
        assert_eq!(backfill(dir.path(), &mut atoms), 1);
        assert_eq!(atoms["probe:c/1.0/h()"].code_path, "src/lib.rs");
    }

    // =========================================================================
    // build_call_graph definition identity
    // =========================================================================

    fn occurrence(symbol: &str, range: [i32; 3], roles: Option<i32>) -> Occurrence {
        Occurrence {
            range: range.to_vec(),
            symbol: symbol.to_string(),
            symbol_roles: roles,
        }
    }

    fn function_record(symbol: &str, signature: &str) -> Symbol {
        Symbol {
            symbol: symbol.to_string(),
            kind: constants::SCIP_KIND_METHOD,
            display_name: Some("f".to_string()),
            documentation: None,
            signature_documentation: SignatureDocumentation {
                language: "rust".to_string(),
                text: signature.to_string(),
            },
            enclosing_symbol: None,
        }
    }

    fn document(path: &str, occurrences: Vec<Occurrence>, symbols: Vec<Symbol>) -> Document {
        Document {
            language: "rust".to_string(),
            relative_path: path.to_string(),
            occurrences,
            symbols,
        }
    }

    fn index_of(documents: Vec<Document>) -> ScipIndex {
        ScipIndex {
            metadata: Metadata {
                tool_info: ScipToolInfo {
                    name: "rust-analyzer".to_string(),
                    version: "0.3.269".to_string(),
                },
                project_root: String::new(),
                text_document_encoding: 0,
            },
            documents,
        }
    }

    const DUP: &str = "rust-analyzer cargo c 1.0 m/impl#[S]f().";

    /// Trait method declarations (SCIP kind `TraitMethod`), with or without a
    /// body, are local definitions. A call to one resolves to its atom, not to
    /// an external stub.
    #[test]
    fn test_trait_method_declaration_is_local_atom() {
        const DECL: &str = "rust-analyzer cargo c 1.0 m/Tr#inv().";
        const CALLER: &str = "rust-analyzer cargo c 1.0 m/check().";
        let mut decl = function_record(DECL, "spec fn inv(self) -> bool;");
        decl.kind = constants::SCIP_KIND_TRAIT_METHOD;
        let mut caller = function_record(CALLER, "fn check()");
        caller.kind = constants::SCIP_KIND_FUNCTION;
        let index = index_of(vec![document(
            "src/m.rs",
            vec![
                occurrence(DECL, [3, 12, 15], Some(1)),
                occurrence(CALLER, [6, 3, 8], Some(1)),
                occurrence(DECL, [7, 4, 7], None),
            ],
            vec![decl, caller],
        )]);
        let atoms = convert_to_atoms_with_lines(&build_call_graph(&index));
        let decl_atom = atoms
            .iter()
            .find(|a| a.code_name == "probe:c/1.0/m/Tr#inv()")
            .expect("trait method declaration is an atom");
        assert_eq!(decl_atom.code_path, "src/m.rs");
        assert_eq!(decl_atom.code_text.lines_start, 4);
        let caller_atom = atoms
            .iter()
            .find(|a| a.code_name == "probe:c/1.0/m/check()")
            .unwrap();
        assert!(caller_atom.dependencies.contains("probe:c/1.0/m/Tr#inv()"));
    }

    /// Definitions sharing a symbol on the same line (in different files, or in
    /// different columns of one file) each get a node.
    #[test]
    fn test_call_graph_keeps_colliding_definitions() {
        let index = index_of(vec![
            document(
                "src/a.rs",
                vec![
                    occurrence(DUP, [10, 4, 5], Some(1)),
                    occurrence(DUP, [10, 20, 21], Some(1)),
                ],
                vec![
                    function_record(DUP, "fn a1()"),
                    function_record(DUP, "fn a2()"),
                ],
            ),
            document(
                "src/b.rs",
                vec![occurrence(DUP, [10, 4, 5], Some(1))],
                vec![function_record(DUP, "fn b()")],
            ),
        ]);
        let definition_count = index
            .documents
            .iter()
            .flat_map(|d| &d.occurrences)
            .filter(|o| is_definition(o.symbol_roles))
            .count();
        let graph = build_call_graph(&index);
        assert_eq!(graph.len(), definition_count);

        // Final code_names stay distinct.
        let mut names: Vec<String> = convert_to_atoms_with_lines(&graph)
            .into_iter()
            .map(|a| a.code_name)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "probe:c/1.0/m/impl#[S]f()@src/a.rs:11:21",
                "probe:c/1.0/m/impl#[S]f()@src/a.rs:11:5",
                "probe:c/1.0/m/impl#[S]f()@src/b.rs:11:5",
            ]
        );

        // Each definition carries the record paired with it, in document order.
        let mut by_location: Vec<(String, Vec<i32>, String)> = graph
            .values()
            .map(|n| {
                (
                    n.relative_path.clone(),
                    n.range.clone(),
                    n.signature_text.clone(),
                )
            })
            .collect();
        by_location.sort();
        assert_eq!(
            by_location,
            vec![
                (
                    "src/a.rs".to_string(),
                    vec![10, 4, 5],
                    "fn a1()".to_string()
                ),
                (
                    "src/a.rs".to_string(),
                    vec![10, 20, 21],
                    "fn a2()".to_string()
                ),
                ("src/b.rs".to_string(), vec![10, 4, 5], "fn b()".to_string()),
            ]
        );
    }

    /// One record for several definitions describes all of them.
    #[test]
    fn test_call_graph_single_record_for_several_definitions() {
        let index = index_of(vec![document(
            "src/a.rs",
            vec![
                occurrence(DUP, [30, 4, 5], Some(1)),
                occurrence(DUP, [10, 4, 5], Some(1)),
            ],
            vec![function_record(DUP, "fn shared()")],
        )]);
        let graph = build_call_graph(&index);
        assert_eq!(graph.len(), 2);
        assert!(graph.values().all(|n| n.signature_text == "fn shared()"));
    }

    /// A call between two definitions that share a symbol is an edge, not
    /// recursion; only the caller's own code_name is excluded.
    #[test]
    fn test_call_between_same_symbol_definitions_is_kept() {
        let index = index_of(vec![document(
            "src/a.rs",
            vec![
                occurrence(DUP, [10, 4, 5], Some(1)),
                occurrence(DUP, [11, 8, 9], None),
                occurrence(DUP, [20, 4, 5], Some(1)),
            ],
            vec![
                function_record(DUP, "fn a()"),
                function_record(DUP, "fn b()"),
            ],
        )]);
        let atoms = convert_to_atoms_with_lines(&build_call_graph(&index));
        let first = atoms
            .iter()
            .find(|a| a.code_text.lines_start == 11)
            .unwrap();
        let second = atoms
            .iter()
            .find(|a| a.code_text.lines_start == 21)
            .unwrap();
        assert_eq!(
            first.dependencies.iter().collect::<Vec<_>>(),
            vec![&second.code_name]
        );
        assert!(second.dependencies.is_empty());
    }

    /// Plain recursion is still not a dependency.
    #[test]
    fn test_recursive_call_is_not_a_dependency() {
        let f = "rust-analyzer cargo c 1.0 m/f().";
        let index = index_of(vec![document(
            "src/a.rs",
            vec![
                occurrence(f, [10, 4, 5], Some(1)),
                occurrence(f, [11, 8, 9], None),
            ],
            vec![function_record(f, "fn f()")],
        )]);
        let atoms = convert_to_atoms_with_lines(&build_call_graph(&index));
        assert_eq!(atoms.len(), 1);
        assert!(atoms[0].dependencies.is_empty());
    }

    // =========================================================================
    // extract_display_name_from_code_name tests
    // =========================================================================

    #[test]
    fn test_extract_display_name_method() {
        assert_eq!(
            extract_display_name_from_code_name(
                "probe:x25519-dalek/2.0.1/x25519/impl#[StaticSecret]diffie_hellman()"
            ),
            "diffie_hellman"
        );
    }

    #[test]
    fn test_extract_display_name_scip_suffix() {
        // SCIP symbols end with `().` (trailing dot)
        assert_eq!(
            extract_display_name_from_code_name(
                "probe:x25519-dalek/2.0.1/x25519/impl#[StaticSecret]diffie_hellman()."
            ),
            "diffie_hellman"
        );
    }

    #[test]
    fn test_extract_display_name_free_function() {
        assert_eq!(
            extract_display_name_from_code_name("probe:curve25519-dalek/4.1.3/field/reduce()"),
            "reduce"
        );
    }

    #[test]
    fn test_extract_display_name_trait_impl() {
        assert_eq!(
            extract_display_name_from_code_name(
                "probe:curve25519-dalek/4.1.3/edwards/CompressedEdwardsY#[ConstantTimeEq]ct_eq()"
            ),
            "ct_eq"
        );
    }

    // =========================================================================
    // is_external_function_symbol tests
    // =========================================================================

    #[test]
    fn test_external_function_detected() {
        let known = HashSet::new();
        assert!(constants::is_external_function_symbol(
            "rust-analyzer cargo x25519-dalek 2.0.1 x25519/impl#[StaticSecret]diffie_hellman().",
            &known,
        ));
    }

    #[test]
    fn test_known_symbol_not_external() {
        let mut known = HashSet::new();
        known.insert("rust-analyzer cargo crate 1.0 foo/bar().".to_string());
        assert!(!constants::is_external_function_symbol(
            "rust-analyzer cargo crate 1.0 foo/bar().",
            &known,
        ));
    }

    #[test]
    fn test_type_symbol_not_external_function() {
        let known = HashSet::new();
        assert!(!constants::is_external_function_symbol(
            "rust-analyzer cargo x25519-dalek 2.0.1 x25519/StaticSecret#",
            &known,
        ));
    }

    #[test]
    fn test_field_symbol_not_external_function() {
        let known = HashSet::new();
        assert!(!constants::is_external_function_symbol(
            "rust-analyzer cargo crate 1.0 module/Struct#field.",
            &known,
        ));
    }

    // =========================================================================
    // normalize_code_name tests
    // =========================================================================

    #[test]
    fn test_normalize_code_name_strips_trailing_dot() {
        assert_eq!(
            normalize_code_name("probe:x25519-dalek/2.0.1/x25519/diffie_hellman()."),
            "probe:x25519-dalek/2.0.1/x25519/diffie_hellman()"
        );
    }

    #[test]
    fn test_normalize_code_name_no_dot() {
        assert_eq!(
            normalize_code_name("probe:crate/1.0/module/func()"),
            "probe:crate/1.0/module/func()"
        );
    }

    #[test]
    fn test_code_name_no_trailing_dot() {
        let symbol =
            "rust-analyzer cargo x25519-dalek 2.0.1 x25519/impl#[StaticSecret]diffie_hellman().";
        let code_name = symbol_to_code_name(symbol, None);
        assert!(
            !code_name.ends_with('.'),
            "code_name should not end with '.': {}",
            code_name
        );
    }

    // =========================================================================
    // add_external_stubs tests
    // =========================================================================

    #[test]
    fn test_add_external_stubs_creates_missing() {
        let mut atoms_dict = BTreeMap::new();
        let mut deps = BTreeSet::new();
        deps.insert("probe:external-crate/1.0/mod/func()".to_string());

        atoms_dict.insert(
            "probe:my-crate/1.0/caller()".to_string(),
            AtomWithLines {
                display_name: "caller".to_string(),
                code_name: "probe:my-crate/1.0/caller()".to_string(),
                dependencies: deps,
                dependencies_with_locations: Vec::new(),
                code_module: String::new(),
                code_path: "src/lib.rs".to_string(),
                code_text: CodeTextInfo {
                    lines_start: 10,
                    lines_end: 20,
                },
                kind: DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: None,
                is_public: None,
                is_public_api: None,
                has_body: None,
                is_external: None,
                is_cfg_gated: None,
            },
        );

        let count = add_external_stubs(&mut atoms_dict);
        assert_eq!(count, 1);
        assert_eq!(atoms_dict.len(), 2);

        let stub = atoms_dict
            .get("probe:external-crate/1.0/mod/func()")
            .unwrap();
        assert_eq!(stub.display_name, "func");
        assert!(stub.code_path.is_empty());
        assert_eq!(stub.code_text.lines_start, 0);
        assert!(stub.dependencies.is_empty());
    }

    #[test]
    fn test_add_external_stubs_skips_existing() {
        let mut atoms_dict = BTreeMap::new();
        let mut deps = BTreeSet::new();
        deps.insert("probe:my-crate/1.0/other()".to_string());

        atoms_dict.insert(
            "probe:my-crate/1.0/caller()".to_string(),
            AtomWithLines {
                display_name: "caller".to_string(),
                code_name: "probe:my-crate/1.0/caller()".to_string(),
                dependencies: deps,
                dependencies_with_locations: Vec::new(),
                code_module: String::new(),
                code_path: "src/lib.rs".to_string(),
                code_text: CodeTextInfo {
                    lines_start: 10,
                    lines_end: 20,
                },
                kind: DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: None,
                is_public: None,
                is_public_api: None,
                has_body: None,
                is_external: None,
                is_cfg_gated: None,
            },
        );
        atoms_dict.insert(
            "probe:my-crate/1.0/other()".to_string(),
            AtomWithLines {
                display_name: "other".to_string(),
                code_name: "probe:my-crate/1.0/other()".to_string(),
                dependencies: BTreeSet::new(),
                dependencies_with_locations: Vec::new(),
                code_module: String::new(),
                code_path: "src/lib.rs".to_string(),
                code_text: CodeTextInfo {
                    lines_start: 30,
                    lines_end: 40,
                },
                kind: DeclKind::Exec,
                language: "rust".to_string(),
                rust_qualified_name: None,
                is_public: None,
                is_public_api: None,
                has_body: None,
                is_external: None,
                is_cfg_gated: None,
            },
        );

        let count = add_external_stubs(&mut atoms_dict);
        assert_eq!(count, 0);
        assert_eq!(atoms_dict.len(), 2);
    }

    #[test]
    fn test_language_field_defaults_to_rust_on_old_json() {
        let old_json = serde_json::json!({
            "display-name": "foo",
            "dependencies": [],
            "code-module": "",
            "code-path": "src/lib.rs",
            "code-text": { "lines-start": 1, "lines-end": 10 },
            "kind": "exec"
        });
        let atom: AtomWithLines = serde_json::from_value(old_json).unwrap();
        assert_eq!(atom.language, "rust");
    }

    #[test]
    fn test_language_field_preserved_from_json() {
        let lean_json = serde_json::json!({
            "display-name": "Foo.bar",
            "dependencies": [],
            "code-module": "",
            "code-path": "Foo.lean",
            "code-text": { "lines-start": 1, "lines-end": 10 },
            "kind": "exec",
            "language": "lean"
        });
        let atom: AtomWithLines = serde_json::from_value(lean_json).unwrap();
        assert_eq!(atom.language, "lean");
    }

    #[test]
    fn test_language_field_serialized_in_output() {
        let atom = AtomWithLines {
            display_name: "foo".to_string(),
            code_name: "probe:crate/1.0/foo()".to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: "src/lib.rs".to_string(),
            code_text: CodeTextInfo {
                lines_start: 1,
                lines_end: 10,
            },
            kind: DeclKind::Exec,
            language: "rust".to_string(),
            rust_qualified_name: None,
            is_public: None,
            is_public_api: None,
            has_body: None,
            is_external: None,
            is_cfg_gated: None,
        };
        let json = serde_json::to_value(&atom).unwrap();
        assert_eq!(json["language"], "rust");
    }

    #[test]
    fn test_envelope_aware_atom_loading() {
        use crate::metadata::unwrap_envelope;

        let enveloped = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "3.0",
            "tool": { "name": "probe-verus", "version": "2.0.0", "command": "atomize" },
            "source": {
                "repo": "", "commit": "", "language": "rust",
                "package": "test", "package-version": "1.0.0"
            },
            "timestamp": "2026-03-06T12:00:00Z",
            "data": {
                "probe:test/1.0.0/foo()": {
                    "display-name": "foo",
                    "dependencies": [],
                    "code-module": "",
                    "code-path": "src/lib.rs",
                    "code-text": { "lines-start": 1, "lines-end": 10 },
                    "kind": "exec",
                    "language": "rust"
                }
            }
        });

        let data = unwrap_envelope(enveloped);
        let atoms: BTreeMap<String, AtomWithLines> = serde_json::from_value(data).unwrap();
        assert_eq!(atoms.len(), 1);
        assert!(atoms.contains_key("probe:test/1.0.0/foo()"));
        assert_eq!(atoms["probe:test/1.0.0/foo()"].language, "rust");
    }

    #[test]
    fn test_bare_dict_atom_loading() {
        use crate::metadata::unwrap_envelope;

        let bare = serde_json::json!({
            "probe:test/1.0.0/foo()": {
                "display-name": "foo",
                "dependencies": [],
                "code-module": "",
                "code-path": "src/lib.rs",
                "code-text": { "lines-start": 1, "lines-end": 10 },
                "kind": "exec"
            }
        });

        let data = unwrap_envelope(bare);
        let atoms: BTreeMap<String, AtomWithLines> = serde_json::from_value(data).unwrap();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms["probe:test/1.0.0/foo()"].language, "rust");
    }

    #[test]
    fn test_derive_rust_qualified_name_free_function() {
        let rqn =
            derive_rust_qualified_name("curve25519-dalek/src/backend/mod.rs", "variable_base_mul");
        assert_eq!(rqn.unwrap(), "curve25519_dalek::backend::variable_base_mul");
    }

    #[test]
    fn test_derive_rust_qualified_name_method() {
        let rqn = derive_rust_qualified_name(
            "curve25519-dalek/src/backend/serial/u64/field.rs",
            "FieldElement51::reduce",
        );
        assert_eq!(
            rqn.unwrap(),
            "curve25519_dalek::backend::serial::u64::field::FieldElement51::reduce"
        );
    }

    #[test]
    fn test_derive_rust_qualified_name_lib_root() {
        let rqn = derive_rust_qualified_name("my-crate/src/lib.rs", "init");
        assert_eq!(rqn.unwrap(), "my_crate::init");
    }

    #[test]
    fn test_derive_rust_qualified_name_empty_path() {
        assert!(derive_rust_qualified_name("", "foo").is_none());
    }

    #[test]
    fn test_derive_rust_qualified_name_no_src() {
        assert!(derive_rust_qualified_name("some/path/file.rs", "foo").is_none());
    }

    // =========================================================================
    // derive_module_path_from_code_path tests
    // =========================================================================

    #[test]
    fn test_derive_module_path_with_crate_src() {
        assert_eq!(
            derive_module_path_from_code_path(
                "curve25519-dalek/src/lemmas/common_lemmas/bit_lemmas.rs"
            ),
            "lemmas/common_lemmas/bit_lemmas"
        );
    }

    #[test]
    fn test_derive_module_path_bare_src_prefix() {
        assert_eq!(
            derive_module_path_from_code_path("src/lemmas/common_lemmas/bit_lemmas.rs"),
            "lemmas/common_lemmas/bit_lemmas"
        );
    }

    #[test]
    fn test_derive_module_path_mod_and_crate_root() {
        assert_eq!(
            derive_module_path_from_code_path("curve25519-dalek/src/backend/serial/mod.rs"),
            "backend/serial"
        );
        assert_eq!(derive_module_path_from_code_path("src/lib.rs"), "");
        assert_eq!(derive_module_path_from_code_path("src/main.rs"), "");
    }

    #[test]
    fn test_derive_module_path_no_src() {
        assert_eq!(derive_module_path_from_code_path("build.rs"), "build");
    }

    #[test]
    fn test_derive_module_path_simple() {
        assert_eq!(
            derive_module_path_from_code_path("my-crate/src/field.rs"),
            "field"
        );
    }

    #[test]
    fn test_rust_qualified_name_serialized_when_present() {
        let atom = AtomWithLines {
            display_name: "reduce".to_string(),
            code_name: "probe:crate/1.0/reduce()".to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: "my-crate/src/field.rs".to_string(),
            code_text: CodeTextInfo {
                lines_start: 1,
                lines_end: 10,
            },
            kind: DeclKind::Exec,
            language: "rust".to_string(),
            rust_qualified_name: Some("my_crate::field::reduce".to_string()),
            is_public: None,
            is_public_api: None,
            has_body: None,
            is_external: None,
            is_cfg_gated: None,
        };
        let json = serde_json::to_value(&atom).unwrap();
        assert_eq!(json["rust-qualified-name"], "my_crate::field::reduce");
    }

    #[test]
    fn test_rust_qualified_name_omitted_when_none() {
        let atom = AtomWithLines {
            display_name: "foo".to_string(),
            code_name: "probe:crate/1.0/foo()".to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: String::new(),
            code_text: CodeTextInfo {
                lines_start: 0,
                lines_end: 0,
            },
            kind: DeclKind::Exec,
            language: "rust".to_string(),
            rust_qualified_name: None,
            is_public: None,
            is_public_api: None,
            has_body: None,
            is_external: None,
            is_cfg_gated: None,
        };
        let json = serde_json::to_value(&atom).unwrap();
        assert!(json.get("rust-qualified-name").is_none());
    }

    // =========================================================================
    // is_signature_public tests
    // =========================================================================

    #[test]
    fn test_is_signature_public_pub_fn() {
        assert!(is_signature_public("pub fn foo()"));
    }

    #[test]
    fn test_is_signature_public_pub_unsafe_fn() {
        assert!(is_signature_public("pub unsafe fn bar()"));
    }

    #[test]
    fn test_is_signature_public_pub_async_fn() {
        assert!(is_signature_public("pub async fn baz()"));
    }

    #[test]
    fn test_is_signature_public_not_pub_crate() {
        assert!(!is_signature_public("pub(crate) fn foo()"));
    }

    #[test]
    fn test_is_signature_public_not_pub_super() {
        assert!(!is_signature_public("pub(super) fn foo()"));
    }

    #[test]
    fn test_is_signature_public_private() {
        assert!(!is_signature_public("fn foo()"));
    }

    #[test]
    fn test_is_signature_public_empty() {
        assert!(!is_signature_public(""));
    }

    #[test]
    fn test_is_signature_public_leading_whitespace() {
        assert!(is_signature_public("  pub fn foo()"));
    }

    // =========================================================================
    // is_trait_impl_code_name tests
    // =========================================================================

    #[test]
    fn test_trait_impl_borrowed_self() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul()"
        ));
    }

    #[test]
    fn test_trait_impl_generic_self() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&EdwardsPoint>`]from()"
        ));
    }

    #[test]
    fn test_trait_impl_plain_trait() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/window/impl#[LookupTable][Clone]clone()"
        ));
    }

    #[test]
    fn test_trait_impl_with_line_suffix() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/scalar/impl#[Scalar][`FromSpecImpl<u8>`]from_spec()@1080"
        ));
    }

    #[test]
    fn test_inherent_impl_not_trait() {
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/backend/serial/u64/field/impl#[FieldElement51]square()"
        ));
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/field/impl#[`crate::lizard::lizard_constants::FieldElement51`]is_zero()"
        ));
    }

    #[test]
    fn test_free_function_not_trait() {
        assert!(!is_trait_impl_code_name("probe:crate/1.0/montgomery/mul()"));
    }

    #[test]
    fn test_trait_method_declaration_not_trait_impl() {
        assert!(!is_trait_impl_code_name(
            "probe:core/1.0/cmp/PartialEq#eq()"
        ));
    }

    #[test]
    fn test_empty_string_not_trait() {
        assert!(!is_trait_impl_code_name(""));
    }

    // =========================================================================
    // classify_public_api tests
    // =========================================================================

    #[test]
    fn test_classify_public_api_external_stub() {
        let map = HashMap::new();
        assert_eq!(
            classify_public_api(true, "", "", DeclKind::Exec, &map, true),
            None
        );
    }

    #[test]
    fn test_classify_public_api_binary_crate() {
        let map = HashMap::new();
        assert_eq!(
            classify_public_api(true, "", "src/main.rs", DeclKind::Exec, &map, false),
            Some(false)
        );
    }

    fn mi(is_pub: bool) -> ModuleInfo {
        ModuleInfo {
            is_pub_chain: is_pub,
            is_cfg: false,
            from_cfg_only: false,
        }
    }

    #[test]
    fn test_classify_public_api_spec_fn() {
        let mut map = HashMap::new();
        map.insert("src/lib.rs".to_string(), mi(true));
        assert_eq!(
            classify_public_api(true, "", "src/lib.rs", DeclKind::Spec, &map, true),
            Some(false)
        );
    }

    #[test]
    fn test_classify_public_api_proof_fn() {
        let mut map = HashMap::new();
        map.insert("src/lib.rs".to_string(), mi(true));
        assert_eq!(
            classify_public_api(true, "", "src/lib.rs", DeclKind::Proof, &map, true),
            Some(false)
        );
    }

    #[test]
    fn test_classify_public_api_private_fn() {
        let mut map = HashMap::new();
        map.insert("src/lib.rs".to_string(), mi(true));
        assert_eq!(
            classify_public_api(false, "", "src/lib.rs", DeclKind::Exec, &map, true),
            Some(false)
        );
    }

    #[test]
    fn test_classify_public_api_pub_exec_in_pub_module() {
        let mut map = HashMap::new();
        map.insert("src/scalar.rs".to_string(), mi(true));
        assert_eq!(
            classify_public_api(true, "", "src/scalar.rs", DeclKind::Exec, &map, true),
            Some(true)
        );
    }

    #[test]
    fn test_classify_public_api_pub_exec_in_private_module() {
        let mut map = HashMap::new();
        map.insert("src/internal.rs".to_string(), mi(false));
        assert_eq!(
            classify_public_api(true, "", "src/internal.rs", DeclKind::Exec, &map, true),
            Some(false)
        );
    }

    #[test]
    fn test_classify_public_api_unknown_file() {
        let map = HashMap::new();
        assert_eq!(
            classify_public_api(true, "", "src/unknown.rs", DeclKind::Exec, &map, true),
            Some(false)
        );
    }

    #[test]
    fn test_classify_public_api_trait_impl_in_pub_module() {
        let mut map = HashMap::new();
        map.insert("src/lib.rs".to_string(), mi(true));
        let code_name = "probe:mycrate/0.1.0/impl#[Counter][`Add<Counter>`]add()";
        assert_eq!(
            classify_public_api(false, code_name, "src/lib.rs", DeclKind::Exec, &map, true),
            Some(true)
        );
    }

    #[test]
    fn test_classify_public_api_trait_impl_in_private_module() {
        let mut map = HashMap::new();
        map.insert("src/internal.rs".to_string(), mi(false));
        let code_name = "probe:mycrate/0.1.0/impl#[Counter][`Add<Counter>`]add()";
        assert_eq!(
            classify_public_api(
                false,
                code_name,
                "src/internal.rs",
                DeclKind::Exec,
                &map,
                true
            ),
            Some(false)
        );
    }

    #[test]
    fn test_classify_public_api_private_fn_not_trait_impl() {
        let mut map = HashMap::new();
        map.insert("src/lib.rs".to_string(), mi(true));
        assert_eq!(
            classify_public_api(
                false,
                "probe-verus://mycrate/0.1.0/regular_fn()",
                "src/lib.rs",
                DeclKind::Exec,
                &map,
                true
            ),
            Some(false)
        );
    }

    // =========================================================================
    // is_library_crate tests
    // =========================================================================

    #[test]
    fn test_is_library_crate_with_lib_rs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        assert!(is_library_crate(dir.path()));
    }

    #[test]
    fn test_is_library_crate_with_lib_section() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n\n[lib]\nname = \"test\"",
        )
        .unwrap();
        assert!(is_library_crate(dir.path()));
    }

    #[test]
    fn test_is_library_crate_binary_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        assert!(!is_library_crate(dir.path()));
    }

    // =========================================================================
    // is-public / is-public-api serialization tests
    // =========================================================================

    #[test]
    fn test_is_public_serialized_when_present() {
        let atom = AtomWithLines {
            display_name: "foo".to_string(),
            code_name: "probe:crate/1.0/foo()".to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: "src/lib.rs".to_string(),
            code_text: CodeTextInfo {
                lines_start: 1,
                lines_end: 10,
            },
            kind: DeclKind::Exec,
            language: "rust".to_string(),
            rust_qualified_name: None,
            is_public: Some(true),
            is_public_api: Some(true),
            has_body: Some(true),
            is_external: Some(false),
            is_cfg_gated: Some(false),
        };
        let json = serde_json::to_value(&atom).unwrap();
        assert_eq!(json["is-public"], true);
        assert_eq!(json["is-public-api"], true);
        assert_eq!(json["has-body"], true);
        assert_eq!(json["is-external"], false);
        assert_eq!(json["is-cfg-gated"], false);
    }

    #[test]
    fn test_is_public_omitted_when_none() {
        let atom = AtomWithLines {
            display_name: "foo".to_string(),
            code_name: "probe:crate/1.0/foo()".to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: String::new(),
            code_text: CodeTextInfo {
                lines_start: 0,
                lines_end: 0,
            },
            kind: DeclKind::Exec,
            language: "rust".to_string(),
            rust_qualified_name: None,
            is_public: None,
            is_public_api: None,
            has_body: None,
            is_external: None,
            is_cfg_gated: None,
        };
        let json = serde_json::to_value(&atom).unwrap();
        assert!(json.get("is-public").is_none());
        assert!(json.get("is-public-api").is_none());
    }

    #[test]
    fn test_is_public_deserialized_from_old_json() {
        let old_json = serde_json::json!({
            "display-name": "foo",
            "dependencies": [],
            "code-module": "",
            "code-path": "src/lib.rs",
            "code-text": { "lines-start": 1, "lines-end": 10 },
            "kind": "exec"
        });
        let atom: AtomWithLines = serde_json::from_value(old_json).unwrap();
        assert_eq!(atom.is_public, None);
        assert_eq!(atom.is_public_api, None);
    }

    // =========================================================================
    // build_module_visibility_map tests
    // =========================================================================

    #[test]
    fn test_build_module_visibility_map_simple() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();

        std::fs::write(src.join("lib.rs"), "pub mod scalar;\nmod internal;\n").unwrap();
        std::fs::write(src.join("scalar.rs"), "pub fn foo() {}\n").unwrap();
        std::fs::write(src.join("internal.rs"), "pub fn bar() {}\n").unwrap();

        let map = build_module_visibility_map(dir.path());

        let lib = map.get("src/lib.rs").unwrap();
        assert!(lib.is_pub_chain);
        assert!(!lib.is_cfg);
        let scalar = map.get("src/scalar.rs").unwrap();
        assert!(scalar.is_pub_chain);
        assert!(!scalar.is_cfg);
        let internal = map.get("src/internal.rs").unwrap();
        assert!(!internal.is_pub_chain);
        assert!(!internal.is_cfg);
    }

    #[test]
    fn test_build_module_visibility_map_nested() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("backend")).unwrap();

        std::fs::write(src.join("lib.rs"), "pub mod backend;\n").unwrap();
        std::fs::write(src.join("backend/mod.rs"), "pub mod serial;\n").unwrap();
        std::fs::write(src.join("backend/serial.rs"), "").unwrap();

        let map = build_module_visibility_map(dir.path());

        assert!(map.get("src/lib.rs").unwrap().is_pub_chain);
        assert!(map.get("src/backend/mod.rs").unwrap().is_pub_chain);
        assert!(map.get("src/backend/serial.rs").unwrap().is_pub_chain);
    }

    #[test]
    fn test_build_module_visibility_map_chain_broken() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("internal")).unwrap();

        std::fs::write(src.join("lib.rs"), "mod internal;\n").unwrap();
        std::fs::write(src.join("internal/mod.rs"), "pub mod deep;\n").unwrap();
        std::fs::write(src.join("internal/deep.rs"), "").unwrap();

        let map = build_module_visibility_map(dir.path());

        assert!(!map.get("src/internal/mod.rs").unwrap().is_pub_chain);
        assert!(
            !map.get("src/internal/deep.rs").unwrap().is_pub_chain,
            "deep is pub but its parent is not, so chain is broken"
        );
    }

    #[test]
    fn test_build_module_visibility_map_cfg_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();

        std::fs::write(
            src.join("lib.rs"),
            "pub mod normal;\n#[cfg(feature = \"alloc\")]\npub mod gated;\n",
        )
        .unwrap();
        std::fs::write(src.join("normal.rs"), "").unwrap();
        std::fs::write(src.join("gated.rs"), "").unwrap();

        let map = build_module_visibility_map(dir.path());

        let normal = map.get("src/normal.rs").unwrap();
        assert!(normal.is_pub_chain);
        assert!(!normal.is_cfg);

        let gated = map.get("src/gated.rs").unwrap();
        assert!(gated.is_pub_chain);
        assert!(gated.is_cfg, "cfg-gated module should have is_cfg=true");
    }

    // =========================================================================
    // resolve_workspace_root tests
    // =========================================================================

    #[test]
    fn test_resolve_workspace_root_package_only() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let result = resolve_workspace_root(tmp.path(), None).unwrap();
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn test_resolve_workspace_root_hybrid() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\n\n[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let result = resolve_workspace_root(tmp.path(), None).unwrap();
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn test_resolve_workspace_root_single_member_auto() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"my-crate\"]\n",
        )
        .unwrap();
        let member = tmp.path().join("my-crate");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let result = resolve_workspace_root(tmp.path(), None).unwrap();
        assert_eq!(result, member);
    }

    #[test]
    fn test_resolve_workspace_root_multi_member_with_package() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crate-a\", \"crate-b\"]\n",
        )
        .unwrap();
        for name in &["crate-a", "crate-b"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap();
        }

        let result = resolve_workspace_root(tmp.path(), Some("crate-b")).unwrap();
        assert_eq!(result, tmp.path().join("crate-b"));
    }

    #[test]
    fn test_resolve_workspace_root_multi_member_no_package_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crate-a\", \"crate-b\"]\n",
        )
        .unwrap();
        for name in &["crate-a", "crate-b"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )
            .unwrap();
        }

        let err = resolve_workspace_root(tmp.path(), None).unwrap_err();
        assert!(err.contains("workspace root"), "error: {err}");
        assert!(err.contains("crate-a"), "error should list members: {err}");
        assert!(err.contains("crate-b"), "error should list members: {err}");
    }

    #[test]
    fn test_resolve_workspace_root_no_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_workspace_root(tmp.path(), None).unwrap();
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn test_resolve_workspace_root_package_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crate-a\"]\n",
        )
        .unwrap();
        let dir = tmp.path().join("crate-a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let err = resolve_workspace_root(tmp.path(), Some("nonexistent")).unwrap_err();
        assert!(err.contains("no member matches"), "error: {err}");
    }

    #[test]
    fn test_resolve_workspace_root_single_member_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"ghost-crate\"]\n",
        )
        .unwrap();

        let err = resolve_workspace_root(tmp.path(), None).unwrap_err();
        assert!(
            err.contains("does not exist"),
            "should mention missing dir: {err}"
        );
    }

    #[test]
    fn test_resolve_workspace_root_invalid_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "not valid {{ toml").unwrap();
        let result = resolve_workspace_root(tmp.path(), None).unwrap();
        assert_eq!(result, tmp.path());
    }

    #[test]
    fn test_resolve_workspace_root_workspace_without_members() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let result = resolve_workspace_root(tmp.path(), None).unwrap();
        assert_eq!(result, tmp.path());
    }
}
