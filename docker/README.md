# probe-verus Docker

Self-contained Docker image for running `probe-verus atomize` and `probe-verus verify` commands.

## Quick Start

Pull the pre-built image:

```bash
docker pull ghcr.io/beneficial-ai-foundation/probe-verus:latest
```

Or build locally:

```bash
cd /path/to/probe-verus
docker build -t probe-verus -f docker/Dockerfile .
```

## Usage

```bash
# Using the helper script (recommended)
./docker/run.sh /path/to/project [OPTIONS]

# Or directly with docker (using pre-built image)
docker run --rm --user root \
  -v /path/to/project:/workspace/project \
  -v /path/to/output:/workspace/output \
  ghcr.io/beneficial-ai-foundation/probe-verus:latest \
  /workspace/project -o /workspace/output [OPTIONS]
```

**Options:**
- `-o, --output <dir>` - Output directory (default: ./output)
- `--atomize-only` - Run only the atomize command
- `--verify-only` - Run only the verify command
- `-p, --package <name>` - Package name for workspace projects
- `--regenerate-scip` - Force regeneration of the SCIP index
- `-v, --verbose` - Enable verbose output

**Output files (inside the project directory):**
- `.verilib/probes/verus_<pkg>_<ver>.json` - Call graph atoms from atomize
- `.verilib/probes/verus_<pkg>_<ver>_proofs.json` - Verification results from verify

**Output files (in the output directory):**
- `run_summary.json` - Overall run status

All JSON outputs are wrapped in a [Schema 2.0 metadata envelope](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md). Use `jq '.data'` to access the payload.

## Examples

### Workspace project with package selection

```bash
# For a Cargo workspace, specify the package to verify
docker run --rm --user root \
  -v ~/my-workspace:/workspace/project \
  -v ~/output:/workspace/output \
  ghcr.io/beneficial-ai-foundation/probe-verus:latest \
  /workspace/project -o /workspace/output --package my-crate
```

### Atomize only (skip verification)

```bash
docker run --rm --user root \
  -v ~/my-project:/workspace/project \
  -v ~/output:/workspace/output \
  ghcr.io/beneficial-ai-foundation/probe-verus:latest \
  /workspace/project -o /workspace/output --atomize-only
```

### Force SCIP regeneration

```bash
docker run --rm --user root \
  -v ~/my-project:/workspace/project \
  -v ~/output:/workspace/output \
  ghcr.io/beneficial-ai-foundation/probe-verus:latest \
  /workspace/project -o /workspace/output --regenerate-scip
```

### Verbose output for debugging

```bash
docker run --rm --user root \
  -v ~/my-project:/workspace/project \
  -v ~/output:/workspace/output \
  ghcr.io/beneficial-ai-foundation/probe-verus:latest \
  /workspace/project -o /workspace/output --verbose
```

## Output Files

All JSON outputs are wrapped in a [Schema 2.0 metadata envelope](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md). The actual payload is in the `data` field.

| File | Location | Description |
|------|----------|-------------|
| `verus_<pkg>_<ver>.json` | `<project>/.verilib/probes/` | Call graph atoms with dependencies and line ranges |
| `verus_<pkg>_<ver>_proofs.json` | `<project>/.verilib/probes/` | Verification results (verified/failed/unverified functions) |
| `run_summary.json` | `<output-dir>/` | Overall run status and summary |

### Atoms format (`probe-verus/atoms`)

```json
{
  "schema": "probe-verus/atoms",
  "schema-version": "2.0",
  "tool": { "name": "probe-verus", "version": "2.0.0", "command": "atomize" },
  "source": { "repo": "...", "commit": "...", "language": "rust", "package": "...", "package-version": "..." },
  "timestamp": "2026-03-06T12:00:00Z",
  "data": {
    "probe:crate/1.0.0/module/function()": {
      "display-name": "function",
      "dependencies": ["probe:crate/1.0.0/module/helper()"],
      "code-module": "module",
      "code-path": "src/lib.rs",
      "code-text": { "lines-start": 10, "lines-end": 25 },
      "kind": "exec",
      "language": "rust"
    }
  }
}
```

### Proofs format (`probe-verus/proofs`)

