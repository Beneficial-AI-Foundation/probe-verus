# probe-verus

Probe Verus projects: generate call graph atoms and analyze verification results.

## Installation

### Pre-built binaries (recommended)

Download the latest release for your platform from
[GitHub Releases](https://github.com/Beneficial-AI-Foundation/probe-verus/releases),
or use the one-line installers:

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Beneficial-AI-Foundation/probe-verus/releases/latest/download/probe-verus-installer.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy ByPass -c "irm https://github.com/Beneficial-AI-Foundation/probe-verus/releases/latest/download/probe-verus-installer.ps1 | iex"
```

### From source

```bash
cargo install --path .
```

### External tool dependencies

Some commands (`atomize`, `extract`) require external tools. After installing
probe-verus, run `setup` to auto-download them:

```bash
probe-verus setup            # downloads verus-analyzer and scip
probe-verus setup --status   # check what's installed and where
```

Version resolution picks the latest GitHub release by default. Override with
environment variables (`PROBE_VERUS_ANALYZER_VERSION`, `PROBE_SCIP_VERSION`)
or place your own binaries on `PATH`.

For manual installation options, see [tools/INSTALL.md](tools/INSTALL.md).

| Command | Required Tools |
|---------|----------------|
| `atomize` | verus-analyzer, scip |
| `list-functions` | None |
| `run-verus` | cargo verus |
| `specify` | None |
| `setup` | None |
| `extract` | verus-analyzer, scip, cargo verus |

## Commands

```
probe-verus <COMMAND>

Commands:
  atomize         Generate call graph atoms with line numbers from SCIP indexes
  callee-crates   Find which crates a function's callees belong to
  list-functions  List all functions in a Rust/Verus project
  merge-atoms     Combine independently-indexed atoms.json files
  run-verus       Run Verus verification and analyze results
  setup           Install or check status of external tools
  specify         Extract function specifications from atoms.json
  stubify         Convert .md files with YAML frontmatter to JSON
  extract         Run unified pipeline: atomize + specify + run-verus (designed for Docker/CI)
```

---

### `stubify` - Convert Stub Files to JSON

Convert a directory hierarchy of `.md` files with YAML frontmatter to a JSON file. This is useful for processing verification stub files (like those in `.verilib/structure`).

```bash
probe-verus stubify <PATH> [OPTIONS]

Options:
  -o, --output <FILE>           Output file path (default: .verilib/probes/verus_<pkg>_<ver>_stubs.json)
      --project-path <PATH>     Project root for metadata (default: auto-detect via Cargo.toml)
```

**Examples:**
```bash
probe-verus stubify .verilib/structure
probe-verus stubify .verilib/structure -o stubs.json
```

**Expected input format:**

Each `.md` file should have YAML frontmatter with the following fields:

```markdown
---
code-line: 123
code-path: src/lib.rs
code-name: scip:crate/1.0.0/module#function()
---
```

**Output format:**

The output is a dictionary keyed by relative file path:

```json
{
  "edwards.rs/EdwardsPoint.identity().md": {
    "code-line": 821,
    "code-path": "curve25519-dalek/src/edwards.rs",
    "code-name": "scip:curve25519-dalek/4.1.3/edwards/EdwardsPoint#Identity<EdwardsPoint>#identity()"
  },
  "subdir/another.md": {
    "code-line": 456,
    "code-path": "src/main.rs",
    "code-name": "scip:crate/1.0.0/module#another()"
  }
}
```

**Field descriptions:**
- **Key**: Relative path of the `.md` file from the input directory
- **`code-line`**: Line number in the source file
- **`code-path`**: Path to the source file
- **`code-name`**: SCIP-style identifier for the function

---

### `atomize` - Generate Call Graph Data

Generate call graph atoms with line numbers from SCIP indexes.

```bash
probe-verus atomize <PROJECT_PATH> [OPTIONS]

Options:
  -o, --output <FILE>     Output file path (default: .verilib/probes/verus_<pkg>_<ver>_atoms.json)
  -r, --regenerate-scip   Force regeneration of the SCIP index
      --with-locations    Include detailed per-call location info (precondition/postcondition/inner)
      --auto-install      Automatically download missing tools without prompting
```

**Examples:**
```bash
probe-verus atomize ./my-rust-project
probe-verus atomize ./my-rust-project -o atoms.json
probe-verus atomize ./my-rust-project --regenerate-scip
probe-verus atomize ./my-rust-project --with-locations  # extended output
probe-verus atomize ./my-rust-project --auto-install    # download tools if missing
```

**Output format:**

The output is wrapped in a Schema 2.0 metadata envelope. The `data` payload is a dictionary keyed by `probe-name` (a URI-style identifier):

```json
{
  "schema": "probe-verus/atoms",
  "schema-version": "2.0",
  "tool": { "name": "probe-verus", "version": "2.0.0", "command": "atomize" },
  "source": { "repo": "...", "commit": "...", "language": "rust", "package": "...", "package-version": "..." },
  "timestamp": "2026-03-06T12:00:00Z",
  "data": {
    "probe:curve25519-dalek/4.1.3/module/MyType#my_function()": {
      "display-name": "my_function",
      "dependencies": [
        "probe:curve25519-dalek/4.1.3/other_module/helper()"
      ],
      "code-module": "module",
      "code-path": "src/lib.rs",
      "code-text": { "lines-start": 42, "lines-end": 100 },
      "kind": "proof",
      "language": "rust"
    }
  }
}
```

**Field descriptions:**
- **Key (`probe-name`)**: URI-style identifier in format `probe:<crate>/<version>/<module>/<Type>#<method>()`
- **`display-name`**: The function/method name
- **`dependencies`**: List of probe-names this function calls (deduplicated)
- **`code-module`**: The module path (e.g., `"foo/bar"` for nested modules, empty for top-level)
- **`code-path`**: Relative file path
- **`code-text`**: Line range of the function body
- **`kind`**: Declaration kind (`"exec"`, `"proof"`, or `"spec"`)
- **`language`**: Source language (always `"rust"` for probe-verus)

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

This is useful for verification analysis since calls in specifications have different semantics than calls in executable code.

**Note:** Duplicate `probe-name` values are a fatal error (exit code 1).

---

### `list-functions` - List Functions

List all functions in a Rust/Verus project with optional metadata.

```bash
probe-verus list-functions <PATH> [OPTIONS]

Options:
  -f, --format <FORMAT>          text, json, or detailed (default: text)
      --exclude-verus-constructs Exclude spec/proof/exec functions
      --exclude-methods          Exclude trait and impl methods
      --show-visibility          Show pub/private
      --show-kind                Show fn/spec fn/proof fn/etc.
  -o, --output <FILE>            Write JSON to file
```

**Examples:**
```bash
probe-verus list-functions ./src
probe-verus list-functions ./src --format detailed --show-visibility --show-kind
probe-verus list-functions ./my-project --format json
```

---

### `specify` - Extract Function Specifications

Extract function specifications (requires/ensures clauses) from source files, keyed by probe-name from atoms.json. Optionally classify each function's spec with taxonomy labels.

```bash
probe-verus specify <PATH> -a <ATOMS_FILE> [OPTIONS]

Options:
  -o, --output <FILE>              Output file path (default: .verilib/probes/verus_<pkg>_<ver>_specs.json)
  -a, --with-atoms <FILE>          Path to atoms.json for code-name lookup (required)
      --with-spec-text             Include raw specification text in output
      --taxonomy-config <FILE>     Path to TOML file defining spec classification rules
      --project-path <PATH>        Project root for metadata (default: auto-detect via Cargo.toml)
```

**Examples:**
```bash
# Extract specs using atoms.json for probe-name mapping
probe-verus specify ./src -a atoms.json

# Include raw requires/ensures text
probe-verus specify ./src -a atoms.json --with-spec-text

# Classify specs with taxonomy labels
probe-verus specify ./src -a atoms.json --with-spec-text --taxonomy-config spec_taxonomy_examples/spec-taxonomy-curve25519-dalek.toml

# Custom output file
probe-verus specify ./src -a atoms.json -o specs.json
```

**Output format:**

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "code-path": "src/lib.rs",
    "spec-text": {
      "lines-start": 42,
      "lines-end": 60
    },
    "kind": "exec",
    "specified": true,
    "has_requires": true,
    "has_ensures": true,
    "has_decreases": false,
    "has_trusted_assumption": false
  }
}
```

**Field descriptions:**
- **Key**: The probe-name from atoms.json
- **`code-path`**: Source file path
- **`spec-text`**: Function span with `lines-start` and `lines-end`
- **`kind`**: Declaration kind (`"exec"`, `"proof"`, or `"spec"`)
- **`specified`**: Whether the function has a specification (`has_requires` or `has_ensures` is true)
- **`has_requires`**: Whether the function has a `requires` clause (precondition)
- **`has_ensures`**: Whether the function has an `ensures` clause (postcondition)
- **`has_decreases`**: Whether the function has a `decreases` clause (termination proof)
- **`has_trusted_assumption`**: Whether the function contains `assume()` or `admit()`

**Extended output (`--with-spec-text`):**

When `--with-spec-text` is used, additional fields are included:
- **`requires_text`**: Raw text of the requires clause
- **`ensures_text`**: Raw text of the ensures clause
- **`ensures-calls`**: Function names called in the ensures clause (extracted from AST)
- **`requires-calls`**: Function names called in the requires clause (extracted from AST)

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

**Taxonomy classification (`--taxonomy-config`):**

When a taxonomy config TOML file is provided, each function is classified with `spec-labels` based on structured AST data (function mode, called function names in ensures/requires). This uses no regex -- classification is based on verus_syn AST walking.

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "code-path": "src/lib.rs",
    "spec-text": { "lines-start": 42, "lines-end": 60 },
    "kind": "exec",
    "specified": true,
    "has_ensures": true,
    "ensures-calls": ["is_canonical_scalar52", "scalar52_to_nat"],
    "spec-labels": ["functional-correctness", "data-invariant"]
  }
}
```

