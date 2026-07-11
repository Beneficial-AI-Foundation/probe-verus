//! Evaluation of `#[cfg(...)]` predicates against the verification build config.
//!
//! Implements KB P26: a function whose governing cfg predicate is *false* under
//! the verification build's active configuration is not compiled and therefore
//! out of verification scope (not backlog).
//!
//! Evaluation is deliberately **conservative**: it returns `Some(false)` only
//! when a predicate is definitively inactive given fully-known inputs, and
//! `None` when it references a flag/target key the tool cannot resolve. Callers
//! must treat `None` as "leave as-is" and never hide a backlog atom on a guess
//! (P26).

use std::collections::HashSet;

/// A parsed `#[cfg(...)]` predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum CfgExpr {
    /// A bare flag: `verus_keep_ghost`, `test`, `nightly`, `docsrs`, ...
    Flag(String),
    /// A `key = "value"` predicate: `feature = "alloc"`, `target_arch = "x86_64"`.
    KeyValue(String, String),
    Not(Box<CfgExpr>),
    All(Vec<CfgExpr>),
    Any(Vec<CfgExpr>),
}

/// The active configuration of the verification build.
#[derive(Debug, Clone, Default)]
pub struct CfgConfig {
    /// Resolved active cargo features (transitive closure of the default set).
    /// `None` = features could not be resolved (e.g. unreadable/unparseable
    /// `Cargo.toml`), in which case `feature = "..."` predicates are undecidable
    /// (`None`) rather than false — so we never hide backlog on a guess (P26).
    pub features: Option<HashSet<String>>,
    /// Whether `verus_keep_ghost` is set (true for `cargo verus verify`).
    pub verus_keep_ghost: bool,
}

impl CfgConfig {
    /// Evaluate a predicate. `Some(true)`/`Some(false)` when decidable from
    /// known inputs; `None` when it references something we cannot resolve
    /// (unknown flag or a `key = value` other than `feature`).
    pub fn eval(&self, expr: &CfgExpr) -> Option<bool> {
        match expr {
            CfgExpr::Flag(name) => match name.as_str() {
                "verus_keep_ghost" => Some(self.verus_keep_ghost),
                // A verification build is never a `#[cfg(test)]` build.
                "test" => Some(false),
                // Unknown flags (nightly, docsrs, custom cfgs): undecidable.
                _ => None,
            },
            CfgExpr::KeyValue(key, value) => {
                if key == "feature" {
                    // Undecidable when the feature set could not be resolved.
                    self.features.as_ref().map(|f| f.contains(value))
                } else {
                    // target_arch / backend / bits / diagnostics: not resolved here.
                    None
                }
            }
            CfgExpr::Not(inner) => self.eval(inner).map(|b| !b),
            CfgExpr::All(items) => {
                let mut all_true = true;
                for it in items {
                    match self.eval(it) {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => all_true = false,
                    }
                }
                if all_true {
                    Some(true)
                } else {
                    None
                }
            }
            CfgExpr::Any(items) => {
                let mut all_false = true;
                for it in items {
                    match self.eval(it) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => all_false = false,
                    }
                }
                if all_false {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }

    /// Whether a predicate is definitively inactive in the verification build
    /// (i.e. the guarded item is not compiled). Only `Some(false)` counts.
    pub fn is_inactive(&self, predicate: &str) -> bool {
        parse_cfg(predicate).is_some_and(|e| self.eval(&e) == Some(false))
    }
}

/// Parse a cfg predicate string (the tokens inside `#[cfg(...)]`, e.g.
/// `all(feature = "alloc", not(verus_keep_ghost))`). Returns `None` on malformed
/// input, which callers treat as "cannot decide" (conservative).
pub fn parse_cfg(s: &str) -> Option<CfgExpr> {
    let tokens = tokenize(s)?;
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_expr()?;
    if p.pos == p.tokens.len() {
        Some(expr)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    LParen,
    RParen,
    Comma,
    Eq,
}

/// Tokenize a cfg predicate. Returns `None` on any unexpected character, so a
/// malformed predicate yields `parse_cfg(..) == None` (conservative: undecidable).
fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                out.push(Tok::LParen);
            }
            ')' => {
                chars.next();
                out.push(Tok::RParen);
            }
            ',' => {
                chars.next();
                out.push(Tok::Comma);
            }
            '=' => {
                chars.next();
                out.push(Tok::Eq);
            }
            '"' => {
                chars.next();
                let mut val = String::new();
                for ch in chars.by_ref() {
                    if ch == '"' {
                        break;
                    }
                    val.push(ch);
                }
                out.push(Tok::Str(val));
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' => {
                let mut ident = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                        ident.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Ident(ident));
            }
            _ => {
                // Unexpected character: bail (whole parse fails → conservative).
                return None;
            }
        }
    }
    Some(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Option<CfgExpr> {
        let ident = match self.next()? {
            Tok::Ident(i) => i,
            _ => return None,
        };
        match self.peek() {
            // `ident(...)` — combinator or bare-flag-that-looks-like-call is
            // impossible; only all/any/not take parens.
            Some(Tok::LParen) => {
                self.next(); // consume (
                match ident.as_str() {
                    "not" => {
                        let inner = self.parse_expr()?;
                        self.expect(Tok::RParen)?;
                        Some(CfgExpr::Not(Box::new(inner)))
                    }
                    "all" | "any" => {
                        let items = self.parse_list()?;
                        self.expect(Tok::RParen)?;
                        if ident == "all" {
                            Some(CfgExpr::All(items))
                        } else {
                            Some(CfgExpr::Any(items))
                        }
                    }
                    _ => None,
                }
            }
            // `ident = "value"`
            Some(Tok::Eq) => {
                self.next(); // consume =
                match self.next()? {
                    Tok::Str(v) => Some(CfgExpr::KeyValue(ident, v)),
                    _ => None,
                }
            }
            // bare flag
            _ => Some(CfgExpr::Flag(ident)),
        }
    }

    fn parse_list(&mut self) -> Option<Vec<CfgExpr>> {
        let mut items = vec![self.parse_expr()?];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.next(); // consume ,
                         // allow trailing comma before )
            if matches!(self.peek(), Some(Tok::RParen)) {
                break;
            }
            items.push(self.parse_expr()?);
        }
        Some(items)
    }

    fn expect(&mut self, t: Tok) -> Option<()> {
        if self.peek() == Some(&t) {
            self.next();
            Some(())
        } else {
            None
        }
    }
}

