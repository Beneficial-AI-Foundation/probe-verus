//! Callee-crates command - Find which crates a function's callees belong to.
//!
//! Given a function and a depth N, traverses the call graph (BFS) up to
//! depth N and reports which crates the discovered callees belong to,
//! grouped by crate name and version.

use probe_verus::AtomWithLines;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::io::Read;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct CalleeCratesOutput {
    pub function: String,
    pub depth: usize,
    pub crates: Vec<CrateEntry>,
}

#[derive(Serialize)]
pub struct CrateEntry {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub version: String,
    pub functions: Vec<String>,
}

/// Extract (crate_name, version) from a `probe:` code-name.
///
/// Standard library crates use a GitHub URL instead of a semver version
/// (e.g. `probe:core/https://github.com/rust-lang/rust/...`), so we
/// return `"stdlib"` for those.
pub fn extract_crate_info(code_name: &str) -> Option<(&str, &str)> {
    let rest = code_name.strip_prefix("probe:")?;
    let mut parts = rest.splitn(3, '/');
    let crate_name = parts.next()?;
    let version = parts.next()?;
    if version.starts_with("https:") {
        Some((crate_name, "stdlib"))
    } else {
        Some((crate_name, version))
    }
}

/// BFS traversal collecting all callees reachable within depth 1..=max_depth.
/// Returns the set of callee code-names (excluding the root function itself).
pub fn collect_callees_up_to_depth(
    atoms: &BTreeMap<String, AtomWithLines>,
    root: &str,
    max_depth: usize,
) -> BTreeSet<String> {
    let mut visited = HashSet::new();
    visited.insert(root.to_string());

    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((root.to_string(), 0));

    let mut result = BTreeSet::new();

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        if let Some(atom) = atoms.get(&current) {
            for dep in &atom.dependencies {
                if visited.insert(dep.clone()) {
                    result.insert(dep.clone());
                    queue.push_back((dep.clone(), depth + 1));
                }
            }
        }
    }

    result
}

/// Group a set of code-names by (crate, version), returning sorted CrateEntry list.
pub fn group_by_crate(code_names: &BTreeSet<String>) -> Vec<CrateEntry> {
    let mut groups: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();

    for name in code_names {
        if let Some((crate_name, version)) = extract_crate_info(name) {
            groups
                .entry((crate_name.to_string(), version.to_string()))
                .or_default()
                .insert(name.clone());
        }
    }

    groups
        .into_iter()
        .map(|((crate_name, version), functions)| CrateEntry {
            crate_name,
            version,
            functions: functions.into_iter().collect(),
        })
        .collect()
}

/// Resolve a function argument to a code-name key in the atoms map.
///
/// If the argument starts with `probe:`, it is used as-is.
/// Otherwise, search for keys whose display-name matches the argument.
fn resolve_function(
    atoms: &BTreeMap<String, AtomWithLines>,
    function_arg: &str,
) -> Result<String, String> {
    if function_arg.starts_with("probe:") {
        if atoms.contains_key(function_arg) {
            return Ok(function_arg.to_string());
        }
        return Err(format!(
            "Function '{}' not found in atoms data",
            function_arg
        ));
    }

    let matches: Vec<&String> = atoms
        .iter()
        .filter(|(_, atom)| atom.display_name == function_arg)
        .map(|(key, _)| key)
        .collect();

    match matches.len() {
        0 => {
            let partial: Vec<&String> = atoms
                .iter()
                .filter(|(key, atom)| {
                    atom.display_name.contains(function_arg) || key.contains(function_arg)
                })
                .map(|(key, _)| key)
                .collect();
            if partial.len() == 1 {
                return Ok(partial[0].clone());
            }
            if partial.is_empty() {
                Err(format!(
                    "No function matching '{}' found in atoms data",
                    function_arg
                ))
            } else {
                let mut msg = format!(
                    "Ambiguous function '{}'. {} matches found:\n",
                    function_arg,
                    partial.len()
                );
                for (i, key) in partial.iter().enumerate().take(10) {
                    msg.push_str(&format!("  {}. {}\n", i + 1, key));
                }
                if partial.len() > 10 {
                    msg.push_str(&format!("  ... and {} more\n", partial.len() - 10));
                }
                Err(msg)
            }
        }
        1 => Ok(matches[0].clone()),
        _ => {
            let mut msg = format!(
                "Ambiguous display-name '{}'. {} matches found:\n",
                function_arg,
                matches.len()
            );
            for (i, key) in matches.iter().enumerate().take(10) {
                msg.push_str(&format!("  {}. {}\n", i + 1, key));
            }
            if matches.len() > 10 {
                msg.push_str(&format!("  ... and {} more\n", matches.len() - 10));
            }
            Err(msg)
        }
    }
}

