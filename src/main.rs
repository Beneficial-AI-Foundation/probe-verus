//! Probe Verus - Analyze Verus projects: call graphs and verification
//!
//! This tool provides multiple subcommands:
//! - `atomize`: Generate call graph atoms with line numbers from SCIP indexes
//! - `callee-crates`: Find which crates a function's callees belong to at a given depth
//! - `list-functions`: List all functions in a Rust/Verus project
//! - `run-verus`: Run Verus verification and analyze results (or analyze existing output)
//! - `specify`: Extract function specifications (requires/ensures) to JSON
//! - `merge-atoms`: Combine independently-indexed atoms.json files
//! - `stubify`: Convert .md files with YAML frontmatter to JSON
//! - `setup`: Install or check status of external tools (verus-analyzer, scip)
//! - `extract`: Unified pipeline - atomize + specify + run-verus

use clap::{Parser, Subcommand};
use probe_verus::constants::DEFAULT_OUTPUT_DIR;
use std::path::PathBuf;

// Import command implementations
mod commands;
use commands::{
    cmd_atomize, cmd_callee_crates, cmd_extract, cmd_functions, cmd_merge_atoms, cmd_run_verus,
    cmd_setup, cmd_specify, cmd_specs_data, cmd_stubify, cmd_tracked_csv, OutputFormat,
};

