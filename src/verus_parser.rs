//! Parser module using verus_syn to extract accurate function spans.
//!
//! SCIP only provides the location of function names, not their full body spans.
//! This module parses the actual source files to get accurate start/end line numbers.
//!
//! This module also provides functionality to find all functions in a project,
//! including support for Verus-specific constructs (spec, proof, exec functions).

use crate::DeclKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use verus_syn::spanned::Spanned;
use verus_syn::visit::Visit;
use verus_syn::{Attribute, FnMode, ImplItemFn, Item, ItemFn, ItemMacro, TraitItemFn, Visibility};
use walkdir::WalkDir;

fn default_true() -> bool {
    true
}

/// Remove comments from a single line of source code.
///
/// `in_block_comment` tracks whether we are inside a `/* ... */` block
/// across successive lines.  Returns the portion of the line that is
/// actual code (may be empty).
fn strip_comments(line: &str, in_block_comment: &mut bool) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if *in_block_comment {
            if i + 1 < len && chars[i] == '*' && chars[i + 1] == '/' {
                *in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
        } else if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            break;
        } else if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            *in_block_comment = true;
            i += 2;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Replace string literal contents with spaces so that keywords inside
/// strings (e.g. `"admit()"`) are not matched by text searches.
/// Handles `"..."` and raw strings `r#"..."#` at a best-effort level.
fn strip_string_literals(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '"' {
            result.push('"');
            i += 1;
            while i < len && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < len {
                    result.push(' ');
                    result.push(' ');
                    i += 2;
                } else {
                    result.push(' ');
                    i += 1;
                }
            }
            if i < len {
                result.push('"');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// Type alias for spec clause line ranges: (requires_range, ensures_range)
/// Each range is Option<(start_line, end_line)> using 1-based line numbers.
pub type SpecRanges = (Option<(usize, usize)>, Option<(usize, usize)>);

/// Function span information
#[derive(Debug, Clone)]
pub struct FunctionSpan {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    /// Declaration kind (spec, proof, exec)
    pub kind: DeclKind,
    /// Whether this function was found inside a `verus!{}` block
    pub is_verus: bool,
    /// Line range of requires clause (start, end), if present
    pub requires_range: Option<(usize, usize)>,
    /// Line range of ensures clause (start, end), if present
    pub ensures_range: Option<(usize, usize)>,
    /// Whether the function has a body (false for bodiless trait declarations)
    pub has_body: bool,
    /// Whether `#[verifier::external]` (direct or via `cfg_attr`) is present
    pub is_external: bool,
    /// Whether the function or an enclosing item has `#[cfg(...)]`
    pub is_cfg: bool,
}

/// Convert FnMode to DeclKind
fn convert_kind(mode: &FnMode) -> DeclKind {
    match mode {
        FnMode::Spec(_) | FnMode::SpecChecked(_) => DeclKind::Spec,
        FnMode::Proof(_) | FnMode::ProofAxiom(_) => DeclKind::Proof,
        FnMode::Exec(_) | FnMode::Default => DeclKind::Exec,
    }
}

/// Check whether `attrs` contains `#[verifier::<attr_name>]` using the AST
/// (e.g., `has_verifier_attr(attrs, "external_body")`).
fn has_verifier_attr(attrs: &[Attribute], attr_name: &str) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        let segments: Vec<_> = path.segments.iter().collect();
        segments.len() == 2 && segments[0].ident == "verifier" && segments[1].ident == attr_name
    })
}

/// Check whether `attrs` contains any `#[cfg(...)]` attribute.
///
/// Public wrapper for use from `lib.rs` module visibility map.
pub fn has_any_cfg_attr_pub(attrs: &[Attribute]) -> bool {
    has_any_cfg_attr(attrs)
}

fn has_any_cfg_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        let segments: Vec<_> = path.segments.iter().collect();
        segments.len() == 1 && segments[0].ident == "cfg"
    })
}

/// Check whether `attrs` contains `#[verifier::external]`, either directly
/// or wrapped in `#[cfg_attr(_, verifier::external)]`.
///
/// `verus_syn` preserves `cfg_attr` as-is (does not expand it), so we need
/// to inspect the token stream inside `cfg_attr(...)` for the inner attribute.
fn has_verifier_external(attrs: &[Attribute]) -> bool {
    if has_verifier_attr(attrs, "external") {
        return true;
    }
    attrs.iter().any(|attr| {
        let path = attr.path();
        let segments: Vec<_> = path.segments.iter().collect();
        if segments.len() != 1 || segments[0].ident != "cfg_attr" {
            return false;
        }
        // Parse inside: cfg_attr(PREDICATE, ATTR)
        // The token stream contains: predicate , verifier :: external
        let tokens = match &attr.meta {
            verus_syn::Meta::List(list) => &list.tokens,
            _ => return false,
        };
        let s = tokens.to_string();
        s.contains("verifier") && s.contains("external")
    })
}

/// A collected function call from a spec clause.
#[derive(Debug, Clone)]
struct CollectedCall {
    /// Last path segment (e.g., "is_canonical" from "crate::spec::is_canonical")
    short_name: String,
    /// Full qualified path (e.g., "crate::spec::is_canonical"), if available.
    /// Method calls only have the short name.
    full_path: Option<String>,
    /// Whether this is a method call (ExprMethodCall) vs a function call (ExprCall)
    is_method: bool,
}

/// Visitor that walks verus_syn Expr nodes and collects function call names.
///
/// Used to extract called function names from requires/ensures clauses
/// for taxonomy classification.
struct CallNameCollector {
    calls: Vec<CollectedCall>,
}

impl CallNameCollector {
    fn new() -> Self {
        Self { calls: Vec::new() }
    }

    /// Get all call names (short names, for backward compatibility).
    fn names(&self) -> Vec<String> {
        self.calls.iter().map(|c| c.short_name.clone()).collect()
    }

    /// Get full paths where available, falling back to short name.
    fn full_paths(&self) -> Vec<String> {
        self.calls
            .iter()
            .map(|c| c.full_path.clone().unwrap_or_else(|| c.short_name.clone()))
            .collect()
    }

    /// Get only function calls (ExprCall, not method calls).
    fn fn_call_names(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter(|c| !c.is_method)
            .map(|c| c.short_name.clone())
            .collect()
    }

    /// Get only method call names (ExprMethodCall).
    fn method_call_names(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter(|c| c.is_method)
            .map(|c| c.short_name.clone())
            .collect()
    }
}

impl<'ast> Visit<'ast> for CallNameCollector {
    fn visit_expr_call(&mut self, node: &'ast verus_syn::ExprCall) {
        // Extract function name from Expr::Path (e.g., is_canonical_scalar52(...))
        if let verus_syn::Expr::Path(path) = &*node.func {
            if let Some(last) = path.path.segments.last() {
                let short_name = last.ident.to_string();
                // Build full path from all segments
                let full_path = if path.path.segments.len() > 1 {
                    Some(
                        path.path
                            .segments
                            .iter()
                            .map(|seg| seg.ident.to_string())
                            .collect::<Vec<_>>()
                            .join("::"),
                    )
                } else {
                    None
                };
                self.calls.push(CollectedCall {
                    short_name,
                    full_path,
                    is_method: false,
                });
            }
        }
        // Continue walking sub-expressions (nested calls in arguments)
        verus_syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast verus_syn::ExprMethodCall) {
        self.calls.push(CollectedCall {
            short_name: node.method.to_string(),
            full_path: None, // Method calls don't have a path, only the method name
            is_method: true,
        });
        verus_syn::visit::visit_expr_method_call(self, node);
    }
}

/// Visitor that collects function spans from an AST
struct FunctionSpanVisitor {
    functions: Vec<FunctionSpan>,
    /// Depth counter: >0 when visiting inside a `verus!{}` macro
    inside_verus: usize,
    /// Depth counter: >0 when visiting inside a `#[cfg(...)]` impl block
    inside_cfg_impl: usize,
    /// Depth counter: >0 when visiting inside a `#[cfg(...)]` mod block
    inside_cfg_mod: usize,
    /// Depth counter: >0 when visiting inside a `cfg_if!` branch
    inside_cfg_if: usize,
}

impl FunctionSpanVisitor {
    fn new() -> Self {
        Self {
            functions: Vec::new(),
            inside_verus: 0,
            inside_cfg_impl: 0,
            inside_cfg_mod: 0,
            inside_cfg_if: 0,
        }
    }

    fn is_inside_cfg(&self) -> bool {
        self.inside_cfg_impl > 0 || self.inside_cfg_mod > 0 || self.inside_cfg_if > 0
    }

    /// Extract requires/ensures line ranges from a signature's spec
    fn extract_spec_ranges(sig: &verus_syn::Signature) -> SpecRanges {
        let requires_range = sig.spec.requires.as_ref().map(|req| {
            let span = req.span();
            (span.start().line, span.end().line)
        });

        let ensures_range = sig.spec.ensures.as_ref().map(|ens| {
            let span = ens.span();
            (span.start().line, span.end().line)
        });

        (requires_range, ensures_range)
    }
}

