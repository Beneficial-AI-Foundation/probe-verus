# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

probe-verus is a Rust CLI tool that generates compact function call graph data from SCIP (Source Code Index Protocol) indexes and analyzes Verus verification results. Subcommands:
- **extract**: Unified pipeline - atomize + specify + run-verus (designed for Docker/CI usage)
- **atomize**: Generate call graph atoms with accurate line numbers
- **callee-crates**: Find which crates a function's callees belong to at a given depth
- **list-functions**: List all functions in a Rust/Verus project (no external tools needed)
- **merge-atoms**: Combine independently-indexed atoms.json files
- **run-verus**: Run Verus verification and analyze results (standalone)
- **setup**: Install or check status of external tools (verus-analyzer, scip) via auto-download
- **specify**: Extract function specifications from atoms.json, with optional taxonomy classification
- **stubify**: Convert .md files with YAML frontmatter to JSON

## Build and Test Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Optimized release build
cargo install --path .         # Install locally

# Test
cargo test                     # All tests
cargo test --lib --verbose     # Unit tests only
cargo test --test duplicate_symbols --verbose    # Integration test
cargo test --test function_coverage --verbose -- --nocapture
cargo test --test extract_backward_compat -- --nocapture  # Backward compat (needs verus-analyzer, scip)

# Update backward-compat golden file after intentional output changes
BLESS=1 cargo test --test extract_backward_compat -- --nocapture

# Code quality (all enforced in CI)
cargo fmt --all                # Format code
cargo clippy --all-targets -- -D warnings  # Lint (no warnings allowed)

