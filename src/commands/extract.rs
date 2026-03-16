//! Extract command - Unified pipeline: atomize + specify + run-verus.

use super::atomize::atomize_internal;
use super::run_verus::{run_verus_internal, VerifySummary};
use super::specify::specify_internal;
use probe_verus::metadata::{
    find_default_atoms_path, gather_metadata, get_default_output_path, unwrap_envelope,
    wrap_in_envelope, AtomizeInternalConfig, ExtractInternalConfig, ProjectMetadata,
    SpecifyInternalConfig,
};
use probe_verus::{
    split_clauses, AtomWithLines, CallLocation, SpecCondition, SpecConditionKind, UnifiedAtom,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct ExtractPipelineResult {
    status: String,
    atomize: Option<StepResult>,
    specify: Option<StepResult>,
    verify: Option<ExtractStepResult>,
}

#[derive(Serialize)]
struct StepResult {
    success: bool,
    output_file: String,
    total_functions: Option<usize>,
    error: Option<String>,
}

#[derive(Serialize)]
struct ExtractStepResult {
    success: bool,
    output_file: String,
    summary: Option<ExtractSummaryOutput>,
    error: Option<String>,
}

#[derive(Serialize, Clone)]
struct ExtractSummaryOutput {
    total_functions: usize,
    verified: usize,
    failed: usize,
    unverified: usize,
}

impl From<VerifySummary> for ExtractSummaryOutput {
    fn from(s: VerifySummary) -> Self {
        Self {
            total_functions: s.total_functions,
            verified: s.verified,
            failed: s.failed,
            unverified: s.unverified,
        }
    }
}

/// Execute the unified extract command.
///
/// Runs atomize, specify, and run-verus as a 3-step pipeline, then merges the
/// outputs into a single unified JSON file (schema `probe-verus/extract`).
#[allow(clippy::too_many_arguments)]
pub fn cmd_extract(
    project_path: PathBuf,
    output_dir: PathBuf,
    skip_atomize: bool,
    skip_specify: bool,
    skip_verify: bool,
    package: Option<String>,
    regenerate_scip: bool,
    verbose: bool,
    use_rust_analyzer: bool,
    allow_duplicates: bool,
    auto_install: bool,
    with_atoms: Option<PathBuf>,
    with_spec_text: bool,
    taxonomy_config: Option<PathBuf>,
    verus_args: Vec<String>,
    separate_outputs: bool,
) {
    if !project_path.exists() {
        eprintln!(
            "Error: Project path does not exist: {}",
            project_path.display()
        );
        std::process::exit(1);
    }

    let cargo_toml = project_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        eprintln!(
            "Error: Not a valid Rust project (Cargo.toml not found): {}",
            project_path.display()
        );
        std::process::exit(1);
    }

    let metadata = gather_metadata(&project_path);

    let atoms_path = get_default_output_path(&project_path, &metadata, "atoms");
    let specs_path = get_default_output_path(&project_path, &metadata, "specs");
    let results_path = get_default_output_path(&project_path, &metadata, "proofs");

    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        eprintln!("Error: Failed to create output directory: {}", e);
        std::process::exit(1);
    }

    print_header(&project_path, &output_dir, &package);

    let mut result = ExtractPipelineResult {
        status: "success".to_string(),
        atomize: None,
        specify: None,
        verify: None,
    };

    // === Step 1: Atomize ===
    if !skip_atomize {
        let config = AtomizeInternalConfig {
            project_path: &project_path,
            output: &atoms_path,
            regenerate_scip,
            verbose,
            use_rust_analyzer,
            allow_duplicates,
            auto_install,
            with_locations: true,
            metadata: &metadata,
        };
        run_atomize_step(&config, &mut result);
    }

    // Resolve the atoms path for subsequent steps: explicit --with-atoms > default from atomize > auto-discover
    let resolved_atoms = with_atoms
        .as_deref()
        .map(Path::to_path_buf)
        .or_else(|| {
            if atoms_path.exists() {
                Some(atoms_path.clone())
            } else {
                None
            }
        })
        .or_else(|| find_default_atoms_path(&project_path, &metadata));

    // === Step 2: Specify ===
    if !skip_specify {
        match &resolved_atoms {
            Some(ap) if ap.exists() => {
                let config = SpecifyInternalConfig {
                    path: &project_path,
                    output: &specs_path,
                    atoms_path: ap,
                    with_spec_text,
                    taxonomy_config_path: taxonomy_config.as_deref(),
                    taxonomy_explain: false,
                    metadata: &metadata,
                };
                run_specify_step(&config, &mut result);
            }
            _ => {
                if skip_atomize {
                    eprintln!(
                        "Error: specify requires atoms.json; provide --with-atoms or remove --skip-atomize"
                    );
                    result.status = "specify_failed".to_string();
                    result.specify = Some(StepResult {
                        success: false,
                        output_file: specs_path.display().to_string(),
                        total_functions: None,
                        error: Some("No atoms.json available; specify requires atoms".to_string()),
                    });
                } else {
                    eprintln!("  Warning: skipping specify (atomize did not produce atoms)");
                }
            }
        }
    }

    // === Step 3: Run-Verus (cargo verus) ===
    if !skip_verify {
        let config = ExtractInternalConfig {
            project_path: &project_path,
            output: &results_path,
            package: package.as_deref(),
            atoms_path: resolved_atoms.as_deref(),
            verbose,
            verus_args: &verus_args,
            metadata: &metadata,
        };
        run_verify_step(&config, &mut result);
    }

    // === Step 4: Merge into unified output ===
    // Only pass paths for steps that actually ran (skip_* means no new output for that step).
    let merge_specs = if skip_specify {
        None
    } else {
        Some(specs_path.as_path())
    };
    let merge_proofs = if skip_verify {
        None
    } else {
        Some(results_path.as_path())
    };
    let unified_path = run_unified_merge(
        &atoms_path,
        merge_specs,
        merge_proofs,
        &project_path,
        &metadata,
        separate_outputs,
        &result,
    );

    // === Summary ===
    print_summary(&result);
    if let Some(ref up) = unified_path {
        println!("  Primary output: {}", up.display());
        println!();
    }

    let summary_path = output_dir.join("extract_summary.json");
    let envelope = wrap_in_envelope("probe-verus/extract-summary", "extract", &result, &metadata);
    if let Ok(json) = serde_json::to_string_pretty(&envelope) {
        if let Err(e) = std::fs::write(&summary_path, &json) {
            eprintln!("Warning: Could not write summary: {}", e);
        }
    }

    let exit_code = match result.status.as_str() {
        "success" => 0,
        "verification_failed" => 0,
        _ => 1,
    };
    std::process::exit(exit_code);
}

