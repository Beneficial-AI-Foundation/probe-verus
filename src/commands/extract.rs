//! Extract command - Unified pipeline: atomize + specify + run-verus.

use super::atomize::atomize_internal;
use super::run_verus::{run_verus_internal, VerifySummary};
use super::specify::specify_internal;
use crate::metadata::{
    find_default_atoms_path, gather_metadata, get_default_output_path, unwrap_envelope,
    wrap_in_envelope, AtomizeInternalConfig, ExtractInternalConfig, ProjectMetadata,
    SpecifyInternalConfig,
};
use crate::verification::VerusRunner;
use crate::verus_parser::AssumeSpecInfo;
use crate::{resolve_workspace_root, AtomWithLines, CallLocation, DeclKind, UnifiedAtom};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct ExtractPipelineResult {
    status: String,
    atomize: Option<StepResult>,
    specify: Option<StepResult>,
    verify: Option<ExtractStepResult>,
    #[serde(rename = "trust-base", skip_serializing_if = "Option::is_none")]
    trust_base: Option<TrustBaseSummary>,
}

/// Post-override verification status counts from the unified extract output.
#[derive(Serialize, Clone)]
struct TrustBaseSummary {
    verified: usize,
    trusted: usize,
    unverified: usize,
    failed: usize,
    absent: usize,
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
    _with_spec_text: bool,
    taxonomy_config: Option<PathBuf>,
    verus_args: Vec<String>,
    with_public_api: bool,
    skip_enrich: bool,
) -> Result<(), String> {
    if auto_install {
        eprintln!(
            "Warning: --auto-install is deprecated and will be removed in a future major version."
        );
        eprintln!("  Use instead: probe-verus setup --from-project <project-path>");
        eprintln!();
    }

    if !project_path.exists() {
        return Err(format!(
            "Project path does not exist: {}",
            project_path.display()
        ));
    }

    let cargo_toml = project_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(format!(
            "Not a valid Rust project (Cargo.toml not found): {}",
            project_path.display()
        ));
    }

    let project_path = resolve_workspace_root(&project_path, package.as_deref())?;

    let metadata = gather_metadata(&project_path);

    // Ensure the Verus version env var is set so VerusRunner resolves the
    // correct managed binary (not just the newest installed version).
    if std::env::var(crate::tool_manager::VERUS_VERSION_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .is_none()
    {
        if let Some(v) = crate::metadata::detect_verus_version(&project_path) {
            unsafe { std::env::set_var(crate::tool_manager::VERUS_VERSION_ENV, &v) };
        }
    }

    let atoms_path = get_default_output_path(&project_path, &metadata, "atoms");
    let specs_path = get_default_output_path(&project_path, &metadata, "specs");
    let results_path = get_default_output_path(&project_path, &metadata, "proofs");

    print_header(&project_path, &package);

    let mut result = ExtractPipelineResult {
        status: "success".to_string(),
        atomize: None,
        specify: None,
        verify: None,
        trust_base: None,
    };

    // === Step 1: Atomize ===
    if !skip_atomize {
        let config = AtomizeInternalConfig {
            project_path: &project_path,
            output: &atoms_path,
            package: package.as_deref(),
            regenerate_scip,
            verbose,
            use_rust_analyzer,
            allow_duplicates,
            auto_install,
            with_locations: true,
            with_public_api,
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
                    with_spec_text: true,
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
    let (unified_path, trust_base) = run_unified_merge(
        &atoms_path,
        merge_specs,
        merge_proofs,
        &project_path,
        &metadata,
    );
    result.trust_base = trust_base;

    // === Step 5: Enrich verification status (transitive propagation) ===
    if !skip_enrich {
        if let Some(ref up) = unified_path {
            enrich_unified_output(up);
        }
    }

    // === Summary ===
    print_summary(&result);
    if let Some(ref up) = unified_path {
        println!("  Primary output: {}", up.display());
        println!();
    }

    let summary_path = get_default_output_path(&project_path, &metadata, "extract_summary");
    let envelope = wrap_in_envelope("probe-verus/extract-summary", "extract", &result, &metadata);
    if let Ok(json) = serde_json::to_string_pretty(&envelope) {
        if let Err(e) = std::fs::write(&summary_path, &json) {
            eprintln!("Warning: Could not write summary: {}", e);
        }
    }

    match result.status.as_str() {
        "success" | "verification_failed" => Ok(()),
        status => Err(format!("extract pipeline failed with status: {status}")),
    }
}

fn print_header(project_path: &Path, package: &Option<String>) {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  probe-verus extract");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("  Project: {}", project_path.display());
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

    if !VerusRunner::is_available() {
        eprintln!("  Warning: 'cargo verus' not found; skipping verification.");
        eprintln!("    Install with: probe-verus setup --from-project <project-path>");
        eprintln!("    Or manually: https://github.com/verus-lang/verus");
        println!();
        return;
    }

    if let Some(binary) = VerusRunner::resolve_binary() {
        println!("  Using cargo-verus: {}", binary.display());
    }

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

/// Deserialization target for specs entries (text fields + taxonomy labels).
#[derive(Deserialize)]
struct SpecsEntry {
    #[serde(default)]
    requires_text: Option<String>,
    #[serde(default)]
    ensures_text: Option<String>,
    #[serde(rename = "spec-labels", default)]
    spec_labels: Vec<String>,
    /// Whether the function body contains `admit()` — an axiom (trust base).
    #[serde(default)]
    contains_admit: bool,
    /// Whether the function has `#[verifier::external_body]` — trusted without proof.
    #[serde(default)]
    is_external_body: bool,
}

/// Deserialization wrapper for the specs.json `data` section.
///
/// The data section is a flat dict of code-name → SpecsEntry,
/// with an optional `assume-specifications` sibling key.
/// Using `#[serde(flatten)]` lets us deserialize both the function
/// dict and the metadata key from the same JSON object.
#[derive(Deserialize)]
struct SpecsDataRaw {
    #[serde(flatten)]
    functions: BTreeMap<String, SpecsEntry>,
    #[serde(rename = "assume-specifications", default)]
    assume_specifications: Vec<AssumeSpecInfo>,
}

/// Minimal deserialization target for proofs entries (only the `status` field).
#[derive(Deserialize)]
struct ProofsEntryMinimal {
    status: String,
}

/// Map a Verus `VerificationStatus` string to the web status.
///
/// `"sorries"` covers both `assume()` and `admit()`; at this level it maps to
/// `"unverified"`.  The `merge_into_unified` step further overrides to `"trusted"`
/// for trust-base atoms: `admit()` (via `contains_admit`), `#[verifier::external_body]`
/// (via `is_external_body`), or `assume_specification` targets.
///
/// `"warning"` is mapped to `"unverified"` as a defensive measure: the `Warning`
/// variant is never produced by the current pipeline, but could appear in
/// hand-edited proofs.json or future Verus output.
fn map_verification_status(status: &str) -> &'static str {
    match status {
        "success" => "verified",
        "failure" => "failed",
        "sorries" | "warning" => "unverified",
        _ => "failed",
    }
}

/// Match `assume_specification` declarations to external stub atoms.
///
/// For each declaration, take the last 2 path segments (e.g. `["ConditionallySelectable",
/// "conditional_swap"]`) and search atoms with empty `code-path` for code-names where
/// both segments appear separated by `#`.  Returns the set of matched atom code-names.
/// Result of matching an `assume_specification` to an atom: the spec text to
/// propagate onto the external stub.
struct AssumeSpecMatch {
    spec_text: String,
}

fn match_assume_specs_to_atoms(
    assume_specs: &[AssumeSpecInfo],
    atoms: &BTreeMap<String, AtomWithLines>,
) -> BTreeMap<String, AssumeSpecMatch> {
    let mut matched = BTreeMap::new();

    for aspec in assume_specs {
        if aspec.path_segments.len() < 2 {
            eprintln!(
                "  Warning: assume_specification has fewer than 2 path segments: {:?}",
                aspec.path_display
            );
            continue;
        }

        let type_seg = &aspec.path_segments[aspec.path_segments.len() - 2];
        let method_seg = &aspec.path_segments[aspec.path_segments.len() - 1];

        let candidates: Vec<&String> = atoms
            .iter()
            .filter(|(_, atom)| atom.code_path.is_empty())
            .filter(|(name, _)| {
                name.contains(&format!("{}#", type_seg))
                    && name.contains(&format!("{}()", method_seg))
            })
            .map(|(name, _)| name)
            .collect();

        match candidates.len() {
            1 => {
                let mut parts = Vec::new();
                if let Some(ref t) = aspec.requires_text {
                    parts.push(t.as_str());
                }
                if let Some(ref t) = aspec.ensures_text {
                    parts.push(t.as_str());
                }
                matched.insert(
                    candidates[0].clone(),
                    AssumeSpecMatch {
                        spec_text: parts.join("\n"),
                    },
                );
            }
            0 => {
                eprintln!(
                    "  Warning: no matching atom for assume_specification[{}]",
                    aspec.path_display
                );
            }
            n => {
                eprintln!(
                    "  Warning: {} atoms match assume_specification[{}], skipping (ambiguous): {:?}",
                    n, aspec.path_display, candidates
                );
            }
        }
    }

    matched
}

/// Build the full spec text from a specs entry (requires + ensures concatenated).
fn build_spec_text(entry: &SpecsEntry) -> String {
    let mut parts = Vec::new();
    if let Some(ref t) = entry.requires_text {
        parts.push(t.as_str());
    }
    if let Some(ref t) = entry.ensures_text {
        parts.push(t.as_str());
    }
    parts.join("\n")
}

/// Merge atoms, specs, and proofs into a unified `BTreeMap<String, UnifiedAtom>`.
///
/// Dependencies are kept as the full union. When location data is available,
/// three subcategories (`requires-dependencies`, `ensures-dependencies`,
/// `body-dependencies`) are derived from it.
///
/// This is `pub` so integration tests can call it directly.
pub fn merge_into_unified(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
) -> Result<BTreeMap<String, UnifiedAtom>, String> {
    let atoms = load_enveloped_data::<AtomWithLines>(atoms_path, "atoms")?;

    let specs_raw: Option<SpecsDataRaw> = specs_path
        .filter(|p| p.exists())
        .map(|p| load_enveloped_data_single(p, "specs"))
        .transpose()?;

    let specs = specs_raw.as_ref().map(|s| &s.functions);
    let assume_specs = specs_raw
        .as_ref()
        .map(|s| s.assume_specifications.as_slice())
        .unwrap_or(&[]);

    let assume_spec_matched = match_assume_specs_to_atoms(assume_specs, &atoms);

    let proofs: Option<BTreeMap<String, ProofsEntryMinimal>> = proofs_path
        .filter(|p| p.exists())
        .map(|p| load_enveloped_data(p, "proofs"))
        .transpose()?;

    let mut unified: BTreeMap<String, UnifiedAtom> = BTreeMap::new();

    for (code_name, atom) in atoms {
        let specs_entry = specs.and_then(|s| s.get(&code_name));

        let mut spec_text: Option<String> = specs_entry.map(build_spec_text);

        // `is_disabled` semantics: `None` = function was not analyzed for specs;
        // `Some(true)` = function was analyzed but has no spec; `Some(false)` = function has a spec.
        // Trusted atoms are forced to `Some(false)` further below, after the
        // trust reason is determined.
        //
        // When specs were loaded but a particular atom has no matching specs entry,
        // distinguish external stubs (empty code_path → leave as None) from internal
        // atoms the parser missed (e.g. functions inside proptest! macros → treat as
        // analyzed-but-unspecified: primary_spec = "", is_disabled = true).
        let is_disabled = match &spec_text {
            Some(text) => Some(text.is_empty()),
            None if specs.is_some() && !atom.code_path.is_empty() => {
                spec_text = Some(String::new());
                Some(true)
            }
            None => None,
        };

        // Derive categorized dependencies from location data
        let mut requires_deps = BTreeSet::new();
        let mut ensures_deps = BTreeSet::new();
        let mut body_deps = BTreeSet::new();
        for d in &atom.dependencies_with_locations {
            match d.location {
                CallLocation::Precondition => {
                    requires_deps.insert(d.code_name.clone());
                }
                CallLocation::Postcondition => {
                    ensures_deps.insert(d.code_name.clone());
                }
                CallLocation::Inner => {
                    body_deps.insert(d.code_name.clone());
                }
            }
        }

        // Determine trusted status and reason
        let has_admit = specs_entry.is_some_and(|e| e.contains_admit);
        let has_external_body = specs_entry.is_some_and(|e| e.is_external_body);
        let assume_spec_match = assume_spec_matched.get(&code_name);

        let trusted_reason = if has_admit {
            Some("admit".to_string())
        } else if has_external_body {
            Some("external-body".to_string())
        } else if assume_spec_match.is_some() {
            Some("assume-specification".to_string())
        } else {
            None
        };

        let verification_status = if trusted_reason.is_some() {
            Some("trusted".to_string())
        } else {
            proofs
                .as_ref()
                .and_then(|p| p.get(&code_name))
                .map(|e| map_verification_status(&e.status).to_string())
        };

        // A trust reason is a deliberate human act that puts the atom in
        // analysis scope, so trusted atoms are never disabled (KB P25:
        // has-verification-status ⟹ ¬is-disabled). This covers spec-less
        // `external_body` functions (which would otherwise be Some(true))
        // and `assume_specification` targets (which would otherwise be None
        // despite carrying a propagated primary-spec; KB P24).
        let is_disabled = if trusted_reason.is_some() {
            Some(false)
        } else {
            is_disabled
        };

        // For assume_specification targets, propagate the declared spec text
        if let Some(asm) = assume_spec_match {
            if !asm.spec_text.is_empty() {
                spec_text = Some(asm.spec_text.clone());
            }
        }

        let spec_labels: Vec<String> = specs_entry
            .map(|e| e.spec_labels.clone())
            .unwrap_or_default();

        unified.insert(
            code_name,
            UnifiedAtom {
                atom,
                requires_dependencies: requires_deps,
                ensures_dependencies: ensures_deps,
                body_dependencies: body_deps,
                primary_spec: spec_text,
                is_disabled,
                verification_status,
                trusted_reason,
                spec_labels,
            },
        );
    }

    Ok(unified)
}

/// Load an enveloped (or bare-dict) JSON file and deserialize its data section as a dict.
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

/// Load an enveloped JSON file, deserializing the data section as a single `T`.
fn load_enveloped_data_single<T: serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
) -> Result<T, String> {
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

/// Compute post-override verification status counts from unified output.
fn compute_trust_base_summary(unified: &BTreeMap<String, UnifiedAtom>) -> TrustBaseSummary {
    let mut verified = 0;
    let mut trusted = 0;
    let mut unverified = 0;
    let mut failed = 0;
    let mut absent = 0;

    for atom in unified.values() {
        match atom.verification_status.as_deref() {
            Some("verified") => verified += 1,
            Some("trusted") => trusted += 1,
            Some("unverified") => unverified += 1,
            Some("failed") => failed += 1,
            _ => absent += 1,
        }
    }

    TrustBaseSummary {
        verified,
        trusted,
        unverified,
        failed,
        absent,
    }
}

/// Warn about proof-kind atoms that ended up without a verification status.
///
/// Verus verifies every `proof fn` it compiles, so a proof atom with source
/// (non-empty `code-path`) but no `verification-status` always indicates a
/// result-matching failure (e.g. a `code-path` that doesn't line up with the
/// verify step's paths), never a legitimate state.
fn warn_proof_atoms_without_status(unified: &BTreeMap<String, UnifiedAtom>) {
    let missing: Vec<&String> = unified
        .iter()
        .filter(|(_, a)| {
            a.atom.kind == DeclKind::Proof
                && !a.atom.code_path.is_empty()
                && a.verification_status.is_none()
        })
        .map(|(name, _)| name)
        .collect();

    if !missing.is_empty() {
        eprintln!(
            "  Warning: {} proof atom(s) have no verification-status (result matching likely failed):",
            missing.len()
        );
        for name in missing.iter().take(10) {
            eprintln!("    - {}", name);
        }
        if missing.len() > 10 {
            eprintln!("    ... and {} more", missing.len() - 10);
        }
    }
}

/// Run the merge step: produce unified output.
fn run_unified_merge(
    atoms_path: &Path,
    specs_path: Option<&Path>,
    proofs_path: Option<&Path>,
    project_path: &Path,
    metadata: &ProjectMetadata,
) -> (Option<PathBuf>, Option<TrustBaseSummary>) {
    if !atoms_path.exists() {
        eprintln!("  Warning: skipping unified output (no atoms file)");
        return (None, None);
    }

    let specs_opt = specs_path.filter(|p| p.exists());
    let proofs_opt = proofs_path.filter(|p| p.exists());

    match merge_into_unified(atoms_path, specs_opt, proofs_opt) {
        Ok(unified) => {
            // Only meaningful when verification results were merged; without
            // proofs.json every proof atom legitimately lacks a status.
            if proofs_opt.is_some() {
                warn_proof_atoms_without_status(&unified);
            }
            let trust_base = compute_trust_base_summary(&unified);
            let unified_path = get_default_output_path(project_path, metadata, "");
            if let Some(parent) = unified_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!("  Warning: Could not create output directory: {}", e);
                    return (None, Some(trust_base));
                }
            }

            let envelope = wrap_in_envelope("probe-verus/extract", "extract", &unified, metadata);
            match serde_json::to_string_pretty(&envelope) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&unified_path, &json) {
                        eprintln!("  Warning: Could not write unified output: {}", e);
                        return (None, Some(trust_base));
                    }
                    let public_api_count = unified
                        .values()
                        .filter(|a| a.atom.is_public_api == Some(true))
                        .count();
                    println!(
                        "  unified: ✓ {} functions ({} public API) → {}",
                        unified.len(),
                        public_api_count,
                        unified_path.display()
                    );

                    (Some(unified_path), Some(trust_base))
                }
                Err(e) => {
                    eprintln!("  Warning: Could not serialize unified output: {}", e);
                    (None, Some(trust_base))
                }
            }
        }
        Err(e) => {
            eprintln!("  Warning: Could not merge outputs: {}", e);
            (None, None)
        }
    }
}

