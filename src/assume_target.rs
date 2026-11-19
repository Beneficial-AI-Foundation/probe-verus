//! Crates of the types and traits that an `assume_specification` target names.
//!
//! A target such as `Choice::from` depends on imports, so its name alone does
//! not identify the function. The SCIP index resolves the type and trait
//! segments of the target path (`Choice` -> `subtle 2.6.1 Choice#`), but not
//! the method. Rust's orphan rule puts the impl in the crate of the trait or of
//! a type that the target names, so the crates of the types and traits inside
//! the target brackets limit which external stubs the specification can attach
//! to.

use crate::constants::SCIP_SYMBOL_PREFIX;
use crate::path_utils::paths_match_by_suffix;
use crate::verus_parser::AssumeSpecInfo;
use crate::{Document, ScipIndex, PROBE_URI_PREFIX};
use std::collections::BTreeSet;
use std::path::Path;

/// Rust primitive types: they have no SCIP symbol, so they need no resolution.
const PRIMITIVES: [&str; 17] = [
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

/// For each `assume_specification` (same order), the code-name prefixes
/// (`probe:<crate>/<version>/`) of the crates that define the types and traits
/// named in its target, or the reason why they cannot be established. The Self
/// type and the trait ([`owner_names`]) must resolve.
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
        .ok_or_else(|| format!("cannot locate the target brackets at {file}:{}", spec.line))?;
    let document = find_document(index, source_root, file)?;

    // Type and trait symbols referenced inside the brackets, by name.
    let referenced: Vec<(&str, &str)> = document
        .occurrences
        .iter()
        .filter(|o| o.range.len() >= 2 && (start..=end).contains(&(o.range[0], o.range[1])))
        .filter(|o| o.symbol.starts_with(SCIP_SYMBOL_PREFIX) && o.symbol.ends_with('#'))
        .map(|o| (type_name(&o.symbol), o.symbol.as_str()))
        .collect();

    for name in owner_names(spec) {
        if !referenced.iter().any(|(n, _)| *n == name) {
            return Err(format!("the index does not resolve `{name}`"));
        }
    }
    let mut crates = BTreeSet::new();
    for (_, symbol) in &referenced {
        crates.insert(crate_prefix(symbol).ok_or_else(|| format!("bad symbol `{symbol}`"))?);
    }
    if crates.is_empty() {
        return Err("the target names no type or trait that the index can resolve".to_string());
    }
    Ok(crates)
}

/// The index document of `file`. Its path relative to the index root decides;
/// without that, the one document whose path ends with `file`.
fn find_document<'a>(
    index: &'a ScipIndex,
    source_root: &Path,
    file: &str,
) -> Result<&'a Document, String> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let index_root = index.metadata.project_root.strip_prefix("file://");
    let exact = index_root.and_then(|root| {
        let path = canonical(&source_root.join(file));
        let relative = path
            .strip_prefix(canonical(Path::new(root)))
            .ok()?
            .to_str()?
            .to_owned();
        index
            .documents
            .iter()
            .find(|d| d.relative_path.trim_start_matches('/') == relative)
    });
    if let Some(document) = exact {
        return Ok(document);
    }
    let mut matches = index
        .documents
        .iter()
        .filter(|d| paths_match_by_suffix(file, d.relative_path.trim_start_matches('/')));
    match (matches.next(), matches.next()) {
        (Some(document), None) => Ok(document),
        (None, _) => Err(format!("{file} is not in the SCIP index")),
        (Some(_), Some(_)) => Err(format!("{file} matches several files in the SCIP index")),
    }
}

/// Names of the Self type and trait of the target that must have a SCIP symbol:
/// identifiers that are not primitive types. `From<u8>` gives `From`,
/// `core::hash::Hash` gives `Hash`, `&'a mut Foo` gives `Foo`, `[T; N]` gives
/// nothing.
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
            let owner = strip_indirection(owner);
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

/// The type behind references and raw pointers: `Foo` of `& 'a mut Foo`.
fn strip_indirection(mut ty: &str) -> &str {
    loop {
        ty = ty.trim_start();
        let Some(rest) = ty.strip_prefix(['&', '*']) else {
            return ty;
        };
        let mut rest = rest.trim_start();
        if rest.starts_with('\'') {
            rest = rest
                .find(char::is_whitespace)
                .map_or("", |i| &rest[i..])
                .trim_start();
        }
        ty = rest
            .strip_prefix("mut ")
            .or_else(|| rest.strip_prefix("const "))
            .unwrap_or(rest);
    }
}

