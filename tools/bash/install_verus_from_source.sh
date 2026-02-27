#!/bin/bash
#
# Verus Builder from Release
#
# Builds Verus from source for a specific release/tag.
# Useful for platforms without pre-built binaries (e.g., ARM64 Linux).
#
# Prerequisites:
#   - Rust toolchain (rustup): https://rustup.rs
#   - Z3 theorem prover (4.12.5+) installed and in PATH, or VERUS_Z3_PATH set
#   - git, curl
#
# Requirements: curl, jq, git
#

set -e

# Default values
VERSION=""
PRE_RELEASE=false
INSTALL_DIR=""
BUILD_DIR=""
KEEP_BUILD=false
LIST_RELEASES=false
JOBS=""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

NUM_RELEASES=20

GITHUB_REPO="verus-lang/verus"
TOOL_NAME="Verus"
CARGO_BIN_DIR="$HOME/.cargo/bin"

usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Build and install Verus from source.

This script is useful for platforms without pre-built binaries (e.g., ARM64 Linux).
Assumes Z3 is already installed and available in PATH or via VERUS_Z3_PATH.

Options:
  -v, --version VERSION   Build a specific version/tag (e.g., "v0.2025.08.25")
  -p, --pre-release       Build the latest pre-release version
  -i, --install-dir DIR   Installation directory (default: ~/.cargo/bin/verus-<version>)
  -b, --build-dir DIR     Build directory (default: temporary directory)
  -k, --keep-build        Keep the build directory after installation
  -j, --jobs N            Number of parallel jobs for cargo (default: auto)
  -l, --list-releases     List available releases and exit
  -h, --help              Show this help message

Prerequisites:
  - Rust toolchain (rustup): https://rustup.rs
  - Z3 theorem prover installed (set VERUS_Z3_PATH if not in PATH)
  - git, curl, jq

Examples:
  $(basename "$0")                              # Build latest stable
  $(basename "$0") --version v0.2025.08.25      # Build specific version
  $(basename "$0") --pre-release                # Build latest pre-release
  $(basename "$0") --install-dir /opt/verus     # Custom install location
EOF
}

