# Usage Guide

## Commands

### `extract`

Run the unified 3-step pipeline: `atomize` + `specify` + `run-verus`. Produces
a single unified JSON file (`probe-verus/extract` schema) where each atom entry
includes optional `verification-status` and `primary-spec` fields. A pipeline
summary (`extract_summary.json`) is also written to the output directory.

```
probe-verus extract <PROJECT_PATH> [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--output <DIR>` | `-o` | Output directory (default: `./output`) |
| `--package <NAME>` | `-p` | Package to verify (for workspaces) |
| `--skip-atomize` | | Skip the atomize step |
| `--skip-specify` | | Skip the specify step |
| `--skip-verify` | | Skip the run-verus step |
| `--separate-outputs` | | Also write individual atoms, specs, and proofs files |
| `--regenerate-scip` | | Force regeneration of the SCIP index |
| `--verbose` | `-v` | Verbose output |
| `--rust-analyzer` | | Use rust-analyzer instead of verus-analyzer for SCIP |
| `--allow-duplicates` | | Allow duplicate probe-names (normally fatal) |
| `--auto-install` | | Automatically download missing tools without prompting |
| `--with-atoms <PATH>` | `-a` | Path to atoms.json (for use with `--skip-atomize`) |
| `--with-spec-text` | | Include raw specification text in specify output |
| `--taxonomy-config <PATH>` | | Path to TOML file for spec classification |
| `--verus-args <ARGS>...` | | Extra arguments passed to cargo verus |

### Examples

```bash
# Full pipeline on a workspace member
probe-verus extract ./my-verus-project -p my-crate

# CI-friendly with auto-install
probe-verus extract ./my-verus-project --auto-install

# Also write individual atoms/specs/proofs files
probe-verus extract ./my-verus-project --separate-outputs

# Skip atomize, use existing atoms
probe-verus extract ./my-verus-project --skip-atomize -a path/to/atoms.json
```

---

### `atomize`

Generate call graph atoms with line numbers from SCIP indexes.

```
probe-verus atomize <PROJECT_PATH> [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--output <FILE>` | `-o` | Output file path (default: `.verilib/probes/verus_<pkg>_<ver>_atoms.json`) |
| `--regenerate-scip` | `-r` | Force regeneration of the SCIP index |
| `--with-locations` | | Include detailed per-call location info (precondition/postcondition/inner) |
| `--auto-install` | | Automatically download missing tools without prompting |

### Examples

```bash
probe-verus atomize ./my-rust-project
probe-verus atomize ./my-rust-project --with-locations
probe-verus atomize ./my-rust-project --auto-install
```

**Extended output (`--with-locations`):**