/// 0-based (line, column) of the `[` and `]` around the target of the
/// `assume_specification` declared at 1-based `line`. Brackets inside the
/// generics (`<T: AsRef<[u8]>>`) are skipped. `None` if the line has no
/// `assume_specification` or has several.
fn target_range(source: &str, line: usize) -> Option<((i32, i32), (i32, i32))> {
    let mut start = None;
    let mut depth = 0usize;
    let mut seen_keyword = false;
    // `<` depth of the generics before the target; the `>` of `->` does not count.
    let mut angle = 0usize;
    let mut prev = ' ';
    for (l, text) in source.lines().enumerate().skip(line.checked_sub(1)?) {
        let mut chars = text.char_indices().peekable();
        if !seen_keyword {
            let pos = text.find("assume_specification")?;
            if text.matches("assume_specification").count() > 1 {
                return None;
            }
            seen_keyword = true;
            while chars.peek().is_some_and(|(i, _)| *i < pos) {
                chars.next();
            }
        }
        for (col, c) in chars {
            match c {
                '<' if start.is_none() => angle += 1,
                '>' if start.is_none() && angle > 0 && prev != '-' => angle -= 1,
                '[' if start.is_none() && angle > 0 => {}
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
            prev = c;
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
    use crate::{Metadata, Occurrence, ScipToolInfo};

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
        let source =
            "pub assume_specification<T: AsRef<[u8]>, F: Fn() -> u8>[ Hasher::update::<T> ](\n";
        assert_eq!(target_range(source, 1), Some(((0, 55), (0, 77))));
        let source = "assume_specification[ A::f ]; assume_specification[ B::f ];\n";
        assert_eq!(target_range(source, 1), None);
    }

    /// Every type and trait in the brackets adds its crate: the trait argument
    /// `Choice` of `<u8 as From<Choice>>::from`.
    #[test]
    fn test_trait_argument_adds_its_crate() {
        let from =
            "rust-analyzer cargo core https://github.com/rust-lang/rust/library/core convert/From#";
        let dir = source_dir("\npub assume_specification[ <u8 as From<Choice>>::from ](\n");
        let idx = index(&[(from, [1, 33, 37]), (CHOICE, [1, 38, 44])]);
        let s = spec(&["From", "from"], Some("u8"), Some("From<Choice>"));
        let result = resolve_target_crates(&[s], &idx, dir.path());
        assert_eq!(
            result[0].as_ref().unwrap(),
            &BTreeSet::from([
                "probe:core/https://github.com/rust-lang/rust/library/core/".to_string(),
                "probe:subtle/2.6.1/".to_string(),
            ])
        );
    }

    /// A Self type and a trait with the same name both add their crates.
    #[test]
    fn test_same_name_owners_add_both_crates() {
        let dir = source_dir("\npub assume_specification[ <Choice as other::Choice>::m ](\n");
        let idx = index(&[(CHOICE, [1, 27, 33]), (OTHER_CHOICE, [1, 44, 50])]);
        let s = spec(&["Choice", "m"], Some("Choice"), Some("other::Choice"));
        let result = resolve_target_crates(&[s], &idx, dir.path());
        assert_eq!(
            result[0].as_ref().unwrap(),
            &BTreeSet::from([
                "probe:other/1.0.0/".to_string(),
                "probe:subtle/2.6.1/".to_string(),
            ])
        );
    }

    /// Two workspace members with the same file: the index root picks one, and
    /// without it the suffix match is ambiguous.
    #[test]
    fn test_find_document_in_workspace() {
        let mut idx = index(&[]);
        idx.documents.push(Document {
            language: "rust".to_string(),
            relative_path: "crate-b/src/assumes.rs".to_string(),
            occurrences: vec![],
            symbols: vec![],
        });
        idx.metadata.project_root = "file:///ws".to_string();
        let found = find_document(&idx, Path::new("/ws/crate-b"), "src/assumes.rs").unwrap();
        assert_eq!(found.relative_path, "crate-b/src/assumes.rs");
        idx.metadata.project_root = String::new();
        let err = find_document(&idx, Path::new("/ws/crate-b"), "src/assumes.rs").unwrap_err();
        assert!(err.contains("several"));
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
        assert_eq!(
            owner_names(&spec(&["Neg", "neg"], Some("& 'a mut Foo"), Some("Neg"))),
            vec!["Foo", "Neg"]
        );
        assert_eq!(
            owner_names(&spec(&["Hash", "hash"], Some("& u8"), Some("Hash"))),
            vec!["Hash"]
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
