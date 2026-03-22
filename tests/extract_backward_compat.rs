//! Backward compatibility test for the extract command.
//!
//! Runs `cmd_extract` on the `verus_micro` fixture and compares the output
//! against a committed golden JSON file. The test enforces that:
//! - All fields present in the golden file still exist (Missing = FAIL)
//! - All values match (ValueMismatch / TypeMismatch = FAIL)
//! - New fields are allowed (Extra = INFO, printed but passes)
//!
//! Volatile envelope fields (timestamp, commit, repo, tool version) are
//! automatically ignored by `probe_extract_check::golden::compare`.
//!
//! ## Bless mode
//!
//! To update the golden file after intentional changes:
//! ```text
//! BLESS=1 cargo test --test extract_backward_compat -- --nocapture
//! ```

use probe_extract_check::golden::{compare, DiffKind};
use std::path::{Path, PathBuf};

const VERUS_MICRO: &str = "../probe/probe-extract-check/tests/fixtures/verus_micro";
const GOLDEN_FILE: &str = "tests/fixtures/extract_golden/golden.json";

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn find_unified_output(dir: &Path) -> Option<PathBuf> {
    let suffixes = ["_atoms.json", "_specs.json", "_proofs.json"];
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| {
                    n.starts_with("verus_")
                        && n.ends_with(".json")
                        && !suffixes.iter().any(|s| n.ends_with(s))
                })
        })
        .map(|e| e.path())
}

fn tools_available() -> bool {
    use probe_verus::tool_manager::{resolve_tool, Tool};
    resolve_tool(Tool::VerusAnalyzer).is_ok() && resolve_tool(Tool::Scip).is_ok()
}

#[test]
fn extract_backward_compat() {
    let fixture = Path::new(VERUS_MICRO);
    if !fixture.exists() {
        eprintln!(
            "SKIP: verus_micro fixture not found at {}. \
             Clone the probe repo as a sibling to run this test.",
            fixture.display()
        );
        return;
    }

    if !tools_available() {
        eprintln!(
            "SKIP: verus-analyzer or scip not found. \
             Run `probe-verus setup` or install them to run this test."
        );
        return;
    }

    let project_dir = tempfile::tempdir().unwrap();
    copy_dir_recursive(fixture, project_dir.path());

    let output_dir = tempfile::tempdir().unwrap();
    let project_path = project_dir.path().to_path_buf();
    probe_verus::commands::cmd_extract(
        project_path.clone(),
        output_dir.path().to_path_buf(),
        false,  // skip_atomize
        false,  // skip_specify
        true,   // skip_verify
        None,   // package
        true,   // regenerate_scip
        false,  // verbose
        false,  // use_rust_analyzer
        false,  // allow_duplicates
        false,  // auto_install
        None,   // with_atoms
        false,  // _with_spec_text
        None,   // taxonomy_config
        vec![], // verus_args
        false,  // separate_outputs
    )
    .expect("cmd_extract failed");

    let probes_dir = project_path.join(".verilib").join("probes");
    let actual_path = find_unified_output(&probes_dir)
        .unwrap_or_else(|| panic!("no unified output found in {}", probes_dir.display()));
    let actual_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&actual_path).unwrap())
            .expect("failed to parse extract output");

    if std::env::var("BLESS").is_ok() {
        let golden_path = Path::new(GOLDEN_FILE);
        if let Some(parent) = golden_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let pretty = serde_json::to_string_pretty(&actual_json).unwrap();
        std::fs::write(golden_path, format!("{pretty}\n")).unwrap();
        eprintln!("BLESSED: wrote golden file to {}", golden_path.display());
        return;
    }

    let golden_path = Path::new(GOLDEN_FILE);
    if !golden_path.exists() {
        panic!(
            "Golden file not found at {}. Run with BLESS=1 to generate it:\n  \
             BLESS=1 cargo test --test extract_backward_compat -- --nocapture",
            golden_path.display()
        );
    }
    let golden_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(golden_path).unwrap())
            .expect("failed to parse golden file");

    let diffs = compare(&golden_json, &actual_json);

    let mut breaking = Vec::new();
    let mut extras = Vec::new();
    for diff in &diffs {
        match &diff.kind {
            DiffKind::Extra => extras.push(diff),
            _ => breaking.push(diff),
        }
    }

    if !extras.is_empty() {
        eprintln!(
            "INFO: {} new field(s) in extract output (non-breaking):",
            extras.len()
        );
        for d in &extras {
            eprintln!("  {d}");
        }
    }

    if !breaking.is_empty() {
        eprintln!(
            "BACKWARD COMPAT FAILURE: {} breaking change(s) detected:",
            breaking.len()
        );
        for d in &breaking {
            eprintln!("  {d}");
        }
        eprintln!(
            "\nIf these changes are intentional, update the golden file:\n  \
             BLESS=1 cargo test --test extract_backward_compat -- --nocapture"
        );
        panic!(
            "extract output has {} backward-incompatible change(s) vs golden file",
            breaking.len()
        );
    }
}
