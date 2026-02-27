#!/bin/bash
#
# Rust Analyzer Latest Release Downloader
#
# Downloads the latest release of Rust Analyzer from GitHub releases.
# Supports latest stable release or latest pre-release.
#
# Requirements: curl, jq, gunzip
#

set -e

# Default values
PRE_RELEASE=false
OUTPUT_DIR="."
INSTALL_DIR=""
PLATFORM=""
LIST_ASSETS=false
NO_EXTRACT=false
NO_PATH=false
VSIX=false

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

GITHUB_REPO="rust-lang/rust-analyzer"
TOOL_NAME="Rust Analyzer"
BINARY_NAME="rust-analyzer"
DEFAULT_INSTALL_DIR="$HOME/.local/bin"

usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Download and install the latest Rust Analyzer release.

Options:
  -p, --pre-release       Download the latest pre-release version instead of stable
  -o, --output-dir DIR    Download directory (default: current directory)
  -i, --install-dir DIR   Installation directory (default: ~/.local/bin)
  --platform PATTERN      Platform pattern to search for (e.g., x86_64-unknown-linux-gnu)
  -l, --list-assets       List all available assets without downloading
  --no-extract            Download only, do not extract or install
  --no-path               Do not modify PATH configuration
  --vsix                  Download VS Code extension (.vsix) instead of binary
  -h, --help              Show this help message

Examples:
  $(basename "$0")
  $(basename "$0") --pre-release
  $(basename "$0") --install-dir /opt/rust-analyzer
  $(basename "$0") --vsix
  $(basename "$0") --list-assets
EOF
}

get_platform_pattern() {
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m | tr '[:upper:]' '[:lower:]')

    # Map OS names
    case "$os" in
        linux)
            case "$arch" in
                x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
                aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
                armv7l|arm) echo "arm-unknown-linux-gnueabihf" ;;
                *) echo "Warning: Unknown architecture $arch" >&2; return 1 ;;
            esac
            ;;
        darwin)
            case "$arch" in
                x86_64|amd64) echo "x86_64-apple-darwin" ;;
                aarch64|arm64) echo "aarch64-apple-darwin" ;;
                *) echo "Warning: Unknown architecture $arch" >&2; return 1 ;;
            esac
            ;;
        mingw*|msys*|cygwin*)
            case "$arch" in
                x86_64|amd64) echo "x86_64-pc-windows-msvc" ;;
                aarch64|arm64) echo "aarch64-pc-windows-msvc" ;;
                i686|i386) echo "i686-pc-windows-msvc" ;;
                *) echo "Warning: Unknown architecture $arch" >&2; return 1 ;;
            esac
            ;;
        *)
            echo "Warning: Unknown OS $os" >&2
            return 1
            ;;
    esac
}

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

ensure_path() {
    local config_file
    config_file=$(get_shell_config_file)
    local path_line="export PATH=\"\$HOME/.local/bin:\$PATH\"  # Added by Rust Analyzer installer"

    # Check if ~/.local/bin is already in PATH
    if [[ ":$PATH:" == *":$HOME/.local/bin:"* ]]; then
        echo -e "${GREEN}✓${NC} ~/.local/bin is already in PATH"
        return 0
    fi

    # Check if it's already in the config file
    if [[ -f "$config_file" ]] && grep -q '\.local/bin' "$config_file" 2>/dev/null; then
        echo -e "${YELLOW}⚠${NC} ~/.local/bin is configured in $config_file but not in current session"
        echo "   Run: source $config_file"
        return 0
    fi

    echo "Adding ~/.local/bin to PATH in $config_file"
    echo "" >> "$config_file"
    echo "# Local binaries" >> "$config_file"
    echo "$path_line" >> "$config_file"

    echo -e "${GREEN}✓${NC} PATH updated in $config_file"
    echo "   Run: source $config_file"
}

verify_installation() {
    local binary_path="$1"
    echo "Verifying Rust Analyzer installation..."

    if "$binary_path" --version >/dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} Rust Analyzer is working! Version info:"
        "$binary_path" --version 2>&1 || true
        return 0
    else
        echo -e "${YELLOW}⚠${NC} Rust Analyzer binary exists but may have issues"
        return 1
    fi
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -p|--pre-release|--prerelease)
            PRE_RELEASE=true
            shift
            ;;
        -o|--output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        -i|--install-dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        --platform)
            PLATFORM="$2"
            shift 2
            ;;
        -l|--list-assets)
            LIST_ASSETS=true
            shift
            ;;
        --no-extract)
            NO_EXTRACT=true
            shift
            ;;
        --no-path)
            NO_PATH=true
            shift
            ;;
        --vsix)
            VSIX=true
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