fn print_header(project_path: &Path, output_dir: &Path, package: &Option<String>) {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  probe-verus extract");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("  Project: {}", project_path.display());
    println!("  Output:  {}", output_dir.display());
    if let Some(ref pkg) = package {
        println!("  Package: {}", pkg);
    }
    println!();
}

fn run_atomize_step(config: &AtomizeInternalConfig, result: &mut ExtractPipelineResult) {
    println!("───────────────────────────────────────────────────────────────");
    println!("  Step 1/3: Atomize (generate call graph)");
    println!("───────────────────────────────────────────────────────────────");
    println!();

    match atomize_internal(config) {
        Ok(count) => {
            println!("  ✓ Atomize completed: {} functions", count);
            println!("  → {}", config.output.display());
            result.atomize = Some(StepResult {
                success: true,
                output_file: config.output.display().to_string(),
                total_functions: Some(count),
                error: None,
            });
        }
        Err(e) => {
            eprintln!("  ✗ Atomize failed: {}", e);
            result.status = "atomize_failed".to_string();
            result.atomize = Some(StepResult {
                success: false,
                output_file: config.output.display().to_string(),
                total_functions: None,
                error: Some(e),
            });
        }
    }
    println!();
}

fn run_specify_step(config: &SpecifyInternalConfig, result: &mut ExtractPipelineResult) {
    println!("───────────────────────────────────────────────────────────────");
    println!("  Step 2/3: Specify (extract specifications)");
    println!("───────────────────────────────────────────────────────────────");
    println!();

    match specify_internal(config) {
        Ok(count) => {
            println!("  ✓ Specify completed: {} functions", count);
            println!("  → {}", config.output.display());
            result.specify = Some(StepResult {
                success: true,
                output_file: config.output.display().to_string(),
                total_functions: Some(count),
                error: None,
            });
        }
        Err(e) => {
            eprintln!("  ✗ Specify failed: {}", e);
            if result.status == "success" {
                result.status = "specify_failed".to_string();
            }
            result.specify = Some(StepResult {
                success: false,
                output_file: config.output.display().to_string(),
                total_functions: None,
                error: Some(e),
            });
        }
    }
    println!();
}