impl<'ast> Visit<'ast> for FunctionSpanVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        let span = node.span();
        let start_line = span.start().line;
        let end_line = span.end().line;
        let kind = convert_kind(&node.sig.mode);
        let (requires_range, ensures_range) = Self::extract_spec_ranges(&node.sig);

        self.functions.push(FunctionSpan {
            name,
            start_line,
            end_line,
            kind,
            is_verus: self.inside_verus > 0,
            requires_range,
            ensures_range,
            has_body: true,
            is_external: has_verifier_external(&node.attrs),
            is_cfg: has_any_cfg_attr(&node.attrs) || self.is_inside_cfg(),
        });

        verus_syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let name = node.sig.ident.to_string();
        let span = node.span();
        let start_line = span.start().line;
        let end_line = span.end().line;
        let kind = convert_kind(&node.sig.mode);
        let (requires_range, ensures_range) = Self::extract_spec_ranges(&node.sig);

        self.functions.push(FunctionSpan {
            name,
            start_line,
            end_line,
            kind,
            is_verus: self.inside_verus > 0,
            requires_range,
            ensures_range,
            has_body: true,
            is_external: has_verifier_external(&node.attrs),
            is_cfg: has_any_cfg_attr(&node.attrs) || self.is_inside_cfg(),
        });

        verus_syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        let name = node.sig.ident.to_string();
        let span = node.span();
        let start_line = span.start().line;
        let end_line = span.end().line;
        let kind = convert_kind(&node.sig.mode);
        let (requires_range, ensures_range) = Self::extract_spec_ranges(&node.sig);

        self.functions.push(FunctionSpan {
            name,
            start_line,
            end_line,
            kind,
            is_verus: self.inside_verus > 0,
            requires_range,
            ensures_range,
            has_body: node.default.is_some(),
            is_external: has_verifier_external(&node.attrs),
            is_cfg: has_any_cfg_attr(&node.attrs) || self.is_inside_cfg(),
        });

        verus_syn::visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast verus_syn::ItemImpl) {
        let cfg_gated = has_any_cfg_attr(&node.attrs);
        if cfg_gated {
            self.inside_cfg_impl += 1;
        }
        verus_syn::visit::visit_item_impl(self, node);
        if cfg_gated {
            self.inside_cfg_impl -= 1;
        }
    }

    fn visit_item_trait(&mut self, node: &'ast verus_syn::ItemTrait) {
        verus_syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast verus_syn::ItemMod) {
        let cfg_gated = has_any_cfg_attr(&node.attrs);
        if cfg_gated {
            self.inside_cfg_mod += 1;
        }
        verus_syn::visit::visit_item_mod(self, node);
        if cfg_gated {
            self.inside_cfg_mod -= 1;
        }
    }

    fn visit_item_macro(&mut self, node: &'ast ItemMacro) {
        if let Some(ident) = &node.mac.path.get_ident() {
            if *ident == "verus" {
                if let Ok(items) = verus_syn::parse2::<VerusMacroBody>(node.mac.tokens.clone()) {
                    self.inside_verus += 1;
                    for item in items.items {
                        self.visit_item(&item);
                    }
                    self.inside_verus -= 1;
                }
            } else if *ident == "cfg_if" {
                if let Ok(branches) = verus_syn::parse2::<CfgIfMacroBody>(node.mac.tokens.clone()) {
                    self.inside_cfg_if += 1;
                    for items in branches.all_items {
                        for item in items {
                            self.visit_item(&item);
                        }
                    }
                    self.inside_cfg_if -= 1;
                }
            }
        }
        verus_syn::visit::visit_item_macro(self, node);
    }
}

/// Helper struct to parse verus! macro body as a list of items
struct VerusMacroBody {
    items: Vec<Item>,
}

impl verus_syn::parse::Parse for VerusMacroBody {
    fn parse(input: verus_syn::parse::ParseStream) -> verus_syn::Result<Self> {
        let mut items = Vec::new();
        while !input.is_empty() {
            items.push(input.parse()?);
        }
        Ok(VerusMacroBody { items })
    }
}

/// Helper struct to parse cfg_if! macro body
/// The syntax is: if #[cfg(...)] { items } else if #[cfg(...)] { items } else { items }
struct CfgIfMacroBody {
    all_items: Vec<Vec<Item>>,
}

impl verus_syn::parse::Parse for CfgIfMacroBody {
    fn parse(input: verus_syn::parse::ParseStream) -> verus_syn::Result<Self> {
        use verus_syn::Token;

        let mut all_items = Vec::new();

        // Parse: if #[cfg(...)] { items }
        if input.peek(Token![if]) {
            input.parse::<Token![if]>()?;

            // Skip the #[cfg(...)] attribute
            // In macro token streams, the tokens are:
            //   # followed by a Group{delimiter: Bracket} containing the attribute content
            // So we parse # and then a Group, not using bracketed! which expects [ ] tokens
            input.parse::<Token![#]>()?;
            let _attr_group: proc_macro2::Group = input.parse()?;

            // Parse the block { items }
            let content;
            verus_syn::braced!(content in input);
            let mut items = Vec::new();
            while !content.is_empty() {
                items.push(content.parse()?);
            }
            all_items.push(items);
        }

        // Parse any else if or else branches
        while input.peek(Token![else]) {
            input.parse::<Token![else]>()?;

            if input.peek(Token![if]) {
                // else if #[cfg(...)] { items }
                input.parse::<Token![if]>()?;
                input.parse::<Token![#]>()?;
                let _attr_group: proc_macro2::Group = input.parse()?;

                let content;
                verus_syn::braced!(content in input);
                let mut items = Vec::new();
                while !content.is_empty() {
                    items.push(content.parse()?);
                }
                all_items.push(items);
            } else {
                // else { items }
                let content;
                verus_syn::braced!(content in input);
                let mut items = Vec::new();
                while !content.is_empty() {
                    items.push(content.parse()?);
                }
                all_items.push(items);
                break; // else is always last
            }
        }

        Ok(CfgIfMacroBody { all_items })
    }
}

/// Parse a single source file and extract all function spans.
///
/// Returns a vector of (function_name, start_line, end_line) tuples.
pub fn parse_file_for_spans(file_path: &Path) -> Result<Vec<FunctionSpan>, String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file {}: {}", file_path.display(), e))?;

    let syntax_tree = verus_syn::parse_file(&content)
        .map_err(|e| format!("Failed to parse file {}: {}", file_path.display(), e))?;

    let mut visitor = FunctionSpanVisitor::new();
    visitor.visit_file(&syntax_tree);

    Ok(visitor.functions)
}

/// Span and declaration kind information for a function
#[derive(Debug, Clone)]
pub struct SpanAndMode {
    pub end_line: usize,
    pub kind: DeclKind,
    /// Whether this function was found inside a `verus!{}` block
    pub is_verus: bool,
    /// Line range of requires clause (start, end), if present
    pub requires_range: Option<(usize, usize)>,
    /// Line range of ensures clause (start, end), if present
    pub ensures_range: Option<(usize, usize)>,
    /// Whether the function has a body (false for bodiless trait declarations)
    pub has_body: bool,
    /// Whether `#[verifier::external]` (direct or via `cfg_attr`) is present
    pub is_external: bool,
    /// Whether the function or an enclosing item has `#[cfg(...)]`
    pub is_cfg: bool,
}

/// Parse all source files in a project and build a lookup map.
///
/// Returns a map from (relative_path, function_name, definition_line) -> SpanAndMode.
/// We use definition_line (from SCIP) as part of the key to handle multiple
/// functions with the same name in the same file (e.g., different impl blocks).
pub fn build_function_span_map(
    project_root: &Path,
    relative_paths: &[String],
) -> HashMap<(String, String, usize), SpanAndMode> {
    let mut span_map = HashMap::new();

    for rel_path in relative_paths {
        let full_path = project_root.join(rel_path);
        if !full_path.exists() {
            continue;
        }

        if let Ok(functions) = parse_file_for_spans(&full_path) {
            for func in functions {
                // Key: (relative_path, function_name, start_line)
                // Value: SpanAndMode (end_line + mode + spec ranges)
                let key = (rel_path.clone(), func.name.clone(), func.start_line);
                span_map.insert(
                    key,
                    SpanAndMode {
                        end_line: func.end_line,
                        kind: func.kind,
                        is_verus: func.is_verus,
                        requires_range: func.requires_range,
                        ensures_range: func.ensures_range,
                        has_body: func.has_body,
                        is_external: func.is_external,
                        is_cfg: func.is_cfg,
                    },
                );
            }
        }
    }

    span_map
}

/// Extract the bare function name from a possibly enriched display name.
///
/// SCIP display names are enriched with type info (e.g., "EdwardsPoint::eq"),
/// but verus_syn only stores the bare function name ("eq"). This strips the
/// `Type::` prefix to enable matching.
fn bare_function_name(function_name: &str) -> &str {
    function_name.rsplit("::").next().unwrap_or(function_name)
}

/// Look up a `SpanAndMode` entry by (path, name, line).
///
/// Tries an exact key match first, then falls back to a containment match
/// where the SCIP-reported start line falls within the parsed span.
pub fn get_span_and_mode<'a>(
    span_map: &'a HashMap<(String, String, usize), SpanAndMode>,
    relative_path: &str,
    function_name: &str,
    start_line: usize,
) -> Option<&'a SpanAndMode> {
    let bare_name = bare_function_name(function_name);

    let key = (relative_path.to_string(), bare_name.to_string(), start_line);
    if let Some(sam) = span_map.get(&key) {
        return Some(sam);
    }

    for ((path, name, parsed_start), sam) in span_map.iter() {
        if path == relative_path
            && name == bare_name
            && start_line >= *parsed_start
            && start_line <= sam.end_line
        {
            return Some(sam);
        }
    }

    None
}