check_prerequisites() {
    local missing=()

    for cmd in curl jq git rustup cargo; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done

    if [[ ${#missing[@]} -gt 0 ]]; then
        echo -e "${RED}Error: Missing required tools: ${missing[*]}${NC}"
        if [[ " ${missing[*]} " =~ " rustup " ]] || [[ " ${missing[*]} " =~ " cargo " ]]; then
            echo "Install Rust toolchain from: https://rustup.rs"
        fi
        exit 1
    fi

    # Check for Z3
    if [[ -z "$VERUS_Z3_PATH" ]]; then
        if command -v z3 &>/dev/null; then
            export VERUS_Z3_PATH=$(command -v z3)
            echo "Found Z3 at: $VERUS_Z3_PATH"
        else
            echo -e "${YELLOW}Warning: Z3 not found in PATH and VERUS_Z3_PATH not set${NC}"
            echo "Verus requires Z3. Either:"
            echo "  - Install Z3 and ensure it's in PATH"
            echo "  - Set VERUS_Z3_PATH to point to the z3 binary"
            echo ""
            read -p "Continue anyway? (y/N): " CONTINUE
            if [[ ! "$CONTINUE" =~ ^[Yy]$ ]]; then
                exit 1
            fi
        fi
    else
        if [[ ! -x "$VERUS_Z3_PATH" ]]; then
            echo -e "${RED}Error: VERUS_Z3_PATH is set but file not found or not executable: $VERUS_Z3_PATH${NC}"
            exit 1
        fi
        echo "Using Z3 from VERUS_Z3_PATH: $VERUS_Z3_PATH"
    fi
}

get_release_info() {
    local version="$1"
    local prerelease="$2"

    if [[ -n "$version" ]]; then
        # Fetch specific version
        echo "Fetching release info for version: $version..." >&2
        local releases
        releases=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases")

        # Try exact tag match first
        local release
        release=$(echo "$releases" | jq -r --arg v "$version" \
            '[.[] | select(.tag_name == $v)][0]')

        if [[ "$release" == "null" ]]; then
            # Try partial match
            release=$(echo "$releases" | jq -r --arg v "$version" \
                '[.[] | select(.tag_name | contains($v))][0]')
        fi

        if [[ "$release" == "null" ]]; then
            echo -e "${RED}Error: Version '$version' not found${NC}" >&2
            echo "Available versions:" >&2
            echo "$releases" | jq -r '.[0:10] | .[] | "  - \(.tag_name)"' >&2
            exit 1
        fi

        echo "$release"
    elif [[ "$prerelease" == "true" ]]; then
        echo "Fetching latest pre-release..." >&2
        local releases
        releases=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases")
        local release
        release=$(echo "$releases" | jq -r '[.[] | select(.prerelease == true)][0]')

        if [[ "$release" == "null" ]]; then
            echo -e "${RED}Error: No pre-release versions found${NC}" >&2
            exit 1
        fi

        echo "$release"
    else
        echo "Fetching latest stable release..." >&2
        curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest"
    fi
}

list_releases() {
    echo "Fetching available releases..."
    local releases
    releases=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases?per_page=${NUM_RELEASES}")

    echo ""
    echo "Available Verus releases:"
    echo "$releases" | jq -r '.[] | "  \(.tag_name)\(if .prerelease then " (pre-release)" else "" end) - \(.published_at | split("T")[0])"'
}

select_release_interactive() {
    echo "Fetching Verus releases from GitHub..." >&2
    local releases_json
    releases_json=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases?per_page=${NUM_RELEASES}")

    # Extract release info
    local releases
    releases=$(echo "$releases_json" | jq -r '.[] | "\(.tag_name)\t\(.published_at | split("T")[0])\t\(.prerelease)"')

    if [[ -z "$releases" ]]; then
        echo -e "${RED}Error:${NC} No releases found" >&2
        exit 1
    fi

    # Display releases
    echo "" >&2
    echo "Available Verus releases:" >&2
    echo "─────────────────────────────────────────────────" >&2
    printf "${CYAN}%-4s %-30s %-12s %s${NC}\n" "#" "Version" "Date" "Status" >&2
    echo "─────────────────────────────────────────────────" >&2

    local i=1
    declare -a versions
    while IFS=$'\t' read -r tag date prerelease; do
        versions+=("$tag")
        local status=""
        [[ "$prerelease" == "true" ]] && status="(pre-release)"
        printf "%-4s %-30s %-12s %s\n" "$i)" "$tag" "$date" "$status" >&2
        ((i++))
    done <<< "$releases"

    # Ask user to select
    echo "" >&2
    printf "Select a release (1-${#versions[@]}) [1]: " >&2
    { read SELECTION || read SELECTION </dev/tty; } 2>/dev/null || SELECTION=""
    SELECTION=${SELECTION:-1}

    # Validate selection
    if ! [[ "$SELECTION" =~ ^[0-9]+$ ]] || [[ "$SELECTION" -lt 1 ]] || [[ "$SELECTION" -gt ${#versions[@]} ]]; then
        echo -e "${RED}Error:${NC} Invalid selection" >&2
        exit 1
    fi

    local selected_version="${versions[$((SELECTION-1))]}"
    echo "" >&2
    echo "Selected: $selected_version" >&2

    # Return the release JSON for the selected version (to stdout)
    echo "$releases_json" | jq --arg tag "$selected_version" '.[] | select(.tag_name == $tag)'
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -v|--version)
            VERSION="$2"
            shift 2
            ;;
        -p|--pre-release|--prerelease)
            PRE_RELEASE=true
            shift
            ;;
        -i|--install-dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        -b|--build-dir)
            BUILD_DIR="$2"
            shift 2
            ;;
        -k|--keep-build)
            KEEP_BUILD=true
            shift
            ;;
        -j|--jobs)
            JOBS="$2"
            shift 2
            ;;
        -l|--list-releases)
            LIST_RELEASES=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            usage
            exit 1
            ;;
    esac
done

# List releases if requested
if $LIST_RELEASES; then
    list_releases
    exit 0
fi

# Validate arguments
if [[ -n "$VERSION" ]] && $PRE_RELEASE; then
    echo "Error: Cannot specify both --version and --pre-release"
    exit 1
fi

# Check prerequisites
echo "Checking prerequisites..."
check_prerequisites

# Get release information
if [[ -n "$VERSION" ]] || $PRE_RELEASE; then
    # Use specified version or pre-release flag
    RELEASE_JSON=$(get_release_info "$VERSION" "$PRE_RELEASE")
else
    # Interactive selection
    RELEASE_JSON=$(select_release_interactive)
fi

TAG_NAME=$(echo "$RELEASE_JSON" | jq -r '.tag_name')
PUBLISHED=$(echo "$RELEASE_JSON" | jq -r '.published_at')

# Extract version number from tag name (e.g., "release/0.2026.01.14.88f7396" -> "0.2026.01.14")
VERSION_NUMBER=$(echo "$TAG_NAME" | sed -E 's|.*/([0-9]+\.[0-9]+\.[0-9]+\.[0-9]+).*|\1|')
if [[ -z "$VERSION_NUMBER" ]] || [[ "$VERSION_NUMBER" == "$TAG_NAME" ]]; then
    # Fallback: use full tag name with slashes replaced
    VERSION_NUMBER=$(echo "$TAG_NAME" | tr '/' '-')
fi

# Set install directory with version number
if [[ -z "$INSTALL_DIR" ]]; then
    INSTALL_DIR="$CARGO_BIN_DIR/verus-$VERSION_NUMBER"
fi

echo ""
echo "============================================================"
echo "VERUS BUILDER FROM RELEASE"
echo "============================================================"
echo "Version: $TAG_NAME"
echo "Published: $PUBLISHED"
echo "Install directory: $INSTALL_DIR"

# Create build directory
if [[ -z "$BUILD_DIR" ]]; then
    BUILD_DIR=$(mktemp -d -t verus_build_XXXXXX)
    echo "Build directory: $BUILD_DIR (temporary)"
else
    mkdir -p "$BUILD_DIR"
    echo "Build directory: $BUILD_DIR"
fi

# Clone source at specific tag (using git to preserve version info)
echo ""
echo "Step 1: Cloning source code..."
cd "$BUILD_DIR"

VERUS_SRC="verus"
echo "Cloning verus-lang/verus at tag $TAG_NAME..."
git clone --depth 1 --branch "$TAG_NAME" "https://github.com/${GITHUB_REPO}.git" "$VERUS_SRC"

cd "$VERUS_SRC"
echo "Source directory: $(pwd)"

# Install required Rust toolchain
echo ""
echo "Step 2: Setting up Rust toolchain..."
cd source

if [[ -f "../rust-toolchain.toml" ]] || [[ -f "rust-toolchain.toml" ]]; then
    echo "Installing Rust toolchain from rust-toolchain.toml..."
    rustup show active-toolchain || rustup toolchain install
fi

# Activate the development environment and build
echo ""
echo "Step 3: Building Verus (this may take a while)..."

# Determine the correct activate script for the platform
ACTIVATE_SCRIPT=""
case "$(uname -s)" in
    Linux|Darwin)
        if [[ -f "../tools/activate" ]]; then
            ACTIVATE_SCRIPT="../tools/activate"
        fi
        ;;
    MINGW*|MSYS*|CYGWIN*)
        if [[ -f "../tools/activate.bat" ]]; then
            echo -e "${YELLOW}Warning: Windows detected. You may need to run activate.bat manually.${NC}"
        fi
        ;;