```json
{
  "schema": "probe-verus/proofs",
  "schema-version": "2.0",
  "tool": { "name": "probe-verus", "version": "2.0.0", "command": "verify" },
  "source": { "repo": "...", "commit": "...", "language": "rust", "package": "...", "package-version": "..." },
  "timestamp": "2026-03-06T12:00:00Z",
  "data": {
    "probe:crate/1.0.0/module/function()": {
      "display-name": "function",
      "code-path": "src/lib.rs",
      "code-text": { "lines-start": 10, "lines-end": 25 },
      "verified": true,
      "verification-status": "verified"
    }
  }
}
```

### Run summary format (`probe-verus/run-summary`)

```json
{
  "schema": "probe-verus/run-summary",
  "schema-version": "2.0",
  "tool": { "name": "probe-verus", "version": "2.0.0", "command": "run" },
  "source": { "repo": "...", "commit": "...", "language": "rust", "package": "...", "package-version": "..." },
  "timestamp": "2026-03-06T12:00:00Z",
  "data": {
    "status": "success",
    "atomize": {
      "success": true,
      "output_file": "<project>/.verilib/probes/verus_<pkg>_<ver>.json",
      "total_functions": 42
    },
    "verify": {
      "success": true,
      "output_file": "<project>/.verilib/probes/verus_<pkg>_<ver>_proofs.json",
      "summary": {
        "total_functions": 42,
        "verified": 40,
        "failed": 0,
        "unverified": 2
      }
    }
  }
}
```

## What's Included

The Docker image includes:

- **Rust** (base toolchain + version required by Verus, auto-detected)
- **Verus** (latest stable release)
- **verus-analyzer** (latest, required for SCIP index generation)
- **scip** CLI (latest, required for SCIP to JSON conversion)
- **probe-verus** (built from source)

## Build Arguments

Customize the build with these arguments:

```bash
# Use specific Verus version
docker build -t probe-verus -f docker/Dockerfile \
  --build-arg VERUS_VERSION=release/0.2026.01.23.1650a05 \
  .

# Use latest prerelease (rolling)
docker build -t probe-verus -f docker/Dockerfile \
  --build-arg VERUS_VERSION=prerelease \
  .

# Use specific user/group IDs
docker build -t probe-verus -f docker/Dockerfile \
  --build-arg USER_UID=1001 \
  --build-arg USER_GID=1001 \
  .
```

| Argument | Default | Description |
|----------|---------|-------------|
| `VERUS_VERSION` | `latest` | Verus version: `latest`, `prerelease`, or specific tag (e.g., `release/0.2026.01.23.1650a05`) |
| `RUST_VERSION` | `1.88.0` | Base Rust toolchain (for building probe-verus) |
| `USER_UID` | `1000` | UID for the non-root user |
| `USER_GID` | `1000` | GID for the non-root user |

**Note:** The Rust toolchain required by Verus is automatically detected from the release's `rust-toolchain.toml` file.

**Tip:** To list available Verus releases:

```bash
curl -s https://api.github.com/repos/verus-lang/verus/releases | jq -r '.[].tag_name'
```

Or browse: https://github.com/verus-lang/verus/releases

## Security

The container image defaults to a non-root user (`verus`, UID 1000) for security.

However, the `run.sh` helper script uses `--user root` because Verus verification needs to write build artifacts to the mounted project directory. If your host user isn't UID 1000, you'll get permission errors otherwise.

For direct `docker run` usage, add `--user root` if you encounter permission issues:

```bash
docker run --rm --user root -v ... probe-verus ...
```

## Troubleshooting

### Permission denied on output directory

The container runs as UID 1000 by default. Make sure your output directory is writable:

```bash
mkdir -p ./output
chmod 777 ./output  # Or use matching UID
```

### SCIP index errors

Try regenerating the SCIP index:

```bash
docker run ... probe-verus /workspace/project -o /workspace/output --regenerate-scip
```

### Debugging issues

Use verbose mode to see detailed output:

```bash
docker run ... probe-verus /workspace/project -o /workspace/output --verbose
```

### Verification timeout

For large projects, verification may take a long time. The Docker container doesn't impose time limits.

## Helper Script

The `run.sh` script simplifies common usage:

```bash
# Basic usage (uses pre-built GHCR image by default)
./docker/run.sh ~/my-project

# With options
./docker/run.sh ~/my-project --package my-crate --verbose

# Custom output directory
./docker/run.sh ~/my-project --output ./my-output

# Use locally built image instead
PROBE_VERUS_IMAGE=probe-verus ./docker/run.sh ~/my-project
```
