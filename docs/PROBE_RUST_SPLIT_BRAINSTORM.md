# Brainstorm: Splitting probe-verus into probe-rust + probe-verus

*Date: 2026-03-10*

Should we factor out pure-Rust atomization into a new `probe-rust` tool (with `atomize` and `callee-crates`) and keep `probe-verus` for Verus-only concerns?

## Current Coupling Analysis

The codebase already has a natural seam between pure-Rust and Verus-specific code.

### Pure Rust (no verus_syn dependency)

| Module / Command | Notes |
|---|---|
| `callee-crates` | BFS over `atoms.json`, grouping by crate. No Verus deps at all. |
| `merge-atoms` | Generic envelope merge logic. Works with any atoms envelope. |
| `stubify` | YAML frontmatter to JSON. |
| `setup` | Tool manager (downloads verus-analyzer, scip). |
| `scip_cache.rs` | SCIP index generation. Supports both verus-analyzer and rust-analyzer. |
| `tool_manager.rs` | Auto-download manager. Generic resolution/download logic. |
| `metadata.rs` (partial) | `Envelope<T>`, `ProjectMetadata`, `wrap_in_envelope`, `gather_metadata`. |
| `lib.rs` (partial) | SCIP types, `FunctionNode`, `AtomWithLines`, `build_call_graph`, `parse_scip_json`. |

### Verus-Specific

| Module / Command | Notes |
|---|---|
| `extract` | Parses `cargo verus` output, maps errors to functions. |
| `list-functions` | Uses `verus_syn` AST visitor. |
| `specify` | Extracts requires/ensures via `verus_syn`, optional taxonomy. |
| `specs-data` | Uses `verus_parser::parse_all_functions_ext`. |
| `tracked-csv` | Uses `verus_parser::parse_all_functions_ext`. |
| `verus_parser.rs` | Full `verus_syn` parser: `FnMode`, `verus!{}` blocks, spec clauses. |
| `verification.rs` | Parses verification output, interval-tree mapping. |
| `taxonomy.rs` | Spec classification from TOML rules. Operates on Verus `FunctionInfo`. |

### Mixed

| Module / Command | Notes |
|---|---|
| `atomize` | Core is pure Rust (SCIP parsing, call graph). Accurate spans use `verus_parser::build_function_span_map` (verus_syn). Two codepaths: `convert_to_atoms_with_lines` (SCIP-only) and `convert_to_atoms_with_parsed_spans` (verus_syn). Already has `--rust_analyzer` flag. |
| `run` | Orchestrates atomize + verify. Verify step is Verus-specific. |
| `lib.rs` (partial) | `DeclKind` (exec/proof/spec) and `CallLocation` (Precondition/Postcondition/Inner) are Verus concepts embedded in shared types like `AtomWithLines`. |
| `metadata.rs` (partial) | Hardcoded `"probe-verus"` tool name, `verus_{pkg}_{ver}` output path prefix, `ExtractInternalConfig`. |

## What a Split Would Look Like

The `callee-crates` sharing requirement means a split produces **three** crates, not two:

```
probe-core (shared library)
├── Envelope types, metadata, SCIP parsing, call graph, AtomWithLines
├── callee-crates, merge-atoms logic
└── tool_manager, scip_cache

probe-rust (binary, depends on probe-core)
├── atomize (SCIP-only spans)
├── callee-crates (re-exported from probe-core)
└── setup (rust-analyzer + scip)

probe-verus (binary, depends on probe-core)
├── atomize (verus_syn-enhanced spans)
├── callee-crates (re-exported from probe-core)
├── verify, specify, list-functions, specs-data, tracked-csv
└── verus_parser, verification, taxonomy
```

## Why the Effort Doesn't Pay Off (Yet)

1. **Large refactoring surface for modest gain.** `metadata.rs` has hardcoded `"probe-verus"` tool names and `verus_{pkg}_{ver}` output paths. `lib.rs` mixes SCIP types with `DeclKind` in the same `AtomWithLines` struct. All of this needs parameterizing or generalizing.

2. **The existing architecture already handles pure Rust.** The `--rust_analyzer` flag + `convert_to_atoms_with_lines` already gives a working pure-Rust atomize path. `callee-crates` doesn't care whether the atoms came from Verus or Rust.

3. **Three-crate maintenance cost.** Version coordination, shared CI, synchronized releases, separate changelogs. Downstream consumer `verilib-cli` would potentially depend on two CLIs.

4. **Schema divergence complexity.** `probe-verus/atoms` vs `probe-rust/atoms` schemas would need to stay compatible for `merge-atoms` and downstream tooling. `DeclKind` would be "always exec" in probe-rust atoms, which is mostly noise.

## Lighter Alternative: Cargo Features

Instead of splitting repos, use Cargo features to make `verus_syn` optional:

```toml
[features]
default = ["verus"]
verus = ["dep:verus_syn"]
```

- `cargo install probe-verus` -- full Verus tool (current behavior)
- `cargo install probe-verus --no-default-features` -- pure Rust tool (no verus_syn, Verus-specific commands hidden/disabled)

This gives a smaller binary for pure-Rust users without the repo split. The `callee-crates` sharing problem vanishes because it's all one crate.

Another option: **subcommand grouping** (`probe-verus rust atomize` vs `probe-verus verus verify`) to clarify which commands are Verus-specific vs generic.

## When a Split Would Make Sense

- **probe-rust gets its own verification story** (e.g., integrating with Kani, Prusti, or Creusot) -- then the "verify" divergence justifies separate tools.
- **Significant non-Verus user base emerges** that is confused or burdened by the Verus-specific commands.
- **verus_syn becomes a build headache** (long compile times, nightly-only, etc.) that pure-Rust users shouldn't have to bear.

## Decision

**Keep as one crate for now.** If a cleaner UX for pure-Rust users is needed later, start with Cargo features (compile-time split, zero maintenance overhead) before considering a full repo split.