esac

# Source the activation script to set up vargo
if [[ -n "$ACTIVATE_SCRIPT" ]]; then
    echo "Sourcing $ACTIVATE_SCRIPT..."
    set +e
    source "$ACTIVATE_SCRIPT"
    set -e
else
    echo "No activate script found, proceeding with direct build..."
fi

# Build with vargo
VARGO_ARGS="build --release"
if [[ -n "$JOBS" ]]; then
    VARGO_ARGS="$VARGO_ARGS -j $JOBS"
fi

# Check if vargo is available (either from PATH or built by activate script)
if command -v vargo &>/dev/null; then
    echo "Running: vargo $VARGO_ARGS"
    vargo $VARGO_ARGS
elif [[ -f "../tools/vargo/target/release/vargo" ]]; then
    echo "Running: ../tools/vargo/target/release/vargo $VARGO_ARGS"
    ../tools/vargo/target/release/vargo $VARGO_ARGS
else
    # Build vargo first, then use it
    echo "Building vargo first..."
    if [[ -d "../tools/vargo" ]]; then
        (cd ../tools/vargo && cargo build --release)
        echo "Running: ../tools/vargo/target/release/vargo $VARGO_ARGS"
        ../tools/vargo/target/release/vargo $VARGO_ARGS
    else
        echo -e "${RED}Error: Cannot find vargo. Please check the Verus source structure.${NC}"
        exit 1
    fi
fi

echo -e "${GREEN}✓${NC} Build completed successfully!"

# Find the built binaries
echo ""
echo "Step 4: Installing to $INSTALL_DIR..."

# Look for target directory
TARGET_DIR=""
for dir in "target-verus/release" "target/release" "../target-verus/release" "../target/release"; do
    if [[ -d "$dir" ]]; then
        TARGET_DIR="$dir"
        break
    fi
done

if [[ -z "$TARGET_DIR" ]]; then
    echo -e "${RED}Error: Could not find build output directory${NC}"
    echo "Searched for: target-verus/release, target/release"
    exit 1
fi

echo "Found build output: $TARGET_DIR"