# Check dependencies
for cmd in curl jq; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "Error: $cmd is required but not installed."
        exit 1
    fi
done

# Set default install directory
[[ -z "$INSTALL_DIR" ]] && INSTALL_DIR="$DEFAULT_INSTALL_DIR"

# Fetch release information
if $PRE_RELEASE; then
    echo "Fetching latest Rust Analyzer pre-release..."
    RELEASES_JSON=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases")
    RELEASE_JSON=$(echo "$RELEASES_JSON" | jq -r '[.[] | select(.prerelease == true)][0]')
    if [[ "$RELEASE_JSON" == "null" ]]; then
        echo "Error: No pre-release versions found"
        exit 1
    fi
else
    echo "Fetching latest stable Rust Analyzer release..."
    RELEASE_JSON=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest")
fi

TAG_NAME=$(echo "$RELEASE_JSON" | jq -r '.tag_name')
PUBLISHED=$(echo "$RELEASE_JSON" | jq -r '.published_at')
IS_PRERELEASE=$(echo "$RELEASE_JSON" | jq -r '.prerelease')
RELEASE_NAME=$(echo "$RELEASE_JSON" | jq -r '.name')

echo "Found release: $TAG_NAME"
echo "Published: $PUBLISHED"
echo "Pre-release: $IS_PRERELEASE"
echo "Description: $RELEASE_NAME"

