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
    is_definition, is_external_function_symbol, is_function_like_kind, LINE_TOLERANCE,
    PROBE_URI_PREFIX, SCIP_SYMBOL_PREFIX,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ScipToolInfo {
    pub name: String,
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
    let ty = ty.strip_prefix("mut ").unwrap_or(ty);
    let ty = ty.split('<').next().unwrap_or(ty);
    ty.rsplit("::").next().unwrap_or(ty)
}

/// Remove lifetime parameters from a SCIP symbol, so code_names do not depend
/// on lifetime names: `` [`From<&'a EdwardsPoint>`] `` -> `` [`From<&EdwardsPoint>`] ``.
fn strip_lifetimes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let starts_lifetime =
            c == '\'' && chars.peek().is_some_and(|n| n.is_alphabetic() || *n == '_');
        if !starts_lifetime {
            out.push(c);
            continue;
        }
        while chars
            .peek()
            .is_some_and(|n| n.is_alphanumeric() || *n == '_')
        {
            chars.next();
        }
        // Drop the separator that followed the lifetime (`'a T`, `'a, T`).
        if chars.peek() == Some(&',') {
            chars.next();
        }
        if chars.peek() == Some(&' ') {
            chars.next();
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

/// Create a unique call-graph key for a function definition.
///
/// SCIP symbols are unique per definition except for a few analyzer bugs
/// (e.g. spec-only trait impls whose trait is dropped from the symbol), so the
/// definition line is included to keep such definitions apart.
fn make_unique_key(symbol: &str, line: i32) -> String {
    format!("{}@{}", symbol, line)
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

/// Whether a SCIP index uses the pre-2026-08-22 verus-analyzer symbol format
/// (`module/Type#Trait#method().`, Self type sometimes missing) instead of the
/// rust-analyzer format (`module/impl#[Type][Trait]method().`).
///
/// probe-verus code_names are derived directly from the symbol, so a legacy
/// index yields ambiguous and inconsistent code_names.
#[must_use]
pub fn uses_legacy_symbol_format(scip_data: &ScipIndex) -> bool {
    let mut has_method = false;
    for symbol in scip_data.documents.iter().flat_map(|d| &d.symbols) {
        if !is_function_like(symbol.kind) {
            continue;
        }
        if symbol.symbol.contains("impl#[") {
            return false;
        }
        let last_segment = symbol.symbol.rsplit('/').next().unwrap_or("");
        has_method |= last_segment.contains('#');
    }
    has_method
}

/// Build a call graph from SCIP data, keyed by `symbol@definition_line`.
pub fn build_call_graph(scip_data: &ScipIndex) -> HashMap<String, FunctionNode> {
    let mut call_graph: HashMap<String, FunctionNode> = HashMap::new();
    let mut all_function_symbols: HashSet<String> = HashSet::new();

    // Pre-pass: find where each symbol is DEFINED. A symbol normally has one
    // definition; analyzer bugs can yield several, which are kept apart by line.
    // Maps symbol -> Vec<(path, line_number)>, sorted by line.
    let mut symbol_to_definitions: HashMap<String, Vec<(String, i32)>> = HashMap::new();
    for doc in &scip_data.documents {
        let rel_path = doc.relative_path.trim_start_matches('/').to_string();
        for occurrence in &doc.occurrences {
            if is_definition(occurrence.symbol_roles) && !occurrence.range.is_empty() {
                symbol_to_definitions
                    .entry(occurrence.symbol.clone())
                    .or_default()
                    .push((rel_path.clone(), occurrence.range[0]));
            }
        }
    }
    for defs in symbol_to_definitions.values_mut() {
        defs.sort_by_key(|(_, line)| *line);
    }

    // First pass: create a node per project function definition. The nth
    // `symbols[]` entry for a symbol is paired with its nth definition.
    let mut symbol_line_to_key: HashMap<(String, i32), String> = HashMap::new();
    let mut symbol_seen_count: HashMap<String, usize> = HashMap::new();
    for doc in &scip_data.documents {
        for symbol in &doc.symbols {
            if !is_function_like(symbol.kind) {
                continue;
            }
            let base_display_name = symbol
                .display_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let display_name = enrich_display_name(&symbol.symbol, &base_display_name);

            let seen = symbol_seen_count.entry(symbol.symbol.clone()).or_insert(0);
            let def_index = *seen;
            *seen += 1;

            // Track ALL function symbols for dependency tracking
            all_function_symbols.insert(symbol.symbol.clone());

            // Only add to call_graph if DEFINED in this project
            let Some((rel_path, line)) = symbol_to_definitions
                .get(&symbol.symbol)
                .and_then(|defs| defs.get(def_index))
            else {
                continue;
            };
            let unique_key = make_unique_key(&symbol.symbol, *line);
            symbol_line_to_key.insert((symbol.symbol.clone(), *line), unique_key.clone());
            call_graph.insert(
                unique_key,
                FunctionNode {
                    symbol: symbol.symbol.clone(),
                    display_name,
                    signature_text: symbol.signature_documentation.text.clone(),
                    relative_path: rel_path.clone(),
                    callees: HashSet::new(),
                    range: Vec::new(),
                },
            );
        }
    }

    // Second pass: build call relationships and extract ranges
    for doc in &scip_data.documents {
        let mut current_function_key: Option<String> = None;

        let mut ordered_occurrences = doc.occurrences.clone();
        ordered_occurrences.retain(|o| o.range.len() >= 2);
        ordered_occurrences.sort_by(|a, b| {
            let a_start = (a.range[0], a.range[1]);
            let b_start = (b.range[0], b.range[1]);
            a_start.cmp(&b_start)
        });

        for occurrence in &ordered_occurrences {
            let line = occurrence.range[0];

            // Track when we enter a project function definition
            if is_definition(occurrence.symbol_roles) {
                if let Some(key) = symbol_line_to_key.get(&(occurrence.symbol.clone(), line)) {
                    current_function_key = Some(key.clone());
                    if let Some(node) = call_graph.get_mut(key) {
                        node.range = occurrence.range.clone();
                    }
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
            if let Some(caller_node) = current_function_key
                .as_ref()
                .and_then(|key| call_graph.get_mut(key))
            {
                if caller_node.symbol != occurrence.symbol {
                    caller_node.callees.insert(CalleeInfo {
                        symbol: occurrence.symbol.clone(),
                        line,
                    });
                }
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
    // duplicates get an `@line` suffix.
    let mut code_name_count: HashMap<&str, usize> = HashMap::new();
    for data in &node_data {
        *code_name_count.entry(&data.base_code_name).or_insert(0) += 1;
    }
    let final_code_names: Vec<String> = node_data
        .iter()
        .map(|data| {
            let is_duplicate = code_name_count[data.base_code_name.as_str()] > 1;
            if is_duplicate && data.lines_start > 0 {
                symbol_to_code_name(&data.node.symbol, Some(data.lines_start))
            } else {
                data.base_code_name.clone()
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
                    Some(names) => names.clone(),
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

/// Whether `code_name` is the method `method` owned by `owner`, where the owner
/// is the impl's Self type or trait (`impl#[Owner][..]method()`,
/// `impl#[..][Owner<..>]method()`) or the trait of a trait method declaration
/// (`Owner#method()`). Generic arguments, references and paths are ignored.
#[must_use]
pub fn code_name_has_owner_and_method(code_name: &str, owner: &str, method: &str) -> bool {
    if extract_display_name_from_code_name(code_name) != method {
        return false;
    }
    match parse_impl_segment(code_name) {
        Some(seg) => {
            bare_type_name(seg.self_type) == owner
                || seg.trait_type.is_some_and(|t| bare_type_name(t) == owner)
        }
        None => code_name
            .rsplit('/')
            .next()
            .and_then(|last| last.split_once('#'))
            .is_some_and(|(o, _)| o == owner),
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

/// Backfill atoms from `verus_parser` for functions that SCIP missed.
///
/// Runs the verus source parser over the project, identifies functions that are not yet
/// present in `atoms_dict`, and inserts minimal atoms for them.  This closes the gap
/// for code inside `#[cfg(verus_keep_ghost)] verus! { … }` blocks that verus-analyzer's
/// SCIP mode cannot expand.
///
/// Returns the number of atoms added.
pub fn backfill_atoms_from_parser(
    project_path: &Path,
    atoms_dict: &mut BTreeMap<String, AtomWithLines>,
    pkg_name: &str,
    pkg_version: &str,
    file_module_pub: &HashMap<String, ModuleInfo>,
    is_library: bool,
    code_path_prefix: &str,
) -> usize {
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

    let mut added = 0usize;

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

        let already_present = atoms_dict.values().any(|atom| {
            if atom.display_name != fi.name
                && !atom.display_name.ends_with(&format!("::{}", fi.name))
            {
                return false;
            }
            let path_ok = paths_match_by_suffix(&code_path, &atom.code_path)
                || extract_src_suffix(&code_path) == extract_src_suffix(&atom.code_path);
            if !path_ok {
                return false;
            }
            let diff = (fi.spec_text.lines_start as isize - atom.code_text.lines_start as isize)
                .unsigned_abs();
            diff <= LINE_TOLERANCE
                || (atom.code_text.lines_start >= fi.spec_text.lines_start
                    && atom.code_text.lines_start <= fi.spec_text.lines_end)
        });

        if already_present {
            continue;
        }

        let module_path = derive_module_path_from_code_path(&code_path);

        let code_name = format!(
            "{}{}{}/{}/{}()",
            PROBE_URI_PREFIX,
            pkg_name,
            pkg_version_segment(pkg_version),
            module_path,
            fi.name
        );

        let has_spec = fi.has_requires || fi.has_ensures;
        let is_replacement = if let Some(existing) = atoms_dict.get(&code_name) {
            if has_spec && existing.code_text.lines_start != fi.spec_text.lines_start {
                true
            } else {
                continue;
            }
        } else {
            false
        };

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
        let rqn = derive_rust_qualified_name(&rqn_path, &fi.name);
        atoms_dict.insert(
            code_name.clone(),
            AtomWithLines {
                display_name: fi.name.clone(),
                code_name: code_name.clone(),
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
                is_public_api: classify_public_api(
                    vis_public,
                    &code_name,
                    &code_path,
                    fi.kind,
                    file_module_pub,
                    is_library,
                ),
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
        );
        if !is_replacement {
            added += 1;
        }
    }

    added
}

fn derive_module_path_from_code_path(code_path: &str) -> String {
    let after_src = code_path
        .find("/src/")
        .map(|pos| &code_path[pos + 5..])
        .or_else(|| code_path.strip_prefix("src/"))
        .unwrap_or(code_path);
    after_src.trim_end_matches(".rs").to_string()
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
        assert_eq!(
            bare_type_name("crate::lizard::lizard_constants::FieldElement51"),
            "FieldElement51"
        );
    }

    #[test]
    fn test_code_name_has_owner_and_method() {
        let imp = "probe:subtle/2.6.1/impl#[u64][ConditionallySelectable]conditional_swap()";
        assert!(code_name_has_owner_and_method(
            imp,
            "u64",
            "conditional_swap"
        ));
        assert!(code_name_has_owner_and_method(
            imp,
            "ConditionallySelectable",
            "conditional_swap"
        ));
        assert!(!code_name_has_owner_and_method(
            imp,
            "u32",
            "conditional_swap"
        ));
        assert!(!code_name_has_owner_and_method(
            imp,
            "u64",
            "conditional_assign"
        ));
        let decl = "probe:subtle/2.6.1/ConditionallySelectable#conditional_swap()";
        assert!(code_name_has_owner_and_method(
            decl,
            "ConditionallySelectable",
            "conditional_swap"
        ));
        assert!(!code_name_has_owner_and_method(
            decl,
            "u64",
            "conditional_swap"
        ));
        let generic = "probe:subtle/2.6.1/impl#[Choice][`From<u8>`]from()";
        assert!(code_name_has_owner_and_method(generic, "Choice", "from"));
        assert!(code_name_has_owner_and_method(generic, "From", "from"));
    }

    fn index_with_symbols(symbols: &[&str]) -> ScipIndex {
        ScipIndex {
            metadata: Metadata {
                tool_info: ScipToolInfo {
                    name: "verus-analyzer".to_string(),
                    version: "0".to_string(),
                },
                project_root: String::new(),
                text_document_encoding: 0,
            },
            documents: vec![Document {
                language: "rust".to_string(),
                relative_path: "src/lib.rs".to_string(),
                occurrences: vec![],
                symbols: symbols
                    .iter()
                    .map(|s| Symbol {
                        symbol: s.to_string(),
                        kind: 6,
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

    #[test]
    fn test_uses_legacy_symbol_format() {
        let legacy = index_with_symbols(&[
            "rust-analyzer cargo c 1.0 montgomery/Mul#mul().",
            "rust-analyzer cargo c 1.0 lemmas/lemma_foo().",
        ]);
        assert!(uses_legacy_symbol_format(&legacy));
        let current = index_with_symbols(&[
            "rust-analyzer cargo c 1.0 montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul().",
            "rust-analyzer cargo c 1.0 lemmas/lemma_foo().",
        ]);
        assert!(!uses_legacy_symbol_format(&current));
        let free_functions_only =
            index_with_symbols(&["rust-analyzer cargo c 1.0 lemmas/lemma_foo()."]);
        assert!(!uses_legacy_symbol_format(&free_functions_only));
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
