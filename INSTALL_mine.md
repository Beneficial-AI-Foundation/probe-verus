# Installation

## Installing probe-verus

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
# Binary will be at target/release/probe-verus
```

---

## Prerequisites

Different commands require different external tools:

| Command | Required Tools |
|---------|----------------|
| `atomize` | verus-analyzer, scip |
| `list-functions` | None (uses built-in verus_syn) |
| `verify` | cargo verus |
| `analyze` | None (parses existing output) |

---

## Installing verus-analyzer

[verus-analyzer](https://github.com/verus-lang/verus-analyzer) is a fork of rust-analyzer that understands Verus syntax. It's required for generating SCIP indexes.

### Option 1: Using the installer script (recommended)

```bash
git clone https://github.com/Beneficial-AI-Foundation/installers_for_various_tools
cd installers_for_various_tools
python3 verus_analyzer_installer.py
```

### Option 2: Build from source

```bash
git clone https://github.com/verus-lang/verus-analyzer
cd verus-analyzer
cargo install --path crates/verus-analyzer
```

### Verify installation

```bash
verus-analyzer --version
```

---

## Installing scip CLI

[scip](https://github.com/sourcegraph/scip) is the SCIP (Source Code Index Protocol) CLI tool. It's used to convert binary SCIP indexes to JSON.

### Option 1: Using the installer script (recommended)

```bash
git clone https://github.com/Beneficial-AI-Foundation/installers_for_various_tools
cd installers_for_various_tools
python3 scip_installer.py
```

### Option 2: Download pre-built binary

Download from [GitHub releases](https://github.com/sourcegraph/scip/releases).

### Verify installation

```bash
scip --version
```

---

## Installing Verus

[Verus](https://github.com/verus-lang/verus) is a verification tool for Rust. It's required for the `verify` command.

### Option 1: Using the installer script

```bash
git clone https://github.com/Beneficial-AI-Foundation/installers_for_various_tools
cd installers_for_various_tools
python3 verus_installer.py
```

### Option 2: Build from source

Follow the instructions in the [Verus repository](https://github.com/verus-lang/verus).

### Verify installation

```bash
cargo verus --version
```

---

## Quick Start

Once you have the prerequisites installed, you can use probe-verus:

```bash
# List functions in a project (no external tools needed)
probe-verus list-functions ./my-project

# Generate call graph atoms (requires verus-analyzer + scip)
probe-verus atomize ./my-project

# Run Verus verification (requires cargo verus)
probe-verus verify ./my-project
```
