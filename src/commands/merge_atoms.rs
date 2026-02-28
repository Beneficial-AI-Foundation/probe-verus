//! Merge-atoms command - Combine independently-indexed atoms.json files.
//!
//! Replaces stub atoms (external function placeholders) with real atoms
//! from other indexed projects, enabling cross-project call graphs without
//! requiring a single combined workspace.

use probe_verus::{normalize_code_name, AtomWithLines};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A stub is an atom with no source: empty code_path and zero-length span.
fn is_stub(atom: &AtomWithLines) -> bool {
    atom.code_path.is_empty() && atom.code_text.lines_start == 0 && atom.code_text.lines_end == 0
}

/// Normalize all keys and dependency references in an atoms map.
/// Returns the normalized map and a count of keys that were changed.
fn normalize_atoms_map(
    atoms: BTreeMap<String, AtomWithLines>,
) -> (BTreeMap<String, AtomWithLines>, usize) {
    let mut normalized = BTreeMap::new();
    let mut changed = 0;

    for (key, mut atom) in atoms {
        let norm_key = normalize_code_name(&key);
        if norm_key != key {
            changed += 1;
        }
        atom.code_name = norm_key.clone();
        atom.dependencies = atom
            .dependencies
            .into_iter()
            .map(|d| normalize_code_name(&d))
            .collect();
        atom.dependencies_with_locations
            .iter_mut()
            .for_each(|d| d.code_name = normalize_code_name(&d.code_name));
        if let Some(existing) = normalized.get(&norm_key) {
            if is_stub(existing) && !is_stub(&atom) {
                normalized.insert(norm_key, atom);
            }
        } else {
            normalized.insert(norm_key, atom);
        }
    }

    (normalized, changed)
}

/// Load an atoms.json file into a BTreeMap, reconstructing code_name fields
/// from the dictionary keys (since code_name is skip_serializing).
fn load_atoms_file(path: &PathBuf) -> Result<BTreeMap<String, AtomWithLines>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let mut atoms: BTreeMap<String, AtomWithLines> = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

    for (key, atom) in atoms.iter_mut() {
        atom.code_name = key.clone();
    }

    Ok(atoms)
}

/// Merge result statistics.
pub struct MergeStats {
    pub total_atoms: usize,
    pub stubs_replaced: usize,
    pub stubs_remaining: usize,
    pub atoms_added: usize,
    pub keys_normalized: usize,
    pub conflicts: usize,
}

/// Merge multiple atoms maps into one.
///
/// The first map is the base. For each subsequent map:
/// - Stubs in the base are replaced by real atoms from the incoming map
/// - New atoms (not in base) are added
/// - Real-vs-real conflicts keep the base version (first wins)
pub fn merge_atoms_maps(
    maps: Vec<BTreeMap<String, AtomWithLines>>,
) -> (BTreeMap<String, AtomWithLines>, MergeStats) {
    let mut stats = MergeStats {
        total_atoms: 0,
        stubs_replaced: 0,
        stubs_remaining: 0,
        atoms_added: 0,
        keys_normalized: 0,
        conflicts: 0,
    };

    let mut maps_iter = maps.into_iter();
    let first = maps_iter.next().unwrap_or_default();
    let (mut base, norm_count) = normalize_atoms_map(first);
    stats.keys_normalized += norm_count;

    for incoming in maps_iter {
        let (incoming, norm_count) = normalize_atoms_map(incoming);
        stats.keys_normalized += norm_count;

        for (key, incoming_atom) in incoming {
            match base.get(&key) {
                Some(existing) if is_stub(existing) && !is_stub(&incoming_atom) => {
                    base.insert(key, incoming_atom);
                    stats.stubs_replaced += 1;
                }
                Some(existing) if !is_stub(existing) && !is_stub(&incoming_atom) => {
                    stats.conflicts += 1;
                    eprintln!(
                        "  Warning: conflict for '{}' (keeping base version from {})",
                        key, existing.code_path
                    );
                }
                Some(_) => {
                    // Both stubs or incoming is stub -- keep base
                }
                None => {
                    base.insert(key, incoming_atom);
                    stats.atoms_added += 1;
                }
            }
        }
    }

    stats.stubs_remaining = base.values().filter(|a| is_stub(a)).count();
    stats.total_atoms = base.len();

    (base, stats)
}