/// Load atoms from a file or stdin.
fn load_atoms(atoms_file: Option<PathBuf>) -> Result<BTreeMap<String, AtomWithLines>, String> {
    let content = match atoms_file {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?,
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            buf
        }
    };

    let atoms: BTreeMap<String, AtomWithLines> =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse atoms JSON: {}", e))?;

    Ok(atoms)
}

/// Execute the callee-crates command.
pub fn cmd_callee_crates(
    function: String,
    depth: usize,
    atoms_file: Option<PathBuf>,
    output: Option<PathBuf>,
) {
    let atoms = match load_atoms(atoms_file) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let resolved = match resolve_function(&atoms, &function) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    let callees = collect_callees_up_to_depth(&atoms, &resolved, depth);
    let crates = group_by_crate(&callees);

    let output_data = CalleeCratesOutput {
        function: resolved,
        depth,
        crates,
    };

    let json = serde_json::to_string_pretty(&output_data).expect("Failed to serialize JSON");

    match output {
        Some(path) => {
            std::fs::write(&path, &json).expect("Failed to write output file");
            eprintln!("Output written to {}", path.display());
        }
        None => {
            println!("{}", json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_verus::{CodeTextInfo, FunctionMode};

    fn make_atom(name: &str, code_path: &str, deps: &[&str], mode: FunctionMode) -> AtomWithLines {
        AtomWithLines {
            display_name: name.to_string(),
            code_name: String::new(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            dependencies_with_locations: Vec::new(),
            code_module: String::new(),
            code_path: code_path.to_string(),
            code_text: CodeTextInfo {
                lines_start: 1,
                lines_end: 10,
            },
            mode,
        }
    }

    fn make_stub(name: &str, deps: &[&str]) -> AtomWithLines {
        make_atom(name, "", deps, FunctionMode::Exec)
    }

    #[test]
    fn test_extract_crate_info() {
        let (name, ver) =
            extract_crate_info("probe:curve25519-dalek/4.1.3/scalar/Scalar#add()").unwrap();
        assert_eq!(name, "curve25519-dalek");
        assert_eq!(ver, "4.1.3");

        let (name, ver) =
            extract_crate_info("probe:vstd/0.0.0-2026-01-11-0057/mul/arithmetic/f()").unwrap();
        assert_eq!(name, "vstd");
        assert_eq!(ver, "0.0.0-2026-01-11-0057");

        let (name, ver) = extract_crate_info(
            "probe:core/https://github.com/rust-lang/rust/library/core/result/impl#foo()",
        )
        .unwrap();
        assert_eq!(name, "core");
        assert_eq!(ver, "stdlib");

        let (name, ver) = extract_crate_info(
            "probe:alloc/https://github.com/rust-lang/rust/library/alloc/vec/impl#bar()",
        )
        .unwrap();
        assert_eq!(name, "alloc");
        assert_eq!(ver, "stdlib");

        assert!(extract_crate_info("not-a-probe-name").is_none());
        assert!(extract_crate_info("probe:").is_none());
        assert!(extract_crate_info("probe:crate-only").is_none());
    }

    #[test]
    fn test_collect_callees_depth_1() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/f()".to_string(),
            make_atom(
                "f",
                "src/lib.rs",
                &["probe:a/1.0/mod/g()", "probe:b/1.0/mod/h()"],
                FunctionMode::Exec,
            ),
        );
        atoms.insert(
            "probe:a/1.0/mod/g()".to_string(),
            make_atom(
                "g",
                "src/lib.rs",
                &["probe:c/1.0/mod/i()"],
                FunctionMode::Proof,
            ),
        );
        atoms.insert("probe:b/1.0/mod/h()".to_string(), make_stub("h", &[]));
        atoms.insert("probe:c/1.0/mod/i()".to_string(), make_stub("i", &[]));

        let callees = collect_callees_up_to_depth(&atoms, "probe:a/1.0/mod/f()", 1);
        assert_eq!(callees.len(), 2);
        assert!(callees.contains("probe:a/1.0/mod/g()"));
        assert!(callees.contains("probe:b/1.0/mod/h()"));
        assert!(!callees.contains("probe:c/1.0/mod/i()"));
    }

    #[test]
    fn test_collect_callees_depth_2() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/f()".to_string(),
            make_atom(
                "f",
                "src/lib.rs",
                &["probe:a/1.0/mod/g()", "probe:b/1.0/mod/h()"],
                FunctionMode::Exec,
            ),
        );
        atoms.insert(
            "probe:a/1.0/mod/g()".to_string(),
            make_atom(
                "g",
                "src/lib.rs",
                &["probe:c/1.0/mod/i()"],
                FunctionMode::Proof,
            ),
        );
        atoms.insert("probe:b/1.0/mod/h()".to_string(), make_stub("h", &[]));
        atoms.insert("probe:c/1.0/mod/i()".to_string(), make_stub("i", &[]));

        let callees = collect_callees_up_to_depth(&atoms, "probe:a/1.0/mod/f()", 2);
        assert_eq!(callees.len(), 3);
        assert!(callees.contains("probe:a/1.0/mod/g()"));
        assert!(callees.contains("probe:b/1.0/mod/h()"));
        assert!(callees.contains("probe:c/1.0/mod/i()"));
    }

    #[test]
    fn test_collect_callees_handles_cycles() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/f()".to_string(),
            make_atom(
                "f",
                "src/lib.rs",
                &["probe:a/1.0/mod/g()"],
                FunctionMode::Exec,
            ),
        );
        atoms.insert(
            "probe:a/1.0/mod/g()".to_string(),
            make_atom(
                "g",
                "src/lib.rs",
                &["probe:a/1.0/mod/f()"],
                FunctionMode::Exec,
            ),
        );

        let callees = collect_callees_up_to_depth(&atoms, "probe:a/1.0/mod/f()", 10);
        assert_eq!(callees.len(), 1);
        assert!(callees.contains("probe:a/1.0/mod/g()"));
    }

    #[test]
    fn test_collect_callees_depth_0() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/f()".to_string(),
            make_atom(
                "f",
                "src/lib.rs",
                &["probe:a/1.0/mod/g()"],
                FunctionMode::Exec,
            ),
        );

        let callees = collect_callees_up_to_depth(&atoms, "probe:a/1.0/mod/f()", 0);
        assert!(callees.is_empty());
    }

    #[test]
    fn test_group_by_crate() {
        let mut names = BTreeSet::new();
        names.insert("probe:vstd/0.1.0/mod/f()".to_string());
        names.insert("probe:vstd/0.1.0/mod/g()".to_string());
        names.insert("probe:dalek/4.1.3/scalar/add()".to_string());

        let groups = group_by_crate(&names);
        assert_eq!(groups.len(), 2);

        assert_eq!(groups[0].crate_name, "dalek");
        assert_eq!(groups[0].version, "4.1.3");
        assert_eq!(groups[0].functions.len(), 1);

        assert_eq!(groups[1].crate_name, "vstd");
        assert_eq!(groups[1].version, "0.1.0");
        assert_eq!(groups[1].functions.len(), 2);
    }

    #[test]
    fn test_resolve_function_exact_code_name() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/f()".to_string(),
            make_atom("f", "src/lib.rs", &[], FunctionMode::Exec),
        );

        let resolved = resolve_function(&atoms, "probe:a/1.0/mod/f()").unwrap();
        assert_eq!(resolved, "probe:a/1.0/mod/f()");
    }

    #[test]
    fn test_resolve_function_by_display_name() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/my_func()".to_string(),
            make_atom("my_func", "src/lib.rs", &[], FunctionMode::Exec),
        );

        let resolved = resolve_function(&atoms, "my_func").unwrap();
        assert_eq!(resolved, "probe:a/1.0/mod/my_func()");
    }

    #[test]
    fn test_resolve_function_not_found() {
        let atoms = BTreeMap::new();
        assert!(resolve_function(&atoms, "nonexistent").is_err());
    }

    #[test]
    fn test_resolve_function_ambiguous() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/f()".to_string(),
            make_atom("f", "src/a.rs", &[], FunctionMode::Exec),
        );
        atoms.insert(
            "probe:b/1.0/mod/f()".to_string(),
            make_atom("f", "src/b.rs", &[], FunctionMode::Exec),
        );

        assert!(resolve_function(&atoms, "f").is_err());
    }

    #[test]
    fn test_resolve_function_partial_match_unique() {
        let mut atoms = BTreeMap::new();
        atoms.insert(
            "probe:a/1.0/mod/unique_name()".to_string(),
            make_atom("unique_name", "src/lib.rs", &[], FunctionMode::Exec),
        );
        atoms.insert(
            "probe:a/1.0/mod/other()".to_string(),
            make_atom("other", "src/lib.rs", &[], FunctionMode::Exec),
        );

        let resolved = resolve_function(&atoms, "unique").unwrap();
        assert_eq!(resolved, "probe:a/1.0/mod/unique_name()");
    }
}
