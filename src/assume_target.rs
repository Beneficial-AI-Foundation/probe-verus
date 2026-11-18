//! Crates of the types and traits that an `assume_specification` target names.
//!
//! A target such as `Choice::from` depends on imports, so its name alone does
//! not identify the function. The SCIP index resolves the type and trait
//! segments of the target path (`Choice` -> `subtle 2.6.1 Choice#`), but not
//! the method. The crates of those segments limit which external stubs the
//! specification can attach to.

use crate::constants::SCIP_SYMBOL_PREFIX;
use crate::path_utils::paths_match_by_suffix;
use crate::verus_parser::AssumeSpecInfo;
use crate::{ScipIndex, PROBE_URI_PREFIX};
use std::collections::BTreeSet;
use std::path::Path;

/// Rust primitive types: they have no SCIP symbol, so they need no resolution.
const PRIMITIVES: [&str; 17] = [
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

/// For each `assume_specification` (same order), the code-name prefixes
/// (`probe:<crate>/<version>/`) of the crates that define its named Self type
/// and trait, or the reason why they cannot be established.
pub fn resolve_target_crates(
    specs: &[AssumeSpecInfo],
    index: &ScipIndex,
    source_root: &Path,
) -> Vec<Result<BTreeSet<String>, String>> {
    specs
        .iter()
        .map(|spec| resolve_one(spec, index, source_root))
        .collect()
}

fn resolve_one(
    spec: &AssumeSpecInfo,
    index: &ScipIndex,
    source_root: &Path,
) -> Result<BTreeSet<String>, String> {
    let file = spec.file.as_deref().ok_or("the declaration has no file")?;
    let source = std::fs::read_to_string(source_root.join(file))
        .map_err(|e| format!("cannot read {file}: {e}"))?;
    let (start, end) = target_range(&source, spec.line)
        .ok_or_else(|| format!("cannot find the target brackets at {file}:{}", spec.line))?;
    let document = index
        .documents
        .iter()
        .find(|d| paths_match_by_suffix(file, d.relative_path.trim_start_matches('/')))
        .ok_or_else(|| format!("{file} is not in the SCIP index"))?;

    // Type and trait symbols referenced inside the brackets, by name.
    let referenced: Vec<(&str, &str)> = document
        .occurrences
        .iter()
        .filter(|o| o.range.len() >= 2 && (start..=end).contains(&(o.range[0], o.range[1])))
        .filter(|o| o.symbol.starts_with(SCIP_SYMBOL_PREFIX) && o.symbol.ends_with('#'))
        .map(|o| (type_name(&o.symbol), o.symbol.as_str()))
        .collect();

    let mut crates = BTreeSet::new();
    for name in owner_names(spec) {
        let symbol = referenced
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| *s)
            .ok_or_else(|| format!("the index does not resolve `{name}`"))?;
        crates.insert(crate_prefix(symbol).ok_or_else(|| format!("bad symbol `{symbol}`"))?);
    }
    if crates.is_empty() {
        return Err("the target names no type or trait that the index can resolve".to_string());
    }
    Ok(crates)
}

/// Names of the Self type and trait of the target that need a SCIP symbol:
/// identifiers that are not primitive types. `From<u8>` gives `From`,
/// `core::hash::Hash` gives `Hash`, `[T; N]` gives nothing.
fn owner_names(spec: &AssumeSpecInfo) -> Vec<String> {
    let mut owners: Vec<&str> = Vec::new();
    match (&spec.self_type, &spec.trait_type) {
        (Some(self_type), trait_type) => {
            owners.push(self_type);
            owners.extend(trait_type.as_deref());
        }
        (None, _) => {
            let segments = &spec.path_segments;
            if segments.len() >= 2 {
                owners.push(&segments[segments.len() - 2]);
            }
        }
    }
    owners
        .into_iter()
        .filter_map(|owner| {
            let without_generics = owner.split('<').next().unwrap_or(owner).trim();
            let name = without_generics
                .rsplit("::")
                .next()
                .unwrap_or(without_generics);
            let is_ident = name
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_alphanumeric() || c == '_');
            (is_ident && !PRIMITIVES.contains(&name)).then(|| name.to_string())
        })
        .collect()
}