/// Execute the merge-atoms command.
pub fn cmd_merge_atoms(inputs: Vec<PathBuf>, output: PathBuf) {
    println!("═══════════════════════════════════════════════════════════");
    println!("  Probe Verus - Merge Atoms: Combine Indexed Projects");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    if inputs.len() < 2 {
        eprintln!("Error: merge-atoms requires at least 2 input files");
        std::process::exit(1);
    }

    let mut maps = Vec::new();
    for path in &inputs {
        println!("  Loading {}...", path.display());
        match load_atoms_file(path) {
            Ok(atoms) => {
                println!("    {} atoms loaded", atoms.len());
                maps.push(atoms);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
    println!();

    println!("Merging {} files...", inputs.len());
    let (merged, stats) = merge_atoms_maps(maps);

    let json = serde_json::to_string_pretty(&merged).expect("Failed to serialize JSON");
    std::fs::write(&output, &json).expect("Failed to write output file");

    println!();
    println!("═══════════════════════════════════════════════════════════");
    println!("  Merge complete");
    println!("═══════════════════════════════════════════════════════════");
    println!();
    println!("Output: {}", output.display());
    println!("  Total atoms:      {}", stats.total_atoms);
    println!("  Stubs replaced:   {}", stats.stubs_replaced);
    println!("  Stubs remaining:  {}", stats.stubs_remaining);
    println!("  New atoms added:  {}", stats.atoms_added);
    if stats.keys_normalized > 0 {
        println!("  Keys normalized:  {}", stats.keys_normalized);
    }
    if stats.conflicts > 0 {
        println!("  Conflicts (kept base): {}", stats.conflicts);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_verus::{CodeTextInfo, FunctionMode};
    use std::collections::BTreeSet;

    fn make_real_atom(name: &str, code_name: &str, code_path: &str) -> AtomWithLines {
        AtomWithLines {
            display_name: name.to_string(),
            code_name: code_name.to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: code_path.to_string(),
            code_text: CodeTextInfo {
                lines_start: 10,
                lines_end: 20,
            },
            mode: FunctionMode::Exec,
        }
    }

    fn make_stub(name: &str, code_name: &str) -> AtomWithLines {
        AtomWithLines {
            display_name: name.to_string(),
            code_name: code_name.to_string(),
            dependencies: BTreeSet::new(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: String::new(),
            code_text: CodeTextInfo {
                lines_start: 0,
                lines_end: 0,
            },
            mode: FunctionMode::Exec,
        }
    }

    #[test]
    fn test_merge_replaces_stubs() {
        let mut base = BTreeMap::new();
        let mut caller = make_real_atom("caller", "probe:crate-a/1.0/mod/caller()", "src/lib.rs");
        caller
            .dependencies
            .insert("probe:crate-b/1.0/mod/helper()".to_string());
        base.insert("probe:crate-a/1.0/mod/caller()".to_string(), caller);
        base.insert(
            "probe:crate-b/1.0/mod/helper()".to_string(),
            make_stub("helper", "probe:crate-b/1.0/mod/helper()"),
        );

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "probe:crate-b/1.0/mod/helper()".to_string(),
            make_real_atom("helper", "probe:crate-b/1.0/mod/helper()", "src/lib.rs"),
        );

        let (merged, stats) = merge_atoms_maps(vec![base, incoming]);

        assert_eq!(stats.stubs_replaced, 1);
        assert_eq!(stats.stubs_remaining, 0);
        let helper = merged.get("probe:crate-b/1.0/mod/helper()").unwrap();
        assert_eq!(helper.code_path, "src/lib.rs");
        assert_eq!(helper.code_text.lines_start, 10);
    }

    #[test]
    fn test_merge_adds_new_atoms() {
        let mut base = BTreeMap::new();
        base.insert(
            "probe:crate-a/1.0/mod/foo()".to_string(),
            make_real_atom("foo", "probe:crate-a/1.0/mod/foo()", "src/lib.rs"),
        );

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "probe:crate-b/1.0/mod/bar()".to_string(),
            make_real_atom("bar", "probe:crate-b/1.0/mod/bar()", "src/bar.rs"),
        );

        let (merged, stats) = merge_atoms_maps(vec![base, incoming]);

        assert_eq!(stats.atoms_added, 1);
        assert_eq!(merged.len(), 2);
        assert!(merged.contains_key("probe:crate-b/1.0/mod/bar()"));
    }

    #[test]
    fn test_merge_normalizes_trailing_dot() {
        let mut base = BTreeMap::new();
        // Base has a stub with trailing dot (old format)
        base.insert(
            "probe:crate-b/1.0/mod/helper().".to_string(),
            make_stub("helper", "probe:crate-b/1.0/mod/helper()."),
        );

        let mut incoming = BTreeMap::new();
        // Incoming has a real atom without trailing dot (correct format)
        incoming.insert(
            "probe:crate-b/1.0/mod/helper()".to_string(),
            make_real_atom("helper", "probe:crate-b/1.0/mod/helper()", "src/lib.rs"),
        );

        let (merged, stats) = merge_atoms_maps(vec![base, incoming]);

        assert_eq!(stats.keys_normalized, 1);
        assert_eq!(stats.stubs_replaced, 1);
        let helper = merged.get("probe:crate-b/1.0/mod/helper()").unwrap();
        assert_eq!(helper.code_path, "src/lib.rs");
        assert!(!merged.contains_key("probe:crate-b/1.0/mod/helper()."));
    }

    #[test]
    fn test_merge_updates_dependency_refs() {
        let mut base = BTreeMap::new();
        let mut caller = make_real_atom("caller", "probe:crate-a/1.0/mod/caller()", "src/lib.rs");
        // Dependency has trailing dot (old format)
        caller
            .dependencies
            .insert("probe:crate-b/1.0/mod/helper().".to_string());
        base.insert("probe:crate-a/1.0/mod/caller()".to_string(), caller);

        let (merged, _stats) = merge_atoms_maps(vec![base]);

        let caller = merged.get("probe:crate-a/1.0/mod/caller()").unwrap();
        assert!(caller
            .dependencies
            .contains("probe:crate-b/1.0/mod/helper()"));
        assert!(!caller
            .dependencies
            .contains("probe:crate-b/1.0/mod/helper()."));
    }

    #[test]
    fn test_merge_unmatched_stubs_remain() {
        let mut base = BTreeMap::new();
        base.insert(
            "probe:crate-a/1.0/mod/foo()".to_string(),
            make_real_atom("foo", "probe:crate-a/1.0/mod/foo()", "src/lib.rs"),
        );
        base.insert(
            "probe:external/1.0/mod/unknown()".to_string(),
            make_stub("unknown", "probe:external/1.0/mod/unknown()"),
        );

        let incoming = BTreeMap::new(); // empty

        let (merged, stats) = merge_atoms_maps(vec![base, incoming]);

        assert_eq!(stats.stubs_remaining, 1);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_merge_multiple_files() {
        let mut map_a = BTreeMap::new();
        let mut caller = make_real_atom("caller", "probe:a/1.0/caller()", "a/src/lib.rs");
        caller
            .dependencies
            .insert("probe:b/1.0/helper()".to_string());
        caller.dependencies.insert("probe:c/1.0/util()".to_string());
        map_a.insert("probe:a/1.0/caller()".to_string(), caller);
        map_a.insert(
            "probe:b/1.0/helper()".to_string(),
            make_stub("helper", "probe:b/1.0/helper()"),
        );
        map_a.insert(
            "probe:c/1.0/util()".to_string(),
            make_stub("util", "probe:c/1.0/util()"),
        );

        let mut map_b = BTreeMap::new();
        map_b.insert(
            "probe:b/1.0/helper()".to_string(),
            make_real_atom("helper", "probe:b/1.0/helper()", "b/src/lib.rs"),
        );

        let mut map_c = BTreeMap::new();
        map_c.insert(
            "probe:c/1.0/util()".to_string(),
            make_real_atom("util", "probe:c/1.0/util()", "c/src/lib.rs"),
        );

        let (merged, stats) = merge_atoms_maps(vec![map_a, map_b, map_c]);

        assert_eq!(stats.stubs_replaced, 2);
        assert_eq!(stats.stubs_remaining, 0);
        assert_eq!(merged.len(), 3);
        assert_eq!(
            merged.get("probe:b/1.0/helper()").unwrap().code_path,
            "b/src/lib.rs"
        );
        assert_eq!(
            merged.get("probe:c/1.0/util()").unwrap().code_path,
            "c/src/lib.rs"
        );
    }

    #[test]
    fn test_is_stub() {
        let stub = make_stub("f", "probe:c/1.0/f()");
        assert!(is_stub(&stub));

        let real = make_real_atom("f", "probe:c/1.0/f()", "src/lib.rs");
        assert!(!is_stub(&real));
    }
}
