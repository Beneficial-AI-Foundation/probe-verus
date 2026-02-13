# probe-verus

Probe Verus projects: generate call graph atoms and analyze verification results.

## Installation

```bash
cargo install --path .
```

**Prerequisites:** Some commands require external tools (verus-analyzer, scip, cargo verus).  
See [INSTALL.md](INSTALL.md) for detailed installation instructions.

## Commands

```
probe-verus <COMMAND>

Commands:
  stubify         Convert .md files with YAML frontmatter to JSON
  atomize         Generate call graph atoms with line numbers from SCIP indexes
  list-functions  List all functions in a Rust/Verus project
  specify         Extract function specifications from atoms.json
  verify          Run Verus verification and analyze results
```

---

### `stubify` - Convert Stub Files to JSON

Convert a directory hierarchy of `.md` files with YAML frontmatter to a JSON file. This is useful for processing verification stub files (like those in `.verilib/structure`).

```bash
probe-verus stubify <PATH> [OPTIONS]

Options:
  -o, --output <FILE>    Output file path (default: stubs.json)
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
  -o, --output <FILE>     Output file path (default: atoms.json)
  -r, --regenerate-scip   Force regeneration of the SCIP index
      --with-locations    Include detailed per-call location info (precondition/postcondition/inner)
```

**Examples:**
```bash
probe-verus atomize ./my-rust-project
probe-verus atomize ./my-rust-project -o atoms.json
probe-verus atomize ./my-rust-project --regenerate-scip
probe-verus atomize ./my-rust-project --with-locations  # extended output
```

**Output format:**

The output is a dictionary keyed by `probe-name` (a URI-style identifier):

```json
{
  "probe:curve25519-dalek/4.1.3/module/MyType#my_function()": {
    "display-name": "my_function",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/other_module/helper()"
    ],
    "code-module": "module",
    "code-path": "src/lib.rs",
    "code-text": { "lines-start": 42, "lines-end": 100 },
    "mode": "proof"
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
- **`mode`**: Verus function mode (`"exec"`, `"proof"`, or `"spec"`)

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
    "mode": "exec"
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
  -o, --output <FILE>              Output file path (default: specs.json)
  -a, --with-atoms <FILE>          Path to atoms.json for code-name lookup (required)
      --with-spec-text             Include raw specification text in output
      --taxonomy-config <FILE>     Path to TOML file defining spec classification rules
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
    "mode": "exec",
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
- **`mode`**: Verus function mode (`"exec"`, `"proof"`, or `"spec"`)
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
    "mode": "exec",
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
    "mode": "exec",
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

### `verify` - Run Verus Verification

Run Verus verification on a project and analyze results. Supports caching for quick re-analysis.

```bash
probe-verus verify [PROJECT_PATH] [OPTIONS]

Options:
      --from-file <FILE>         Analyze existing output file instead of running verification
      --exit-code <CODE>         Exit code (only used with --from-file)
  -p, --package <NAME>           Package to verify (for workspaces)
      --verify-only-module <MOD> Module to verify
      --verify-function <FUNC>   Function to verify
  -o, --output <FILE>            Write JSON results to file (default: proofs.json)
      --no-cache                 Don't cache the verification output
  -a, --with-atoms [FILE]        Enrich results with code-names from atoms.json
```

**Caching Workflow:**

```bash
# First run: runs verification and caches output to data/
probe-verus verify ./my-verus-project -p my-crate

# Subsequent runs: uses cached output (no need to re-run verification)
probe-verus verify
```

**Examples:**
```bash
# Run verification (caches output automatically)
probe-verus verify ./my-verus-project
probe-verus verify ./my-workspace -p my-crate

# Use cached output (no project path needed)
probe-verus verify

# Analyze existing output file (from CI, etc.)
probe-verus verify ./my-project --from-file verification_output.txt

# Enrich results with probe-names from atoms.json
probe-verus verify -a
probe-verus verify -a path/to/atoms.json
```

**Output format (with `-a/--with-atoms`):**

When using `--with-atoms`, the output is a dictionary keyed by code-name:

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

**Note:** The `-a/--with-atoms` flag is required to generate this format. Without it, the legacy format is used for backwards compatibility.

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