/// Get the end line for a function given its path, name, and start line.
pub fn get_function_end_line(
    span_map: &HashMap<(String, String, usize), SpanAndMode>,
    relative_path: &str,
    function_name: &str,
    start_line: usize,
) -> Option<usize> {
    get_span_and_mode(span_map, relative_path, function_name, start_line).map(|sam| sam.end_line)
}

/// Get the declaration kind (exec, proof, spec) given its path, name, and start line.
///
/// Returns `(kind, is_verus)` -- `is_verus` is true when the function was
/// inside a `verus!{}` block.
pub fn get_function_kind(
    span_map: &HashMap<(String, String, usize), SpanAndMode>,
    relative_path: &str,
    function_name: &str,
    start_line: usize,
) -> Option<(DeclKind, bool)> {
    get_span_and_mode(span_map, relative_path, function_name, start_line)
        .map(|sam| (sam.kind, sam.is_verus))
}

/// Get the spec ranges (requires/ensures) for a function.
///
/// Returns (requires_range, ensures_range) where each is Option<(start_line, end_line)>.
pub fn get_function_spec_ranges(
    span_map: &HashMap<(String, String, usize), SpanAndMode>,
    relative_path: &str,
    function_name: &str,
    start_line: usize,
) -> SpecRanges {
    get_span_and_mode(span_map, relative_path, function_name, start_line)
        .map(|sam| (sam.requires_range, sam.ensures_range))
        .unwrap_or((None, None))
}

/// Line range for spec text
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecText {
    #[serde(rename = "lines-start")]
    pub lines_start: usize,
    #[serde(rename = "lines-end")]
    pub lines_end: usize,
}

/// Detailed function information for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionInfo {
    #[serde(skip_serializing)]
    pub name: String,
    #[serde(rename = "code-path", skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(rename = "spec-text")]
    pub spec_text: SpecText,
    /// Declaration kind (spec, proof, exec)
    pub kind: DeclKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>, // "impl", "trait", or "standalone"
    /// Whether the function has a specification (requires or ensures clause)
    #[serde(default)]
    pub specified: bool,
    /// Whether the function has requires clause (precondition)
    #[serde(default)]
    pub has_requires: bool,
    /// Whether the function has ensures clause (postcondition)
    #[serde(default)]
    pub has_ensures: bool,
    /// Whether the function has a decreases clause (termination proof)
    #[serde(default)]
    pub has_decreases: bool,
    /// Whether the function body contains assume() or admit() (trusted assumptions)
    #[serde(default)]
    pub has_trusted_assumption: bool,
    /// Whether the function body contains admit() — an axiom whose correctness
    /// is assumed without proof (the Verus analogue of Lean's `sorry`/`axiom`).
    #[serde(default)]
    pub contains_admit: bool,
    /// Whether the function has #[verifier::external_body] attribute
    #[serde(default)]
    pub is_external_body: bool,
    /// Whether the function has #[verifier::external] (direct or via cfg_attr)
    #[serde(default)]
    pub is_external: bool,
    /// Whether the function has a body (false for bodiless trait declarations)
    #[serde(default = "default_true")]
    pub has_body: bool,
    /// Whether the function or an enclosing item has #[cfg(...)]
    #[serde(default)]
    pub is_cfg: bool,
    /// Whether the function has #[verifier::exec_allows_no_decreases_clause] attribute
    #[serde(default)]
    pub has_no_decreases_attr: bool,
    /// Raw text of the requires clause (precondition), if present and requested
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_text: Option<String>,
    /// Raw text of the ensures clause (postcondition), if present and requested
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ensures_text: Option<String>,
    /// Function names called in the ensures clause (extracted from AST, short names)
    #[serde(
        rename = "ensures-calls",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ensures_calls: Vec<String>,
    /// Function names called in the requires clause (extracted from AST, short names)
    #[serde(
        rename = "requires-calls",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub requires_calls: Vec<String>,
    /// Full qualified paths of function calls in ensures (e.g., "crate::spec::is_canonical")
    #[serde(
        rename = "ensures-calls-full",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ensures_calls_full: Vec<String>,
    /// Full qualified paths of function calls in requires
    #[serde(
        rename = "requires-calls-full",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub requires_calls_full: Vec<String>,
    /// Function (non-method) call names in ensures clause
    #[serde(
        rename = "ensures-fn-calls",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ensures_fn_calls: Vec<String>,
    /// Method call names in ensures clause
    #[serde(
        rename = "ensures-method-calls",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub ensures_method_calls: Vec<String>,
    /// Function (non-method) call names in requires clause
    #[serde(
        rename = "requires-fn-calls",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub requires_fn_calls: Vec<String>,
    /// Method call names in requires clause
    #[serde(
        rename = "requires-method-calls",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub requires_method_calls: Vec<String>,

    // === Fields for specs-data generation ===
    /// Display name including impl type (e.g., "FieldElement51::mul" instead of just "mul")
    #[serde(
        rename = "display-name",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub display_name: Option<String>,
    /// The impl block type name (e.g., "FieldElement51"), if this is a method
    #[serde(rename = "impl-type", skip_serializing_if = "Option::is_none", default)]
    pub impl_type: Option<String>,
    /// Doc comment text extracted from /// comments above the function
    #[serde(
        rename = "doc-comment",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub doc_comment: Option<String>,
    /// The function signature text (everything before the opening brace)
    #[serde(
        rename = "signature-text",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub signature_text: Option<String>,
    /// Full function body text (for spec functions; includes signature)
    #[serde(rename = "body-text", skip_serializing_if = "Option::is_none", default)]
    pub body_text: Option<String>,
    /// Module path derived from file path (e.g., "specs::field_specs")
    #[serde(
        rename = "module-path",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub module_path: Option<String>,
    /// Line of the fn keyword / function name (from sig.ident), matching SCIP's
    /// definition line. Unlike spec_text.lines_start (which includes preceding
    /// attributes and doc comments), this points to the actual fn signature.
    #[serde(skip)]
    pub fn_line: usize,
}

impl FunctionInfo {
    /// Whether this function has a complete proof (spec is verified, not trusted).
    ///
    /// A function is considered proved when it has a specification (requires/ensures)
    /// and does **not** rely on any escape hatches:
    /// - no `assume()` / `admit()` calls (`has_trusted_assumption`)
    /// - no `#[verifier::external_body]` (`is_external_body`)
    /// - no `#[verifier::exec_allows_no_decreases_clause]` (`has_no_decreases_attr`)
    pub fn is_proved(&self) -> bool {
        let has_spec = self.has_requires || self.has_ensures;
        has_spec
            && !self.has_trusted_assumption
            && !self.is_external_body
            && !self.has_no_decreases_attr
    }
}

/// Metadata for a Verus `assume_specification[path]` declaration.
///
/// These are axioms: the project declares a spec for an external function
/// without providing a proof. They form part of the trust base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssumeSpecInfo {
    /// The last meaningful path segments for matching (e.g. `["Choice", "from"]`
    /// or `["ConditionallySelectable", "conditional_swap"]`).
    #[serde(rename = "path-segments")]
    pub path_segments: Vec<String>,
    /// Human-readable Verus path (e.g. `<u64 as ConditionallySelectable>::conditional_swap`).
    #[serde(rename = "path-display")]
    pub path_display: String,
    /// Source file (project-relative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line number of the declaration.
    pub line: usize,
    #[serde(default)]
    pub has_requires: bool,
    #[serde(default)]
    pub has_ensures: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ensures_text: Option<String>,
}

