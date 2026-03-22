# BAIF Verus Extract Action (Unified Pipeline)

A GitHub Action that runs [probe-verus extract](https://github.com/Beneficial-AI-Foundation/probe-verus) — a single unified pipeline that atomizes call graphs, extracts specifications with optional taxonomy labels, and runs Verus verification. Produces one merged JSON output instead of three separate files.

## Why use this instead of `action/`?

The existing [`action/action.yml`](../action/action.yml) runs `atomize` and `run-verus` as separate steps and does not run `specify`. If you want:

- Spec extraction with taxonomy classification in CI
- A single unified `probe-verus/extract` JSON merging atoms + specs + verification status
- Simpler workflow configuration (one step instead of three)

…then use this action.

The existing `action/` is **not deprecated** — it continues to work for callers that only need atoms + proofs.

## Usage

### Basic

```yaml
- uses: beneficial-ai-foundation/probe-verus/action-extract@v5
  id: extract
  with:
    project-path: ./my-verus-crate
```

### With taxonomy and SMT logging

```yaml
- uses: beneficial-ai-foundation/probe-verus/action-extract@v5
  id: extract
  with:
    project-path: ./my-verus-crate
    verus-args: '--log smt --log-dir ./smt-logs -V spinoff-all'
    taxonomy-config: spec-taxonomy.toml
```

### Workspace project

```yaml
- uses: beneficial-ai-foundation/probe-verus/action-extract@v5
  id: extract
  with:
    project-path: ./my-workspace
    package: my-verus-crate
```

### Using outputs

```yaml
- uses: beneficial-ai-foundation/probe-verus/action-extract@v5
  id: extract
  with:
    project-path: ./my-verus-crate

- name: Display results
  run: |
    echo "Verified: ${{ steps.extract.outputs.verified-count }} / ${{ steps.extract.outputs.total-functions }}"
    echo "Extract file: ${{ steps.extract.outputs.extract-file }}"
    echo "Summary file: ${{ steps.extract.outputs.extract-summary-file }}"
```

## Inputs

| Input | Required | Default | Description |
|-------|----------|---------|-------------|
| `project-path` | Yes | | Path to the Verus project directory |
| `package` | No | | Package name for workspace projects |
| `verus-version` | No | auto-detect | Verus version (e.g., `1.85.0`) |
| `rust-version` | No | auto-detect | Rust toolchain version |
| `output-dir` | No | `.` | Directory for `extract_summary.json` |
| `token` | No | `github.token` | GitHub token for API calls (avoids rate limiting) |
| `verus-args` | No | | Extra arguments passed to Verus |
| `taxonomy-config` | No | auto-detect | Path to `spec-taxonomy.toml`; auto-detects at project root if absent |

## Outputs

| Output | Description |
|--------|-------------|
| `extract-file` | Path to the unified `probe-verus/extract` JSON (merged atoms+specs+proofs) |
| `extract-summary-file` | Path to `extract_summary.json` |
| `verified-count` | Number of functions verified |
| `total-functions` | Total number of functions |
| `verus-version` | Verus version used |
| `rust-version` | Rust toolchain version used |
| `smt-log-dir` | Path to the SMT log directory (if `verus-args` includes `--log-dir`) |

## Auto-Detection

### Versions

If `verus-version` or `rust-version` are not provided, the action looks for them in your project's `Cargo.toml`:

```toml
[package.metadata.verus]
release = "1.85.0"
rust-version = "nightly-2025-01-01"
```

If no explicit release is found, the action falls back to resolving the version from `vstd`/`verus_builtin` git dependency `rev` fields against GitHub release tags.

### Taxonomy config

If `taxonomy-config` is not provided, the action checks for `spec-taxonomy.toml` at the project root and uses it automatically if present.

## Output File Format

All JSON outputs use the [Schema 2.0 metadata envelope](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md). The actual payload is in the `data` field.

### Unified extract output (`probe-verus/extract` schema)

The primary output is a dictionary keyed by code-name, where each entry is a `UnifiedAtom` combining call graph data, specifications, and verification status:

```json
{
  "schema": "probe-verus/extract",
  "schema-version": "2.0",
  "tool": { "name": "probe-verus", "version": "5.2.0", "command": "extract" },
  "source": { "repo": "...", "commit": "...", "language": "rust", "package": "...", "package-version": "..." },
  "timestamp": "2026-03-22T12:00:00Z",
  "data": {
    "probe:my-crate/1.0.0/module/my_function()": {
      "display-name": "my_function",
      "dependencies": ["probe:my-crate/1.0.0/module/helper()"],
      "code-module": "module",
      "code-path": "src/lib.rs",
      "code-text": { "lines-start": 10, "lines-end": 25 },
      "kind": "exec",
      "language": "verus",
      "primary-spec": "requires\n    x > 0\nensures\n    result > x",
      "is-disabled": false,
      "verification-status": "verified",
      "spec-labels": ["label-A"]
    }
  }
}
```

### Extract summary (`extract_summary.json`)

Contains pipeline status and per-step results:

```json
{
  "schema": "probe-verus/extract-summary",
  "data": {
    "status": "success",
    "atomize": { "success": true, "output_file": "...", "total_functions": 42 },
    "specify": { "success": true, "output_file": "...", "total_functions": 42 },
    "verify": { "success": true, "output_file": "...", "summary": { "total_functions": 42, "verified": 40, "failed": 1, "unverified": 1 } }
  }
}
```

## Complete Example: Extract, Verify, and Certify

```yaml
name: Verify and Certify

on:
  push:
    branches: [main]
    paths:
      - 'src/**/*.rs'

jobs:
  verify-and-certify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      # Run unified extract pipeline
      - uses: beneficial-ai-foundation/probe-verus/action-extract@v5
        id: extract
        with:
          project-path: ./my-verus-crate
          taxonomy-config: spec-taxonomy.toml

      # Certify results on Ethereum
      - uses: beneficial-ai-foundation/certify/action@v1
        id: certify
        with:
          source: ${{ steps.extract.outputs.extract-file }}
          description: "Verus verification: ${{ steps.extract.outputs.verified-count }}/${{ steps.extract.outputs.total-functions }} verified"
          network: sepolia
          rpc-url: ${{ secrets.SEPOLIA_RPC_URL }}
          private-key: ${{ secrets.SEPOLIA_PRIVATE_KEY }}
          certify-address: ${{ vars.CERTIFY_ADDRESS }}

      - name: Summary
        run: |
          echo "## Verification Results" >> $GITHUB_STEP_SUMMARY
          echo "" >> $GITHUB_STEP_SUMMARY
          echo "- **Verified**: ${{ steps.extract.outputs.verified-count }} / ${{ steps.extract.outputs.total-functions }}" >> $GITHUB_STEP_SUMMARY
          echo "- **Extract output**: ${{ steps.extract.outputs.extract-file }}" >> $GITHUB_STEP_SUMMARY
          echo "- **Certification**: [${{ steps.certify.outputs.tx-hash }}](${{ steps.certify.outputs.etherscan-url }})" >> $GITHUB_STEP_SUMMARY
```

## Requirements

- Linux runner (`ubuntu-latest` recommended)
- Project must be a valid Verus/Rust project
- Either provide versions via inputs or include `[package.metadata.verus]` in Cargo.toml

## License

MIT
