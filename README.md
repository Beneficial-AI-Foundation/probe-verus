# probe-verus

Probe Verus projects: generate call graph atoms, extract specifications, and analyze verification results.

`probe-verus` analyzes Rust/Verus codebases and produces structured JSON describing every function, its dependencies, source locations, specifications, and verification status. Output follows the Schema 2.0 envelope format; see [docs/SCHEMA.md](docs/SCHEMA.md) for the full specification.

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

### External tool dependencies

Some commands require external tools. After installing probe-verus, run `setup` to auto-download them:

```bash
probe-verus setup            # downloads verus-analyzer and scip
probe-verus setup --status   # check what's installed and where
```

For manual installation options, see [tools/INSTALL.md](tools/INSTALL.md).

| Command | Required Tools |
|---------|----------------|
| `extract` | verus-analyzer, scip, cargo verus |
| `atomize` | verus-analyzer, scip |
| `specify` | None |
| `run-verus` | cargo verus |
| `list-functions` | None |
| `setup` | None |

## Quick Start

```bash
# Unified pipeline: atomize + specify + verify (recommended)
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
  "tool": { "name": "probe-verus", "version": "2.0.0", "command": "extract" },
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

## How It Works

See [docs/HOW_IT_WORKS.md](docs/HOW_IT_WORKS.md) for detailed technical documentation on:

- SCIP-based call graph generation
- Accurate line spans with verus_syn parsing
- Disambiguation of trait implementations
- Verification output parsing and function categorization

See [docs/VERIFICATION_ARCHITECTURE.md](docs/VERIFICATION_ARCHITECTURE.md) for the verification analysis architecture.

## License

MIT