/// Output format for function listing
#[derive(Debug, Serialize, Deserialize)]
pub struct ParsedOutput {
    pub functions: Vec<FunctionInfo>,
    pub functions_by_file: HashMap<String, Vec<FunctionInfo>>,
    #[serde(
        rename = "assume-specifications",
        skip_serializing_if = "Vec::is_empty",
        default
    )]
    pub assume_specifications: Vec<AssumeSpecInfo>,
    pub summary: ParseSummary,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ParseSummary {
    pub total_functions: usize,
    pub total_files: usize,
}

/// Visitor that collects detailed function information
struct FunctionInfoVisitor {
    functions: Vec<FunctionInfo>,
    assume_specifications: Vec<AssumeSpecInfo>,
    file_path: Option<String>,
    file_content: Option<String>,
    include_verus_constructs: bool,
    include_methods: bool,
    show_visibility: bool,
    show_kind: bool,
    include_spec_text: bool,
    /// Enable extraction of doc comments, signatures, bodies, display names, etc.
    include_extended_info: bool,
    /// Current impl block type name (set while visiting an impl block)
    current_impl_type: Option<String>,
    /// Whether we are currently inside an `impl Trait for Type` block.
    /// Trait impl methods are inherently public even without an explicit `pub` keyword.
    in_trait_impl: bool,
    /// Depth counter: >0 when visiting inside a `#[cfg(...)]` impl block
    inside_cfg_impl: usize,
    /// Depth counter: >0 when visiting inside a `#[cfg(...)]` mod block
    inside_cfg_mod: usize,
    /// Depth counter: >0 when visiting inside a `cfg_if!` branch
    inside_cfg_if: usize,
}

impl FunctionInfoVisitor {
    fn new(
        file_path: Option<String>,
        file_content: Option<String>,
        include_verus_constructs: bool,
        include_methods: bool,
        show_visibility: bool,
        show_kind: bool,
        include_spec_text: bool,
    ) -> Self {
        Self {
            functions: Vec::new(),
            assume_specifications: Vec::new(),
            file_path,
            file_content,
            include_verus_constructs,
            include_methods,
            show_visibility,
            show_kind,
            include_spec_text,
            include_extended_info: false,
            current_impl_type: None,
            in_trait_impl: false,
            inside_cfg_impl: 0,
            inside_cfg_mod: 0,
            inside_cfg_if: 0,
        }
    }

    /// Extract raw text from source content given a span (line range).
    /// Returns the text from start_line to end_line (inclusive, 1-indexed).
    fn extract_text_from_span(&self, start_line: usize, end_line: usize) -> Option<String> {
        let content = self.file_content.as_ref()?;
        let lines: Vec<&str> = content.lines().collect();

        // Convert to 0-indexed
        let start_idx = start_line.saturating_sub(1);
        let end_idx = end_line.min(lines.len());

        if start_idx >= lines.len() || start_idx >= end_idx {
            return None;
        }

        let text = lines[start_idx..end_idx].join("\n");
        Some(text.trim().to_string())
    }

    /// Extract doc comment from /// lines at the start of a function span.
    ///
    /// verus_syn includes doc comments (which are #[doc] attributes) in the function span,
    /// so the span start line is the first /// line. We scan forward from start_line
    /// collecting /// lines until we hit a non-doc-comment line.
    fn extract_doc_comment(&self, start_line: usize) -> Option<String> {
        if !self.include_extended_info {
            return None;
        }
        let content = self.file_content.as_ref()?;
        let lines: Vec<&str> = content.lines().collect();
        let start_idx = start_line.saturating_sub(1); // convert to 0-indexed

        let mut doc_lines: Vec<&str> = Vec::new();
        for line in &lines[start_idx..] {
            let stripped = line.trim();
            if stripped.starts_with("///") {
                let text = stripped.strip_prefix("///").unwrap_or("");
                let text = text.strip_prefix(' ').unwrap_or(text);
                doc_lines.push(text);
            } else {
                break;
            }
        }

        if doc_lines.is_empty() {
            return None;
        }
        Some(doc_lines.join("\n"))
    }

    /// Extract the function signature text from source (everything before the opening brace).
    ///
    /// Skips doc comments (`///`) and attribute lines (`#[`) at the start of the span,
    /// then collects from the `fn` keyword line until the body-opening `{`.
    fn extract_signature_text(&self, start_line: usize, end_line: usize) -> Option<String> {
        if !self.include_extended_info {
            return None;
        }
        let content = self.file_content.as_ref()?;
        let lines: Vec<&str> = content.lines().collect();
        let start_idx = start_line.saturating_sub(1);
        let end_idx = end_line.min(lines.len());

        if start_idx >= lines.len() {
            return None;
        }

        // Phase 1: skip doc comments, attributes, and block comments to find the fn line.
        let mut sig_start = start_idx;
        let mut in_block_comment = false;
        for line in &lines[start_idx..end_idx] {
            let trimmed = line.trim();
            if in_block_comment {
                sig_start += 1;
                if trimmed.contains("*/") {
                    in_block_comment = false;
                }
                continue;
            }
            if trimmed.starts_with("/*") {
                sig_start += 1;
                if !trimmed.contains("*/") {
                    in_block_comment = true;
                }
                continue;
            }
            if trimmed.starts_with("///") || trimmed.starts_with("#[") {
                sig_start += 1;
            } else {
                break;
            }
        }

        // Phase 2: collect from the fn declaration until the body-opening `{`,
        // preserving indentation relative to the fn line.
        let base_indent = lines
            .get(sig_start)
            .map_or(0, |l| l.len() - l.trim_start().len());
        let mut sig_lines = Vec::new();
        for line in &lines[sig_start..end_idx] {
            if let Some(brace_pos) = line.find('{') {
                let before = line[..brace_pos].trim_end();
                if !before.is_empty() {
                    let stripped =
                        if before.len() > base_indent && before[..base_indent].trim().is_empty() {
                            &before[base_indent..]
                        } else {
                            before.trim_start()
                        };
                    sig_lines.push(stripped);
                }
                break;
            }
            let stripped = if line.len() > base_indent && line[..base_indent].trim().is_empty() {
                line[base_indent..].trim_end()
            } else {
                line.trim()
            };
            sig_lines.push(stripped);
        }

        if sig_lines.is_empty() {
            return None;
        }
        Some(sig_lines.join("\n"))
    }

    /// Extract full function body text (signature + body) from source.
    fn extract_body_text(&self, start_line: usize, end_line: usize) -> Option<String> {
        if !self.include_extended_info {
            return None;
        }
        self.extract_text_from_span(start_line, end_line)
    }

    /// Extract spec text (requires or ensures) from a signature spec clause.
    fn extract_spec_text<T: Spanned>(&self, spec_clause: Option<&T>) -> Option<String> {
        if !self.include_spec_text {
            return None;
        }
        let clause = spec_clause?;
        let span = clause.span();
        self.extract_text_from_span(span.start().line, span.end().line)
    }

