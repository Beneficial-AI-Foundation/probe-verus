# Installation Scripts

Scripts for downloading and installing Verus development tools from GitHub releases. Available in both Python (`.py`) and shell script (`.sh`) versions.

## Scripts Included

| Tool | Python | Shell |
|------|--------|-------|
| Verus installer | `install_verus.py` | `install_verus.sh` |
| Verus builder (from source) | `install_verus_from_source.py` | `install_verus_from_source.sh` |
| Rust Analyzer installer | `install_rust_analyzer.py` | `install_rust_analyzer.sh` |
| Verus Analyzer installer | `install_verus_analyzer.py` | `install_verus_analyzer.sh` |
| SCIP installer | `install_scip.py` | `install_scip.sh` |
| Z3 SMT solver | `install_z3.py` | `install_z3.sh` |

Shell scripts require `curl` and `jq`. The Z3 installer also requires `unzip`.

## Features

- Downloads latest stable or pre-release versions
- Automatically detects your platform (Linux, macOS, Windows)
- Extracts and installs to user directories by default
- Sets up executable permissions automatically
- Configures PATH in your shell configuration
- Verifies installations work correctly

## Quick Start

### Install Verus
```bash
# Download and install the latest Verus release
python3 install_verus.py

# Install a specific version
python3 install_verus.py --version "0.2025.08.25.63ab0cb"

# List available releases without installing
python3 install_verus.py --list-assets

# Include pre-release versions
python3 install_verus.py --pre-release
```

### Install Verus Analyzer
```bash
# Install latest stable Verus Analyzer
python3 install_verus_analyzer.py

# Install latest pre-release
python3 install_verus_analyzer.py --pre-release

# List available releases
python3 install_verus_analyzer.py --list-assets
```

### Install Rust Analyzer
```bash
# Install latest stable Rust Analyzer
python3 install_rust_analyzer.py

# Install latest pre-release
python3 install_rust_analyzer.py --pre-release

# Download VS Code extension instead of binary
python3 install_rust_analyzer.py --vsix
```

### Install SCIP
```bash
# Install latest stable SCIP
python3 install_scip.py

# Install with custom directory
python3 install_scip.py --install-dir /opt/scip

# Check available releases
python3 install_scip.py --list-assets
```

### Install Z3

```bash
# Interactive: lists releases and prompts for selection
python3 install_z3.py

# List available releases without installing
python3 install_z3.py --list

# Shell script alternative
./install_z3.sh
./install_z3.sh --list
```

### Building Verus from Source

For platforms without pre-built binaries (e.g., ARM64 Linux):

```bash
# Build latest stable release (requires rustup and Z3 pre-installed)
python3 install_verus_from_source.py

# Build specific version
python3 install_verus_from_source.py --version v0.2025.08.25

# List available releases
python3 install_verus_from_source.py --list-releases

# Shell script alternative
./install_verus_from_source.sh
./install_verus_from_source.sh --version v0.2025.08.25
```

## Command Line Options

### Verus Installer
```
--version, -v             Download a specific version (e.g., "0.2025.08.25.63ab0cb")
--pre-release             Include pre-release versions
--output-dir, -o          Download directory (default: current directory)
--install-dir, -i         Installation directory (default: ~/verus)
--platform               Platform pattern to search for (e.g., x86-linux)
--list-assets            List all available assets without downloading
--no-extract             Download only, do not extract or install
--no-path                Do not modify PATH configuration
```

### Verus Analyzer Installer
```
--pre-release, --prerelease    Download pre-release version instead of stable
--output-dir, -o              Download directory (default: current directory)
--install-dir, -i             Installation directory (default: ~/verus-analyzer)
--platform                    Platform pattern (e.g., x86_64-unknown-linux-gnu)
--list-assets                 List all available assets without downloading
--no-extract                  Download only, do not extract or install
--no-path                     Do not modify PATH configuration
--vsix                        Download VS Code extension instead of binary
```

### Rust Analyzer Installer
```
--pre-release, --prerelease    Download pre-release version instead of stable
--output-dir, -o              Download directory (default: current directory)
--install-dir, -i             Installation directory (default: ~/rust-analyzer)
--platform                    Platform pattern (e.g., x86_64-unknown-linux-gnu)
--list-assets                 List all available assets without downloading
--no-extract                  Download only, do not extract or install
--no-path                     Do not modify PATH configuration
--vsix                        Download VS Code extension instead of binary
```