# Development workflow
cargo fmt && cargo clippy --all-targets && cargo test
```

## Project Structure

```
src/
├── main.rs           # CLI entry point with subcommand routing
├── lib.rs            # Core data structures and SCIP JSON parsing
├── metadata.rs       # Schema 2.0 envelope construction, project metadata gathering
├── commands/         # Subcommand implementations (extract, atomize, run_verus, specify, setup, etc.)
├── scip_cache.rs     # SCIP index generation, caching, and tool resolution
├── taxonomy.rs       # Spec taxonomy classification from TOML rules
├── tool_manager.rs   # Auto-download manager for external tools (verus-analyzer, scip)
├── verification.rs   # Verification output parsing & analysis
└── verus_parser.rs   # AST parsing using verus_syn for function spans
```

## Architecture

### Main Pipelines

1. **Extract Pipeline** (`extract` command): Unified 3-step pipeline (atomize + specify + run-verus) producing a single unified JSON output (`probe-verus/extract` schema) where each atom includes optional `primary-spec`, `is-disabled`, `verification-status`, `spec-labels`, and categorized dependency fields. Uses `--separate-outputs` to also write individual files. Recommended CI/Docker entrypoint.
2. **Atomize Pipeline** (`atomize` command): SCIP JSON → call graph parsing → spans via verus_syn → Schema 2.0 envelope → `.verilib/probes/`
3. **List Functions Pipeline** (`list-functions` command): Source files → AST visitor → function list
4. **Run-Verus Pipeline** (`run-verus` command): Cargo verus output → error parsing → function mapping → Schema 2.0 envelope → `.verilib/probes/`
5. **Specify Pipeline** (`specify` command): Source files + atoms.json → spec extraction → optional taxonomy classification via TOML rules → Schema 2.0 envelope → `.verilib/probes/`
6. **Setup Pipeline** (`setup` command): Resolve versions → download from GitHub → decompress to `~/.probe-verus/tools/`

### Key Architectural Patterns

**Accurate Line Spans**: SCIP only provides function name locations. Uses `verus_syn` AST visitor to parse actual function body spans (~95% accuracy). Handles Verus-specific syntax (`verus!{}` blocks, `spec fn`, `proof fn`).

**Interval Trees for Performance**: Error-to-function mapping uses `rust-lapper` for O(log n) lookups instead of linear scans.

**Trait Implementation Disambiguation**: Multiple strategies to resolve SCIP symbol conflicts for trait impls: signature text extraction, self type from parameters, definition type context, line number fallback.

**SCIP Data Caching**: Generated SCIP data is cached in `<project>/data/` to avoid re-running slow external tools.

**Auto-download Tool Manager**: External tools (verus-analyzer, scip) can be auto-downloaded to `~/.probe-verus/tools/`. Version resolution: env var override → GitHub `/releases/latest` API → compiled-in fallback. Supports `--auto-install` flag for non-interactive CI usage.

**AST-based Spec Taxonomy**: The `specify` command can classify specs using taxonomy rules defined in TOML. Classification uses structured AST data (function mode, called function names extracted via `verus_syn` visitor) rather than regex on text. A `CallNameCollector` visitor walks `ExprCall`/`ExprMethodCall` nodes in ensures/requires clauses to extract called function names.

**Schema 2.0 Metadata Envelope**: All JSON outputs are wrapped in a standardized envelope containing `schema`, `schema-version`, `tool`, `source`, `timestamp`, and `data` fields. The `metadata.rs` module handles envelope construction, project metadata gathering (git commit, repo URL, Cargo.toml parsing), and default output path resolution to `.verilib/probes/`.

**Config Structs for Internal APIs**: `atomize_internal`, `specify_internal`, and `run_verus_internal` use `AtomizeInternalConfig`, `SpecifyInternalConfig`, and `ExtractInternalConfig` structs (defined in `metadata.rs`) instead of long parameter lists. The `extract` command gathers metadata once and passes it via config structs so all steps share a consistent timestamp.

### Key Types

- `FunctionNode`: Call graph node with callees and type context
- `AtomWithLines`: Output format with line ranges
- `UnifiedAtom`: `AtomWithLines` + optional `primary-spec` text, `is-disabled`, `verification-status`, `spec-labels`, and categorized dependency fields (extract pipeline output)
- `FunctionInfo`: Function metadata with mode, specs, ensures/requires calls
- `TaxonomyConfig`, `TaxonomyRule`, `MatchCriteria`: TOML-based spec classification rules
- `FunctionInterval`: Interval tree entry for error→function mapping
- `CompilationError`, `VerificationFailure`: Error types for verification analysis
- `Envelope<T>`, `MergedEnvelope<T>`: Schema 2.0 metadata wrappers for JSON output
- `ProjectMetadata`: Git commit, repo URL, timestamp, package name/version
- `AtomizeInternalConfig`, `SpecifyInternalConfig`, `ExtractInternalConfig`: Config structs for internal command APIs

## External Tool Dependencies

- **extract command**: Same as atomize + specify + run-verus (unified pipeline)
- **atomize command**: Requires `verus-analyzer` and `scip` CLI (auto-downloadable via `setup` or `--auto-install`)
- **list-functions command**: None (uses verus_syn only)
- **run-verus command**: Requires `cargo verus`
- **specify command**: None (uses verus_syn only; optional TOML config for taxonomy)
- **setup command**: None (downloads tools itself)

## Before Committing

Always run fmt and clippy before committing and pushing:

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

## Commit Message Style

Use conventional commits: `feat(module):`, `fix(module):`, `perf(module):`, `refactor(module):`

Examples from history:
- `feat(specify): output dictionary keyed by probe-name from atoms.json`
- `fix(verification): update atoms.json reader for new schema`
- `perf(verify): use interval tree for error-to-function mapping`

## Probe Ecosystem Knowledge Base

The **probe KB** in the sibling `probe` repo (`baif/probe/kb/`) is the source of truth for cross-cutting properties and schema definitions. Key references:

- **`kb/engineering/properties.md`** -- Properties P1–P19 that all probe tools must satisfy (e.g., P14: deterministic output, P15: dependency completeness)
- **`kb/engineering/schema.md`** -- Schema 2.0 envelope and atom field definitions
- **`kb/tools/probe-verus.md`** -- probe-verus-specific documentation in the KB

If implementation contradicts the KB, fix the code, not the KB.

## Versioning Policy

This project follows [Semantic Versioning](https://semver.org/) (see [issue #7](https://github.com/Beneficial-AI-Foundation/probe-verus/issues/7)). Downstream tools like `verilib-cli` invoke `probe-verus` as a subprocess and depend on a stable CLI contract. The version number must accurately signal compatibility.

All notable changes must be recorded in `CHANGELOG.md` using [Keep a Changelog](https://keepachangelog.com/) format.

The backward compatibility test (`tests/extract_backward_compat.rs`) enforces JSON output stability by comparing extract output against a golden file. New additive fields pass; removed or changed fields fail. Update the golden file with `BLESS=1 cargo test --test extract_backward_compat -- --nocapture` when making intentional changes.

### What requires a major version bump

Any non-backward-compatible change to the **public contract**:

- Renamed or removed subcommands (`extract`, `atomize`, `run-verus`, `specify`, `list-functions`, `stubify`, `specs-data`, `tracked-csv`)
- Renamed or removed CLI flags (e.g., `--with-atoms`, `--output`, `--with-locations`)
- Changed semantics of existing flags
- Changed JSON output field names or structure (e.g., renaming `display-name`, changing dict output to array)
- Changed exit codes (currently 0 = success, 1 = error)
- Changed required input file formats

Major bumps must include a `Breaking` section in `CHANGELOG.md`.

### What is a minor version bump

Backward-compatible additions:

- New subcommands
- New optional flags on existing subcommands
- New optional fields in JSON output (additive)
- New output formats selectable via new flags

### What is a patch version bump

- Bug fixes that don't change the public contract
- Performance improvements
- Documentation updates