    /// Check whether any of `patterns` appear in the function body (between
    /// start and end lines), after stripping comments and string literals.
    fn body_contains_any(&self, start_line: usize, end_line: usize, patterns: &[&str]) -> bool {
        if let Some(content) = &self.file_content {
            let lines: Vec<&str> = content.lines().collect();
            let start_idx = start_line.saturating_sub(1);
            let end_idx = end_line.min(lines.len());

            if start_idx >= end_idx {
                return false;
            }

            let mut in_block_comment = false;
            for line in &lines[start_idx..end_idx] {
                let code = strip_comments(line, &mut in_block_comment);
                let code = strip_string_literals(&code);
                if patterns.iter().any(|p| code.contains(p)) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if the function body contains assume() or admit().
    fn has_trusted_assumption(&self, start_line: usize, end_line: usize) -> bool {
        self.body_contains_any(start_line, end_line, &["assume(", "admit("])
    }

    /// Check if the function body contains admit() specifically (axiom —
    /// correctness assumed without proof).
    fn contains_admit(&self, start_line: usize, end_line: usize) -> bool {
        self.body_contains_any(start_line, end_line, &["admit("])
    }

    fn extract_function_kind(&self, sig: &verus_syn::Signature) -> String {
        let mode_str = match sig.mode {
            FnMode::Spec(_) => "spec",
            FnMode::SpecChecked(_) => "spec(checked)",
            FnMode::Proof(_) => "proof",
            FnMode::ProofAxiom(_) => "proof(axiom)",
            FnMode::Exec(_) => "exec",
            FnMode::Default => "",
        };

        if sig.constness.is_some() {
            if mode_str.is_empty() {
                "const fn".to_string()
            } else {
                format!("{} const fn", mode_str)
            }
        } else if !mode_str.is_empty() {
            format!("{} fn", mode_str)
        } else {
            "fn".to_string()
        }
    }

    fn extract_visibility(&self, vis: &Visibility) -> String {
        match vis {
            Visibility::Public(_) => "pub".to_string(),
            Visibility::Restricted(r) => {
                if r.path.segments.len() == 1 {
                    let seg = &r.path.segments[0];
                    format!("pub({})", seg.ident)
                } else {
                    "pub(restricted)".to_string()
                }
            }
            Visibility::Inherited => "private".to_string(),
        }
    }

    fn should_include_function(&self, sig: &verus_syn::Signature) -> bool {
        if self.include_verus_constructs {
            // Include all functions including spec fn
            true
        } else {
            // Exclude only spec fn (no body to verify)
            // Include: fn, proof fn, exec fn (these have bodies that get verified)
            !matches!(sig.mode, FnMode::Spec(_) | FnMode::SpecChecked(_))
        }
    }

    fn is_inside_cfg(&self) -> bool {
        self.inside_cfg_impl > 0 || self.inside_cfg_mod > 0 || self.inside_cfg_if > 0
    }

    #[allow(clippy::too_many_arguments)]
    fn add_function(
        &mut self,
        name: String,
        span: proc_macro2::Span,
        sig: &verus_syn::Signature,
        vis: &Visibility,
        attrs: &[Attribute],
        context: Option<String>,
        has_body: bool,
    ) {
        if !self.should_include_function(sig) {
            return;
        }

        let kind_display = if self.show_kind {
            Some(self.extract_function_kind(sig))
        } else {
            None
        };

        let visibility = if self.show_visibility {
            Some(self.extract_visibility(vis))
        } else {
            None
        };

        let start_line = span.start().line;
        let end_line = span.end().line;
        let fn_line = sig.ident.span().start().line;

        // Extract declaration kind
        let kind = convert_kind(&sig.mode);

        // Extract spec information
        let has_requires = sig.spec.requires.is_some();
        let has_ensures = sig.spec.ensures.is_some();
        let has_decreases = sig.spec.decreases.is_some();
        let has_trusted_assumption = self.has_trusted_assumption(start_line, end_line);
        let contains_admit = self.contains_admit(start_line, end_line);
        let is_external_body = has_verifier_attr(attrs, "external_body");
        let is_external = has_verifier_external(attrs);
        let is_cfg = has_any_cfg_attr(attrs) || self.is_inside_cfg();
        let has_no_decreases_attr = has_verifier_attr(attrs, "exec_allows_no_decreases_clause");

        // Extract spec text if requested
        let requires_text = self.extract_spec_text(sig.spec.requires.as_ref());
        let ensures_text = self.extract_spec_text(sig.spec.ensures.as_ref());

        // Extract called function names from ensures/requires clauses (AST walk)
        let ensures_collector = sig.spec.ensures.as_ref().map(|ens| {
            let mut collector = CallNameCollector::new();
            for expr in ens.exprs.exprs.iter() {
                collector.visit_expr(expr);
            }
            collector
        });

        let requires_collector = sig.spec.requires.as_ref().map(|req| {
            let mut collector = CallNameCollector::new();
            for expr in req.exprs.exprs.iter() {
                collector.visit_expr(expr);
            }
            collector
        });

        let ensures_calls = ensures_collector
            .as_ref()
            .map(|c| c.names())
            .unwrap_or_default();
        let ensures_calls_full = ensures_collector
            .as_ref()
            .map(|c| c.full_paths())
            .unwrap_or_default();
        let ensures_fn_calls = ensures_collector
            .as_ref()
            .map(|c| c.fn_call_names())
            .unwrap_or_default();
        let ensures_method_calls = ensures_collector
            .as_ref()
            .map(|c| c.method_call_names())
            .unwrap_or_default();

        let requires_calls = requires_collector
            .as_ref()
            .map(|c| c.names())
            .unwrap_or_default();
        let requires_calls_full = requires_collector
            .as_ref()
            .map(|c| c.full_paths())
            .unwrap_or_default();
        let requires_fn_calls = requires_collector
            .as_ref()
            .map(|c| c.fn_call_names())
            .unwrap_or_default();
        let requires_method_calls = requires_collector
            .as_ref()
            .map(|c| c.method_call_names())
            .unwrap_or_default();

        // Extended info fields (for specs-data generation)
        let impl_type = self.current_impl_type.clone();
        let display_name = if self.include_extended_info {
            Some(match &impl_type {
                Some(t) => format!("{}::{}", t, name),
                None => name.clone(),
            })
        } else {
            None
        };
        let doc_comment = self.extract_doc_comment(start_line);
        let signature_text = self.extract_signature_text(start_line, end_line);
        let body_text = if self.include_extended_info && kind == DeclKind::Spec {
            self.extract_body_text(start_line, end_line)
        } else {
            None
        };

        self.functions.push(FunctionInfo {
            name,
            file: self.file_path.clone(),
            spec_text: SpecText {
                lines_start: start_line,
                lines_end: end_line,
            },
            kind,
            kind_display,
            visibility,
            context,
            specified: has_requires || has_ensures,
            has_requires,
            has_ensures,
            has_decreases,
            has_trusted_assumption,
            contains_admit,
            is_external_body,
            is_external,
            has_body,
            is_cfg,
            has_no_decreases_attr,
            requires_text,
            ensures_text,
            ensures_calls,
            requires_calls,
            ensures_calls_full,
            requires_calls_full,
            ensures_fn_calls,
            ensures_method_calls,
            requires_fn_calls,
            requires_method_calls,
            display_name,
            impl_type,
            doc_comment,
            signature_text,
            body_text,
            module_path: None,
            fn_line,
        });
    }
}

impl<'ast> Visit<'ast> for FunctionInfoVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        let span = node.span();
        self.add_function(
            name,
            span,
            &node.sig,
            &node.vis,
            &node.attrs,
            Some("standalone".to_string()),
            true,
        );
        verus_syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if !self.include_methods {
            return;
        }

        let name = node.sig.ident.to_string();
        let span = node.span();
        let vis_public = Visibility::Public(verus_syn::token::Pub::default());
        let vis = if self.in_trait_impl {
            &vis_public
        } else {
            &node.vis
        };
        self.add_function(
            name,
            span,
            &node.sig,
            vis,
            &node.attrs,
            Some("impl".to_string()),
            true,
        );
        verus_syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        if !self.include_methods {
            return;
        }

        let name = node.sig.ident.to_string();
        let span = node.span();
        let vis = Visibility::Inherited;
        self.add_function(
            name,
            span,
            &node.sig,
            &vis,
            &node.attrs,
            Some("trait".to_string()),
            node.default.is_some(),
        );
        verus_syn::visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast verus_syn::ItemImpl) {
        // Extract the Self type name for display_name enrichment
        let prev_impl_type = self.current_impl_type.take();
        let prev_in_trait_impl = self.in_trait_impl;
        self.in_trait_impl = node.trait_.is_some();
        if self.include_extended_info {
            let ty = &node.self_ty;
            let type_str = quote::quote! { #ty }.to_string();
            // Clean up: remove spaces around :: and angle brackets for readability
            let mut cleaned = type_str
                .replace(" :: ", "::")
                .replace("< ", "<")
                .replace(" >", ">");
            // Strip reference/lifetime prefixes from trait impls for reference types.
            // e.g. `impl<'a> Neg for &'a EdwardsPoint` produces self_ty "& 'a EdwardsPoint";
            // we want just "EdwardsPoint" for display purposes.
            if cleaned.starts_with('&') {
                cleaned = cleaned
                    .trim_start_matches('&')
                    .trim_start()
                    .trim_start_matches(|c: char| c == '\'' || c.is_ascii_lowercase())
                    .trim_start()
                    .to_string();
            }
            self.current_impl_type = Some(cleaned);
        }
        let cfg_gated = has_any_cfg_attr(&node.attrs);
        if cfg_gated {
            self.inside_cfg_impl += 1;
        }
        verus_syn::visit::visit_item_impl(self, node);
        if cfg_gated {
            self.inside_cfg_impl -= 1;
        }
        self.current_impl_type = prev_impl_type;
        self.in_trait_impl = prev_in_trait_impl;
    }

    fn visit_item_trait(&mut self, node: &'ast verus_syn::ItemTrait) {
        let prev_impl_type = self.current_impl_type.take();
        if self.include_extended_info {
            self.current_impl_type = Some(node.ident.to_string());
        }
        verus_syn::visit::visit_item_trait(self, node);
        self.current_impl_type = prev_impl_type;
    }

    fn visit_item_mod(&mut self, node: &'ast verus_syn::ItemMod) {
        let cfg_gated = has_any_cfg_attr(&node.attrs);
        if cfg_gated {
            self.inside_cfg_mod += 1;
        }
        verus_syn::visit::visit_item_mod(self, node);
        if cfg_gated {
            self.inside_cfg_mod -= 1;
        }
    }

    fn visit_item_macro(&mut self, node: &'ast ItemMacro) {
        if let Some(ident) = &node.mac.path.get_ident() {
            if *ident == "verus" {
                if let Ok(items) = verus_syn::parse2::<VerusMacroBody>(node.mac.tokens.clone()) {
                    for item in items.items {
                        self.visit_item(&item);
                    }
                }
            } else if *ident == "cfg_if" {
                if let Ok(branches) = verus_syn::parse2::<CfgIfMacroBody>(node.mac.tokens.clone()) {
                    self.inside_cfg_if += 1;
                    for items in branches.all_items {
                        for item in items {
                            self.visit_item(&item);
                        }
                    }
                    self.inside_cfg_if -= 1;
                }
            }
        }
        verus_syn::visit::visit_item_macro(self, node);
    }

    fn visit_assume_specification(&mut self, node: &'ast verus_syn::AssumeSpecification) {
        let line = node.assume_specification.span.start().line;

        let path_segments = extract_assume_spec_segments(node);
        let path_display = format_assume_spec_path(node);

        let has_requires = node.requires.is_some();
        let has_ensures = node.ensures.is_some();
        let requires_text = self.extract_spec_text(node.requires.as_ref());
        let ensures_text = self.extract_spec_text(node.ensures.as_ref());

        self.assume_specifications.push(AssumeSpecInfo {
            path_segments,
            path_display,
            file: self.file_path.clone(),
            line,
            has_requires,
            has_ensures,
            requires_text,
            ensures_text,
        });

        verus_syn::visit::visit_assume_specification(self, node);
    }
}

/// Extract the matching segments from an `assume_specification` path.
///
/// For `<u64 as ConditionallySelectable>::conditional_swap`, returns
/// `["ConditionallySelectable", "conditional_swap"]` (trait + method).
///
/// For `Choice::from`, returns `["Choice", "from"]` (type + method).
fn extract_assume_spec_segments(node: &verus_syn::AssumeSpecification) -> Vec<String> {
    let segments: Vec<String> = node
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();

    // The path always ends with the method name.  For matching we
    // want the last two meaningful segments:
    // - trait impl: qself is Some → path starts with Trait::method
    // - inherent:   qself is None → path is Type::method (or longer)
    if segments.len() >= 2 {
        segments[segments.len() - 2..].to_vec()
    } else {
        segments
    }
}

/// Format the `assume_specification` path for human display.
///
/// Reconstructs `<u64 as Trait>::method` or `Choice::from` from the AST nodes.
fn format_assume_spec_path(node: &verus_syn::AssumeSpecification) -> String {
    let path_str = node
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");

    if let Some(ref qself) = node.qself {
        let self_ty = &qself.ty;
        let ty_str = quote::quote! { #self_ty }
            .to_string()
            .replace(" :: ", "::")
            .replace("< ", "<")
            .replace(" >", ">");
        // `path_str` is the part after `as Trait>`, e.g. "ConditionallySelectable::conditional_swap"
        // `ty_str` is the self type, e.g. "u64" or "[T ; N]"
        // qself.position indicates how many path segments belong to the qualified part
        let trait_segments: Vec<_> = node
            .path
            .segments
            .iter()
            .take(qself.position)
            .map(|s| s.ident.to_string())
            .collect();
        let method_segments: Vec<_> = node
            .path
            .segments
            .iter()
            .skip(qself.position)
            .map(|s| s.ident.to_string())
            .collect();

        if trait_segments.is_empty() {
            format!("<{}>::{}", ty_str, method_segments.join("::"))
        } else {
            format!(
                "<{} as {}>::{}",
                ty_str,
                trait_segments.join("::"),
                method_segments.join("::")
            )
        }
    } else {
        path_str
    }
}

/// Parse a file and extract detailed function information
pub fn parse_file_for_functions(
    file_path: &Path,
    include_verus_constructs: bool,
    include_methods: bool,
    show_visibility: bool,
    show_kind: bool,
    include_spec_text: bool,
) -> Result<Vec<FunctionInfo>, String> {
    parse_file_for_functions_ext(
        file_path,
        include_verus_constructs,
        include_methods,
        show_visibility,
        show_kind,
        include_spec_text,
        false,
    )
    .map(|(funcs, _)| funcs)
}

/// Parse a file with optional extended info (doc comments, signatures, bodies, display names).
pub fn parse_file_for_functions_ext(
    file_path: &Path,
    include_verus_constructs: bool,
    include_methods: bool,
    show_visibility: bool,
    show_kind: bool,
    include_spec_text: bool,
    include_extended_info: bool,
) -> Result<(Vec<FunctionInfo>, Vec<AssumeSpecInfo>), String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file {}: {}", file_path.display(), e))?;

    let syntax_tree = verus_syn::parse_file(&content)
        .map_err(|e| format!("Failed to parse file {}: {}", file_path.display(), e))?;

    let mut visitor = FunctionInfoVisitor::new(
        Some(file_path.to_string_lossy().to_string()),
        Some(content),
        include_verus_constructs,
        include_methods,
        show_visibility,
        show_kind,
        include_spec_text,
    );
    visitor.include_extended_info = include_extended_info;
    visitor.visit_file(&syntax_tree);

    Ok((visitor.functions, visitor.assume_specifications))
}

/// Find all Rust files in a directory (sorted for deterministic output)
fn find_rust_files(path: &Path) -> Vec<std::path::PathBuf> {
    WalkDir::new(path)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "rs"))
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Parse all functions from a path (file or directory)
pub fn parse_all_functions(
    path: &Path,
    include_verus_constructs: bool,
    include_methods: bool,
    show_visibility: bool,
    show_kind: bool,
    include_spec_text: bool,
) -> ParsedOutput {
    parse_all_functions_ext(
        path,
        include_verus_constructs,
        include_methods,
        show_visibility,
        show_kind,
        include_spec_text,
        false,
    )
}

/// Parse all functions with optional extended info for specs-data generation.
pub fn parse_all_functions_ext(
    path: &Path,
    include_verus_constructs: bool,
    include_methods: bool,
    show_visibility: bool,
    show_kind: bool,
    include_spec_text: bool,
    include_extended_info: bool,
) -> ParsedOutput {
    let mut all_functions = Vec::new();
    let mut all_assume_specs = Vec::new();
    let mut functions_by_file: HashMap<String, Vec<FunctionInfo>> = HashMap::new();
    let mut total_files = 0;

    // Get the base directory to strip from paths (to make them project-relative)
    // This matches how verus-analyzer generates relative_path in SCIP:
    // - For a directory: use the directory itself as base, so paths are relative to it
    // - For a file: use grandparent to include the parent directory name
    let base_dir: Option<&Path> = if path.is_file() {
        path.parent().and_then(|p| p.parent())
    } else {
        Some(path)
    };

    // Helper to make path relative to project root (like atoms.json format)
    let make_relative = |full_path: &Path| -> String {
        if let Some(base) = base_dir {
            if let Ok(rel) = full_path.strip_prefix(base) {
                return rel.to_string_lossy().to_string();
            }
        }
        full_path.to_string_lossy().to_string()
    };

    if path.is_file() {
        match parse_file_for_functions_ext(
            path,
            include_verus_constructs,
            include_methods,
            show_visibility,
            show_kind,
            include_spec_text,
            include_extended_info,
        ) {
            Ok((mut functions, mut assume_specs)) => {
                let relative_path = make_relative(path);
                let module_path = derive_module_path(&relative_path);
                for func in &mut functions {
                    func.file = Some(relative_path.clone());
                    if include_extended_info {
                        func.module_path = Some(module_path.clone());
                    }
                }
                for aspec in &mut assume_specs {
                    aspec.file = Some(relative_path.clone());
                }
                if !functions.is_empty() {
                    functions_by_file.insert(relative_path, functions.clone());
                    all_functions.extend(functions);
                    total_files = 1;
                }
                all_assume_specs.extend(assume_specs);
            }
            Err(e) => {
                eprintln!("Error parsing file: {}", e);
            }
        }
    } else {
        let rust_files = find_rust_files(path);
        total_files = rust_files.len();

        for file_path in rust_files {
            match parse_file_for_functions_ext(
                &file_path,
                include_verus_constructs,
                include_methods,
                show_visibility,
                show_kind,
                include_spec_text,
                include_extended_info,
            ) {
                Ok((mut functions, mut assume_specs)) => {
                    let relative_path = make_relative(&file_path);
                    let module_path = derive_module_path(&relative_path);
                    for func in &mut functions {
                        func.file = Some(relative_path.clone());
                        if include_extended_info {
                            func.module_path = Some(module_path.clone());
                        }
                    }
                    for aspec in &mut assume_specs {
                        aspec.file = Some(relative_path.clone());
                    }
                    if !functions.is_empty() {
                        functions_by_file.insert(relative_path, functions.clone());
                        all_functions.extend(functions);
                    }
                    all_assume_specs.extend(assume_specs);
                }
                Err(e) => {
                    eprintln!("Warning: {}", e);
                }
            }
        }
    }

    ParsedOutput {
        functions: all_functions.clone(),
        functions_by_file,
        assume_specifications: all_assume_specs,
        summary: ParseSummary {
            total_functions: all_functions.len(),
            total_files,
        },
    }
}

/// Derive a Rust-style module path from a file path.
///
/// Examples:
/// - "curve25519-dalek/src/specs/field_specs.rs" -> "specs::field_specs"
/// - "src/backend/serial/u64/scalar.rs" -> "backend::serial::u64::scalar"
/// - "src/field.rs" -> "field"
/// - "src/lib.rs" -> ""
/// - "src/backend/serial/u64/mod.rs" -> "backend::serial::u64"
pub fn derive_module_path(file_path: &str) -> String {
    let path = file_path.replace('\\', "/");

    // Strip everything up to and including "src/"
    let after_src = if let Some(idx) = path.find("/src/") {
        &path[idx + 5..]
    } else if let Some(stripped) = path.strip_prefix("src/") {
        stripped
    } else {
        &path
    };

    // Remove .rs extension
    let without_ext = after_src.strip_suffix(".rs").unwrap_or(after_src);

    // Remove trailing /mod or just "mod"
    let cleaned = if let Some(stripped) = without_ext.strip_suffix("/mod") {
        stripped
    } else if without_ext == "mod" || without_ext == "lib" {
        ""
    } else {
        without_ext
    };

    // Convert / to ::
    cleaned.replace('/', "::")
}

/// Compute a project prefix from a source path for GitHub link generation.
///
/// If `src_path` is like `/path/to/curve25519-dalek/src`, returns
/// `Some("curve25519-dalek/src")` so file paths become
/// `curve25519-dalek/src/module/file.rs`.
pub fn compute_project_prefix(src_path: &Path) -> Option<String> {
    let path_str = src_path.to_string_lossy();
    let path_str = path_str.replace('\\', "/");

    // Look for a pattern like "something/src" at the end
    if path_str.ends_with("/src") || path_str.ends_with("/src/") {
        let trimmed = path_str.trim_end_matches('/');
        if let Some(parent_start) = trimmed.rfind('/') {
            let parent_path = &trimmed[..parent_start];
            if let Some(grandparent_start) = parent_path.rfind('/') {
                let project_name = &parent_path[grandparent_start + 1..];
                return Some(format!("{}/src", project_name));
            }
        }
    }

    None
}

/// Find all functions with their line numbers (simplified output format)
/// Returns a map from file path to list of (function_name, line_number)
pub fn find_all_functions(
    path: &Path,
    include_verus_constructs: bool,
) -> HashMap<String, Vec<(String, usize)>> {
    let output = parse_all_functions(path, include_verus_constructs, true, false, false, false);

    output
        .functions_by_file
        .into_iter()
        .map(|(file_path, functions)| {
            let simplified: Vec<(String, usize)> = functions
                .into_iter()
                .map(|f| (f.name, f.spec_text.lines_start))
                .collect();
            (file_path, simplified)
        })
        .collect()
}

/// Get a simple list of unique function names
pub fn get_function_names(path: &Path, include_verus_constructs: bool) -> Vec<String> {
    let output = parse_all_functions(path, include_verus_constructs, true, false, false, false);
    let mut names: std::collections::HashSet<String> =
        output.functions.into_iter().map(|f| f.name).collect();
    let mut sorted: Vec<String> = names.drain().collect();
    sorted.sort();
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_simple_function() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn hello_world() {{
    println!("Hello, world!");
}}

fn another_function(x: i32) -> i32 {{
    x + 1
}}
"#
        )
        .unwrap();