### SCIP Installer
```
--pre-release, --prerelease    Download pre-release version instead of stable
--output-dir, -o              Download directory (default: current directory)
--install-dir, -i             Installation directory (default: ~/scip)
--platform                    Platform pattern (e.g., scip-linux-amd64)
--list-assets                 List all available assets without downloading
--no-extract                  Download only, do not extract or install
--no-path                     Do not modify PATH configuration
```

### Z3 Installer
```
-n, --num-releases N          Number of releases to show (default: 30)
-l, --list                    List releases without installing
--install-dir, -i             Installation directory (default: ~/.local/bin)
--platform                    Platform pattern (e.g., x64-glibc, arm64-osx)
--no-path                     Do not modify PATH configuration
```

### Verus from Source Builder
```
-v, --version VERSION         Build a specific version/tag (e.g., "v0.2025.08.25")
-p, --pre-release             Build the latest pre-release version
-i, --install-dir DIR         Installation directory (default: ~/.cargo/bin/verus-<version>)
-b, --build-dir DIR           Build directory (default: temporary directory)
-k, --keep-build              Keep the build directory after installation
-j, --jobs N                  Number of parallel jobs for cargo (default: auto)
-l, --list-releases           List available releases and exit
```

## Running the Scripts

### Using python3

Install the required dependency using `pip`, then run the scripts directly:

```bash
pip install requests

python3 install_verus.py
python3 install_verus_analyzer.py
python3 install_rust_analyzer.py
python3 install_scip.py
python3 install_z3.py
python3 install_verus_from_source.py
```

### Using uv

The [uv](https://github.com/astral-sh/uv) tool automatically installs dependencies in a temporary cached environment without polluting your local or global Python installation:

```bash
uv run install_verus.py
uv run install_verus_analyzer.py
uv run install_rust_analyzer.py
uv run install_scip.py
uv run install_z3.py
uv run install_verus_from_source.py
```

## Supported Platforms

### Verus Analyzer
- Linux (x86_64, aarch64, armv7)
- macOS (x86_64, ARM64)
- Windows (x86_64, aarch64, i686)

### Rust Analyzer
- Linux (x86_64, aarch64, armv7)
- macOS (x86_64, ARM64)
- Windows (x86_64, aarch64, i686)

### SCIP
- Linux (amd64, arm64, arm)
- macOS (amd64, arm64)
- Windows (amd64, arm64)

### Z3
- Linux (x64, arm64) - glibc builds
- macOS (x64, arm64)
- Windows (x64)

Note: Z3 arm64-glibc builds are sourced from Beneficial-AI-Foundation/z3 for better compatibility.

## After Installation

All tools (Verus Analyzer, Rust Analyzer, SCIP, Z3) install to `~/.local/bin` by default.

The installer will:
1. Install the binary to `~/.local/bin`
2. Ensure `~/.local/bin` is in your PATH (adds to shell config if needed)
3. Verify the installation works

To use immediately after installation:
```bash
source ~/.bashrc  # or ~/.zshrc
verus-analyzer --version
rust-analyzer --version
scip --version
z3 --version
```

## Example Output

### Verus Analyzer Installation
```
Fetching latest stable Verus Analyzer release...
Found release: 0.2025.08.05
Downloading verus-analyzer-x86_64-unknown-linux-gnu.gz (15.2 MB)...
✓ Download completed
✓ Verus Analyzer installed to: ~/.local/bin
✓ Installation verified successfully!
✓ ~/.local/bin is already in PATH
```

### Rust Analyzer Installation
```
Fetching latest stable Rust Analyzer release...
Found release: 2025-08-05
Downloading rust-analyzer-x86_64-unknown-linux-gnu.gz (12.5 MB)...
✓ Download completed
✓ Rust Analyzer installed to: ~/.local/bin
✓ Installation verified successfully!
✓ ~/.local/bin is already in PATH
```

### SCIP Installation
```
Fetching latest stable SCIP release...
Found release: v0.5.2
Downloading scip-linux-amd64.tar.gz (8.1 MB)...
✓ Download completed
✓ SCIP installed to: ~/.local/bin
✓ Installation verified successfully!
✓ ~/.local/bin is already in PATH
```
