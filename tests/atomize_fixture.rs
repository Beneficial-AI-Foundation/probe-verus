//! Atoms and call edges of the `verus_owners` fixture, from a fresh SCIP index.
//!
//! The fixture has the cases that the corpus counts but does not check one by
//! one: trait methods with and without a body, two traits with a method of the
//! same name implemented for one type, an inherent method of that name, calls
//! through each of them, and a pair of cfg alternatives.
//!
//! Needs verus-analyzer and scip (`probe-verus setup`); skips otherwise.

use std::collections::BTreeMap;
use std::path::Path;

const FIXTURE: &str = "tests/fixtures/verus_owners";
const PREFIX: &str = "probe:verus-owners/0.1.0/";

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn tools_available() -> bool {
    use probe_verus::tool_manager::{resolve_tool, Tool};
    resolve_tool(Tool::VerusAnalyzer).is_ok() && resolve_tool(Tool::Scip).is_ok()
}

/// Local atoms by code-name without the package prefix: (start line,
/// dependencies), and the envelope `indexer` field.
fn atomize_fixture() -> (BTreeMap<String, (u64, Vec<String>)>, serde_json::Value) {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(Path::new(FIXTURE), dir.path());
    let output = dir.path().join("atoms.json");
    probe_verus::commands::cmd_atomize(
        dir.path().to_path_buf(),
        Some(output.clone()),
        true,  // regenerate_scip
        false, // with_locations
        false, // use_rust_analyzer
        false, // allow_duplicates
        false, // auto_install
        false, // with_public_api
    )
    .expect("atomize failed");

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(output).unwrap()).unwrap();
    let short = |name: &str| name.strip_prefix(PREFIX).unwrap_or(name).to_string();
    let atoms = json["data"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, atom)| atom["code-path"].as_str().is_some_and(|p| !p.is_empty()))
        .map(|(name, atom)| {
            let deps = atom["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| short(d.as_str().unwrap()))
                .collect();
            (
                short(name),
                (atom["code-text"]["lines-start"].as_u64().unwrap(), deps),
            )
        })
        .collect();
    (atoms, json["indexer"].clone())
}

#[test]
fn atomize_fixture_owners_and_edges() {
    if !tools_available() {
        eprintln!("SKIP: verus-analyzer or scip not found. Run `probe-verus setup`.");
        return;
    }
    let (atoms, indexer) = atomize_fixture();

    // The envelope records the producer of the index and the selected analyzer.
    assert_eq!(indexer["name"], "rust-analyzer", "indexer: {indexer}");
    assert!(
        indexer["version"]
            .as_str()
            .is_some_and(|v| v.starts_with("0.3.")),
        "indexer: {indexer}"
    );
    assert!(
        indexer["selection"]
            .as_str()
            .is_some_and(|s| s.starts_with("verus-analyzer ")),
        "indexer: {indexer}"
    );
    let start = |name: &str| {
        atoms
            .get(name)
            .unwrap_or_else(|| panic!("no atom {name}"))
            .0
    };
    let deps = |name: &str| &atoms[name].1;

    // Every declaration is an atom, at its own location.
    let expected = [
        ("A#a_spec()", 8), // bodyless spec fn in a trait
        ("A#f()", 10),     // bodyless fn in a trait
        ("A#twice()", 12), // default body in a trait
        ("B#f()", 18),     // same method name in another trait
        ("impl#[R][A]a_spec()", 22),
        ("impl#[R][A]f()", 26),
        ("impl#[R][B]f()", 32), // same Self type, other trait
        ("impl#[R]f()", 38),    // inherent method of the same name
        ("call_a()", 43),
        ("call_generic()", 47),
        ("call_inherent()", 51),
        ("alt()@55", 55), // cfg alternative that the analyzer skips
        ("alt()", 61),
    ];
    for (name, line) in expected {
        assert_eq!(start(name), line, "start line of {name}");
    }
    assert_eq!(atoms.len(), expected.len(), "atoms: {:?}", atoms.keys());

    // Required and forbidden edges.
    assert_eq!(deps("call_a()"), &vec!["impl#[R][A]f()".to_string()]);
    assert_eq!(deps("call_generic()"), &vec!["A#f()".to_string()]);
    assert_eq!(deps("call_inherent()"), &vec!["impl#[R]f()".to_string()]);
    assert_eq!(deps("A#twice()"), &vec!["A#f()".to_string()]);
    assert_eq!(deps("alt()"), &vec!["call_inherent()".to_string()]);
    for name in [
        "impl#[R][A]f()",
        "impl#[R][B]f()",
        "impl#[R]f()",
        "alt()@55",
    ] {
        assert!(
            deps(name).is_empty(),
            "{name} calls nothing: {:?}",
            deps(name)
        );
    }
}