        let spans = parse_file_for_spans(file.path()).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "hello_world");
        assert_eq!(spans[1].name, "another_function");

        // End lines should be after start lines
        assert!(spans[0].end_line >= spans[0].start_line);
        assert!(spans[1].end_line >= spans[1].start_line);
    }

    #[test]
    fn test_parse_file_for_functions() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
pub fn public_func() {{}}

fn private_func() {{}}

impl Foo {{
    pub fn method(&self) {{}}
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        assert_eq!(functions.len(), 3);

        // Check visibility is captured
        let public_func = functions.iter().find(|f| f.name == "public_func").unwrap();
        assert_eq!(public_func.visibility, Some("pub".to_string()));

        let private_func = functions.iter().find(|f| f.name == "private_func").unwrap();
        assert_eq!(private_func.visibility, Some("private".to_string()));
    }

    #[test]
    fn test_trait_impl_methods_are_public() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
trait MyTrait {{
    fn trait_method(&self);
}}

struct Bar;

impl MyTrait for Bar {{
    fn trait_method(&self) {{}}
}}

impl Bar {{
    fn inherent_private(&self) {{}}
    pub fn inherent_public(&self) {{}}
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();

        let trait_method = functions
            .iter()
            .find(|f| f.name == "trait_method" && f.context == Some("impl".to_string()))
            .unwrap();
        assert_eq!(
            trait_method.visibility,
            Some("pub".to_string()),
            "trait impl methods should be marked public"
        );

