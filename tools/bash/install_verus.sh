#!/bin/bash
#
# Verus Latest Release Downloader
#
# Downloads the latest release of Verus from GitHub releases.
# Supports latest stable release, latest pre-release, or specific version.
#
# Requirements: curl, jq, unzip (for .zip), tar (for .tar.gz)
#

set -e

# Default values
VERSION=""
PRE_RELEASE=false
OUTPUT_DIR="."
INSTALL_DIR=""
PLATFORM=""
LIST_ASSETS=false
NO_EXTRACT=false
NO_PATH=false

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

GITHUB_REPO="verus-lang/verus"
TOOL_NAME="Verus"
BINARY_NAME="verus"
DEFAULT_INSTALL_DIR="$HOME/verus"

usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Download and install the latest Verus release.

Options:
  -v, --version VERSION   Download a specific version (e.g., "0.2025.08.25.63ab0cb")
  -p, --pre-release       Download the latest pre-release version instead of stable
  -o, --output-dir DIR    Download directory (default: current directory)
  -i, --install-dir DIR   Installation directory (default: ~/verus)
  --platform PATTERN      Platform pattern to search for (e.g., x86-linux)
  -l, --list-assets       List all available assets without downloading
  --no-extract            Download only, do not extract or install
  --no-path               Do not modify PATH configuration
  -h, --help              Show this help message

Examples:
  $(basename "$0")
  $(basename "$0") --pre-release
  $(basename "$0") --version "0.2025.08.25.63ab0cb"
  $(basename "$0") --install-dir /opt/verus
  $(basename "$0") --list-assets
EOF
}

get_platform_pattern() {
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m | tr '[:upper:]' '[:lower:]')

    # Map OS and architecture to Verus naming convention
    case "$os" in
        linux)
            case "$arch" in
                x86_64|amd64) echo "x86-linux" ;;
                *) echo "Warning: Unknown architecture $arch for Linux" >&2; return 1 ;;
            esac
            ;;
        darwin)
            case "$arch" in
                x86_64|amd64) echo "x86-macos" ;;
                aarch64|arm64) echo "arm64-macos" ;;
                *) echo "Warning: Unknown architecture $arch for macOS" >&2; return 1 ;;
            esac
            ;;
        mingw*|msys*|cygwin*)
            case "$arch" in
                x86_64|amd64) echo "x86-win" ;;
                *) echo "Warning: Unknown architecture $arch for Windows" >&2; return 1 ;;
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

update_path_config() {
    local install_dir="$1"
    local config_file
    config_file=$(get_shell_config_file)

    local path_line="export PATH=\"${install_dir}:\$PATH\"  # Added by Verus installer"

    if [[ -f "$config_file" ]] && grep -q "$install_dir" "$config_file" 2>/dev/null; then
        echo "PATH already configured in $config_file"
        return 0
    fi

    echo "Adding Verus to PATH in $config_file"
    echo "" >> "$config_file"
    echo "# Verus installation" >> "$config_file"
    echo "$path_line" >> "$config_file"

    echo "$config_file"
}

make_binaries_executable() {
    local dir="$1"
    echo "Making binaries executable..."

    # Find files that look like ELF or Mach-O executables and make them executable
    find "$dir" -type f | while read -r file; do
        local header
        header=$(xxd -p -l 4 "$file" 2>/dev/null || od -A n -t x1 -N 4 "$file" 2>/dev/null | tr -d ' ')

        case "$header" in
            7f454c46)
                # ELF binary (Linux) - 0x7f 'E' 'L' 'F'
                chmod +x "$file"
                echo "  Made executable: $(basename "$file")"
                ;;
            feedface|feedfacf|cefaedfe|cffaedfe|cafebabe)
                # Mach-O binary (macOS) - various magic numbers including universal
                chmod +x "$file"
                echo "  Made executable: $(basename "$file")"
                ;;
            *)
                # Check known binary names as fallback
                case "$(basename "$file")" in
                    verus|rust_verify|z3|verus.exe|rust_verify.exe|z3.exe)
                        chmod +x "$file"
                        echo "  Made executable: $(basename "$file")"
                        ;;
                esac
                ;;
        esac
    done
}

verify_installation() {
    local binary_path="$1"
    echo "Verifying Verus installation..."

    if "$binary_path" --version >/dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} Verus is working! Version info:"
        "$binary_path" --version 2>&1 || true
        return 0
    else
        echo -e "${YELLOW}⚠${NC} Verus binary exists but may have issues"
        return 1
    fi
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

# Validate arguments
if [[ -n "$VERSION" ]] && $PRE_RELEASE; then
    echo "Error: Cannot specify both --version and --pre-release"
    exit 1
fi

# Set default install directory
[[ -z "$INSTALL_DIR" ]] && INSTALL_DIR="$DEFAULT_INSTALL_DIR"

# Fetch release information
if [[ -n "$VERSION" ]]; then
    echo "Fetching Verus release version: $VERSION..."
    RELEASES_JSON=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases")

    # Find matching release
    RELEASE_JSON=$(echo "$RELEASES_JSON" | jq -r --arg version "$VERSION" \
        '[.[] | select(.tag_name | contains($version)) or select(.name | contains($version))][0]')

    if [[ "$RELEASE_JSON" == "null" ]]; then
        echo "Error: Version '$VERSION' not found"
        echo "Available versions:"
        echo "$RELEASES_JSON" | jq -r '.[0:10] | .[] | "  - \(.tag_name) (\(.name // "No name"))"'
        TOTAL=$(echo "$RELEASES_JSON" | jq -r 'length')
        if [[ "$TOTAL" -gt 10 ]]; then
            echo "  ... and $((TOTAL - 10)) more releases"
        fi
        exit 1
    fi