/// Read the unified extract JSON, run verification status enrichment (P23),
/// and write back in-place.
fn enrich_unified_output(path: &Path) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  Warning: Could not read unified output for enrichment: {e}");
            return;
        }
    };

    let mut raw: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  Warning: Could not parse unified output for enrichment: {e}");
            return;
        }
    };

    let data_value = match raw.get("data").cloned() {
        Some(v) => v,
        None => {
            eprintln!("  Warning: No \"data\" field in unified output; skipping enrichment");
            return;
        }
    };

    let mut atoms: std::collections::BTreeMap<String, probe::types::Atom> =
        match serde_json::from_value(data_value) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("  Warning: Could not deserialize atoms for enrichment: {e}");
                return;
            }
        };

    let (transitive, local, _) = probe::commands::propagate::enrich_verification_status(&mut atoms);

    let enriched_data = serde_json::to_value(&atoms).expect("failed to serialize enriched atoms");
    raw.as_object_mut()
        .expect("envelope is not a JSON object")
        .insert("data".to_string(), enriched_data);

    let json = serde_json::to_string_pretty(&raw).expect("failed to serialize enriched output");
    if let Err(e) = std::fs::write(path, &json) {
        eprintln!("  Warning: Could not write enriched output: {e}");
        return;
    }

    println!("  enrich: ✓ {transitive} transitively-verified, {local} locally verified");
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
                    "language": "verus"
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
                    "ensures-calls": ["helper"],
                    "spec-labels": ["label-A", "label-B"]
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
            assert!(entry.primary_spec.is_none());
            assert!(entry.is_disabled.is_none());
            assert!(entry.spec_labels.is_empty());
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

        let foo = &result["probe:test/0.1.0/module/foo()"];
        assert_eq!(
            foo.primary_spec.as_deref(),
            Some("requires\n    x > 0\nensures\n    result > x")
        );
        assert_eq!(foo.is_disabled, Some(false));
        assert_eq!(foo.spec_labels, vec!["label-A", "label-B"]);

        let bar = &result["probe:test/0.1.0/module/bar()"];
        assert_eq!(bar.primary_spec.as_deref(), Some(""));
        assert_eq!(bar.is_disabled, Some(true));
        assert!(bar.spec_labels.is_empty());

        // External stub has no spec match
        let ext = &result["probe:external/1.0.0/lib/ext()"];
        assert!(ext.primary_spec.is_none());
        assert!(ext.is_disabled.is_none());
        assert!(ext.spec_labels.is_empty());

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
        assert!(result["probe:external/1.0.0/lib/ext()"]
            .verification_status
            .is_none());
        for entry in result.values() {
            assert!(entry.primary_spec.is_none());
            assert!(entry.is_disabled.is_none());
            assert!(entry.spec_labels.is_empty());
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
        assert!(!foo.primary_spec.as_ref().unwrap().is_empty());
        assert_eq!(foo.is_disabled, Some(false));
        assert_eq!(foo.verification_status.as_deref(), Some("verified"));
        assert_eq!(foo.atom.display_name, "foo");
        assert_eq!(foo.spec_labels, vec!["label-A", "label-B"]);

        let bar = &result["probe:test/0.1.0/module/bar()"];
        assert_eq!(bar.primary_spec.as_deref(), Some(""));
        assert_eq!(bar.is_disabled, Some(true));
        assert_eq!(bar.verification_status.as_deref(), Some("failed"));
        assert!(bar.spec_labels.is_empty());

        let ext = &result["probe:external/1.0.0/lib/ext()"];
        assert!(ext.primary_spec.is_none());
        assert!(ext.is_disabled.is_none());
        assert!(ext.verification_status.is_none());
        assert!(ext.spec_labels.is_empty());
    }

    #[test]
    fn test_status_mapping_all_values() {
        assert_eq!(map_verification_status("success"), "verified");
        assert_eq!(map_verification_status("failure"), "failed");
        assert_eq!(map_verification_status("sorries"), "unverified");
        assert_eq!(map_verification_status("warning"), "unverified");
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
        assert!(foo_json["primary-spec"].is_string());
        assert!(!foo_json["primary-spec"].as_str().unwrap().is_empty());
        assert_eq!(foo_json["is-disabled"], false);
        assert_eq!(foo_json["kind"], "exec");
        assert_eq!(foo_json["language"], "rust");
        let labels = foo_json["spec-labels"]
            .as_array()
            .expect("spec-labels should be an array");
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0], "label-A");
        assert_eq!(labels[1], "label-B");

        let bar_json = &json["probe:test/0.1.0/module/bar()"];
        assert_eq!(bar_json["primary-spec"], "");
        assert_eq!(bar_json["is-disabled"], true);
        assert_eq!(bar_json["language"], "verus");
        assert!(
            bar_json.get("spec-labels").is_none(),
            "Empty spec-labels should be omitted from JSON"
        );

        // trusted-reason absent for non-trusted atoms
        assert!(foo_json.get("trusted-reason").is_none());
        assert!(bar_json.get("trusted-reason").is_none());

        let ext_json = &json["probe:external/1.0.0/lib/ext()"];
        assert!(ext_json.get("verification-status").is_none());
        assert!(ext_json.get("trusted-reason").is_none());
        assert!(ext_json.get("primary-spec").is_none());
        assert!(ext_json.get("is-disabled").is_none());
        assert!(ext_json.get("spec-labels").is_none());
        assert_eq!(ext_json["language"], "rust");
    }

    #[test]
    fn test_trusted_from_contains_admit() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let specs_with_admit = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "has_requires": true,
                    "has_ensures": true,
                    "contains_admit": true,
                    "requires_text": "requires\n    x > 0",
                    "ensures_text": "ensures\n    result > x"
                },
                "probe:test/0.1.0/module/bar()": {
                    "spec-text": {"lines-start": 30, "lines-end": 40},
                    "kind": "proof",
                    "specified": false,
                    "contains_admit": false
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_with_admit);

        let proofs_with_sorries = serde_json::json!({
            "schema": "probe-verus/proofs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "run-verus"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "code-path": "src/module.rs",
                    "code-line": 10,
                    "verified": false,
                    "status": "sorries"
                },
                "probe:test/0.1.0/module/bar()": {
                    "code-path": "src/module.rs",
                    "code-line": 30,
                    "verified": true,
                    "status": "success"
                }
            }
        });
        let proofs_path = write_json(&dir, "proofs.json", &proofs_with_sorries);

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("trusted"),
            "Function with contains_admit=true should be 'trusted'"
        );
        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .trusted_reason
                .as_deref(),
            Some("admit"),
        );
        assert_eq!(
            result["probe:test/0.1.0/module/bar()"]
                .verification_status
                .as_deref(),
            Some("verified"),
            "Function without contains_admit should keep its proofs status"
        );
        assert!(result["probe:test/0.1.0/module/bar()"]
            .trusted_reason
            .is_none());
    }

    #[test]
    fn test_assume_only_stays_unverified() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        // has_trusted_assumption=true but contains_admit=false → assume() only
        let specs_assume_only = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "has_trusted_assumption": true,
                    "contains_admit": false
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_assume_only);

        let proofs_sorries = serde_json::json!({
            "schema": "probe-verus/proofs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "run-verus"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "code-path": "src/module.rs",
                    "code-line": 10,
                    "verified": false,
                    "status": "sorries"
                }
            }
        });
        let proofs_path = write_json(&dir, "proofs.json", &proofs_sorries);

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("unverified"),
            "Function with assume() but no admit() should be 'unverified', not 'trusted'"
        );
    }

    #[test]
    fn test_trusted_overrides_proofs_status() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let specs_with_admit = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "contains_admit": true
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_with_admit);

        // Even if proofs says "success", contains_admit overrides to "trusted"
        let proofs_success = serde_json::json!({
            "schema": "probe-verus/proofs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "run-verus"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "code-path": "src/module.rs",
                    "code-line": 10,
                    "verified": true,
                    "status": "success"
                }
            }
        });
        let proofs_path = write_json(&dir, "proofs.json", &proofs_success);

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("trusted"),
            "contains_admit overrides even 'success' proofs status"
        );
    }

    #[test]
    fn test_trusted_from_specs_without_proofs() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let specs_with_admit = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.4.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "contains_admit": true
                },
                "probe:test/0.1.0/module/bar()": {
                    "spec-text": {"lines-start": 30, "lines-end": 40},
                    "kind": "proof",
                    "specified": false,
                    "contains_admit": false
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_with_admit);

        // No proofs.json — specs-only override still sets "trusted"
        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("trusted"),
            "contains_admit sets 'trusted' even without proofs.json"
        );
        assert!(
            result["probe:test/0.1.0/module/bar()"]
                .verification_status
                .is_none(),
            "Non-trusted function without proofs should have no verification status"
        );
    }

    #[test]
    fn test_external_body_overrides_verified_to_trusted() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let specs_eb = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "is_external_body": true,
                    "has_requires": true,
                    "has_ensures": true,
                    "requires_text": "requires\n    x > 0",
                    "ensures_text": "ensures\n    result > x"
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_eb);

        let proofs_success = serde_json::json!({
            "schema": "probe-verus/proofs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "run-verus"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "code-path": "src/module.rs",
                    "code-line": 10,
                    "verified": true,
                    "status": "success"
                }
            }
        });
        let proofs_path = write_json(&dir, "proofs.json", &proofs_success);

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("trusted"),
            "external_body should override 'success' proofs status to 'trusted'"
        );
        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .trusted_reason
                .as_deref(),
            Some("external-body"),
        );
    }

    #[test]
    fn test_external_body_absent_gets_trusted() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let specs_eb = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": false,
                    "is_external_body": true
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_eb);

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("trusted"),
            "external_body without proofs entry should get 'trusted'"
        );
        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .trusted_reason
                .as_deref(),
            Some("external-body"),
        );
        assert_eq!(
            result["probe:test/0.1.0/module/foo()"].is_disabled,
            Some(false),
            "spec-less external_body atom must not be disabled (P25: trusted atoms are in scope)"
        );
    }

    #[test]
    fn test_non_external_body_unaffected() {
        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_json());

        let specs_normal = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "is_external_body": false,
                    "contains_admit": false
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_normal);

        let proofs = serde_json::json!({
            "schema": "probe-verus/proofs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "run-verus"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "code-path": "src/module.rs",
                    "code-line": 10,
                    "verified": true,
                    "status": "success"
                }
            }
        });
        let proofs_path = write_json(&dir, "proofs.json", &proofs);

        let result =
            merge_into_unified(&atoms_path, Some(&specs_path), Some(&proofs_path)).unwrap();

        assert_eq!(
            result["probe:test/0.1.0/module/foo()"]
                .verification_status
                .as_deref(),
            Some("verified"),
            "Non-external_body without admit should keep normal proofs status"
        );
        assert!(result["probe:test/0.1.0/module/foo()"]
            .trusted_reason
            .is_none());
    }

    #[test]
    fn test_assume_spec_matching_single_match() {
        let atoms_with_external = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "atomize"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:subtle/2.6.1/Choice#From#from()": {
                    "display-name": "from",
                    "dependencies": [],
                    "code-module": "",
                    "code-path": "",
                    "code-text": {"lines-start": 0, "lines-end": 0},
                    "kind": "exec",
                    "language": "rust"
                }
            }
        });
        let atoms_data: BTreeMap<String, AtomWithLines> =
            serde_json::from_value(atoms_with_external["data"].clone()).unwrap();

        let assume_specs = vec![crate::verus_parser::AssumeSpecInfo {
            path_segments: vec!["Choice".to_string(), "from".to_string()],
            path_display: "Choice::from".to_string(),
            file: Some("src/assumes.rs".to_string()),
            line: 10,
            has_requires: false,
            has_ensures: true,
            requires_text: None,
            ensures_text: Some("ensures (u == 1) == choice_is_true(c)".to_string()),
        }];

        let result = match_assume_specs_to_atoms(&assume_specs, &atoms_data);
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("probe:subtle/2.6.1/Choice#From#from()"));
        let m = &result["probe:subtle/2.6.1/Choice#From#from()"];
        assert_eq!(m.spec_text, "ensures (u == 1) == choice_is_true(c)");
    }

    #[test]
    fn test_assume_spec_matching_no_match() {
        let atoms_with_external = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "atomize"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:subtle/2.6.1/Choice#From#from()": {
                    "display-name": "from",
                    "dependencies": [],
                    "code-module": "",
                    "code-path": "",
                    "code-text": {"lines-start": 0, "lines-end": 0},
                    "kind": "exec",
                    "language": "rust"
                }
            }
        });
        let atoms_data: BTreeMap<String, AtomWithLines> =
            serde_json::from_value(atoms_with_external["data"].clone()).unwrap();

        let assume_specs = vec![crate::verus_parser::AssumeSpecInfo {
            path_segments: vec!["Formatter".to_string(), "write_str".to_string()],
            path_display: "Formatter::write_str".to_string(),
            file: Some("src/assumes.rs".to_string()),
            line: 20,
            has_requires: false,
            has_ensures: false,
            requires_text: None,
            ensures_text: None,
        }];

        let result = match_assume_specs_to_atoms(&assume_specs, &atoms_data);
        assert!(
            result.is_empty(),
            "No atom should match Formatter::write_str"
        );
    }

    #[test]
    fn test_assume_spec_trusted_in_merge() {
        let dir = TempDir::new().unwrap();

        let atoms_with_external = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "atomize"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "display-name": "foo",
                    "dependencies": [],
                    "code-module": "module",
                    "code-path": "src/module.rs",
                    "code-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "language": "rust"
                },
                "probe:subtle/2.6.1/Choice#unwrap_u8()": {
                    "display-name": "unwrap_u8",
                    "dependencies": [],
                    "code-module": "",
                    "code-path": "",
                    "code-text": {"lines-start": 0, "lines-end": 0},
                    "kind": "exec",
                    "language": "rust"
                }
            }
        });
        let atoms_path = write_json(&dir, "atoms.json", &atoms_with_external);

        let specs_with_assume = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.5.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-07T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/foo()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true
                },
                "assume-specifications": [
                    {
                        "path-segments": ["Choice", "unwrap_u8"],
                        "path-display": "Choice::unwrap_u8",
                        "file": "src/assumes.rs",
                        "line": 10,
                        "has_requires": false,
                        "has_ensures": true,
                        "ensures_text": "ensures choice_is_true(*c) ==> u == 1u8"
                    }
                ]
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs_with_assume);

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();

        let stub = &result["probe:subtle/2.6.1/Choice#unwrap_u8()"];
        assert_eq!(
            stub.verification_status.as_deref(),
            Some("trusted"),
            "External stub matched by assume_specification should be 'trusted'"
        );
        assert_eq!(stub.trusted_reason.as_deref(), Some("assume-specification"),);
        assert_eq!(
            stub.primary_spec.as_deref(),
            Some("ensures choice_is_true(*c) ==> u == 1u8"),
            "Spec text from assume_specification should be propagated"
        );
        assert_eq!(
            stub.is_disabled,
            Some(false),
            "assume_specification target carries a spec, so it must not be disabled (P24)"
        );

        let foo = &result["probe:test/0.1.0/module/foo()"];
        assert!(
            foo.verification_status.is_none(),
            "Non-matched atom without proofs should have no status"
        );
        assert!(foo.trusted_reason.is_none());
    }

    #[test]
    fn test_dep_categorization_with_locations() {
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
                        "probe:test/0.1.0/specs/is_valid()",
                        "probe:test/0.1.0/specs/helper()"
                    ],
                    "dependencies-with-locations": [
                        {"code-name": "probe:test/0.1.0/module/bar()", "location": "inner", "line": 15},
                        {"code-name": "probe:test/0.1.0/specs/is_valid()", "location": "precondition", "line": 12},
                        {"code-name": "probe:test/0.1.0/specs/helper()", "location": "postcondition", "line": 13}
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
                    "has_ensures": true,
                    "requires_text": "requires\n    is_valid(x)",
                    "ensures_text": "ensures\n    helper(x)"
                }
            }
        });

        let dir = TempDir::new().unwrap();
        let atoms_path = write_json(&dir, "atoms.json", &atoms_with_locs);
        let specs_path = write_json(&dir, "specs.json", &specs_with_pre);

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();
        let foo = &result["probe:test/0.1.0/module/foo()"];

        // dependencies is the full union (unchanged)
        assert_eq!(foo.atom.dependencies.len(), 3);
        assert!(foo
            .atom
            .dependencies
            .contains("probe:test/0.1.0/module/bar()"));
        assert!(foo
            .atom
            .dependencies
            .contains("probe:test/0.1.0/specs/is_valid()"));
        assert!(foo
            .atom
            .dependencies
            .contains("probe:test/0.1.0/specs/helper()"));

        // Categorized deps
        assert_eq!(foo.requires_dependencies.len(), 1);
        assert!(foo
            .requires_dependencies
            .contains("probe:test/0.1.0/specs/is_valid()"));
        assert_eq!(foo.ensures_dependencies.len(), 1);
        assert!(foo
            .ensures_dependencies
            .contains("probe:test/0.1.0/specs/helper()"));
        assert_eq!(foo.body_dependencies.len(), 1);
        assert!(foo
            .body_dependencies
            .contains("probe:test/0.1.0/module/bar()"));

        // primary-spec text and is-disabled
        assert_eq!(
            foo.primary_spec.as_deref(),
            Some("requires\n    is_valid(x)\nensures\n    helper(x)")
        );
        assert_eq!(foo.is_disabled, Some(false));
    }

    /// Internal atoms not matched by specify (e.g. functions inside proptest! macros)
    /// should get `is-disabled: true` + `primary-spec: ""` rather than omitting both.
    #[test]
    fn test_internal_atom_missing_from_specs_gets_disabled() {
        let dir = TempDir::new().unwrap();

        let atoms = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.6.0", "command": "atomize"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-14T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/specified_fn()": {
                    "display-name": "specified_fn",
                    "dependencies": [],
                    "code-module": "module",
                    "code-path": "src/module.rs",
                    "code-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "language": "rust"
                },
                "probe:test/0.1.0/test/module/proptest_fn()": {
                    "display-name": "proptest_fn",
                    "dependencies": [],
                    "code-module": "test/module",
                    "code-path": "src/module.rs",
                    "code-text": {"lines-start": 100, "lines-end": 110},
                    "kind": "exec",
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
        });
        let atoms_path = write_json(&dir, "atoms.json", &atoms);

        let specs = serde_json::json!({
            "schema": "probe-verus/specs",
            "schema-version": "2.0",
            "tool": {"name": "probe-verus", "version": "6.6.0", "command": "specify"},
            "source": {"repo": "", "commit": "", "language": "rust", "package": "test", "package-version": "0.1.0"},
            "timestamp": "2026-04-14T00:00:00Z",
            "data": {
                "probe:test/0.1.0/module/specified_fn()": {
                    "spec-text": {"lines-start": 10, "lines-end": 20},
                    "kind": "exec",
                    "specified": true,
                    "has_requires": true,
                    "has_ensures": true,
                    "requires_text": "requires\n    x > 0",
                    "ensures_text": "ensures\n    result > x"
                }
            }
        });
        let specs_path = write_json(&dir, "specs.json", &specs);

        let result = merge_into_unified(&atoms_path, Some(&specs_path), None).unwrap();

        let specified = &result["probe:test/0.1.0/module/specified_fn()"];
        assert_eq!(
            specified.is_disabled,
            Some(false),
            "specified function → is-disabled: false"
        );
        assert!(!specified.primary_spec.as_ref().unwrap().is_empty());

        let proptest = &result["probe:test/0.1.0/test/module/proptest_fn()"];
        assert_eq!(
            proptest.is_disabled,
            Some(true),
            "internal atom not in specs should get is-disabled: true"
        );
        assert_eq!(
            proptest.primary_spec.as_deref(),
            Some(""),
            "internal atom not in specs should get empty primary-spec"
        );

        let ext = &result["probe:external/1.0.0/lib/ext()"];
        assert!(
            ext.is_disabled.is_none(),
            "external stub should still have is-disabled absent"
        );
        assert!(
            ext.primary_spec.is_none(),
            "external stub should still have primary-spec absent"
        );
    }
}