        let inherent_private = functions
            .iter()
            .find(|f| f.name == "inherent_private")
            .unwrap();
        assert_eq!(
            inherent_private.visibility,
            Some("private".to_string()),
            "inherent impl methods without pub should be private"
        );

        let inherent_public = functions
            .iter()
            .find(|f| f.name == "inherent_public")
            .unwrap();
        assert_eq!(
            inherent_public.visibility,
            Some("pub".to_string()),
            "inherent impl methods with pub should be public"
        );
    }

    // =========================================================================
    // Soundness tests: is_proved() logic (S1)
    // =========================================================================

    fn make_test_func_info(
        has_requires: bool,
        has_ensures: bool,
        has_trusted_assumption: bool,
        is_external_body: bool,
        has_no_decreases_attr: bool,
    ) -> FunctionInfo {
        FunctionInfo {
            name: "test_fn".to_string(),
            file: Some("src/lib.rs".to_string()),
            spec_text: SpecText {
                lines_start: 1,
                lines_end: 10,
            },
            kind: DeclKind::Exec,
            kind_display: Some("exec".to_string()),
            visibility: None,
            context: None,
            specified: has_requires || has_ensures,
            has_requires,
            has_ensures,
            has_decreases: false,
            has_trusted_assumption,
            contains_admit: false,
            is_external_body,
            is_external: false,
            has_body: true,
            is_cfg: false,
            has_no_decreases_attr,
            requires_text: None,
            ensures_text: None,
            ensures_calls: Vec::new(),
            requires_calls: Vec::new(),
            ensures_calls_full: Vec::new(),
            requires_calls_full: Vec::new(),
            ensures_fn_calls: Vec::new(),
            ensures_method_calls: Vec::new(),
            requires_fn_calls: Vec::new(),
            requires_method_calls: Vec::new(),
            display_name: None,
            impl_type: None,
            doc_comment: None,
            signature_text: None,
            body_text: None,
            module_path: None,
            fn_line: 1,
        }
    }

    /// S1: external_body with specs should not count as proved.
    #[test]
    fn test_external_body_with_spec_not_proved() {
        let func = make_test_func_info(true, true, false, true, false);
        assert!(
            !func.is_proved(),
            "external_body function must not be considered proved"
        );
    }

    /// S1: function with spec and no escape hatches should be proved.
    #[test]
    fn test_clean_spec_function_is_proved() {
        let func = make_test_func_info(true, false, false, false, false);
        assert!(
            func.is_proved(),
            "function with requires and no escape hatches should be proved"
        );
    }

    /// S1: function with no spec should not be proved.
    #[test]
    fn test_no_spec_not_proved() {
        let func = make_test_func_info(false, false, false, false, false);
        assert!(
            !func.is_proved(),
            "function without spec should not be proved"
        );
    }

    // =========================================================================
    // Soundness tests: assume/admit detection (S3)
    // =========================================================================