elif $PRE_RELEASE; then
    echo "Fetching latest Verus pre-release..."
    RELEASES_JSON=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases")
    RELEASE_JSON=$(echo "$RELEASES_JSON" | jq -r '[.[] | select(.prerelease == true)][0]')
    if [[ "$RELEASE_JSON" == "null" ]]; then
        echo "Error: No pre-release versions found"
        exit 1
    fi
else
    echo "Fetching latest stable Verus release..."
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
    echo "$RELEASE_JSON" | jq -r '.assets[] | "  - \(.name) (\(.size / 1048576 | floor) MB)"'
    exit 0
fi

# Determine platform pattern
if [[ -z "$PLATFORM" ]]; then
    PLATFORM=$(get_platform_pattern) || {
        echo "Could not determine platform automatically."
        echo "Available assets:"
        echo "$RELEASE_JSON" | jq -r '.assets[] | "  - \(.name)"'
        exit 1
    }
fi

# Find matching asset (select first match)
ASSET_JSON=$(echo "$RELEASE_JSON" | jq -r --arg pattern "$PLATFORM" \
    '[.assets[] | select(.name | ascii_downcase | contains($pattern | ascii_downcase))][0]')

if [[ -z "$ASSET_JSON" ]] || [[ "$ASSET_JSON" == "null" ]]; then
    echo "No asset found for platform pattern: $PLATFORM"
    echo "Available assets:"
    echo "$RELEASE_JSON" | jq -r '.assets[] | "  - \(.name)"'
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
    IS_ZIP=false
    IS_TARGZ=false

    if [[ "$FILENAME" == *.zip ]]; then
        IS_ZIP=true
        if ! command -v unzip &>/dev/null; then
            echo "Error: unzip is required to extract .zip files"
            exit 1
        fi
    elif [[ "$FILENAME" == *.tar.gz ]] || [[ "$FILENAME" == *.tgz ]]; then
        IS_TARGZ=true
    fi

    if $IS_ZIP || $IS_TARGZ; then
        echo ""
        echo "Extracting $ASSET_NAME..."

        TEMP_DIR=$(mktemp -d)

        if $IS_ZIP; then
            unzip -q "$FILENAME" -d "$TEMP_DIR"
        else
            tar -xzf "$FILENAME" -C "$TEMP_DIR"
        fi

        # Find the verus binary
        VERUS_BINARY=$(find "$TEMP_DIR" -name "$BINARY_NAME" -type f | head -1)
        if [[ -z "$VERUS_BINARY" ]]; then
            VERUS_BINARY=$(find "$TEMP_DIR" -name "${BINARY_NAME}.exe" -type f | head -1)
        fi

        if [[ -z "$VERUS_BINARY" ]]; then
            echo "Error: Could not find $BINARY_NAME binary in archive"
            rm -rf "$TEMP_DIR"
            exit 1
        fi

        echo "Found binary: $VERUS_BINARY"

        # Get the directory containing the binary
        BINARY_DIR=$(dirname "$VERUS_BINARY")

        # Install
        if [[ -d "$INSTALL_DIR" ]]; then
            echo "Removing existing installation at $INSTALL_DIR"
            rm -rf "$INSTALL_DIR"
        fi

        echo "Installing Verus to: $INSTALL_DIR"

        # Copy entire binary directory (Verus has multiple binaries)
        cp -r "$BINARY_DIR" "$INSTALL_DIR"

        # Make all binaries executable
        make_binaries_executable "$INSTALL_DIR"

        # Also ensure main binary is executable
        INSTALLED_BINARY="$INSTALL_DIR/$BINARY_NAME"
        if [[ -f "$INSTALLED_BINARY" ]]; then
            chmod +x "$INSTALLED_BINARY"
        elif [[ -f "$INSTALL_DIR/${BINARY_NAME}.exe" ]]; then
            INSTALLED_BINARY="$INSTALL_DIR/${BINARY_NAME}.exe"
        fi

        echo -e "${GREEN}✓${NC} Verus installed to: $INSTALL_DIR"
        echo -e "${GREEN}✓${NC} Verus binary: $INSTALLED_BINARY"

        # Verify
        verify_installation "$INSTALLED_BINARY" || true

        # Update PATH
        if ! $NO_PATH; then
            CONFIG_FILE=$(update_path_config "$INSTALL_DIR")
            echo ""
            echo "Next steps:"
            echo "   1. Restart your terminal or run: source $CONFIG_FILE"
            echo "   2. Type 'verus --version' to verify"
            echo "   Or run directly: $INSTALLED_BINARY --version"
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
        echo "Downloaded file is not a recognized archive: $FILENAME"
        echo "Manual installation may be required."
    fi
else
    echo ""
    echo "To manually extract and install:"
    if [[ "$FILENAME" == *.zip ]]; then
        echo "  unzip '$FILENAME'"
    else
        echo "  tar -xzf '$FILENAME'"
    fi
fi