fn run_verify_step(config: &ExtractInternalConfig, result: &mut ExtractPipelineResult) {
    println!("───────────────────────────────────────────────────────────────");
    println!("  Step 3/3: Run-Verus (cargo verus verification)");
    println!("───────────────────────────────────────────────────────────────");
    println!();

    match run_verus_internal(config) {
        Ok(summary) => {
            println!("  ✓ Verify completed");
            println!("    Total:      {}", summary.total_functions);
            println!("    Verified:   {}", summary.verified);
            println!("    Failed:     {}", summary.failed);
            println!("    Unverified: {}", summary.unverified);
            println!("  → {}", config.output.display());

            if summary.failed > 0 && result.status == "success" {
                result.status = "verification_failed".to_string();
            }

            result.verify = Some(ExtractStepResult {
                success: true,
                output_file: config.output.display().to_string(),
                summary: Some(summary.into()),
                error: None,
            });
        }
        Err(e) => {
            eprintln!("  ✗ Verify failed: {}", e);
            if result.status == "success" {
                result.status = "verify_failed".to_string();
            }
            result.verify = Some(ExtractStepResult {
                success: false,
                output_file: config.output.display().to_string(),
                summary: None,
                error: Some(e),
            });
        }
    }
    println!();
}

fn print_summary(result: &ExtractPipelineResult) {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  Summary");
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    if let Some(ref a) = result.atomize {
        if a.success {
            println!("  atomize:  ✓ Success → {}", a.output_file);
        } else {
            println!("  atomize:  ✗ Failed");
        }
    }

    if let Some(ref s) = result.specify {
        if s.success {
            println!("  specify:  ✓ Success → {}", s.output_file);
        } else {
            println!("  specify:  ✗ Failed");
        }
    }

    if let Some(ref v) = result.verify {
        if v.success {
            println!("  verify:   ✓ Success → {}", v.output_file);
        } else {
            println!("  verify:   ✗ Failed");
        }
    }

    println!();
    println!("  Status: {}", result.status);
    println!();
}

// =============================================================================
// Unified output merge
// =============================================================================

/// Deserialization target for specs entries with full spec data.
#[derive(Deserialize)]
struct SpecsEntry {
    #[serde(default)]
    has_requires: bool,
    #[serde(default)]
    has_ensures: bool,
    #[serde(default)]
    requires_text: Option<String>,
    #[serde(default)]
    ensures_text: Option<String>,
    #[serde(rename = "requires-calls", default)]
    requires_calls: Vec<String>,
    #[serde(rename = "ensures-calls", default)]
    ensures_calls: Vec<String>,
    #[serde(rename = "requires-calls-full", default)]
    requires_calls_full: Vec<String>,
    #[serde(rename = "ensures-calls-full", default)]
    ensures_calls_full: Vec<String>,
}

