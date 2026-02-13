# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

probe-verus is a Rust CLI tool that generates compact function call graph data from SCIP (Source Code Index Protocol) indexes and analyzes Verus verification results. It has four subcommands:
- **atomize**: Generate call graph atoms with accurate line numbers
- **list-functions**: List all functions in a Rust/Verus project (no external tools needed)
- **verify**: Run Verus verification and analyze results
- **specify**: Extract function specifications from atoms.json, with optional taxonomy classification

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
├── taxonomy.rs       # Spec taxonomy classification from TOML rules
├── verification.rs   # Verification output parsing & analysis
└── verus_parser.rs   # AST parsing using verus_syn for function spans
```

## Architecture

### Four Main Pipelines

1. **Atomize Pipeline** (`atomize` command): SCIP JSON → call graph parsing → spans via verus_syn → JSON output
2. **List Functions Pipeline** (`list-functions` command): Source files → AST visitor → function list
3. **Verification Pipeline** (`verify` command): Cargo verus output → error parsing → function mapping → analysis
4. **Specify Pipeline** (`specify` command): Source files + atoms.json → spec extraction → optional taxonomy classification via TOML rules → JSON output

### Key Architectural Patterns

**Accurate Line Spans**: SCIP only provides function name locations. Uses `verus_syn` AST visitor to parse actual function body spans (~95% accuracy). Handles Verus-specific syntax (`verus!{}` blocks, `spec fn`, `proof fn`).

**Interval Trees for Performance**: Error-to-function mapping uses `rust-lapper` for O(log n) lookups instead of linear scans.

**Trait Implementation Disambiguation**: Multiple strategies to resolve SCIP symbol conflicts for trait impls: signature text extraction, self type from parameters, definition type context, line number fallback.

**SCIP Data Caching**: Generated SCIP data is cached in `<project>/data/` to avoid re-running slow external tools.

**AST-based Spec Taxonomy**: The `specify` command can classify specs using taxonomy rules defined in TOML. Classification uses structured AST data (function mode, called function names extracted via `verus_syn` visitor) rather than regex on text. A `CallNameCollector` visitor walks `ExprCall`/`ExprMethodCall` nodes in ensures/requires clauses to extract called function names.

### Key Types

- `FunctionNode`: Call graph node with callees and type context
- `AtomWithLines`: Output format with line ranges
- `FunctionInfo`: Function metadata with mode, specs, ensures/requires calls
- `TaxonomyConfig`, `TaxonomyRule`, `MatchCriteria`: TOML-based spec classification rules
- `FunctionInterval`: Interval tree entry for error→function mapping
- `CompilationError`, `VerificationFailure`: Error types for verification analysis

## External Tool Dependencies

- **atomize command**: Requires `verus-analyzer` and `scip` CLI
- **list-functions command**: None (uses verus_syn only)
- **verify command**: Requires `cargo verus`
- **specify command**: None (uses verus_syn only; optional TOML config for taxonomy)

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