/// Resolve the active feature set = transitive closure of the `default` feature,
/// following only edges that enable *local* features (plain names present as
/// keys in `[features]`). Edges of the form `dep:x`, `x/y`, and `x?/y` enable
/// dependencies or dependency features, not local features, so they are ignored
/// for scope purposes.
pub fn resolve_default_features(cargo_toml: &str) -> Option<HashSet<String>> {
    // Unparseable manifest → features unknown (None), not "no features".
    let value = cargo_toml.parse::<toml::Value>().ok()?;
    let Some(features) = value.get("features").and_then(|f| f.as_table()) else {
        // Parsed, but no `[features]` table → known-empty (every `feature = "x"`
        // is inactive because no feature is declared).
        return Some(HashSet::new());
    };

    // feature -> list of edge strings
    let mut edges: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (name, deps) in features {
        let list = deps
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        edges.insert(name.clone(), list);
    }

    let mut active = HashSet::new();
    let mut work: Vec<String> = edges.get("default").cloned().unwrap_or_default();
    while let Some(feat) = work.pop() {
        // Only local features (no `:` or `/`) can be activated as scope-relevant.
        if feat.contains(':') || feat.contains('/') {
            continue;
        }
        if !edges.contains_key(&feat) {
            // Not a declared feature (e.g. an implicit optional-dep feature) —
            // still record it; a `feature = "x"` cfg may reference it.
            active.insert(feat);
            continue;
        }
        if active.insert(feat.clone()) {
            if let Some(next) = edges.get(&feat) {
                work.extend(next.iter().cloned());
            }
        }
    }
    Some(active)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(features: &[&str]) -> CfgConfig {
        CfgConfig {
            features: Some(features.iter().map(|s| s.to_string()).collect()),
            verus_keep_ghost: true,
        }
    }

    #[test]
    fn test_parse_and_eval_basic() {
        let c = cfg(&["alloc", "digest"]);
        assert_eq!(
            c.eval(&parse_cfg(r#"feature = "alloc""#).unwrap()),
            Some(true)
        );
        assert_eq!(
            c.eval(&parse_cfg(r#"feature = "serde""#).unwrap()),
            Some(false)
        );
        assert_eq!(c.eval(&parse_cfg("verus_keep_ghost").unwrap()), Some(true));
        assert_eq!(
            c.eval(&parse_cfg("not(verus_keep_ghost)").unwrap()),
            Some(false)
        );
        assert_eq!(c.eval(&parse_cfg("test").unwrap()), Some(false));
    }

    #[test]
    fn test_all_any_not() {
        let c = cfg(&["alloc"]);
        // all: any false ⟹ false (inactive)
        assert!(c.is_inactive(r#"all(feature = "alloc", feature = "serde")"#));
        // all of active ⟹ active (not inactive)
        assert!(!c.is_inactive(r#"all(feature = "alloc", verus_keep_ghost)"#));
        // any: all false ⟹ false (inactive)
        assert!(c.is_inactive(r#"any(feature = "serde", feature = "group")"#));
        // any with one active ⟹ active
        assert!(!c.is_inactive(r#"any(feature = "serde", feature = "alloc")"#));
        // non-verus digest variant: all(digest, not(verus_keep_ghost)) ⟹ inactive
        assert!(c.is_inactive(r#"all(feature = "digest", not(verus_keep_ghost))"#));
    }

    #[test]
    fn test_unknown_is_undecidable_not_inactive() {
        let c = cfg(&["alloc"]);
        // Unknown target key ⟹ None ⟹ not treated as inactive (conservative).
        assert_eq!(
            c.eval(&parse_cfg(r#"curve25519_dalek_backend = "simd""#).unwrap()),
            None
        );
        assert!(!c.is_inactive(r#"curve25519_dalek_backend = "simd""#));
        assert_eq!(c.eval(&parse_cfg("nightly").unwrap()), None);
        // all(unknown, nightly) ⟹ None ⟹ not inactive
        assert!(!c.is_inactive(r#"all(curve25519_dalek_backend = "simd", nightly)"#));
    }

    #[test]
    fn test_malformed_is_not_inactive() {
        let c = cfg(&["alloc"]);
        assert!(!c.is_inactive("all(feature ="));
        assert!(parse_cfg("").is_none());
    }

    #[test]
    fn test_resolve_default_features() {
        let toml = r#"
[package]
name = "x"
version = "0.1.0"

[features]
default = ["alloc", "precomputed-tables", "zeroize", "lizard"]
alloc = ["zeroize?/alloc"]
precomputed-tables = []
zeroize = []
digest = ["dep:digest", "dep:sha2"]
lizard = ["digest"]
group = ["dep:group", "rand_core"]
"#;
        let active = resolve_default_features(toml).unwrap();
        assert!(active.contains("alloc"));
        assert!(active.contains("precomputed-tables"));
        assert!(active.contains("zeroize"));
        assert!(active.contains("lizard"));
        assert!(active.contains("digest")); // via lizard
        assert!(!active.contains("group")); // not default
        assert!(!active.contains("rand_core")); // only via group
        assert!(!active.contains("serde"));
    }

    #[test]
    fn test_features_known_empty_vs_unknown() {
        // Parsed manifest with no [features] → known-empty → feature = "x" is false.
        let known_empty = resolve_default_features("[package]\nname=\"x\"\nversion=\"0.1.0\"");
        assert_eq!(known_empty, Some(HashSet::new()));
        // Unparseable manifest → unknown (None).
        assert_eq!(resolve_default_features("this is not : valid toml ["), None);

        // Unknown features ⟹ feature predicates are undecidable (not false), so a
        // feature-gated atom is never marked out of scope on a guess (P26).
        let unknown = CfgConfig {
            features: None,
            verus_keep_ghost: true,
        };
        assert_eq!(
            unknown.eval(&parse_cfg(r#"feature = "alloc""#).unwrap()),
            None
        );
        assert!(!unknown.is_inactive(r#"feature = "serde""#));

        // Known-empty ⟹ feature = "x" is definitively inactive.
        let empty = CfgConfig {
            features: Some(HashSet::new()),
            verus_keep_ghost: true,
        };
        assert!(empty.is_inactive(r#"feature = "serde""#));
    }
}