/// Minimal deserialization target for proofs entries (only the `status` field).
#[derive(Deserialize)]
struct ProofsEntryMinimal {
    status: String,
}

/// Map a Verus `VerificationStatus` string to the 3-value web status matching probe-lean.
fn map_verification_status(status: &str) -> &'static str {
    match status {
        "success" => "verified",
        "failure" => "failed",
        "sorries" => "unverified",
        "warning" => "verified",
        _ => "failed",
    }
}

/// Build a `Vec<SpecCondition>` from a specs entry.
fn build_spec_conditions(entry: &SpecsEntry) -> Vec<SpecCondition> {
    let mut conditions = Vec::new();

    if entry.has_requires {
        conditions.push(SpecCondition {
            kind: SpecConditionKind::Precondition,
            text: entry.requires_text.clone(),
            clauses: entry
                .requires_text
                .as_deref()
                .map(split_clauses)
                .unwrap_or_default(),
            calls: entry.requires_calls.clone(),
            calls_full: entry.requires_calls_full.clone(),
        });
    }

    if entry.has_ensures {
        conditions.push(SpecCondition {
            kind: SpecConditionKind::Postcondition,
            text: entry.ensures_text.clone(),
            clauses: entry
                .ensures_text
                .as_deref()
                .map(split_clauses)
                .unwrap_or_default(),
            calls: entry.ensures_calls.clone(),
            calls_full: entry.ensures_calls_full.clone(),
        });
    }

    conditions
}

/// Merge atoms, specs, and proofs into a unified `BTreeMap<String, UnifiedAtom>`.
///
/// When specs are available, dependencies are filtered to exclude calls in
/// precondition/postcondition locations (those appear in `specs` instead).
///
/// This is `pub` so integration tests can call it directly.
pub fn merge_into_unified(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
) -> Result<BTreeMap<String, UnifiedAtom>, String> {
    let atoms = load_enveloped_data::<AtomWithLines>(atoms_path, "atoms")?;

    let specs: Option<BTreeMap<String, SpecsEntry>> = specs_path
        .filter(|p| p.exists())
        .map(|p| load_enveloped_data(p, "specs"))
        .transpose()?;

    let proofs: Option<BTreeMap<String, ProofsEntryMinimal>> = proofs_path
        .filter(|p| p.exists())
        .map(|p| load_enveloped_data(p, "proofs"))
        .transpose()?;

    let mut unified: BTreeMap<String, UnifiedAtom> = BTreeMap::new();

    for (code_name, mut atom) in atoms {
        let spec_conditions: Option<Vec<SpecCondition>> = specs
            .as_ref()
            .and_then(|s| s.get(&code_name))
            .map(build_spec_conditions);

        // Filter out precondition/postcondition deps when location data is available
        if spec_conditions.is_some() && !atom.dependencies_with_locations.is_empty() {
            let inner_code_names: std::collections::BTreeSet<String> = atom
                .dependencies_with_locations
                .iter()
                .filter(|d| d.location == CallLocation::Inner)
                .map(|d| d.code_name.clone())
                .collect();
            atom.dependencies = inner_code_names;
            atom.dependencies_with_locations
                .retain(|d| d.location == CallLocation::Inner);
        }

        let verification_status = proofs
            .as_ref()
            .and_then(|p| p.get(&code_name))
            .map(|e| map_verification_status(&e.status).to_string());

        unified.insert(
            code_name,
            UnifiedAtom {
                atom,
                verification_status,
                specs: spec_conditions,
            },
        );
    }

    Ok(unified)
}

/// Load an enveloped (or bare-dict) JSON file and deserialize its data section.
fn load_enveloped_data<T: serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
) -> Result<BTreeMap<String, T>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {} file {}: {}", label, path.display(), e))?;
    let json: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {} JSON {}: {}", label, path.display(), e))?;
    let data = unwrap_envelope(json);
    serde_json::from_value(data).map_err(|e| {
        format!(
            "Failed to deserialize {} data from {}: {}",
            label,
            path.display(),
            e
        )
    })
}