#[derive(Parser)]
#[command(name = "probe-verus")]
#[command(author, version, about = "Probe Verus projects: call graphs and verification analysis", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate call graph atoms with line numbers from SCIP indexes
    Atomize {
        /// Path to the Rust/Verus project
        project_path: PathBuf,

        /// Output file path (default: .verilib/probes/verus_<pkg>_<ver>_atoms.json)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Force regeneration of the SCIP index
        #[arg(short, long)]
        regenerate_scip: bool,

        /// Include dependencies-with-locations (detailed per-call location info)
        #[arg(long)]
        with_locations: bool,

        /// Use rust-analyzer instead of verus-analyzer for SCIP generation
        #[arg(long)]
        rust_analyzer: bool,

        /// Continue with warnings instead of failing on duplicate code_names
        #[arg(long)]
        allow_duplicates: bool,

        /// Automatically download missing external tools (verus-analyzer, scip) without prompting
        #[arg(long)]
        auto_install: bool,
    },

    /// Combine independently-indexed atoms.json files, replacing stubs with real atoms
    MergeAtoms {
        /// Two or more atoms.json files to merge
        #[arg(required = true, num_args = 2..)]
        inputs: Vec<PathBuf>,

        /// Output file path (default: merged_atoms.json)
        #[arg(short, long, default_value = "merged_atoms.json")]
        output: PathBuf,
    },

    /// List all functions in a Rust/Verus project
    #[command(name = "list-functions")]
    ListFunctions {
        /// Path to search (file or directory)
        path: PathBuf,

        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Exclude Verus constructs (spec, proof, exec) and only include regular functions
        #[arg(long)]
        exclude_verus_constructs: bool,

        /// Exclude trait and impl methods
        #[arg(long)]
        exclude_methods: bool,

        /// Show function visibility (pub/private)
        #[arg(long)]
        show_visibility: bool,

        /// Show function kind (fn, spec fn, proof fn, etc.)
        #[arg(long)]
        show_kind: bool,

        /// Output JSON to specified file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Run Verus verification and analyze results, or analyze existing output
    ///
    /// If no project_path is given, uses cached verification output from data/verification_output.txt
    #[command(name = "run-verus")]
    RunVerus {
        /// Path to the Rust/Verus project (optional if using cached output)
        project_path: Option<PathBuf>,

        /// Analyze existing verification output file instead of running verification
        #[arg(long)]
        from_file: Option<PathBuf>,

        /// Exit code from the verification command (only used with --from-file)
        #[arg(long)]
        exit_code: Option<i32>,

        /// Package to verify (for workspace projects)
        #[arg(short, long)]
        package: Option<String>,

        /// Module to verify (e.g., backend::serial::u64::field_verus)
        #[arg(long)]
        verify_only_module: Option<String>,

        /// Function to verify
        #[arg(long)]
        verify_function: Option<String>,

        /// Output JSON results to specified file (default: proofs.json)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Don't cache the verification output
        #[arg(long)]
        no_cache: bool,

        /// Path to atoms.json for code-name enrichment (auto-discovers in .verilib/probes/ if omitted)
        #[arg(short = 'a', long)]
        with_atoms: Option<PathBuf>,

        /// Extra arguments passed to Verus after -- (e.g. --log smt --log-dir ./smt-logs -V spinoff-all)
        #[arg(long, num_args = 1.., allow_hyphen_values = true)]
        verus_args: Vec<String>,
    },

    /// Extract function specifications (requires/ensures) to JSON
    Specify {
        /// Path to search (file or directory)
        path: PathBuf,

        /// Output file path (default: .verilib/probes/verus_<pkg>_<ver>_specs.json)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Path to atoms.json file for code-name lookup (required for dictionary output)
        #[arg(short = 'a', long)]
        with_atoms: PathBuf,

        /// Include raw specification text (requires/ensures clauses) in output
        #[arg(long)]
        with_spec_text: bool,

        /// Path to taxonomy TOML config for spec classification labels
        #[arg(long)]
        taxonomy_config: Option<PathBuf>,

        /// Print detailed taxonomy classification explanations (requires --taxonomy-config)
        #[arg(long)]
        taxonomy_explain: bool,

        /// Project root for metadata (default: auto-detect from path via Cargo.toml)
        #[arg(long)]
        project_path: Option<PathBuf>,
    },

    /// Generate specs_data.json for the specs browser
    ///
    /// Replaces the Python scripts (extract_specs.py + analyze_verus_specs_proofs.py)
    /// by auto-discovering all functions from the AST. Outputs JSON matching the
    /// existing specs_data.json schema consumed by docs/specs.js.
    #[command(name = "specs-data")]
    SpecsData {
        /// Path to the source directory (e.g., curve25519-dalek/src)
        src_path: PathBuf,

        /// Output file path (default: specs_data.json)
        #[arg(short, long, default_value = "specs_data.json")]
        output: PathBuf,

        /// GitHub base URL for source links
        #[arg(long)]
        github_base_url: Option<String>,

        /// Path to libsignal entrypoints JSON (focus_dalek_entrypoints.json)
        #[arg(long)]
        libsignal_entrypoints: Option<PathBuf>,

        /// Project root for metadata (default: auto-detect from src_path via Cargo.toml)
        #[arg(long)]
        project_path: Option<PathBuf>,
    },

    /// Generate tracked functions CSV for the dashboard
    ///
    /// Replaces analyze_verus_specs_proofs.py by auto-discovering all functions
    /// with specs from the AST. Outputs CSV with columns:
    /// function,module,link,has_spec,has_proof
    #[command(name = "tracked-csv")]
    TrackedCsv {
        /// Path to the source directory (e.g., curve25519-dalek/src)
        src_path: PathBuf,

        /// Output file path (default: outputs/curve25519_functions.csv)
        #[arg(short, long, default_value = "outputs/curve25519_functions.csv")]
        output: PathBuf,

        /// GitHub base URL for source links
        #[arg(long)]
        github_base_url: Option<String>,
    },

    /// Convert .md files with YAML frontmatter to JSON
    ///
    /// Walks a directory hierarchy of .md files (like those in .verilib/structure),
    /// parses the YAML frontmatter from each file, and outputs a JSON file where
    /// keys are the file paths and values are the frontmatter fields.
    Stubify {
        /// Path to directory containing .md files
        path: PathBuf,

        /// Output file path (default: .verilib/probes/verus_<pkg>_<ver>_stubs.json)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Project root for metadata (default: auto-detect from path via Cargo.toml)
        #[arg(long)]
        project_path: Option<PathBuf>,
    },

    /// Find which crates a function's callees belong to
    ///
    /// Given a function and a depth N, traverses the call graph up to depth N
    /// and reports which crates the discovered callees belong to.
    #[command(name = "callee-crates")]
    CalleeCrates {
        /// Function code-name (probe:...) or display-name to search for
        function: String,

        /// Maximum traversal depth (1 = direct callees, 2 = callees of callees, etc.)
        #[arg(short, long)]
        depth: usize,

        /// Path to atoms.json file (reads from stdin if omitted)
        #[arg(short, long)]
        atoms_file: Option<PathBuf>,

        /// Output file path (prints to stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Exclude standard library crates (core, alloc, std) from output
        #[arg(long)]
        exclude_stdlib: bool,

        /// Exclude specific crates from output (comma-separated list)
        #[arg(long, value_delimiter = ',')]
        exclude_crates: Vec<String>,
    },

    /// Install or check status of external tools (verus-analyzer, scip)
    ///
    /// Resolves and installs verus-analyzer and scip into ~/.probe-verus/tools/.
    /// Version resolution uses, in order: environment variable overrides
    /// (PROBE_VERUS_ANALYZER_VERSION, PROBE_SCIP_VERSION), the latest GitHub
    /// release, and a compiled-in fallback version. Use --status to see which
    /// tools are installed and where they are located.
    Setup {
        /// Show installation status instead of installing
        #[arg(long)]
        status: bool,
    },

    /// Unified pipeline: atomize + specify + run-verus
    ///
    /// This is the recommended entrypoint for Docker containers and CI pipelines.
    /// Runs atomize, specify, and run-verus in sequence, with proper error handling
    /// and JSON output. Individual steps can be skipped with --skip-* flags.
    #[command(name = "extract")]
    Extract {
        /// Path to the Rust/Verus project
        project_path: PathBuf,

        /// Output directory for the extract_summary.json (default: ./output)
        #[arg(short, long, default_value = DEFAULT_OUTPUT_DIR)]
        output: PathBuf,

        /// Skip the atomize step
        #[arg(long)]
        skip_atomize: bool,

        /// Skip the specify step
        #[arg(long)]
        skip_specify: bool,

        /// Skip the run-verus step (cargo verus verification)
        #[arg(long)]
        skip_verify: bool,

        /// Package name for workspace projects (passed to run-verus)
        #[arg(short, long)]
        package: Option<String>,

        /// Force regeneration of the SCIP index
        #[arg(long)]
        regenerate_scip: bool,

        /// Enable verbose output
        #[arg(short, long)]
        verbose: bool,

        /// Use rust-analyzer instead of verus-analyzer for SCIP generation
        #[arg(long)]
        rust_analyzer: bool,

        /// Continue with warnings instead of failing on duplicate code_names
        #[arg(long)]
        allow_duplicates: bool,

        /// Automatically download missing external tools without prompting
        #[arg(long)]
        auto_install: bool,

        /// Path to existing atoms.json (for use with --skip-atomize)
        #[arg(short = 'a', long)]
        with_atoms: Option<PathBuf>,

        /// Include raw specification text (requires/ensures) in specs output
        #[arg(long)]
        with_spec_text: bool,

        /// Path to taxonomy TOML config for spec classification labels
        #[arg(long)]
        taxonomy_config: Option<PathBuf>,

        /// Extra arguments passed to Verus (e.g. --log smt --log-dir ./smt-logs)
        #[arg(long, num_args = 1.., allow_hyphen_values = true)]
        verus_args: Vec<String>,

        /// Also write separate atoms, specs, and proofs files (in addition to unified output)
        #[arg(long)]
        separate_outputs: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Atomize {
            project_path,
            output,
            regenerate_scip,
            with_locations,
            rust_analyzer,
            allow_duplicates,
            auto_install,
        } => {
            cmd_atomize(
                project_path,
                output,
                regenerate_scip,
                with_locations,
                rust_analyzer,
                allow_duplicates,
                auto_install,
            );
        }
        Commands::MergeAtoms { inputs, output } => {
            cmd_merge_atoms(inputs, output);
        }
        Commands::ListFunctions {
            path,
            format,
            exclude_verus_constructs,
            exclude_methods,
            show_visibility,
            show_kind,
            output,
        } => {
            cmd_functions(
                path,
                format,
                exclude_verus_constructs,
                exclude_methods,
                show_visibility,
                show_kind,
                output,
            );
        }
        Commands::RunVerus {
            project_path,
            from_file,
            exit_code,
            package,
            verify_only_module,
            verify_function,
            output,
            no_cache,
            with_atoms,
            verus_args,
        } => {
            cmd_run_verus(
                project_path,
                from_file,
                exit_code,
                package,
                verify_only_module,
                verify_function,
                output,
                no_cache,
                with_atoms,
                verus_args,
            );
        }
        Commands::Specify {
            path,
            output,
            with_atoms,
            with_spec_text,
            taxonomy_config,
            taxonomy_explain,
            project_path,
        } => {
            cmd_specify(
                path,
                output,
                with_atoms,
                with_spec_text,
                taxonomy_config,
                taxonomy_explain,
                project_path,
            );
        }
        Commands::SpecsData {
            src_path,
            output,
            github_base_url,
            libsignal_entrypoints,
            project_path,
        } => {
            cmd_specs_data(
                src_path,
                output,
                github_base_url,
                libsignal_entrypoints,
                project_path,
            );
        }
        Commands::TrackedCsv {
            src_path,
            output,
            github_base_url,
        } => {
            cmd_tracked_csv(src_path, output, github_base_url);
        }
        Commands::CalleeCrates {
            function,
            depth,
            atoms_file,
            output,
            exclude_stdlib,
            exclude_crates,
        } => {
            cmd_callee_crates(
                function,
                depth,
                atoms_file,
                output,
                exclude_stdlib,
                exclude_crates,
            );
        }
        Commands::Stubify {
            path,
            output,
            project_path,
        } => {
            cmd_stubify(path, output, project_path);
        }
        Commands::Setup { status } => {
            cmd_setup(status);
        }
        Commands::Extract {
            project_path,
            output,
            skip_atomize,
            skip_specify,
            skip_verify,
            package,
            regenerate_scip,
            verbose,
            rust_analyzer,
            allow_duplicates,
            auto_install,
            with_atoms,
            with_spec_text,
            taxonomy_config,
            verus_args,
            separate_outputs,
        } => {
            cmd_extract(
                project_path,
                output,
                skip_atomize,
                skip_specify,
                skip_verify,
                package,
                regenerate_scip,
                verbose,
                rust_analyzer,
                allow_duplicates,
                auto_install,
                with_atoms,
                with_spec_text,
                taxonomy_config,
                verus_args,
                separate_outputs,
            );
        }
    }
}