An example taxonomy config for curve25519-dalek is provided in [`spec_taxonomy_examples/spec-taxonomy-curve25519-dalek.toml`](spec_taxonomy_examples/spec-taxonomy-curve25519-dalek.toml). A starter template for new projects is at [`spec_taxonomy_examples/spec-taxonomy-default.toml`](spec_taxonomy_examples/spec-taxonomy-default.toml). The dalek config defines these categories:

| Label | Description | Trust |
|-------|-------------|-------|
| `functional-correctness` | Output matches a mathematical model | Highest |
| `data-invariant` | Representation invariant or structural consistency | High |
| `constant-time-behavior` | Constant-time correctness via Choice/CtOption | High |
| `bounds-safety` | No overflow, values within bounds | High |
| `memory-safety` | Direct structural/memory assertions (zeroization) | High |
| `algebraic-property` | Mathematical/algebraic lemma | Moderate |
| `termination` | Operation terminates (decreases clause) | Moderate |
| `specification-definition` | Pure specification, not a proof | N/A |

Multiple labels per function are supported (e.g., a function can be both `functional-correctness` and `data-invariant`).

See [Taxonomy Config Format](#taxonomy-config-format) for details on writing custom rules.

---

#### Taxonomy Config Format

The taxonomy config is a TOML file defining classification rules. Each rule specifies a label and match criteria. All rules are evaluated independently, and all matching rules contribute their label.

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
- `ensures_calls_contain` checks if ANY function name called in ensures contains ANY of the given substrings

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

---

### `run-verus` - Run Verus Verification

Run Verus verification on a project and analyze results. Supports caching for quick re-analysis.

```bash
probe-verus run-verus [PROJECT_PATH] [OPTIONS]

Options:
      --from-file <FILE>         Analyze existing output file instead of running verification
      --exit-code <CODE>         Exit code (only used with --from-file)
  -p, --package <NAME>           Package to verify (for workspaces)
      --verify-only-module <MOD> Module to verify
      --verify-function <FUNC>   Function to verify
  -o, --output <FILE>            Write JSON results to file (default: .verilib/probes/verus_<pkg>_<ver>_proofs.json)
      --no-cache                 Don't cache the verification output
  -a, --with-atoms <FILE>        Path to atoms.json for code-name enrichment (auto-discovers in .verilib/probes/ if omitted)
      --verus-args <ARGS>...     Extra arguments passed to cargo verus
```

**Caching Workflow:**

```bash
# First run: runs verification and caches output to data/
probe-verus run-verus ./my-verus-project -p my-crate

# Subsequent runs: uses cached output (no need to re-run verification)
probe-verus run-verus
```

**Examples:**
```bash
# Run verification (caches output automatically)
probe-verus run-verus ./my-verus-project
probe-verus run-verus ./my-workspace -p my-crate

# Use cached output (no project path needed)
probe-verus run-verus

# Analyze existing output file (from CI, etc.)
probe-verus run-verus ./my-project --from-file verification_output.txt

# Enrich results with probe-names from atoms.json (auto-discovers in .verilib/probes/)
probe-verus run-verus ./my-project
probe-verus run-verus ./my-project -a path/to/atoms.json
```

**Output format:**

The output is wrapped in a Schema 2.0 envelope. The `data` payload is a dictionary keyed by code-name:

```json
{
  "probe:crate/1.0.0/module/my_function()": {
    "code-path": "src/lib.rs",
    "code-line": 456,
    "verified": true,
    "status": "success"
  },
  "probe:crate/1.0.0/module/other_function()": {
    "code-path": "src/lib.rs",
    "code-line": 123,
    "verified": false,
    "status": "failure"
  }
}
```

**Field descriptions:**
- **Key**: The code-name (probe URI) from atoms.json
- **`code-path`**: Source file path
- **`code-line`**: Starting line number of the function
- **`verified`**: `true` if status is "success" or "warning", `false` otherwise
- **`status`**: One of:
  - `success`: Passed verification, no `assume()`/`admit()`
  - `failure`: Had verification errors
  - `sorries`: Contains `assume()` or `admit()`
  - `warning`: Passed with warnings

**Note:** If `-a` is not provided, probe-verus auto-discovers atoms in `.verilib/probes/`. If no atoms file is found, the output lacks code-name enrichment.

---

### `setup` - Manage External Tools

Install or check status of external tool dependencies (verus-analyzer, scip).

```bash
probe-verus setup [OPTIONS]

Options:
      --status    Show installation status instead of installing
```

**Examples:**
```bash
probe-verus setup             # download and install verus-analyzer + scip
probe-verus setup --status    # show what's installed and where
```

Version resolution uses, in order:
1. Environment variable overrides (`PROBE_VERUS_ANALYZER_VERSION`, `PROBE_SCIP_VERSION`)
2. Latest stable release from GitHub
3. Compiled-in fallback version (if GitHub is unreachable)

Tools are installed to `~/.probe-verus/tools/`. Existing tools on your `PATH` are also recognized.

---

### `extract` - Unified Pipeline (CI/Docker)

Run the unified 3-step pipeline: `atomize` + `specify` + `run-verus`. Produces a single unified JSON file (`probe-verus/extract` schema) where each atom entry includes optional `verification-status` and `specified` fields, matching the `probe-lean verify` output structure. A pipeline summary (`extract_summary.json`) is also written to the output directory.

```bash
probe-verus extract <PROJECT_PATH> [OPTIONS]

Options:
  -o, --output <DIR>           Output directory for extract_summary.json (default: ./output)
  -p, --package <NAME>         Package to verify (for workspaces)
      --skip-atomize           Skip the atomize step
      --skip-specify           Skip the specify step
      --skip-verify            Skip the run-verus step
      --separate-outputs       Also write individual atoms, specs, and proofs files
      --regenerate-scip        Force regeneration of the SCIP index
  -v, --verbose                Verbose output
      --rust-analyzer          Use rust-analyzer instead of verus-analyzer for SCIP
      --allow-duplicates       Allow duplicate probe-names (normally fatal)
      --auto-install           Automatically download missing tools without prompting
  -a, --with-atoms <PATH>      Path to atoms.json (for use with --skip-atomize)
      --with-spec-text         Include raw specification text in specify output
      --taxonomy-config <PATH> Path to TOML file for spec classification
      --verus-args <ARGS>...   Extra arguments passed to cargo verus
```

**Examples:**
```bash
probe-verus extract ./my-verus-project -p my-crate
probe-verus extract ./my-verus-project --auto-install   # CI-friendly
probe-verus extract ./my-verus-project --separate-outputs  # Also write atoms/specs/proofs files
probe-verus extract ./my-verus-project --skip-atomize -a path/to/atoms.json
```

**Docker entrypoint:** `probe-verus extract`

---

## How It Works

See [docs/HOW_IT_WORKS.md](docs/HOW_IT_WORKS.md) for detailed technical documentation on:

- SCIP-based call graph generation
- Accurate line spans with verus_syn parsing
- Disambiguation of trait implementations
- Verification output parsing and function categorization

See [docs/VERIFICATION_ARCHITECTURE.md](docs/VERIFICATION_ARCHITECTURE.md) for the verification analysis architecture.

---

## License

MIT