When using `--with-locations`, an additional `dependencies-with-locations` field is included:

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "display-name": "my_function",
    "dependencies": ["probe:crate/1.0.0/other/helper()"],
    "dependencies-with-locations": [
      {
        "code-name": "probe:crate/1.0.0/other/helper()",
        "location": "precondition",
        "line": 45
      },
      {
        "code-name": "probe:crate/1.0.0/other/helper()",
        "location": "inner",
        "line": 52
      }
    ],
    "code-module": "module",
    "code-path": "src/lib.rs",
    "code-text": { "lines-start": 42, "lines-end": 100 },
    "kind": "exec",
    "language": "rust"
  }
}
```

The `location` field indicates where the call occurs:
- **`precondition`**: Inside a `requires` clause
- **`postcondition`**: Inside an `ensures` clause
- **`inner`**: Inside the function body

---

### `specify`

Extract function specifications (requires/ensures clauses) from source files,
keyed by probe-name from atoms.json. Optionally classify each function's spec
with taxonomy labels.

```
probe-verus specify <PATH> -a <ATOMS_FILE> [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--output <FILE>` | `-o` | Output file path (default: `.verilib/probes/verus_<pkg>_<ver>_specs.json`) |
| `--with-atoms <FILE>` | `-a` | Path to atoms.json for code-name lookup (required) |
| `--with-spec-text` | | Include raw specification text in output |
| `--taxonomy-config <FILE>` | | Path to TOML file defining spec classification rules |
| `--project-path <PATH>` | | Project root for metadata (default: auto-detect via Cargo.toml) |

### Examples

```bash
# Extract specs using atoms.json for probe-name mapping
probe-verus specify ./src -a atoms.json

# Include raw requires/ensures text
probe-verus specify ./src -a atoms.json --with-spec-text

# Classify specs with taxonomy labels
probe-verus specify ./src -a atoms.json --with-spec-text \
  --taxonomy-config spec_taxonomy_examples/spec-taxonomy-curve25519-dalek.toml
```

**Output format:**

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "code-path": "src/lib.rs",
    "spec-text": { "lines-start": 42, "lines-end": 60 },
    "kind": "exec",
    "specified": true,
    "has_requires": true,
    "has_ensures": true,
    "has_decreases": false,
    "has_trusted_assumption": false
  }
}
```

**Extended output (`--with-spec-text`):**

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "code-path": "src/lib.rs",
    "spec-text": { "lines-start": 42, "lines-end": 60 },
    "kind": "exec",
    "specified": true,
    "has_requires": true,
    "has_ensures": true,
    "has_decreases": false,
    "has_trusted_assumption": false,
    "requires_text": "requires\n        x > 0 && y > 0,",
    "ensures_text": "ensures\n        result == x + y,",
    "ensures-calls": ["spec_add"],
    "requires-calls": []
  }
}
```

---

### `run-verus`

Run Verus verification on a project and analyze results.

```
probe-verus run-verus [PROJECT_PATH] [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--from-file <FILE>` | | Analyze existing output file instead of running verification |
| `--exit-code <CODE>` | | Exit code (only used with `--from-file`) |
| `--package <NAME>` | `-p` | Package to verify (for workspaces) |
| `--verify-only-module <MOD>` | | Module to verify |
| `--verify-function <FUNC>` | | Function to verify |
| `--output <FILE>` | `-o` | Output file path (default: `.verilib/probes/verus_<pkg>_<ver>_proofs.json`) |
| `--no-cache` | | Don't cache the verification output |
| `--with-atoms <FILE>` | `-a` | Path to atoms.json for code-name enrichment |
| `--verus-args <ARGS>...` | | Extra arguments passed to cargo verus |

### Examples

```bash
# Run verification (caches output automatically)
probe-verus run-verus ./my-verus-project -p my-crate

# Use cached output (no project path needed)
probe-verus run-verus

# Analyze existing output file
probe-verus run-verus ./my-project --from-file verification_output.txt
```

**Output format:**

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "code-path": "src/lib.rs",
    "code-line": 456,
    "verified": true,
    "status": "success"
  }
}
```

Status values: `success`, `failure`, `sorries`, `warning`.

---

### `list-functions`

List all functions in a Rust/Verus project.

```
probe-verus list-functions <PATH> [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--format <FORMAT>` | `-f` | `text`, `json`, or `detailed` (default: text) |
| `--exclude-verus-constructs` | | Exclude spec/proof/exec functions |
| `--exclude-methods` | | Exclude trait and impl methods |
| `--show-visibility` | | Show pub/private |
| `--show-kind` | | Show fn/spec fn/proof fn/etc. |
| `--output <FILE>` | `-o` | Write JSON to file |

---

### `setup`

Install or check status of external tool dependencies (verus-analyzer, scip).

```
probe-verus setup [OPTIONS]
```

| Flag | Description |
|------|-------------|
| `--status` | Show installation status instead of installing |

Version resolution uses, in order:
1. Environment variable overrides (`PROBE_VERUS_ANALYZER_VERSION`, `PROBE_SCIP_VERSION`)
2. Latest stable release from GitHub
3. Compiled-in fallback version

Tools are installed to `~/.probe-verus/tools/`.

---

### `stubify`

Convert a directory of `.md` files with YAML frontmatter to a JSON file.

```
probe-verus stubify <PATH> [OPTIONS]
```

| Flag | Short | Description |
|------|-------|-------------|
| `--output <FILE>` | `-o` | Output file path (default: `.verilib/probes/verus_<pkg>_<ver>_stubs.json`) |
| `--project-path <PATH>` | | Project root for metadata |

Expected input format: each `.md` file has YAML frontmatter with `code-line`,
`code-path`, and `code-name` fields.

---

## Taxonomy Config Format

The `specify` command can classify specs using taxonomy rules defined in TOML.
Each rule specifies a label and match criteria. All rules are evaluated
independently, and all matching labels are applied.

```toml
[taxonomy]
version = "1"

[[taxonomy.rules]]
label = "functional-correctness"
description = "Output matches a mathematical model"
trust = "highest"

[taxonomy.rules.match]
mode = ["exec"]
ensures_calls_contain = ["spec_", "_to_nat"]
```

**Rule semantics:**
- All specified criteria within a rule must match (AND logic)
- Within a list criterion, any match suffices (OR logic)

**Available match criteria:**

| Criterion | Type | Description |
|-----------|------|-------------|
| `mode` | string list | Function mode: `exec`, `proof`, `spec` |
| `context` | string list | Function context: `impl`, `trait`, `standalone` |
| `ensures_calls_contain` | substring list | Match against function names called in ensures |
| `requires_calls_contain` | substring list | Match against function names called in requires |
| `name_contains` | substring list | Match against function display name |
| `path_contains` | substring list | Match against code-path |
| `has_ensures` | bool | Whether function has ensures clause |
| `has_requires` | bool | Whether function has requires clause |
| `has_decreases` | bool | Whether function has decreases clause |
| `has_trusted_assumption` | bool | Whether function uses assume()/admit() |

Example taxonomy configs are in [`spec_taxonomy_examples/`](../spec_taxonomy_examples/).

---

## Output Format

For the complete JSON schema specification covering all commands, see
[SCHEMA.md](SCHEMA.md).