/// 0-based (line, column) of the `[` and `]` around the target of the
/// `assume_specification` declared at 1-based `line`.
fn target_range(source: &str, line: usize) -> Option<((i32, i32), (i32, i32))> {
    let mut start = None;
    let mut depth = 0usize;
    let mut seen_keyword = false;
    for (l, text) in source.lines().enumerate().skip(line.checked_sub(1)?) {
        let mut chars = text.char_indices().peekable();
        if !seen_keyword {
            let pos = text.find("assume_specification")?;
            seen_keyword = true;
            while chars.peek().is_some_and(|(i, _)| *i < pos) {
                chars.next();
            }
        }
        for (col, c) in chars {
            match c {
                '[' => {
                    if depth == 0 && start.is_none() {
                        start = Some((l as i32, col as i32));
                    }
                    depth += 1;
                }
                ']' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start?, (l as i32, col as i32)));
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Name of a type or trait symbol: `... fmt/Formatter#` gives `Formatter`.
fn type_name(symbol: &str) -> &str {
    let body = symbol.trim_end_matches('#');
    let start = body.rfind(['/', ' ']).map_or(0, |i| i + 1);
    &body[start..]
}

/// `probe:<crate>/<version>/` of `rust-analyzer cargo <crate> <version> ...`.
fn crate_prefix(symbol: &str) -> Option<String> {
    let mut fields = symbol.strip_prefix(SCIP_SYMBOL_PREFIX)?.splitn(3, ' ');
    let krate = fields.next()?;
    let version = fields.next()?;
    Some(format!("{PROBE_URI_PREFIX}{krate}/{version}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, Metadata, Occurrence, ScipToolInfo};

    fn spec(path: &[&str], self_type: Option<&str>, trait_type: Option<&str>) -> AssumeSpecInfo {
        serde_json::from_value(serde_json::json!({
            "path-segments": path,
            "path-display": path.join("::"),
            "file": "src/assumes.rs",
            "line": 2,
            "self-type": self_type,
            "trait-type": trait_type,
        }))
        .unwrap()
    }

    fn index(occurrences: &[(&str, [i32; 3])]) -> ScipIndex {
        ScipIndex {
            metadata: Metadata {
                tool_info: ScipToolInfo {
                    name: "rust-analyzer".to_string(),
                    version: "0.3.269".to_string(),
                },
                project_root: String::new(),
                text_document_encoding: 0,
            },
            documents: vec![Document {
                language: "rust".to_string(),
                relative_path: "crate-a/src/assumes.rs".to_string(),
                occurrences: occurrences
                    .iter()
                    .map(|(s, r)| Occurrence {
                        range: r.to_vec(),
                        symbol: s.to_string(),
                        symbol_roles: None,
                    })
                    .collect(),
                symbols: vec![],
            }],
        }
    }

    const CHOICE: &str = "rust-analyzer cargo subtle 2.6.1 Choice#";
    const SELECTABLE: &str = "rust-analyzer cargo subtle 2.6.1 ConditionallySelectable#";
    const OTHER_CHOICE: &str = "rust-analyzer cargo other 1.0.0 Choice#";

    fn source_dir(text: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/assumes.rs"), text).unwrap();
        dir
    }

    #[test]
    fn test_path_target_resolves_owner_crate() {
        // Line 2: `Choice` in the target at column 26, in the return type at 55.
        let dir = source_dir(
            "use subtle::Choice;\npub assume_specification[ Choice::from ](u: u8) -> (c: Choice);\n",
        );
        let idx = index(&[(CHOICE, [1, 26, 32]), (OTHER_CHOICE, [1, 55, 61])]);
        let result =
            resolve_target_crates(&[spec(&["Choice", "from"], None, None)], &idx, dir.path());
        assert_eq!(
            result[0].as_ref().unwrap(),
            &BTreeSet::from(["probe:subtle/2.6.1/".to_string()])
        );
    }

    #[test]
    fn test_qualified_target_with_primitive_self_type() {
        let dir = source_dir(
            "\npub assume_specification[ <u64 as ConditionallySelectable>::conditional_swap ](\n",
        );
        let idx = index(&[(SELECTABLE, [1, 34, 57])]);
        let s = spec(
            &["ConditionallySelectable", "conditional_swap"],
            Some("u64"),
            Some("ConditionallySelectable"),
        );
        let result = resolve_target_crates(&[s], &idx, dir.path());
        assert_eq!(
            result[0].as_ref().unwrap(),
            &BTreeSet::from(["probe:subtle/2.6.1/".to_string()])
        );
    }

    /// A type that the index does not resolve inside the brackets fails closed,
    /// even when the same name is resolved elsewhere on the line.
    #[test]
    fn test_unresolved_owner_is_an_error() {
        let dir = source_dir("\npub assume_specification[ Choice::from ](u: u8) -> (c: Choice);\n");
        let idx = index(&[(CHOICE, [1, 55, 61])]);
        let result =
            resolve_target_crates(&[spec(&["Choice", "from"], None, None)], &idx, dir.path());
        assert!(result[0].as_ref().unwrap_err().contains("`Choice`"));
    }

    #[test]
    fn test_target_range_with_generics_and_nested_brackets() {
        let source = "x\npub assume_specification<T, const N: usize>[ <[T; N] as Hash>::hash ](\n";
        assert_eq!(target_range(source, 2), Some(((1, 43), (1, 68))));
        assert_eq!(target_range(source, 1), None);
    }

    #[test]
    fn test_owner_names() {
        assert_eq!(
            owner_names(&spec(
                &["Hash", "hash"],
                Some("[T; N]"),
                Some("core::hash::Hash")
            )),
            vec!["Hash"]
        );
        assert_eq!(
            owner_names(&spec(&["From", "from"], Some("Choice"), Some("From<u8>"))),
            vec!["Choice", "From"]
        );
        assert!(owner_names(&spec(&["f"], None, None)).is_empty());
    }

    #[test]
    fn test_type_name_and_crate_prefix() {
        let core = "rust-analyzer cargo core https://github.com/rust-lang/rust/library/core fmt/Formatter#";
        assert_eq!(type_name(core), "Formatter");
        assert_eq!(type_name(CHOICE), "Choice");
        assert_eq!(
            crate_prefix(core).unwrap(),
            "probe:core/https://github.com/rust-lang/rust/library/core/"
        );
    }
}
