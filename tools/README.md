# Tools

## SCIP Index Generator

Generate SCIP indices for code analysis.

**Prerequisites:** Requires `verus-analyzer` (or `rust-analyzer`) and `scip` to be installed. See [INSTALL.md](INSTALL.md) for installation instructions.

### Running the Script

Using python3 and pip:
```bash
pip install requests
python3 generate_scip_index.py /path/to/project
```

Using uv (installs dependencies in a temporary cached environment without polluting your local/global Python installation):
```bash
uv run generate_scip_index.py /path/to/project
```

### Usage

```bash
# Analyze a project with Verus Analyzer (default)
python3 generate_scip_index.py /path/to/project

# Use Rust Analyzer instead
python3 generate_scip_index.py /path/to/project --analyzer rust-analyzer

# Keep project copy and specify output
python3 generate_scip_index.py /path/to/project --keep-copy --output-dir ./analysis
```

### Command Line Options

```
project                       Path to the project to analyze (required)
--analyzer, -a                Analyzer to use: verus-analyzer (default) or rust-analyzer
--output-dir, -o              Directory to copy project to (default: temp directory)
--json-output, -j             Output file for JSON export (default: index_scip.json)
--keep-copy                   Keep the copied project after analysis
--check-tools                 Check if required tools are available and exit
```

### What It Does

The generator will:
1. Copy your project to a working directory
2. Run analyzer SCIP analysis (`verus-analyzer scip .` or `rust-analyzer scip .`)
3. Export SCIP index to JSON (`scip print --json index.scip > index_scip.json`)
4. Clean up temporary files (unless `--keep-copy` is used)

### Example Output

```
============================================================
SCIP INDEX GENERATION
============================================================

1. Copying project...
Copying project from /home/user/my-project to /tmp/scip_analysis_xyz/my-project

2. Running verus-analyzer SCIP analysis...
Executing: verus-analyzer scip .
✓ verus-analyzer SCIP analysis completed successfully
✓ SCIP index file created: /tmp/scip_analysis_xyz/my-project/index.scip

3. Exporting SCIP index to JSON...
Executing: scip print --json index.scip
✓ SCIP JSON export completed successfully
✓ Output file: /tmp/scip_analysis_xyz/my-project/index_scip.json
  File size: 2.5 MB

============================================================
ANALYSIS COMPLETE
============================================================
✓ Project: /home/user/my-project
✓ Analyzer: verus-analyzer
✓ SCIP file: /tmp/scip_analysis_xyz/my-project/index.scip
✓ JSON output: /tmp/scip_analysis_xyz/my-project/index_scip.json

📄 SCIP index JSON available at: /tmp/scip_analysis_xyz/my-project/index_scip.json
```

## Installation Scripts

See [INSTALL.md](INSTALL.md) for scripts to install:
- Verus
- Verus Analyzer
- Rust Analyzer
- SCIP