/// Run the merge step: produce unified output, optionally clean up individual files.
fn run_unified_merge(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
    project_path: &Path,
    metadata: &ProjectMetadata,
    separate_outputs: bool,
    result: &ExtractPipelineResult,
) -> Option<PathBuf> {
    if !atoms_path.exists() {
        eprintln!("  Warning: skipping unified output (no atoms file)");
        return None;
    }

    let specs_opt = specs_path.filter(|p| p.exists());
    let proofs_opt = proofs_path.filter(|p| p.exists());

    match merge_into_unified(atoms_path, specs_opt, proofs_opt) {
        Ok(unified) => {
            let unified_path = get_default_output_path(project_path, metadata, "");
            if let Some(parent) = unified_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!("  Warning: Could not create output directory: {}", e);
                    return None;
                }
            }

            let envelope = wrap_in_envelope("probe-verus/extract", "extract", &unified, metadata);
            match serde_json::to_string_pretty(&envelope) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&unified_path, &json) {
                        eprintln!("  Warning: Could not write unified output: {}", e);
                        return None;
                    }
                    println!(
                        "  unified: ✓ {} functions → {}",
                        unified.len(),
                        unified_path.display()
                    );

                    if !separate_outputs {
                        cleanup_individual_files(atoms_path, specs_opt, proofs_opt, result);
                    }

                    Some(unified_path)
                }
                Err(e) => {
                    eprintln!("  Warning: Could not serialize unified output: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            eprintln!("  Warning: Could not merge outputs: {}", e);
            None
        }
    }
}