    /// S3: assume() in a comment should not be detected as a trusted assumption.
    #[test]
    fn test_assume_in_comment_not_detected_as_trusted() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn safe_function() {{
    // We removed assume() from this function
    let x: u32 = 42;
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let f = functions
            .iter()
            .find(|f| f.name == "safe_function")
            .unwrap();
        assert!(
            !f.has_trusted_assumption,
            "assume() in a line comment must not be detected as a trusted assumption"
        );
    }

    /// S3: A real assume() call should be detected.
    #[test]
    fn test_real_assume_is_detected() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn unsafe_function() {{
    assume(false);
    let x: u32 = 42;
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let f = functions
            .iter()
            .find(|f| f.name == "unsafe_function")
            .unwrap();
        assert!(
            f.has_trusted_assumption,
            "real assume() call should be detected as trusted assumption"
        );
    }

    /// S3: admit() in a string literal must not be detected as a trusted assumption.
    #[test]
    fn test_admit_in_string_literal_not_detected_as_trusted() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn logging_function() {{
    let msg = "checking admit() usage";
    let x: u32 = 42;
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let f = functions
            .iter()
            .find(|f| f.name == "logging_function")
            .unwrap();
        assert!(
            !f.has_trusted_assumption,
            "admit() in a string literal must not be detected as a trusted assumption"
        );
    }

    // =========================================================================
    // contains_admit detection (trusted status)
    // =========================================================================

    /// A real admit() call sets contains_admit = true.
    #[test]
    fn test_contains_admit_real_call() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn axiom_function() {{
    admit();
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let f = functions
            .iter()
            .find(|f| f.name == "axiom_function")
            .unwrap();
        assert!(
            f.contains_admit,
            "real admit() call must set contains_admit"
        );
        assert!(
            f.has_trusted_assumption,
            "admit() also sets has_trusted_assumption"
        );
    }

    /// assume() alone does NOT set contains_admit.
    #[test]
    fn test_assume_does_not_set_contains_admit() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn assume_function() {{
    assume(false);
    let x: u32 = 42;
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let f = functions
            .iter()
            .find(|f| f.name == "assume_function")
            .unwrap();
        assert!(!f.contains_admit, "assume() must not set contains_admit");
        assert!(
            f.has_trusted_assumption,
            "assume() still sets has_trusted_assumption"
        );
    }

    /// admit() in a comment does not set contains_admit.
    #[test]
    fn test_admit_in_comment_not_detected() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
fn clean_function() {{
    // We removed admit() from this proof
    let x: u32 = 42;
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let f = functions
            .iter()
            .find(|f| f.name == "clean_function")
            .unwrap();
        assert!(
            !f.contains_admit,
            "admit() in a comment must not set contains_admit"
        );
    }

    #[test]
    fn test_assume_specification_parsed() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
verus! {{
    pub assume_specification[ Choice::from ](u: u8) -> (c: Choice)
        ensures
            (u == 1) == choice_is_true(c),
    ;

    pub assume_specification[ Choice::unwrap_u8 ](c: &Choice) -> (u: u8)
        ensures
            choice_is_true(*c) ==> u == 1u8,
    ;
}}
"#
        )
        .unwrap();

        let (functions, assume_specs) =
            parse_file_for_functions_ext(file.path(), true, true, true, true, true, false).unwrap();

        assert!(
            functions.is_empty(),
            "assume_specification is not a function"
        );
        assert_eq!(assume_specs.len(), 2, "Should find 2 assume_specifications");

        let choice_from = &assume_specs[0];
        assert_eq!(choice_from.path_segments, vec!["Choice", "from"]);
        assert!(choice_from.path_display.contains("Choice"));
        assert!(choice_from.path_display.contains("from"));
        assert!(choice_from.has_ensures);
        assert!(!choice_from.has_requires);

        let unwrap = &assume_specs[1];
        assert_eq!(unwrap.path_segments, vec!["Choice", "unwrap_u8"]);
        assert!(unwrap.has_ensures);
    }

    #[test]
    fn test_assume_specification_trait_impl_path() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
verus! {{
    pub assume_specification[ <u64 as ConditionallySelectable>::conditional_swap ](
        a: &mut u64,
        b: &mut u64,
        choice: Choice,
    )
        ensures
            choice_is_true(choice) ==> (*a == *old(b) && *b == *old(a)),
    ;
}}
"#
        )
        .unwrap();

        let (_, assume_specs) =
            parse_file_for_functions_ext(file.path(), true, true, true, true, true, false).unwrap();

        assert_eq!(assume_specs.len(), 1);
        let aspec = &assume_specs[0];
        assert_eq!(
            aspec.path_segments,
            vec!["ConditionallySelectable", "conditional_swap"]
        );
        assert!(
            aspec.path_display.contains("u64"),
            "Display should include self type: {}",
            aspec.path_display
        );
        assert!(
            aspec.path_display.contains("ConditionallySelectable"),
            "Display should include trait: {}",
            aspec.path_display
        );
    }

    #[test]
    fn test_cfg_attr_verifier_external_detection() {
        let src = r#"
verus! {
    #[cfg_attr(verus_keep_ghost, verifier::external)]
    pub fn gated_external() {}

    #[verifier::external]
    pub fn direct_external() {}

    pub fn normal() {}
}
"#;
        let parsed = verus_syn::parse_file(src).unwrap();

        // Walk items inside verus! macro to test attribute helpers
        let mut results: Vec<(String, bool, bool)> = Vec::new();
        for item in &parsed.items {
            if let verus_syn::Item::Macro(mac) = item {
                let body: verus_syn::File = verus_syn::parse2(mac.mac.tokens.clone()).unwrap();
                for inner in &body.items {
                    if let verus_syn::Item::Fn(item_fn) = inner {
                        let name = item_fn.sig.ident.to_string();
                        let is_ext = has_verifier_external(&item_fn.attrs);
                        let is_cfg = has_any_cfg_attr(&item_fn.attrs);
                        results.push((name, is_ext, is_cfg));
                    }
                }
            }
        }

        assert_eq!(results.len(), 3);

        let (name, is_ext, is_cfg) = &results[0];
        assert_eq!(name, "gated_external");
        assert!(
            is_ext,
            "cfg_attr-wrapped verifier::external must be detected"
        );
        assert!(
            !is_cfg,
            "cfg_attr wrapping verifier::external is not a #[cfg] gate"
        );

        let (name, is_ext, is_cfg) = &results[1];
        assert_eq!(name, "direct_external");
        assert!(is_ext, "direct verifier::external must be detected");
        assert!(!is_cfg);

        let (name, is_ext, is_cfg) = &results[2];
        assert_eq!(name, "normal");
        assert!(!is_ext);
        assert!(!is_cfg);
    }

    #[test]
    fn test_has_body_trait_methods() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
trait Foo {{
    fn bodiless(&self);
    fn with_default(&self) {{}}
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();
        let bodiless = functions.iter().find(|f| f.name == "bodiless").unwrap();
        assert!(
            !bodiless.has_body,
            "bodiless trait method should have has_body=false"
        );
        let with_default = functions.iter().find(|f| f.name == "with_default").unwrap();
        assert!(
            with_default.has_body,
            "default trait method should have has_body=true"
        );
    }

    #[test]
    fn test_is_external_and_cfg_on_function_info() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
verus! {{
    #[verifier::external]
    pub fn ext_fn() {{}}

    #[cfg(feature = "alloc")]
    pub fn cfg_fn() {{}}

    pub fn plain() {{}}
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();

        let ext = functions.iter().find(|f| f.name == "ext_fn").unwrap();
        assert!(ext.is_external);
        assert!(!ext.is_cfg);

        let cfg = functions.iter().find(|f| f.name == "cfg_fn").unwrap();
        assert!(!cfg.is_external);
        assert!(cfg.is_cfg);

        let plain = functions.iter().find(|f| f.name == "plain").unwrap();
        assert!(!plain.is_external);
        assert!(!plain.is_cfg);
    }

    #[test]
    fn test_cfg_impl_propagation() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
struct Foo;

#[cfg(feature = "alloc")]
impl Foo {{
    fn gated_method(&self) {{}}
}}

impl Foo {{
    fn normal_method(&self) {{}}
}}
"#
        )
        .unwrap();

        let functions =
            parse_file_for_functions(file.path(), true, true, true, true, false).unwrap();

        let gated = functions.iter().find(|f| f.name == "gated_method").unwrap();
        assert!(
            gated.is_cfg,
            "method inside #[cfg] impl should inherit is_cfg"
        );

        let normal = functions
            .iter()
            .find(|f| f.name == "normal_method")
            .unwrap();
        assert!(!normal.is_cfg);
    }

    #[test]
    fn test_has_body_is_external_is_cfg_on_spans() {
        let file_content = r#"
verus! {
    #[verifier::external]
    pub fn ext() {}

    #[cfg(test)]
    pub fn in_test() {}

    pub fn normal() {}
}

trait Bar {
    fn decl_only(&self);
}
"#;
        let parsed = verus_syn::parse_file(file_content).unwrap();
        let mut visitor = FunctionSpanVisitor::new();
        visitor.visit_file(&parsed);

        let ext = visitor.functions.iter().find(|f| f.name == "ext").unwrap();
        assert!(ext.is_external);
        assert!(ext.has_body);
        assert!(!ext.is_cfg);

        let in_test = visitor
            .functions
            .iter()
            .find(|f| f.name == "in_test")
            .unwrap();
        assert!(!in_test.is_external);
        assert!(in_test.is_cfg);
        assert!(in_test.has_body);

        let decl = visitor
            .functions
            .iter()
            .find(|f| f.name == "decl_only")
            .unwrap();
        assert!(
            !decl.has_body,
            "bodiless trait fn should have has_body=false"
        );
        assert!(!decl.is_external);
        assert!(!decl.is_cfg);
    }
}
