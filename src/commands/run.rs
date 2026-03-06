//! Run command - Execute both atomize and verify (designed for Docker/CI usage).

use super::atomize::atomize_internal;
use super::verify::{verify_internal, VerifySummary};
use probe_verus::metadata::{
    gather_metadata, get_default_output_path, wrap_in_envelope, AtomizeInternalConfig,
    VerifyInternalConfig,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Result of the run command for JSON output.
#[derive(Serialize)]
struct RunResult {
    status: String,
    atomize: Option<AtomizeResult>,
    verify: Option<VerifyResult>,
}

#[derive(Serialize)]
struct AtomizeResult {
    success: bool,
    output_file: String,
    total_functions: Option<usize>,
    error: Option<String>,
}

#[derive(Serialize)]
struct VerifyResult {
    success: bool,
    output_file: String,
    summary: Option<VerifySummaryOutput>,
    error: Option<String>,
}

#[derive(Serialize, Clone)]
struct VerifySummaryOutput {
    total_functions: usize,
    verified: usize,
    failed: usize,
    unverified: usize,
}

impl From<VerifySummary> for VerifySummaryOutput {
    fn from(s: VerifySummary) -> Self {
        Self {
            total_functions: s.total_functions,
            verified: s.verified,
            failed: s.failed,
            unverified: s.unverified,
        }
    }
}

/// Execute the run command.
///
/// Runs both atomize and verify commands (designed for Docker/CI usage).
#[allow(clippy::too_many_arguments)]
pub fn cmd_run(
    project_path: PathBuf,
    output_dir: PathBuf,
    atomize_only: bool,
    verify_only: bool,
    package: Option<String>,
    regenerate_scip: bool,
    verbose: bool,
    use_rust_analyzer: bool,
    allow_duplicates: bool,
    auto_install: bool,
) {
    // Validate project path
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

    // Gather metadata once so atomize and verify share the same timestamp
    let metadata = gather_metadata(&project_path);

    // Use .verilib/probes/ convention for output paths, consistent with all other commands
    let atoms_path = get_default_output_path(&project_path, &metadata, "");
    let results_path = get_default_output_path(&project_path, &metadata, "proofs");

    // Create output directory for the summary file
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        eprintln!("Error: Failed to create output directory: {}", e);
        std::process::exit(1);
    }

    print_header(&project_path, &output_dir, &package);

    let mut run_result = RunResult {
        status: "success".to_string(),
        atomize: None,
        verify: None,
    };

    // === Run atomize ===
    if !verify_only {
        let config = AtomizeInternalConfig {
            project_path: &project_path,
            output: &atoms_path,
            regenerate_scip,
            verbose,
            use_rust_analyzer,
            allow_duplicates,
            auto_install,
            metadata: &metadata,
        };
        run_atomize_step(&config, &mut run_result);
    }

    // === Run verify ===
    if !atomize_only {
        let config = VerifyInternalConfig {
            project_path: &project_path,
            output: &results_path,
            package: package.as_deref(),
            atoms_path: if atoms_path.exists() {
                Some(atoms_path.as_path())
            } else {
                None
            },
            verbose,
            verus_args: &[],
            metadata: &metadata,
        };
        run_verify_step(&config, &mut run_result);
    }

    // === Summary ===
    print_summary(&run_result);

    // Write summary JSON wrapped in envelope
    let summary_path = output_dir.join("run_summary.json");
    let envelope = wrap_in_envelope("probe-verus/run-summary", "run", &run_result, &metadata);
    if let Ok(json) = serde_json::to_string_pretty(&envelope) {
        if let Err(e) = std::fs::write(&summary_path, &json) {
            eprintln!("Warning: Could not write summary: {}", e);
        }
    }

    // Exit with appropriate code
    let exit_code = match run_result.status.as_str() {
        "success" => 0,
        "verification_failed" => 0, // Verification ran successfully, just found failures
        _ => 1,
    };
    std::process::exit(exit_code);
}

/// Print the run command header.
fn print_header(project_path: &Path, output_dir: &Path, package: &Option<String>) {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  probe-verus run");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("  Project: {}", project_path.display());
    println!("  Output:  {}", output_dir.display());
    if let Some(ref pkg) = package {
        println!("  Package: {}", pkg);
    }
    println!();
}

/// Run the atomize step.
fn run_atomize_step(config: &AtomizeInternalConfig, run_result: &mut RunResult) {
    println!("───────────────────────────────────────────────────────────────");
    println!("  Step 1: Atomize (generate call graph)");
    println!("───────────────────────────────────────────────────────────────");
    println!();

    let atomize_result = atomize_internal(config);

    match &atomize_result {
        Ok(count) => {
            println!("  ✓ Atomize completed: {} functions", count);
            println!("  → {}", config.output.display());
            run_result.atomize = Some(AtomizeResult {
                success: true,
                output_file: config.output.display().to_string(),
                total_functions: Some(*count),
                error: None,
            });
        }
        Err(e) => {
            eprintln!("  ✗ Atomize failed: {}", e);
            run_result.status = "atomize_failed".to_string();
            run_result.atomize = Some(AtomizeResult {
                success: false,
                output_file: config.output.display().to_string(),
                total_functions: None,
                error: Some(e.clone()),
            });
        }
    }
    println!();
}

/// Run the verify step.
fn run_verify_step(config: &VerifyInternalConfig, run_result: &mut RunResult) {
    println!("───────────────────────────────────────────────────────────────");
    println!("  Step 2: Verify (run Verus verification)");
    println!("───────────────────────────────────────────────────────────────");
    println!();

    let verify_result = verify_internal(config);

    match &verify_result {
        Ok(summary) => {
            println!("  ✓ Verify completed");
            println!("    Total:      {}", summary.total_functions);
            println!("    Verified:   {}", summary.verified);
            println!("    Failed:     {}", summary.failed);
            println!("    Unverified: {}", summary.unverified);
            println!("  → {}", config.output.display());

            run_result.verify = Some(VerifyResult {
                success: true,
                output_file: config.output.display().to_string(),
                summary: Some(summary.clone().into()),
                error: None,
            });

            // Mark as verification_failed if there are failures
            if summary.failed > 0 && run_result.status == "success" {
                run_result.status = "verification_failed".to_string();
            }
        }
        Err(e) => {
            eprintln!("  ✗ Verify failed: {}", e);
            if run_result.status == "success" {
                run_result.status = "verify_failed".to_string();
            }
            run_result.verify = Some(VerifyResult {
                success: false,
                output_file: config.output.display().to_string(),
                summary: None,
                error: Some(e.clone()),
            });
        }
    }
    println!();
}

/// Print the final summary.
fn print_summary(run_result: &RunResult) {
    println!("═══════════════════════════════════════════════════════════════");
    println!("  Summary");
    println!("═══════════════════════════════════════════════════════════════");
    println!();

    if let Some(ref a) = run_result.atomize {
        if a.success {
            println!("  atomize: ✓ Success → {}", a.output_file);
        } else {
            println!("  atomize: ✗ Failed");
        }
    }

    if let Some(ref v) = run_result.verify {
        if v.success {
            println!("  verify:  ✓ Success → {}", v.output_file);
        } else {
            println!("  verify:  ✗ Failed");
        }
    }

    println!();
    println!("  Status: {}", run_result.status);
    println!();
}