/// Remove individual output files that were produced during the pipeline.
/// Only removes files for steps that actually succeeded (have a StepResult with success).
fn cleanup_individual_files(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
    result: &ExtractPipelineResult,
) {
    if result.atomize.as_ref().is_some_and(|a| a.success) && atoms_path.exists() {
        let _ = std::fs::remove_file(atoms_path);
    }
    if let Some(sp) = specs_path {
        if result.specify.as_ref().is_some_and(|s| s.success) && sp.exists() {
            let _ = std::fs::remove_file(sp);
        }
    }
    if let Some(pp) = proofs_path {
        if result.verify.as_ref().is_some_and(|v| v.success) && pp.exists() {
            let _ = std::fs::remove_file(pp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn atoms_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "3.0.0", "command": "atomize"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-03-10T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "display-name": "foo",
                    "dependencies": ["probe:test/0.1.0/module/bar()"],
                    "code-module": "module",
                    "code-path": "src/module.rs",
                    "code-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "language": "rust"
                },
                "probe:test/0.1.0/module/bar()": {
                    "display-name": "bar",
                    "dependencies": [],
                    "code-module": "module",
                    "code-path": "src/module.rs",
                    "code-text": {"lines-start": 30, "lines-end": 40},
                    "kind": "proof",
                    "language": "rust"
                },
                "probe:external/1.0.0/lib/ext()": {
                    "display-name": "ext",
                    "dependencies": [],
                    "code-module": "lib",
                    "code-path": "",
                    "code-text": {"lines-start": 0, "lines-end": 0},
                    "kind": "exec",
                    "language": "rust"
                }
            }
        })
    }

    fn specs_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "3.0.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-03-10T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "has_requires": true,
                    "has_ensures": true,
                    "has_decreases": false,
                    "has_trusted_assumption": false,
                    "is_external_body": false,
                    "has_no_decreases_attr": false,
                    "requires_text": "requires\n    x > 0",
                    "ensures_text": "ensures\n    result > x",
                    "requires-calls": ["is_valid"],
                    "ensures-calls": ["helper"]
                },
                "probe:test/0.1.0/module/bar()": {
                    "spec-text": {"lines-start": 30, "lines-end": 40},
                    "kind": "proof",
                    "specified": false,
                    "has_requires": false,
                    "has_ensures": false,
                    "has_decreases": false,
                    "has_trusted_assumption": false,
                    "is_external_body": false,
                    "has_no_decreases_attr": false
                }
            }
        })
    }

    fn proofs_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "probe-verus/proofs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "3.0.0", "command": "run-verus"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-03-10T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "code-path": "src/module.rs",
                    "code-line": 10,
                    "verified": true,
                    "status": "success"
                },
                "probe:test/0.1.0/module/bar()": {
                    "code-path": "src/module.rs",
                    "code-line": 30,
                    "verified": false,
                    "status": "failure"
                }
            }
        })
    }

    fn write_json(dir: &TempDir, name: &str, value: &serde_json::Value) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
        path
    }

    #[test]
    fn test_merge_atoms_only() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let result = merge_into_unified(&atoms_path, None, None).unwrap();

        assert_eq!(result.len(), 3);
        for entry in result.values() {
            assert!(entry.verification_status.is_none());
            assert!(entry.specs.is_none());
        }
        assert_eq!(
            result["probe:test/0.1.0/module/foo()"].atom.display_name,
            "foo"
        );
    }

    #[test]
    fn test_merge_atoms_plus_specs() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());
        let specs_path = write_json(&dir, "specs.json", &specs_json());

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();

        assert_eq!(result.len(), 3);

        let foo_specs = result["probe:test/0.1.0/module/foo()"]
            .specs
            .as_ref()
            .unwrap();
        assert_eq!(foo_specs.len(), 2);
        assert_eq!(foo_specs[0].kind, SpecConditionKind::Precondition);
        assert_eq!(foo_specs[0].calls, vec!["is_valid"]);
        assert_eq!(foo_specs[1].kind, SpecConditionKind::Postcondition);
        assert_eq!(foo_specs[1].calls, vec!["helper"]);

        let bar_specs = result["probe:test/0.1.0/module/bar()"]
            .specs
            .as_ref()
            .unwrap();
        assert!(bar_specs.is_empty());

        // External stub has no spec match
        assert!(result["probe:external/1.0.0/lib/ext()"].specs.is_none());
        // No proofs -> no verification-status
        for entry in result.values() {
            assert!(entry.verification_status.is_none());
        }
    }

    #[test]
    fn test_merge_atoms_plus_proofs() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());
        let proofs_path = write_json(&dir, "proofs.json", &proofs_json());

        let result = merge_into_unified(&atoms_path, None, Some(&proofs_path)).unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("verified")
        );
        assert_eq!(
            result["probe:test/0.1.0/module/bar()"]
                .verification_status
                .as_deref(),
            Some("failed")
        );
        // External stub has no proof
        assert!(result["probe:external/1.0.0/lib/ext()"]
            .verification_status
            .is_none());
        // No specs -> no specs
        for entry in result.values() {
            assert!(entry.specs.is_none());
        }
    }

    #[test]
    fn test_merge_all_three() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());
        let specs_path = write_json(&dir, "specs.json", &specs_json());
        let proofs_path = write_json(&dir, "proofs.json", &proofs_json());

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();

        assert_eq!(result.len(), 3);

        let foo = &result["probe:test/0.1.0/module/foo()"];
        assert_eq!(foo.specs.as_ref().unwrap().len(), 2);
        assert_eq!(foo.verification_status.as_deref(), Some("verified"));
        assert_eq!(foo.atom.display_name, "foo");

        let bar = &result["probe:test/0.1.0/module/bar()"];
        assert!(bar.specs.as_ref().unwrap().is_empty());
        assert_eq!(bar.verification_status.as_deref(), Some("failed"));

        let ext = &result["probe:external/1.0.0/lib/ext()"];
        assert!(ext.specs.is_none());
        assert!(ext.verification_status.is_none());
    }

    #[test]
    fn test_status_mapping_all_values() {
        assert_eq!(map_verification_status("success"), "verified");
        assert_eq!(map_verification_status("failure"), "failed");
        assert_eq!(map_verification_status("sorries"), "unverified");
        assert_eq!(map_verification_status("warning"), "verified");
        assert_eq!(map_verification_status("unknown"), "failed");
    }

    #[test]
    fn test_unified_atom_serialization() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());
        let specs_path = write_json(&dir, "specs.json", &specs_json());
        let proofs_path = write_json(&dir, "proofs.json", &proofs_json());

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();
        let json = serde_json::to_value(&result).unwrap();

        let foo_json = &json["probe:test/0.1.0/module/foo()"];
        assert_eq!(foo_json["display-name"], "foo");
        assert_eq!(foo_json["verification-status"], "verified");
        assert!(foo_json["specs"].is_array());
        assert_eq!(foo_json["specs"].as_array().unwrap().len(), 2);
        assert_eq!(foo_json["specs"][0]["kind"], "precondition");
        assert_eq!(foo_json["specs"][1]["kind"], "postcondition");
        assert_eq!(foo_json["kind"], "exec");

        let ext_json = &json["probe:external/1.0.0/lib/ext()"];
        assert!(ext_json.get("verification-status").is_none());
        assert!(ext_json.get("specs").is_none());
    }

    #[test]
    fn test_specs_clause_splitting() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());
        let specs_path = write_json(&dir, "specs.json", &specs_json());

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();
        let foo_specs = result["probe:test/0.1.0/module/foo()"]
            .specs
            .as_ref()
            .unwrap();

        let pre = &foo_specs[0];
        assert_eq!(pre.kind, SpecConditionKind::Precondition);
        assert_eq!(pre.clauses, vec!["x > 0"]);
        assert_eq!(pre.text.as_deref(), Some("requires\n    x > 0"));

        let post = &foo_specs[1];
        assert_eq!(post.kind, SpecConditionKind::Postcondition);
        assert_eq!(post.clauses, vec!["result > x"]);
        assert_eq!(post.text.as_deref(), Some("ensures\n    result > x"));
    }

    #[test]
    fn test_dep_filtering_with_locations() {
        let atoms_with_locs = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "5.0.0", "command": "atomize"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-03-10T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "display-name": "foo",
                    "dependencies": [
                        "probe:test/0.1.0/module/bar()",
                        "probe:test/0.1.0/specs/is_valid()"
                    ],
                    "dependencies-with-locations": [
                        {"code-name": "probe:test/0.1.0/module/bar()", "location": "inner", "line": 15},
                        {"code-name": "probe:test/0.1.0/specs/is_valid()", "location": "precondition", "line": 12}
                    ],
                    "code-module": "module",
                    "code-path": "src/module.rs",
                    "code-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "language": "rust"
                }
            }
        });

        let specs_with_pre = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "5.0.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-03-10T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "has_requires": true,
                    "has_ensures": false,
                    "has_decreases": false,
                    "has_trusted_assumption": false,
                    "is_external_body": false,
                    "has_no_decreases_attr": false,
                    "requires_text": "requires\n    is_valid(x)",
                    "requires-calls": ["is_valid"]
                }
            }
        });

        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_with_locs);
        let specs_path = write_json(&dir, "specs.json", &specs_with_pre);

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();
        let foo = &result["probe:test/0.1.0/module/foo()"];

        // Only inner deps remain after filtering
        assert_eq!(foo.atom.dependencies.len(), 1);
        assert!(foo
            .atom
            .dependencies
            .contains("probe:test/0.1.0/module/bar()"));
        assert!(!foo
            .atom
            .dependencies
            .contains("probe:test/0.1.0/specs/is_valid()"));

        // dependencies-with-locations also filtered
        assert_eq!(foo.atom.dependencies_with_locations.len(), 1);
        assert_eq!(
            foo.atom.dependencies_with_locations[0].location,
            CallLocation::Inner
        );
    }
}