# Show release notes if available
RELEASE_BODY=$(echo "$RELEASE_JSON" | jq -r '.body // empty')
if [[ -n "$RELEASE_BODY" ]]; then
    echo "Release notes:"
    echo "$RELEASE_BODY" | head -c 200
    if [[ ${#RELEASE_BODY} -gt 200 ]]; then
        echo "..."
    fi
    echo ""
fi

# List assets if requested
if $LIST_ASSETS; then
    echo ""
    echo "Available assets:"
    echo "$RELEASE_JSON" | jq -r '.assets[] | "  - \(.name) (\(.size / 1048576 | floor) MB) - \(if (.name | endswith(".vsix")) then "VS Code Extension" else "Binary" end)"'
    exit 0
fi

# Handle VS Code extension download
if $VSIX; then
    ASSET_JSON=$(echo "$RELEASE_JSON" | jq -r '[.assets[] | select(.name | endswith(".vsix"))][0]')

    if [[ -z "$ASSET_JSON" ]] || [[ "$ASSET_JSON" == "null" ]]; then
        echo "No VS Code extension found"
        echo "Available .vsix assets:"
        echo "$RELEASE_JSON" | jq -r '.assets[] | select(.name | endswith(".vsix")) | "  - \(.name)"'
        exit 1
    fi

    ASSET_NAME=$(echo "$ASSET_JSON" | jq -r '.name')
    ASSET_SIZE=$(echo "$ASSET_JSON" | jq -r '.size')
    DOWNLOAD_URL=$(echo "$ASSET_JSON" | jq -r '.browser_download_url')
    SIZE_MB=$((ASSET_SIZE / 1048576))

    mkdir -p "$OUTPUT_DIR"
    FILENAME="$OUTPUT_DIR/$ASSET_NAME"

    echo ""
    echo "Downloading $ASSET_NAME (${SIZE_MB} MB)..."
    curl -L --progress-bar -o "$FILENAME" "$DOWNLOAD_URL"

    echo -e "${GREEN}✓${NC} Download completed: $FILENAME"
    echo ""
    echo "VS Code extension downloaded: $FILENAME"
    echo "To install the extension in VS Code, run:"
    echo "   code --install-extension $FILENAME"
    exit 0
fi

# Determine platform pattern for binary
if [[ -z "$PLATFORM" ]]; then
    PLATFORM=$(get_platform_pattern) || {
        echo "Could not determine platform automatically."
        echo "Available assets:"
        echo "$RELEASE_JSON" | jq -r '.assets[] | select(.name | endswith(".gz")) | select(.name | contains("rust-analyzer-")) | select(.name | endswith(".vsix") | not) | "  - \(.name)"'
        exit 1
    }
fi

# Find matching asset (binary, not vsix, must contain rust-analyzer-) - select first match
ASSET_JSON=$(echo "$RELEASE_JSON" | jq -r --arg pattern "$PLATFORM" \
    '[.assets[] | select(.name | ascii_downcase | contains($pattern | ascii_downcase)) | select(.name | endswith(".gz")) | select(.name | contains("rust-analyzer-")) | select(.name | endswith(".vsix") | not)][0]')

if [[ -z "$ASSET_JSON" ]] || [[ "$ASSET_JSON" == "null" ]]; then
    echo "No binary asset found for platform pattern: $PLATFORM"
    echo "Available binary assets:"
    echo "$RELEASE_JSON" | jq -r '.assets[] | select(.name | endswith(".gz")) | select(.name | contains("rust-analyzer-")) | select(.name | endswith(".vsix") | not) | "  - \(.name)"'
    exit 1
fi

ASSET_NAME=$(echo "$ASSET_JSON" | jq -r '.name')
ASSET_SIZE=$(echo "$ASSET_JSON" | jq -r '.size')
DOWNLOAD_URL=$(echo "$ASSET_JSON" | jq -r '.browser_download_url')
SIZE_MB=$((ASSET_SIZE / 1048576))

# Create output directory
mkdir -p "$OUTPUT_DIR"
FILENAME="$OUTPUT_DIR/$ASSET_NAME"

echo ""
echo "Downloading $ASSET_NAME (${SIZE_MB} MB)..."
echo "URL: $DOWNLOAD_URL"
echo "Saving to: $FILENAME"

# Download with progress
curl -L --progress-bar -o "$FILENAME" "$DOWNLOAD_URL"

echo -e "${GREEN}✓${NC} Download completed: $FILENAME"

# Extract and install
if ! $NO_EXTRACT; then
    if [[ "$FILENAME" == *.gz ]]; then
        echo ""
        echo "Extracting $ASSET_NAME..."

        # The extracted filename is the original name without .gz
        EXTRACTED_NAME="${ASSET_NAME%.gz}"
        TEMP_DIR=$(mktemp -d)
        EXTRACTED_PATH="$TEMP_DIR/$EXTRACTED_NAME"

        gunzip -c "$FILENAME" > "$EXTRACTED_PATH"
        chmod +x "$EXTRACTED_PATH"

        echo "Extracted to: $EXTRACTED_PATH"

        # Install
        if [[ -d "$INSTALL_DIR" ]]; then
            echo "Removing existing installation at $INSTALL_DIR"
            rm -rf "$INSTALL_DIR"
        fi

        echo "Installing Rust Analyzer to: $INSTALL_DIR"
        mkdir -p "$INSTALL_DIR"

        INSTALLED_BINARY="$INSTALL_DIR/$BINARY_NAME"
        cp "$EXTRACTED_PATH" "$INSTALLED_BINARY"
        chmod +x "$INSTALLED_BINARY"

        echo -e "${GREEN}✓${NC} Rust Analyzer installed to: $INSTALL_DIR"
        echo -e "${GREEN}✓${NC} Rust Analyzer binary: $INSTALLED_BINARY"

        # Verify
        verify_installation "$INSTALLED_BINARY" || true

        # Ensure ~/.local/bin is in PATH
        if ! $NO_PATH; then
            echo ""
            ensure_path
        fi

        # Cleanup temp
        rm -rf "$TEMP_DIR"

        # Ask to remove archive
        echo ""
        read -p "Remove downloaded archive $FILENAME? (y/N): " REMOVE_ARCHIVE
        if [[ "$REMOVE_ARCHIVE" =~ ^[Yy]$ ]]; then
            rm -f "$FILENAME"
            echo -e "${GREEN}✓${NC} Archive removed"
        fi
    else
        echo "Downloaded file is not a gzipped archive: $FILENAME"
        echo "Manual installation may be required."
    fi
else
    echo ""
    echo "To manually extract and install:"
    echo "  gunzip '$FILENAME'"
    echo "  chmod +x '${FILENAME%.gz}'"
fi