# Remove existing installation
if [[ -d "$INSTALL_DIR" ]]; then
    echo "Removing existing installation at $INSTALL_DIR"
    rm -rf "$INSTALL_DIR"
fi

# Copy entire release directory
echo "Copying release directory..."
cp -r "$TARGET_DIR" "$INSTALL_DIR"

# Create symlinks for verus and cargo-verus
mkdir -p "$CARGO_BIN_DIR"

# Symlink for verus
if [[ -e "$CARGO_BIN_DIR/verus" ]] || [[ -L "$CARGO_BIN_DIR/verus" ]]; then
    rm -rf "$CARGO_BIN_DIR/verus"
fi
ln -s "$INSTALL_DIR/verus" "$CARGO_BIN_DIR/verus"
echo "  Created symlink: $CARGO_BIN_DIR/verus -> $INSTALL_DIR/verus"

# Symlink for cargo-verus
if [[ -e "$CARGO_BIN_DIR/cargo-verus" ]] || [[ -L "$CARGO_BIN_DIR/cargo-verus" ]]; then
    rm -rf "$CARGO_BIN_DIR/cargo-verus"
fi
ln -s "$INSTALL_DIR/cargo-verus" "$CARGO_BIN_DIR/cargo-verus"
echo "  Created symlink: $CARGO_BIN_DIR/cargo-verus -> $INSTALL_DIR/cargo-verus"

echo -e "${GREEN}✓${NC} Verus installed to: $INSTALL_DIR"

# Set up PATH - ensure .cargo/bin is in PATH
echo ""
echo "Step 5: Configuring PATH..."

get_shell_config_file() {
    local shell_name
    shell_name=$(basename "$SHELL" 2>/dev/null || echo "bash")

    case "$shell_name" in
        zsh)
            if [[ -f "$HOME/.zshrc" ]]; then echo "$HOME/.zshrc"
            elif [[ -f "$HOME/.zprofile" ]]; then echo "$HOME/.zprofile"
            else echo "$HOME/.zshrc"
            fi
            ;;
        *)
            if [[ -f "$HOME/.bashrc" ]]; then echo "$HOME/.bashrc"
            elif [[ -f "$HOME/.bash_profile" ]]; then echo "$HOME/.bash_profile"
            elif [[ -f "$HOME/.profile" ]]; then echo "$HOME/.profile"
            else echo "$HOME/.bashrc"
            fi
            ;;
    esac
}

CONFIG_FILE=$(get_shell_config_file)

# Check if .cargo/bin is already in PATH
if echo "$PATH" | tr ':' '\n' | grep -qx "$CARGO_BIN_DIR"; then
    echo ".cargo/bin is already in PATH"
elif grep -q "$CARGO_BIN_DIR" "$CONFIG_FILE" 2>/dev/null; then
    echo ".cargo/bin is configured in $CONFIG_FILE (will be available after restart)"
else
    echo "Adding .cargo/bin to PATH in $CONFIG_FILE"
    echo "" >> "$CONFIG_FILE"
    echo "# Cargo bin directory" >> "$CONFIG_FILE"
    echo "export PATH=\"\$HOME/.cargo/bin:\$PATH\"" >> "$CONFIG_FILE"
fi

# Verify installation
echo ""
echo "Step 6: Verifying installation..."

VERUS_BINARY="$INSTALL_DIR/verus"
if [[ ! -f "$VERUS_BINARY" ]]; then
    VERUS_BINARY="$INSTALL_DIR/rust_verify"
fi

if [[ -f "$VERUS_BINARY" ]]; then
    if "$VERUS_BINARY" --version >/dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} Verus is working!"
        "$VERUS_BINARY" --version 2>&1 || true
    else
        echo -e "${YELLOW}⚠${NC} Verus binary exists but version check failed"
        echo "This may be normal - try running it manually"
    fi
else
    echo -e "${YELLOW}⚠${NC} Could not find verus binary for verification"
fi

# Cleanup
echo ""
if ! $KEEP_BUILD; then
    echo "Cleaning up build directory..."
    rm -rf "$BUILD_DIR"
    echo -e "${GREEN}✓${NC} Build directory removed"
else
    echo "Build directory kept at: $BUILD_DIR"
fi

# Summary
echo ""
echo "============================================================"
echo "BUILD COMPLETE"
echo "============================================================"
echo -e "${GREEN}✓${NC} Version: $TAG_NAME"
echo -e "${GREEN}✓${NC} Installed to: $INSTALL_DIR"
echo ""
echo "Next steps:"
echo "   1. Restart your terminal or run: source $CONFIG_FILE"
echo "   2. Ensure VERUS_Z3_PATH is set if Z3 is not in PATH"
echo "   3. Run 'verus --version' to verify"
