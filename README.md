# probe-verus

Probe Verus projects: generate call graph atoms, extract specifications, and analyze verification results.

`probe-verus` analyzes Rust/Verus codebases and produces structured JSON describing every function, its dependencies, source locations, specifications, and verification status. Output follows the Schema 2.0 envelope format; see [docs/SCHEMA.md](docs/SCHEMA.md) for the full specification.

## Prerequisites

- **Rust toolchain** (`cargo`) -- install via [rustup.rs](https://rustup.rs/)
- **verus-analyzer & scip** -- auto-downloadable via `probe-verus setup --install` or the `--auto-install` flag on `extract`/`atomize`. See [tools/INSTALL.md](tools/INSTALL.md) for manual options.
- **Verus** (`cargo verus`) -- required for the verification step. Must be installed separately; `probe-verus setup` does **not** install Verus. Install the **same Verus version your project targets** (check the project's `rust-toolchain.toml` or documentation); a mismatched version may cause verification failures. If Verus is not installed, `extract` still runs the atomize and specify steps and prints a warning that verification was skipped. Install options:
  - Official guide: [verus-lang.github.io/verus/guide/getting_started.html](https://verus-lang.github.io/verus/guide/getting_started.html)
  - Convenience scripts in this repo (pre-built binary download, specific versions, build from source): see [tools/INSTALL.md](tools/INSTALL.md#install-verus)

| Command | Required Tools | Notes |
|---------|----------------|-------|
| `extract` | verus-analyzer, scip, cargo verus | Gracefully skips verification if Verus is missing |
| `atomize` | verus-analyzer, scip | Auto-downloadable via `--auto-install` |
| `specify` | None | |
| `run-verus` | cargo verus | Requires Verus to be installed |
| `list-functions` | None | |
| `setup` | None | Downloads verus-analyzer & scip only |

## Installation

### Pre-built binaries (recommended)

Download the latest release from [GitHub Releases](https://github.com/Beneficial-AI-Foundation/probe-verus/releases), or use the one-line installers:

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

## Quick Start

```bash
# Unified pipeline: atomize + specify + verify (recommended)
# --auto-install downloads verus-analyzer and scip; Verus must be installed separately
probe-verus extract ./my-verus-project -p my-crate --auto-install

# Or run individual steps
probe-verus atomize ./my-verus-project --auto-install
probe-verus specify ./src -a .verilib/probes/verus_*_atoms.json --with-spec-text
probe-verus run-verus ./my-verus-project -p my-crate
```

## Commands

| Command | Description |
|---------|-------------|
| `extract` | Unified pipeline: atomize + specify + run-verus (recommended for CI/Docker) |
| `atomize` | Generate call graph atoms with line numbers from SCIP indexes |
| `specify` | Extract function specifications from source files |
| `run-verus` | Run Verus verification and analyze results |
| `list-functions` | List all functions in a Rust/Verus project |
| `setup` | Install or check status of external tools |
| `stubify` | Convert `.md` files with YAML frontmatter to JSON |

For the full command reference with all options and examples, see **[docs/USAGE.md](docs/USAGE.md)**. For the complete JSON schema specification, see **[docs/SCHEMA.md](docs/SCHEMA.md)**.

## Example Output

Running `probe-verus extract` produces a JSON envelope. Each entry in `data` describes a function with its call graph, specs, and verification status:

```json
{
  "schema": "probe-verus/extract",
  "schema-version": "2.0",
  "tool": { "name": "probe-verus", "version": "6.0.0", "command": "extract" },
  "source": {
    "repo": "https://github.com/org/project",
    "commit": "abc123...",
    "language": "rust",
    "package": "my-crate",
    "package-version": "1.0.0"
  },
  "timestamp": "2026-03-17T12:00:00Z",
  "data": {
    "probe:my-crate/1.0.0/module/my_function()": {
      "display-name": "my_function",
      "dependencies": ["probe:my-crate/1.0.0/other/helper()"],
      "requires-dependencies": [],
      "ensures-dependencies": ["probe:my-crate/1.0.0/other/helper()"],
      "body-dependencies": ["probe:my-crate/1.0.0/other/helper()"],
      "code-module": "module",
      "code-path": "src/lib.rs",
      "code-text": { "lines-start": 42, "lines-end": 100 },
      "kind": "exec",
      "language": "rust",
      "primary-spec": "requires\n    x > 0\nensures\n    result > x",
      "is-disabled": false,
      "verification-status": "verified"
    }
  }
}
```

## Spec Taxonomy

The `specify` and `extract` commands can classify each function's specification into human-readable categories (e.g., "functional-correctness", "crash-safety", "data-invariant") using rules defined in a TOML config file. This turns a flat list of verified functions into structured output meaningful to non-expert stakeholders.

```bash
probe-verus specify ./src -a atoms.json --with-spec-text \
  --taxonomy-config spec_taxonomy_examples/spec-taxonomy-default.toml

probe-verus extract ./my-verus-project \
  --taxonomy-config spec_taxonomy_examples/spec-taxonomy-curve25519-dalek.toml
```

Classification is AST-based: function call names in `requires`/`ensures` clauses are extracted by walking the `verus_syn` AST, not by regex. Rules are AND-of-OR predicates over structured metadata (function mode, call names, boolean flags). Multiple rules can fire per function, producing multi-label output.

### Taxonomy config format

```toml
[taxonomy]
version = "1"
stop_words = ["len", "old", "unwrap"]   # optional: filter noisy utility calls

[[taxonomy.rules]]
label = "functional-correctness"
description = "Output matches a mathematical model"
trust = "highest"

[taxonomy.rules.match]
mode = ["exec"]
ensures_calls_contain = ["spec_", "_to_nat"]
```

Each rule has a `label`, `description`, `trust` level, and a `[match]` block. Available match criteria:

| Criterion | Type | Description |
|-----------|------|-------------|
| `mode` | string list | Function mode: `exec`, `proof`, `spec` |
| `context` | string list | Function context: `impl`, `trait`, `standalone` |
| `ensures_calls_contain` | substring list | Any ensures call name contains any substring |
| `requires_calls_contain` | substring list | Any requires call name contains any substring |
| `ensures_calls_full_contain` | substring list | Match against full qualified paths in ensures |
| `requires_calls_full_contain` | substring list | Match against full qualified paths in requires |
| `ensures_fn_calls_contain` | substring list | Match only function calls (not method calls) in ensures |
| `ensures_method_calls_contain` | substring list | Match only method calls in ensures |
| `requires_fn_calls_contain` | substring list | Match only function calls in requires |
| `requires_method_calls_contain` | substring list | Match only method calls in requires |
| `name_contains` | substring list | Function name contains any substring |
| `path_contains` | substring list | Source path contains any substring |
| `has_ensures` | bool | Whether function has ensures clause |
| `has_requires` | bool | Whether function has requires clause |
| `has_decreases` | bool | Whether function has decreases clause |
| `has_trusted_assumption` | bool | Whether function uses assume/admit |
| `ensures_calls_empty` | bool | Ensures clause has zero function calls |
| `requires_calls_empty` | bool | Requires clause has zero function calls |

Use `--taxonomy-explain` to debug rule matching (prints per-function match/miss details to stderr).

Starter configs are in [`spec_taxonomy_examples/`](spec_taxonomy_examples/). For a comprehensive real-world example (17 rules, 14 domain categories for elliptic-curve cryptography), see the [dalek-lite spec-taxonomy.toml](https://github.com/Beneficial-AI-Foundation/dalek-lite/blob/main/spec-taxonomy.toml). See [docs/SPEC_TAXONOMY_DESIGN.md](docs/SPEC_TAXONOMY_DESIGN.md) for the full design analysis.

## How It Works

See [docs/HOW_IT_WORKS.md](docs/HOW_IT_WORKS.md) for detailed technical documentation on:

- SCIP-based call graph generation
- Accurate line spans with verus_syn parsing
- Disambiguation of trait implementations
- Verification output parsing and function categorization

See [docs/VERIFICATION_ARCHITECTURE.md](docs/VERIFICATION_ARCHITECTURE.md) for the verification analysis architecture.

## License

MIT
