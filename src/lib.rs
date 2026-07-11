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
    PROBE_URI_PREFIX, SCIP_SYMBOL_PREFIX, TYPE_CONTEXT_LOOKBACK_LINES,
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

/// A call from one function to another, with optional type context for disambiguation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CalleeInfo {
    /// The raw SCIP symbol of the callee
    pub symbol: String,
    /// Type hints found on the same line as the call (e.g., turbofish type parameters)
    /// Used to disambiguate calls to generic trait implementations
    pub type_hints: Vec<String>,
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
    /// Callees with their type context for disambiguation
    pub callees: HashSet<CalleeInfo>,
    pub range: Vec<i32>,
    /// The Self type for trait implementations, extracted from the `method().(self)` symbol.
    /// Used to repair verus-analyzer's inconsistent symbol format.
    /// e.g., "MontgomeryPoint" from "self: &MontgomeryPoint"
    pub self_type: Option<String>,
    /// Type context from the definition site (nearby type references).
    /// Used to disambiguate trait impls like `impl From<T> for Container<X>` vs `Container<Y>`.
    pub definition_type_context: Vec<String>,
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
/// Verus-analyzer SCIP encodes inherent impls as `SelfType#SelfType<Ret>#method()`
/// (the impl name matches the self type), while trait impls use
/// `SelfType#TraitName<Params>#method()` (impl name differs from self type).
///
/// This function requires 2+ `#` separators **and** that the impl-name segment
/// (between the first `#` and the next `<` or `#`) differs from the self-type
/// base name (the identifier before the first `#`, stripped of `&`/`mut/`/generics).
///
/// **Known limitation:** treats ALL trait impl methods as public, including
/// impls of `pub(crate)` or private traits. SCIP symbols do not encode trait
/// visibility. In practice the affected traits are public `core`/`std` traits.
#[must_use]
pub fn is_trait_impl_code_name(code_name: &str) -> bool {
    let s = code_name
        .strip_prefix(PROBE_URI_PREFIX)
        .unwrap_or(code_name);
    if s.matches('#').count() < 2 {
        return false;
    }
    let first_hash = match s.find('#') {
        Some(i) => i,
        None => return false,
    };
    // Self-type segment: everything between the last `/` (before first `#`) and the `#`.
    let before_hash = &s[..first_hash];
    let self_segment = match before_hash.rfind('/') {
        Some(i) => &before_hash[i + 1..],
        None => before_hash,
    };
    // Strip `&` and `mut/` prefixes, then drop `<...>` generics.
    let self_base = self_segment
        .trim_start_matches('&')
        .trim_start_matches("mut/");
    let self_base = self_base.split('<').next().unwrap_or(self_base);

    // Impl-name segment: between first `#` and the next `<` or `#`.
    let after_hash = &s[first_hash + 1..];
    let impl_name = after_hash.split(['<', '#']).next().unwrap_or("");

    !impl_name.is_empty() && impl_name != self_base
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
    /// cfg-inactive in the verification build, or an external-crate stub. Such atoms
    /// carry no `verification-status`. `false` = in scope: a specified function, a
    /// trusted axiom, or the spec-less backlog. Absent = scope not analyzed (specs
    /// not loaded). `has-verification-status ⟹ ¬is-disabled` (KB P24).
    #[serde(rename = "is-disabled", skip_serializing_if = "Option::is_none")]
    pub is_disabled: Option<bool>,
    /// Verification outcome for an in-scope atom. Values: `"verified"`,
    /// `"transitively-verified"`, `"failed"`, `"unverified"`, or `"trusted"` (in the
    /// trust base). Absent for the backlog and for out-of-scope atoms (`is-disabled: true`).
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

/// Create a unique key for a function by combining symbol, signature, self_type, and line number.
///
/// This handles multiple levels of potential collisions:
/// 1. Same symbol, different signature → distinguished by signature
/// 2. Same symbol & signature, different Self type → distinguished by self_type
/// 3. Same symbol, signature & self_type, different line → distinguished by line (fallback)
///
/// The line number fallback handles edge cases like:
/// ```text
/// impl<T> Marker<A> for X { fn mark(self) {} }  // line 10
/// impl<T> Marker<B> for X { fn mark(self) {} }  // line 20
/// ```
/// Where the trait type parameter doesn't appear in the method signature.
fn make_unique_key(
    symbol: &str,
    signature: &str,
    self_type: Option<&str>,
    line: Option<i32>,
) -> String {
    let base = match self_type {
        Some(st) => format!("{}|{}|{}", symbol, signature, st),
        None => format!("{}|{}", symbol, signature),
    };
    match line {
        Some(l) => format!("{}@{}", base, l),
        None => base,
    }
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
/// Extracts the Self type from the SCIP symbol format:
///   `path/Type#Trait<Args>#method().`  ->  `Type::method`
///   `path/&Type#Type<Ret>#method().`   ->  `Type::method`
///   `path/function().`                 ->  `function` (unchanged)
fn enrich_display_name(scip_symbol: &str, base_display_name: &str) -> String {
    let s = scip_symbol
        .strip_prefix(SCIP_SYMBOL_PREFIX)
        .unwrap_or(scip_symbol);
    // After stripping the prefix, the remaining format is "crate version path/..."
    let parts: Vec<&str> = s.splitn(3, ' ').collect();
    if parts.len() < 3 {
        return base_display_name.to_string();
    }
    let path_part = parts[2].trim_end_matches('.');
    // Get the segment after the last '/'
    let last_segment = path_part.rsplit('/').next().unwrap_or(path_part);
    // If it contains '#', the part before the first '#' is the Self type
    if let Some(hash_pos) = last_segment.find('#') {
        let self_type = &last_segment[..hash_pos];
        // Strip leading '&' for borrowed self
        let self_type = self_type.strip_prefix('&').unwrap_or(self_type);
        if !self_type.is_empty() {
            return format!("{}::{}", self_type, base_display_name);
        }
    }
    base_display_name.to_string()
}

/// Extract the base function/method name from a raw SCIP symbol.
///
/// For `rust-analyzer cargo x25519-dalek 2.0.1 x25519/StaticSecret#diffie_hellman().`
/// returns `"diffie_hellman"`.
/// For `rust-analyzer cargo core 1.0.0 mem/swap().` returns `"swap"`.
fn extract_function_name_from_symbol(symbol: &str) -> String {
    let s = symbol.strip_prefix(SCIP_SYMBOL_PREFIX).unwrap_or(symbol);
    let without_suffix = s.strip_suffix("().").unwrap_or(s);
    without_suffix
        .rsplit_once('#')
        .map(|(_, n)| n)
        .or_else(|| without_suffix.rsplit_once('/').map(|(_, n)| n))
        .unwrap_or(without_suffix)
        .to_string()
}

/// Build a call graph from SCIP data.
/// Returns the call graph and a map of all function symbols to their display names.
///
/// Note: Multiple trait implementations (e.g., `impl Mul<A> for B` and `impl Mul<B> for A`)
/// can have the same SCIP symbol string. We use signature_documentation.text to distinguish them.
pub fn build_call_graph(
    scip_data: &ScipIndex,
) -> (HashMap<String, FunctionNode>, HashMap<String, String>) {
    let mut call_graph: HashMap<String, FunctionNode> = HashMap::new();
    let mut project_function_keys: HashSet<String> = HashSet::new();
    let mut all_function_symbols: HashSet<String> = HashSet::new();
    let mut symbol_to_display_name: HashMap<String, String> = HashMap::new();

    // Pre-pass: Find where each symbol is DEFINED (symbol_roles == 1)
    // Collect ALL definition occurrences per symbol (there may be multiple for trait impls)
    // Maps symbol -> Vec<(path, line_number)>
    let mut symbol_to_definitions: HashMap<String, Vec<(String, i32)>> = HashMap::new();
    for doc in &scip_data.documents {
        let rel_path = doc.relative_path.trim_start_matches('/').to_string();
        for occurrence in &doc.occurrences {
            if is_definition(occurrence.symbol_roles) && !occurrence.range.is_empty() {
                let line = occurrence.range[0];
                symbol_to_definitions
                    .entry(occurrence.symbol.clone())
                    .or_default()
                    .push((rel_path.clone(), line));
            }
        }
    }

    // Sort definitions by line number for consistent matching with symbol entries
    for defs in symbol_to_definitions.values_mut() {
        defs.sort_by_key(|(_, line)| *line);
    }

    // Pre-pass: Collect type context for definitions (types near each definition line)
    // This helps disambiguate trait impls like `impl From<T> for Container<X>` vs `Container<Y>`
    // Maps (file_path, line) -> Vec<type_name>
    let mut definition_type_contexts: HashMap<(String, i32), Vec<String>> = HashMap::new();
    for doc in &scip_data.documents {
        let rel_path = doc.relative_path.trim_start_matches('/').to_string();

        // Collect all type references in this document
        let mut type_refs_by_line: HashMap<i32, Vec<String>> = HashMap::new();
        for occ in &doc.occurrences {
            if !is_definition(occ.symbol_roles)
                && !occ.range.is_empty()
                && occ.symbol.ends_with('#')
            {
                let line = occ.range[0];
                if let Some(type_name) = extract_type_name_from_symbol(&occ.symbol) {
                    type_refs_by_line.entry(line).or_default().push(type_name);
                }
            }
        }

        // For each definition line, collect types from nearby lines (within 5 lines before)
        for occ in &doc.occurrences {
            if is_definition(occ.symbol_roles) && !occ.range.is_empty() {
                let def_line = occ.range[0];
                let mut nearby_types = Vec::new();

                // Look at lines from def_line-N to def_line for type context
                for offset in 0..=TYPE_CONTEXT_LOOKBACK_LINES {
                    let check_line = def_line - offset;
                    if check_line >= 0 {
                        if let Some(types) = type_refs_by_line.get(&check_line) {
                            for t in types {
                                if !nearby_types.contains(t) {
                                    nearby_types.push(t.clone());
                                }
                            }
                        }
                    }
                }

                if !nearby_types.is_empty() {
                    definition_type_contexts.insert((rel_path.clone(), def_line), nearby_types);
                }
            }
        }
    }

    // Pre-pass: Collect self_type from `method().(self)` symbols
    // These have enclosing_symbol set and display_name == "self"
    // Since multiple trait impls can have the same symbol (verus-analyzer bug),
    // we collect all self_types per enclosing_symbol in order.
    // Maps enclosing_symbol -> Vec<self_type>
    let mut enclosing_to_self_types: HashMap<String, Vec<String>> = HashMap::new();
    for doc in &scip_data.documents {
        for symbol in &doc.symbols {
            // Look for self parameter symbols (display_name == "self" and has enclosing_symbol)
            if let Some(ref display_name) = symbol.display_name {
                if display_name == "self" {
                    if let Some(ref enclosing) = symbol.enclosing_symbol {
                        let self_sig = &symbol.signature_documentation.text;
                        if let Some(self_type) = extract_self_type(self_sig) {
                            enclosing_to_self_types
                                .entry(enclosing.clone())
                                .or_default()
                                .push(self_type);
                        }
                    }
                }
            }
        }
    }

    // Track how many times we've seen each symbol to pick the right self_type
    let mut symbol_self_type_idx: HashMap<String, usize> = HashMap::new();

    // First pass: identify all function symbols and handle duplicates
    // Track how many times we've seen each symbol to match with definition order
    let mut symbol_seen_count: HashMap<String, usize> = HashMap::new();

    for doc in &scip_data.documents {
        for symbol in &doc.symbols {
            if is_function_like(symbol.kind) {
                let signature = &symbol.signature_documentation.text;
                let base_display_name = symbol
                    .display_name
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                let display_name = enrich_display_name(&symbol.symbol, &base_display_name);

                // Get the nth definition for this symbol (matching symbol entry order with def order)
                let def_index = *symbol_seen_count.get(&symbol.symbol).unwrap_or(&0);
                symbol_seen_count
                    .entry(symbol.symbol.clone())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);

                // Look up self_type from the pre-collected map BEFORE creating unique key
                // Use the index to handle multiple impls with the same symbol
                let self_type =
                    if let Some(self_types) = enclosing_to_self_types.get(&symbol.symbol) {
                        let idx = *symbol_self_type_idx.get(&symbol.symbol).unwrap_or(&0);
                        symbol_self_type_idx
                            .entry(symbol.symbol.clone())
                            .and_modify(|i| *i += 1)
                            .or_insert(1);
                        self_types.get(idx).cloned()
                    } else {
                        None
                    };

                // P21: Re-enrich display name for single-hash trait impl symbols.
                // verus-analyzer emits "module/Trait#method()" (missing Self type)
                // which enrich_display_name turns into "Trait::method". Replace with
                // "SelfType::method" using the self_type from the SCIP pre-pass.
                let display_name = if is_missing_self_type(&symbol.symbol) {
                    if let Some(ref st) = self_type {
                        let bare_st = st.strip_prefix('&').unwrap_or(st);
                        let bare_st = bare_st.strip_prefix("mut ").unwrap_or(bare_st);
                        format!("{bare_st}::{base_display_name}")
                    } else {
                        display_name
                    }
                } else {
                    display_name
                };

                // Track ALL function symbols for dependency tracking
                all_function_symbols.insert(symbol.symbol.clone());
                symbol_to_display_name.insert(symbol.symbol.clone(), display_name.clone());

                // Only add to call_graph if DEFINED in this project
                if let Some(defs) = symbol_to_definitions.get(&symbol.symbol) {
                    if let Some((rel_path, line)) = defs.get(def_index) {
                        // Create unique key using signature, self_type, AND line number
                        // This handles all collision cases:
                        // - Same symbol, different signature → distinguished by signature
                        // - Same symbol & signature, different Self type → distinguished by self_type
                        // - Same symbol, signature & self_type → distinguished by line (fallback)
                        let unique_key = make_unique_key(
                            &symbol.symbol,
                            signature,
                            self_type.as_deref(),
                            Some(*line),
                        );

                        project_function_keys.insert(unique_key.clone());

                        // Look up definition type context (types near this definition line)
                        let def_type_context = definition_type_contexts
                            .get(&(rel_path.clone(), *line))
                            .cloned()
                            .unwrap_or_default();

                        call_graph.insert(
                            unique_key,
                            FunctionNode {
                                symbol: symbol.symbol.clone(),
                                display_name,
                                signature_text: signature.clone(),
                                relative_path: rel_path.clone(),
                                callees: HashSet::new(),
                                range: Vec::new(),
                                self_type,
                                definition_type_context: def_type_context,
                            },
                        );
                    }
                }
            }
        }
    }

    let mut symbol_line_to_key: HashMap<(String, i32), String> = HashMap::new();
    let mut symbol_seen_for_lines: HashMap<String, usize> = HashMap::new();
    let mut symbol_self_type_idx_for_lines: HashMap<String, usize> = HashMap::new();
    for doc in &scip_data.documents {
        for symbol in &doc.symbols {
            if is_function_like(symbol.kind) {
                let signature = &symbol.signature_documentation.text;

                // Get the definition index first so we can look up the line number
                let def_index = *symbol_seen_for_lines.get(&symbol.symbol).unwrap_or(&0);
                symbol_seen_for_lines
                    .entry(symbol.symbol.clone())
                    .and_modify(|c| *c += 1)
                    .or_insert(1);

                // Look up self_type (must match the same logic as the first pass)
                let self_type =
                    if let Some(self_types) = enclosing_to_self_types.get(&symbol.symbol) {
                        let idx = *symbol_self_type_idx_for_lines
                            .get(&symbol.symbol)
                            .unwrap_or(&0);
                        symbol_self_type_idx_for_lines
                            .entry(symbol.symbol.clone())
                            .and_modify(|i| *i += 1)
                            .or_insert(1);
                        self_types.get(idx).cloned()
                    } else {
                        None
                    };

                // Get line number from definitions
                if let Some(defs) = symbol_to_definitions.get(&symbol.symbol) {
                    if let Some((_, line)) = defs.get(def_index) {
                        let unique_key = make_unique_key(
                            &symbol.symbol,
                            signature,
                            self_type.as_deref(),
                            Some(*line),
                        );

                        if call_graph.contains_key(&unique_key) {
                            symbol_line_to_key.insert((symbol.symbol.clone(), *line), unique_key);
                        }
                    }
                }
            }
        }
    }

    // Second pass: build call relationships and extract ranges
    // Also collect type hints (symbols ending with #) for disambiguation
    for doc in &scip_data.documents {
        let mut current_function_key: Option<String> = None;

        let mut ordered_occurrences = doc.occurrences.clone();
        ordered_occurrences.retain(|o| o.range.len() >= 2);
        ordered_occurrences.sort_by(|a, b| {
            let a_start = (a.range[0], a.range[1]);
            let b_start = (b.range[0], b.range[1]);
            a_start.cmp(&b_start)
        });

        // Pre-collect type symbols per line for disambiguation
        // Type symbols are those ending with # (struct/type references)
        let mut line_to_type_hints: HashMap<i32, Vec<String>> = HashMap::new();
        for occ in &ordered_occurrences {
            if !is_definition(occ.symbol_roles) && !occ.range.is_empty() {
                let line = occ.range[0];
                // Check if this is a type reference (symbol ends with #)
                if occ.symbol.ends_with('#') {
                    // Extract just the type name from the symbol
                    // e.g., "rust-analyzer cargo ... curve_models/serial/backend/ProjectiveNielsPoint#"
                    // → "ProjectiveNielsPoint"
                    if let Some(type_name) = extract_type_name_from_symbol(&occ.symbol) {
                        line_to_type_hints.entry(line).or_default().push(type_name);
                    }
                }
            }
        }

        for occurrence in &ordered_occurrences {
            let is_def = is_definition(occurrence.symbol_roles);
            let line = if !occurrence.range.is_empty() {
                occurrence.range[0]
            } else {
                -1
            };

            // Track when we enter a project function definition
            if is_def {
                // Look up the unique key for this (symbol, line) pair
                if let Some(key) = symbol_line_to_key.get(&(occurrence.symbol.clone(), line)) {
                    current_function_key = Some(key.clone());
                    if let Some(node) = call_graph.get_mut(key) {
                        node.range = occurrence.range.clone();
                    }
                }
            }

            // Track ALL function calls (including to external functions)
            // Note: References use the base symbol, not the unique key
            if !is_def
                && (all_function_symbols.contains(&occurrence.symbol)
                    || is_external_function_symbol(&occurrence.symbol, &all_function_symbols))
            {
                // Register newly-discovered external function symbols so downstream
                // code can resolve their display names and code_names without fallback
                if all_function_symbols.insert(occurrence.symbol.clone()) {
                    let base_name = extract_function_name_from_symbol(&occurrence.symbol);
                    let enriched = enrich_display_name(&occurrence.symbol, &base_name);
                    symbol_to_display_name.insert(occurrence.symbol.clone(), enriched);
                }

                if let Some(caller_key) = &current_function_key {
                    if let Some(caller_node) = call_graph.get_mut(caller_key) {
                        // For callees, we store the base symbol with type hints
                        if caller_node.symbol != occurrence.symbol {
                            let type_hints =
                                line_to_type_hints.get(&line).cloned().unwrap_or_default();
                            caller_node.callees.insert(CalleeInfo {
                                symbol: occurrence.symbol.clone(),
                                type_hints,
                                line,
                            });
                        }
                    }
                }
            }
        }
    }

    (call_graph, symbol_to_display_name)
}

/// Extract the type name from a SCIP symbol ending with #
/// e.g., "rust-analyzer cargo curve25519-dalek 4.1.3 curve_models/serial/backend/ProjectiveNielsPoint#"
/// → "ProjectiveNielsPoint"
fn extract_type_name_from_symbol(symbol: &str) -> Option<String> {
    // Strip the trailing #
    let without_hash = symbol.trim_end_matches('#');
    // Get the last path component
    if let Some(last_slash) = without_hash.rfind('/') {
        let name = &without_hash[last_slash + 1..];
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Extract type parameter info from a signature for trait impls.
/// For example, from "fn mul(self, scalar: &Scalar) -> MontgomeryPoint"
/// extracts the self type and parameter types to help distinguish impls.
///
/// This function handles several patterns:
/// 1. Binary ops: `fn mul(self, rhs: &Scalar) -> ...` - extracts "Scalar" from second param
/// 2. From trait: `fn from(value: EdwardsPoint) -> ...` - extracts "EdwardsPoint" from first param
/// 3. Into trait: `fn into(self) -> RistrettoPoint` - extracts "RistrettoPoint" from return type
fn extract_impl_type_info(signature: &str) -> Option<String> {
    let signature = signature.trim();

    // Look for the parameter list
    let params_start = signature.find('(')?;
    let params_end = signature.find(')')?;
    let params = &signature[params_start + 1..params_end];

    // Split by comma and look for typed self or first param after self
    let parts: Vec<&str> = params.split(',').map(|s| s.trim()).collect();

    // Case 1: Two or more parameters (e.g., binary ops like Mul, Add)
    // Pattern: "fn method(self, param: &Type) -> ..."
    if parts.len() >= 2 {
        // Get the type of the second parameter (first after self)
        let second_param = parts[1];
        if let Some(type_str) = extract_type_from_param(second_param) {
            return Some(type_str);
        }
    }

    // Case 2: Single parameter that is NOT self (e.g., From::from)
    // Pattern: "fn from(value: SourceType) -> ..."
    if parts.len() == 1 {
        let first_param = parts[0].trim();
        // Skip if it's just "self" or "self: Type" (not a From-like method)
        if !first_param.is_empty() && !first_param.starts_with("self") && first_param.contains(':')
        {
            if let Some(type_str) = extract_type_from_param(first_param) {
                return Some(type_str);
            }
        }
    }

    // Case 3: No parameters or just self - try to extract from return type (e.g., Into::into)
    // Pattern: "fn into(self) -> TargetType"
    if let Some(arrow_pos) = signature.find("->") {
        let return_type = signature[arrow_pos + 2..].trim();
        // Clean up the return type
        let clean_return = clean_type_string(return_type);
        // Only use return type for disambiguation if it's a concrete type (not Self)
        if !clean_return.is_empty() && clean_return != "Self" {
            return Some(clean_return);
        }
    }

    None
}

/// Extract and clean a type from a parameter declaration like "param: &Type" or "param: Type"
/// Preserves the `&` to distinguish reference vs owned types.
fn extract_type_from_param(param: &str) -> Option<String> {
    let colon_pos = param.find(':')?;
    let type_part = param[colon_pos + 1..].trim();
    let clean = clean_type_string_preserve_ref(type_part);
    if clean.is_empty() {
        None
    } else {
        Some(clean)
    }
}

/// Clean up a type string by removing lifetimes but PRESERVING the reference marker (&).
/// This is important for distinguishing `impl From<&T>` from `impl From<T>`.
fn clean_type_string_preserve_ref(type_str: &str) -> String {
    let type_str = type_str.trim();

    // Check if it's a reference type
    let is_ref = type_str.starts_with('&');

    // Remove the & temporarily to clean up lifetimes
    let without_ref = type_str.trim_start_matches('&').trim();

    // Remove lifetime annotations
    let clean = without_ref
        .trim_start_matches("'a ")
        .trim_start_matches("'b ")
        .trim_start_matches("'_ ")
        .trim_start_matches("mut ")
        .trim();

    if clean.is_empty() {
        String::new()
    } else if is_ref {
        // Re-add the & for reference types
        format!("&{}", clean)
    } else {
        clean.to_string()
    }
}

/// Clean up a type string by removing references, lifetimes, and whitespace
/// Used for return types where we don't care about reference distinction.
fn clean_type_string(type_str: &str) -> String {
    type_str
        .trim()
        .trim_start_matches('&')
        .trim_start_matches("'a ")
        .trim_start_matches("'b ")
        .trim_start_matches("'_ ")
        .trim_start_matches("mut ")
        .trim()
        .to_string()
}

/// Extract the Self type from a self parameter signature.
/// For example, from "self: &MontgomeryPoint" extracts "&MontgomeryPoint".
/// From "self: Scalar" extracts "Scalar".
/// Preserves the `&` to distinguish owned vs reference implementations,
/// matching rust-analyzer's behavior.
fn extract_self_type(self_signature: &str) -> Option<String> {
    // Pattern: "self: &Type" or "self: &'a Type" or "self: Type"
    let self_signature = self_signature.trim();

    if let Some(colon_pos) = self_signature.find(':') {
        let type_part = self_signature[colon_pos + 1..].trim();

        // Check if it's a reference type
        let is_ref = type_part.starts_with('&');

        // Remove lifetime annotations but preserve the & if present
        let clean_type = type_part
            .trim_start_matches('&')
            .trim_start_matches("'a ")
            .trim_start_matches("'b ")
            .trim_start_matches("'_ ")
            .trim();

        if !clean_type.is_empty() {
            // Re-add the & if it was a reference type
            if is_ref {
                return Some(format!("&{}", clean_type));
            } else {
                return Some(clean_type.to_string());
            }
        }
    }

    None
}

/// Check if a symbol path is missing the Self type (verus-analyzer inconsistency).
/// verus-analyzer produces "module/Trait#method()" for reference Self types,
/// but "module/Type#Trait#method()" for owned Self types.
/// This function detects the former pattern.
fn is_missing_self_type(symbol: &str) -> bool {
    // Pattern for missing Self type: "module/Trait#method()" where Trait is capitalized
    // Pattern for present Self type: "module/Type#Trait#method()" has two # separators

    // Count the number of # in the symbol
    let hash_count = symbol.matches('#').count();

    // If there's only one #, and it's followed by a method name, Self type is likely missing
    // e.g., "montgomery/Mul#mul()" vs "montgomery/MontgomeryPoint#Mul#mul()"
    hash_count == 1
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

/// Convert symbol to a scip name, optionally including type info for disambiguation.
///
/// Parameters:
/// - `symbol`: The raw SCIP symbol string
/// - `display_name`: The function/method name
/// - `signature`: Optional function signature (e.g., "fn mul(self, scalar: &Scalar) -> MontgomeryPoint")
/// - `self_type`: Optional Self type extracted from the self parameter (e.g., "MontgomeryPoint")
///
/// This function repairs verus-analyzer's inconsistent symbol format by:
/// 1. Adding trait type parameters (e.g., `Mul` -> `Mul<Scalar>`) for disambiguation
/// 2. Adding the Self type when missing (e.g., `montgomery/Mul#mul` -> `montgomery/MontgomeryPoint#Mul#mul`)
/// 3. Adding line number suffix when type info alone can't disambiguate (e.g., generic impls)
fn symbol_to_code_name(
    symbol: &str,
    display_name: &str,
    signature: Option<&str>,
    self_type: Option<&str>,
) -> String {
    symbol_to_code_name_with_line(symbol, display_name, signature, self_type, None)
}

/// Convert symbol to scip name, with optional line number for disambiguation.
fn symbol_to_code_name_with_line(
    symbol: &str,
    display_name: &str,
    signature: Option<&str>,
    self_type: Option<&str>,
    line_number: Option<usize>,
) -> String {
    symbol_to_code_name_full(
        symbol,
        display_name,
        signature,
        self_type,
        line_number,
        None,
    )
    .unwrap_or_else(|e| {
        eprintln!("Warning: {}", e);
        let raw = symbol.replace("rust-analyzer cargo ", "").replace(' ', "/");
        let normalized = raw.strip_suffix('.').unwrap_or(&raw);
        format!("{}{}", PROBE_URI_PREFIX, normalized)
    })
}

/// Convert symbol to scip name with full disambiguation options.
///
/// # Arguments
/// * `symbol` - The raw SCIP symbol
/// * `display_name` - The function's display name
/// * `signature` - Optional signature text for type extraction
/// * `self_type` - Optional Self type for trait impls
/// * `line_number` - Optional line number (last resort disambiguation)
/// * `target_type` - Optional target type parameter for generic impls (e.g., "ProjectiveNielsPoint")
///
/// # Returns
/// Returns `Ok(String)` with the formatted scip name, or `Err(ProbeError)` if the symbol
/// format is invalid.
fn symbol_to_code_name_full(
    symbol: &str,
    display_name: &str,
    signature: Option<&str>,
    self_type: Option<&str>,
    line_number: Option<usize>,
    target_type: Option<&str>,
) -> Result<String, ProbeError> {
    // Step 1: Strip "rust-analyzer cargo " prefix
    let s = symbol.strip_prefix(SCIP_SYMBOL_PREFIX).ok_or_else(|| {
        ProbeError::invalid_symbol(
            format!("Symbol does not start with '{}'", SCIP_SYMBOL_PREFIX),
            symbol,
        )
    })?;

    // Step 2 & 3: Check if s ends with "method_name()."
    // The display_name may be enriched (e.g., "Mul::mul") but the SCIP symbol uses
    // "#" separators (e.g., "Mul#mul()."), so extract just the method name for matching.
    let method_name = display_name.rsplit("::").next().unwrap_or(display_name);
    let expected_suffix = format!("{}().", method_name);

    if !s.ends_with(&expected_suffix) {
        return Err(ProbeError::invalid_symbol(
            format!("Symbol does not end with '{}'", expected_suffix),
            symbol,
        ));
    }

    // Delete the last character of s
    let mut result = s[..s.len() - 1].to_string();

    // If we have a signature, try to add type info for disambiguation
    // This helps distinguish e.g., Mul<&Scalar>::mul vs Mul<&MontgomeryPoint>::mul
    if let Some(sig) = signature {
        if let Some(type_info) = extract_impl_type_info(sig) {
            // Check if this looks like a trait method (contains #)
            // e.g., "4.1.3 montgomery/Mul#mul()"
            if result.contains('#') {
                // Insert the type parameter before the #
                // "montgomery/Mul#mul()" -> "montgomery/Mul<Scalar>#mul()"
                if let Some(hash_pos) = result.rfind('#') {
                    result = format!(
                        "{}<{}>{}",
                        &result[..hash_pos],
                        type_info,
                        &result[hash_pos..]
                    );
                }
            }
        }
    }

    // If Self type is provided and the symbol is missing it (verus-analyzer inconsistency),
    // insert the Self type to make it consistent with rust-analyzer format.
    // e.g., "montgomery/Mul<Scalar>#mul()" -> "montgomery/MontgomeryPoint#Mul<Scalar>#mul()"
    if let Some(self_t) = self_type {
        if is_missing_self_type(&result) {
            // Find the position after "module/" to insert the Self type
            // Pattern: "version module/Trait#method()" or "version module/Trait<T>#method()"
            if let Some(slash_pos) = result.rfind('/') {
                // Insert Self type after the slash, before the trait
                let before_slash = &result[..=slash_pos];
                let after_slash = &result[slash_pos + 1..];
                result = format!("{}{}#{}", before_slash, self_t, after_slash);
            }
        }
    }

    // If target_type is provided, add it as a type parameter to the struct name.
    // This enriches the symbol to be more like rust-analyzer's format.
    // e.g., "window/NafLookupTable5#From<&EdwardsPoint>#from()"
    //    -> "window/NafLookupTable5<ProjectiveNielsPoint>#From<&EdwardsPoint>#from()"
    let mut target_type_applied = false;
    if let Some(target_t) = target_type {
        // Find the struct name (first # after the module path)
        // Pattern: "version module/StructName#Trait..." or "version module/StructName#Trait<T>#..."
        if let Some(first_hash) = result.find('#') {
            // Check if there's already a type parameter before this #
            let before_hash = &result[..first_hash];
            if !before_hash.ends_with('>') {
                // No existing type parameter, add one
                result = format!("{}<{}>{}", before_hash, target_t, &result[first_hash..]);
                target_type_applied = true;
            }
        }
    }

    // Line number is a fallback when target_type couldn't be applied (no # in symbol,
    // or existing type parameter prevents insertion). Also used directly when no
    // target_type was provided.
    if let Some(line) = line_number {
        if !target_type_applied {
            result = format!("{}@{}", result, line);
        }
    }

    // Convert to probe: URI format
    // "curve25519-dalek 4.1.3 montgomery/MontgomeryPoint#ct_eq()"
    // becomes "probe:curve25519-dalek/4.1.3/montgomery/MontgomeryPoint#ct_eq()"
    Ok(format!("{}{}", PROBE_URI_PREFIX, result.replace(' ', "/")))
}

/// Convert call graph to atoms with line numbers format.
///
/// This version uses only SCIP data, which only provides the function NAME location,
/// so lines_start and lines_end will be the same (or close for multi-line spans).
/// For accurate function body spans, use `convert_to_atoms_with_parsed_spans` instead.
pub fn convert_to_atoms_with_lines(
    call_graph: &HashMap<String, FunctionNode>,
    symbol_to_display_name: &HashMap<String, String>,
) -> Vec<AtomWithLines> {
    let empty_map = HashMap::new();
    convert_to_atoms_with_lines_internal(
        call_graph,
        symbol_to_display_name,
        None,
        false,
        &empty_map,
        false,
        "",
        "",
    )
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
    symbol_to_display_name: &HashMap<String, String>,
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
        symbol_to_display_name,
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
    symbol_to_display_name: &HashMap<String, String>,
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

            let base_code_name = symbol_to_code_name(
                &node.symbol,
                &node.display_name,
                Some(&node.signature_text),
                node.self_type.as_deref(),
            );

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
    let mut code_name_count: HashMap<String, usize> = HashMap::new();
    for data in &node_data {
        *code_name_count
            .entry(data.base_code_name.clone())
            .or_insert(0) += 1;
    }

    // For disambiguation, we need to find "discriminating" types that uniquely identify each impl
    // Group nodes by their base_code_name to find duplicates
    let mut code_name_to_nodes: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, data) in node_data.iter().enumerate() {
        code_name_to_nodes
            .entry(&data.base_code_name)
            .or_default()
            .push(idx);
    }

    // For each group of duplicates, find which types are discriminating
    // (appear in some but not all impls of the same base_code_name)
    let mut node_discriminating_type: HashMap<usize, Option<String>> = HashMap::new();
    for indices in code_name_to_nodes.values() {
        if indices.len() <= 1 {
            // Not a duplicate, no disambiguation needed
            for &idx in indices {
                node_discriminating_type.insert(idx, None);
            }
            continue;
        }

        // Collect all type contexts for this group
        let all_contexts: Vec<&Vec<String>> = indices
            .iter()
            .map(|&idx| &node_data[idx].node.definition_type_context)
            .collect();

        // Find types that appear in exactly one context (discriminating)
        let mut type_counts: HashMap<&str, usize> = HashMap::new();
        for ctx in &all_contexts {
            for t in *ctx {
                *type_counts.entry(t.as_str()).or_insert(0) += 1;
            }
        }

        // For each node in this group, find a discriminating type
        for &idx in indices {
            let ctx = &node_data[idx].node.definition_type_context;
            // Find a type that appears only in this node's context
            let discriminating = ctx
                .iter()
                .find(|t| type_counts.get(t.as_str()).copied().unwrap_or(0) == 1);
            node_discriminating_type.insert(idx, discriminating.cloned());
        }
    }

    // Compute final code_name for each node
    let final_code_names: Vec<String> = node_data
        .iter()
        .enumerate()
        .map(|(idx, data)| {
            let is_duplicate = code_name_count
                .get(&data.base_code_name)
                .copied()
                .unwrap_or(0)
                > 1;

            if is_duplicate {
                // Always pass line_number so it can serve as fallback when target_type
                // can't be applied (e.g., no # in symbol or existing type parameter).
                let line_fallback = if data.lines_start > 0 {
                    Some(data.lines_start)
                } else {
                    None
                };
                let result = if let Some(Some(target_type)) = node_discriminating_type.get(&idx) {
                    symbol_to_code_name_full(
                        &data.node.symbol,
                        &data.node.display_name,
                        Some(&data.node.signature_text),
                        data.node.self_type.as_deref(),
                        line_fallback,
                        Some(target_type),
                    )
                } else if data.lines_start > 0 {
                    symbol_to_code_name_full(
                        &data.node.symbol,
                        &data.node.display_name,
                        Some(&data.node.signature_text),
                        data.node.self_type.as_deref(),
                        Some(data.lines_start),
                        None,
                    )
                } else {
                    Ok(data.base_code_name.clone())
                };
                result.unwrap_or_else(|e| {
                    eprintln!("Warning: {}", e);
                    data.base_code_name.clone()
                })
            } else {
                data.base_code_name.clone()
            }
        })
        .collect();

    // === Phase 3: Build map from raw symbol → list of (code_name, type_context) ===
    // The type_context helps match call-site type hints to the correct implementation
    struct CodeNameWithContext {
        code_name: String,
        /// Types from definition site (nearby type references) for disambiguation
        type_context: Vec<String>,
    }

    let mut raw_symbol_to_code_names: HashMap<String, Vec<CodeNameWithContext>> = HashMap::new();
    for (data, final_name) in node_data.iter().zip(final_code_names.iter()) {
        // Use definition_type_context from FunctionNode (captured during build_call_graph)
        // This contains types that appeared near the definition, like "ProjectiveNielsPoint"
        let type_context = data.node.definition_type_context.clone();

        raw_symbol_to_code_names
            .entry(data.node.symbol.clone())
            .or_default()
            .push(CodeNameWithContext {
                code_name: final_name.clone(),
                type_context,
            });
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
                // Only compute location info if requested (for --with-locations flag)
                let (location, call_line_1based) = if with_locations {
                    let loc = classify_call_location(
                        callee.line,
                        data.requires_range,
                        data.ensures_range,
                    );
                    let line = (callee.line + 1) as usize;
                    (Some(loc), line)
                } else {
                    (None, 0)
                };

                // Check if this callee is a project function with known code_names
                if let Some(code_name_contexts) = raw_symbol_to_code_names.get(&callee.symbol) {
                    if code_name_contexts.len() == 1 {
                        // Only one implementation - use it directly
                        let dep_code_name = code_name_contexts[0].code_name.clone();
                        dependencies.insert(dep_code_name.clone());
                        if let Some(loc) = location.clone() {
                            dependencies_with_locations.push(DependencyWithLocation {
                                code_name: dep_code_name,
                                location: loc,
                                line: call_line_1based,
                            });
                        }
                    } else if !callee.type_hints.is_empty() {
                        // Multiple implementations - try to match using type hints
                        // First, find types in call-site hints that DON'T appear in ALL impl contexts
                        // (i.e., discriminating types like ProjectiveNielsPoint vs AffineNielsPoint)
                        let discriminating_hints: Vec<_> = callee
                            .type_hints
                            .iter()
                            .filter(|hint| {
                                // Count how many impls have this type in their context
                                let matching_count = code_name_contexts
                                    .iter()
                                    .filter(|ctx| ctx.type_context.iter().any(|t| t == *hint))
                                    .count();
                                // Keep hints that match some but not all impls
                                matching_count > 0 && matching_count < code_name_contexts.len()
                            })
                            .collect();

                        let matched: Vec<_> = if !discriminating_hints.is_empty() {
                            // Use discriminating hints to filter
                            code_name_contexts
                                .iter()
                                .filter(|ctx| {
                                    discriminating_hints
                                        .iter()
                                        .any(|hint| ctx.type_context.iter().any(|t| t == *hint))
                                })
                                .collect()
                        } else {
                            // Fallback: use all hints
                            code_name_contexts
                                .iter()
                                .filter(|ctx| {
                                    callee.type_hints.iter().any(|hint| {
                                        ctx.type_context
                                            .iter()
                                            .any(|t| t.contains(hint) || hint.contains(t))
                                    })
                                })
                                .collect()
                        };

                        if matched.len() == 1 {
                            // Found exactly one match - use it
                            let dep_code_name = matched[0].code_name.clone();
                            dependencies.insert(dep_code_name.clone());
                            if let Some(loc) = location.clone() {
                                dependencies_with_locations.push(DependencyWithLocation {
                                    code_name: dep_code_name,
                                    location: loc,
                                    line: call_line_1based,
                                });
                            }
                        } else {
                            // Still ambiguous - include all
                            for ctx in code_name_contexts {
                                dependencies.insert(ctx.code_name.clone());
                                if let Some(loc) = location.clone() {
                                    dependencies_with_locations.push(DependencyWithLocation {
                                        code_name: ctx.code_name.clone(),
                                        location: loc,
                                        line: call_line_1based,
                                    });
                                }
                            }
                        }
                    } else {
                        // No type hints - include all possible implementations
                        for ctx in code_name_contexts {
                            dependencies.insert(ctx.code_name.clone());
                            if let Some(loc) = location.clone() {
                                dependencies_with_locations.push(DependencyWithLocation {
                                    code_name: ctx.code_name.clone(),
                                    location: loc,
                                    line: call_line_1based,
                                });
                            }
                        }
                    }
                } else {
                    // External function - use the raw symbol conversion
                    let display_name = symbol_to_display_name
                        .get(&callee.symbol)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let dep_path = symbol_to_code_name(&callee.symbol, &display_name, None, None);
                    dependencies.insert(dep_path.clone());
                    if let Some(loc) = location {
                        dependencies_with_locations.push(DependencyWithLocation {
                            code_name: dep_path,
                            location: loc,
                            line: call_line_1based,
                        });
                    }
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
    fn test_enrich_impl_method() {
        // Trait impl: Type#Trait<Args>#method()
        let symbol =
            "rust-analyzer cargo curve25519-dalek 4.1.3 edwards/CompressedEdwardsY#ConstantTimeEq<&CompressedEdwardsY>#ct_eq().";
        assert_eq!(
            enrich_display_name(symbol, "ct_eq"),
            "CompressedEdwardsY::ct_eq"
        );
    }

    #[test]
    fn test_enrich_borrowed_self() {
        // Borrowed self: &Type#Type<Ret>#method()
        let symbol =
            "rust-analyzer cargo curve25519-dalek 4.1.3 edwards/&CompressedEdwardsY#CompressedEdwardsY<Option<EdwardsPoint>>#decompress().";
        assert_eq!(
            enrich_display_name(symbol, "decompress"),
            "CompressedEdwardsY::decompress"
        );
    }

    #[test]
    fn test_enrich_inherent_impl() {
        // Inherent impl: Type#method()
        let symbol = "rust-analyzer cargo curve25519-dalek 4.1.3 field/FieldElement51#square().";
        assert_eq!(
            enrich_display_name(symbol, "square"),
            "FieldElement51::square"
        );
    }

    #[test]
    fn test_enrich_free_function_unchanged() {
        // Free function: no '#', keep bare name
        let symbol =
            "rust-analyzer cargo curve25519-dalek 4.1.3 ristretto_specs/specs/spec_ristretto_decompress().";
        assert_eq!(
            enrich_display_name(symbol, "spec_ristretto_decompress"),
            "spec_ristretto_decompress"
        );
    }

    #[test]
    fn test_enrich_trait_impl_add() {
        // Trait impl: &EdwardsPoint#Add<&EdwardsPoint>#add()
        let symbol =
            "rust-analyzer cargo curve25519-dalek 4.1.3 edwards/&EdwardsPoint#Add<&EdwardsPoint>#add().";
        assert_eq!(enrich_display_name(symbol, "add"), "EdwardsPoint::add");
    }

    #[test]
    fn test_enrich_short_symbol_unchanged() {
        // Symbols with fewer than 5 space-separated parts are returned unchanged
        let symbol = "short symbol";
        assert_eq!(enrich_display_name(symbol, "something"), "something");
    }

    #[test]
    fn test_enrich_no_prefix_fallback() {
        // Symbol without the expected prefix still works by splitting on spaces
        let symbol = "other-tool cargo crate 1.0 module/Type#method().";
        assert_eq!(enrich_display_name(symbol, "method"), "Type::method");
    }

    // =========================================================================
    // extract_function_name_from_symbol tests
    // =========================================================================

    #[test]
    fn test_extract_function_name_method() {
        assert_eq!(
            extract_function_name_from_symbol(
                "rust-analyzer cargo x25519-dalek 2.0.1 x25519/StaticSecret#diffie_hellman()."
            ),
            "diffie_hellman"
        );
    }

    #[test]
    fn test_extract_function_name_free_function() {
        assert_eq!(
            extract_function_name_from_symbol("rust-analyzer cargo core 1.0.0 mem/swap()."),
            "swap"
        );
    }

    #[test]
    fn test_extract_function_name_trait_impl() {
        assert_eq!(
            extract_function_name_from_symbol(
                "rust-analyzer cargo curve25519-dalek 4.1.3 edwards/CompressedEdwardsY#ConstantTimeEq#ct_eq()."
            ),
            "ct_eq"
        );
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
    fn test_fallback_code_name_no_trailing_dot() {
        let symbol =
            "rust-analyzer cargo x25519-dalek 2.0.1 x25519/impl#[StaticSecret]diffie_hellman().";
        let code_name = symbol_to_code_name(symbol, "wrong_name_triggers_fallback", None, None);
        assert!(
            !code_name.ends_with('.'),
            "Fallback code_name should not end with '.': {}",
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
            "schema-version": "2.0",
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
    fn test_trait_impl_add() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/edwards/EdwardsPoint#Add<&EdwardsPoint>#add()"
        ));
    }

    #[test]
    fn test_trait_impl_mul() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/montgomery/MontgomeryPoint#Mul<&Scalar>#mul()"
        ));
    }

    #[test]
    fn test_trait_impl_from() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/window/NafLookupTable5<ProjectiveNielsPoint>#From<&EdwardsPoint>#from()"
        ));
    }

    #[test]
    fn test_inherent_impl_not_trait() {
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/montgomery/MontgomeryPoint#ct_eq()"
        ));
    }

    #[test]
    fn test_inherent_impl_two_hashes_not_trait() {
        // verus-analyzer encodes inherent impls as SelfType#SelfType<Ret>#method()
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/scalar/&Scalar#Scalar<Scalar>#reduce()"
        ));
    }

    #[test]
    fn test_inherent_impl_different_return_type_not_trait() {
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/scalar/&Scalar#Scalar<Choice>#is_canonical()"
        ));
    }

    #[test]
    fn test_inherent_impl_ref_self_not_trait() {
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/edwards/&EdwardsPoint#EdwardsPoint<EdwardsPoint>#double()"
        ));
    }

    #[test]
    fn test_inherent_impl_generic_self_not_trait() {
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/window/&LookupTable<AffineNielsPoint>#LookupTable<i8>#select()"
        ));
    }

    #[test]
    fn test_inherent_impl_mut_ref_not_trait() {
        assert!(!is_trait_impl_code_name(
            "probe:crate/1.0/scalar/&mut/Scalar52#Scalar52<u64>#conditional_add_l()"
        ));
    }

    #[test]
    fn test_free_function_not_trait() {
        assert!(!is_trait_impl_code_name("probe:crate/1.0/montgomery/mul()"));
    }

    #[test]
    fn test_trait_impl_with_line_suffix() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/edwards/EdwardsPoint#Add<&EdwardsPoint>#add()@123"
        ));
    }

    #[test]
    fn test_no_probe_prefix() {
        assert!(is_trait_impl_code_name(
            "crate/1.0/edwards/EdwardsPoint#Add#add()"
        ));
    }

    #[test]
    fn test_empty_string_not_trait() {
        assert!(!is_trait_impl_code_name(""));
    }

    #[test]
    fn test_trait_impl_display() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/DalekBits#Display<&Formatter<'_>>#fmt()"
        ));
    }

    #[test]
    fn test_trait_impl_clone() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/window/LookupTable#Clone#clone()"
        ));
    }

    #[test]
    fn test_trait_impl_index() {
        assert!(is_trait_impl_code_name(
            "probe:crate/1.0/scalar/Scalar52#Index<usize>#index()"
        ));
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
        let code_name = "probe-verus://mycrate/0.1.0/Counter#Add<Counter>#add()";
        assert_eq!(
            classify_public_api(false, code_name, "src/lib.rs", DeclKind::Exec, &map, true),
            Some(true)
        );
    }

    #[test]
    fn test_classify_public_api_trait_impl_in_private_module() {
        let mut map = HashMap::new();
        map.insert("src/internal.rs".to_string(), mi(false));
        let code_name = "probe-verus://mycrate/0.1.0/Counter#Add<Counter>#add()";
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
