#!/bin/bash
#
# Z3 SMT Solver Installer
#
# Downloads Z3 from GitHub releases, allowing the user to select a version.
# Installs to ~/.local/bin and ensures it's in PATH.
#
# Requirements: curl, jq, unzip
#

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

GITHUB_REPOS=("Z3Prover/z3" "Beneficial-AI-Foundation/z3")
INSTALL_DIR="$HOME/.local/bin"
NUM_RELEASES=30

usage() {
    cat << EOF
Usage: $(basename "$0") [OPTIONS]

Download and install Z3 SMT solver from GitHub releases.

Options:
  -n, --num-releases N   Number of releases to show (default: 30)
  -l, --list             List releases without installing
  -h, --help             Show this help message

Examples:
  $(basename "$0")
  $(basename "$0") --num-releases 30
  $(basename "$0") --list
EOF
}

get_platform_pattern() {
    local os arch
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)

    # Map OS names to Z3 naming convention
    # Linux builds use "glibc", macOS uses "osx", Windows uses "win"
    case "$os" in
        linux) os="glibc" ;;
        darwin) os="osx" ;;
        mingw*|msys*|cygwin*) os="win" ;;
        *) echo "Error: Unknown OS $os" >&2; return 1 ;;
    esac

    # Map architecture names for Z3 naming convention
    case "$arch" in
        x86_64|amd64) arch="x64" ;;
        aarch64|arm64) arch="arm64" ;;
        *) echo "Error: Unknown architecture $arch" >&2; return 1 ;;
    esac

    # Pattern: arch-os (e.g., x64-glibc, arm64-osx, x64-win)
    echo "${arch}-${os}"
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
    local path_line="export PATH=\"\$HOME/.local/bin:\$PATH\"  # Added by Z3 installer"

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

LIST_ONLY=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -n|--num-releases)
            NUM_RELEASES="$2"
            shift 2
            ;;
        -l|--list)
            LIST_ONLY=true
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
for cmd in curl jq unzip; do
    if ! command -v "$cmd" &>/dev/null; then
        echo -e "${RED}Error:${NC} $cmd is required but not installed."
        exit 1
    fi
done

# Fetch releases from all repos
echo "Fetching Z3 releases from GitHub..."
COMBINED_RELEASES="[]"

for repo in "${GITHUB_REPOS[@]}"; do
    echo "  - $repo"
    REPO_RELEASES=$(curl -fsSL "https://api.github.com/repos/${repo}/releases?per_page=${NUM_RELEASES}" 2>/dev/null || echo "[]")
    # Add repo field to each release
    REPO_RELEASES=$(echo "$REPO_RELEASES" | jq --arg repo "$repo" '[.[] | . + {repo: $repo}]')
    COMBINED_RELEASES=$(echo "$COMBINED_RELEASES $REPO_RELEASES" | jq -s 'add')
done

# Deduplicate by version, keeping earliest date, and sort by date descending
# Group by tag_name, take the one with earliest published_at for each group
RELEASES_JSON=$(echo "$COMBINED_RELEASES" | jq '
    group_by(.tag_name) |
    map(sort_by(.published_at) | first) |
    sort_by(.published_at) |
    reverse |
    .[0:'"${NUM_RELEASES}"']
')

# Keep full combined releases for later lookup across all sources
ALL_RELEASES_JSON="$COMBINED_RELEASES"

# Extract release info (without repo, just version/date/prerelease)
RELEASES=$(echo "$RELEASES_JSON" | jq -r '.[] | "\(.tag_name)\t\(.published_at | split("T")[0])\t\(.prerelease)"')

if [[ -z "$RELEASES" ]]; then
    echo -e "${RED}Error:${NC} No releases found"
    exit 1
fi

# Display releases
echo ""
echo "Available Z3 releases:"
echo "─────────────────────────────────────────"
printf "${CYAN}%-4s %-20s %-12s %s${NC}\n" "#" "Version" "Date" "Status"
echo "─────────────────────────────────────────"

i=1
declare -a VERSIONS
while IFS=$'\t' read -r tag date prerelease; do
    VERSIONS+=("$tag")
    status=""
    [[ "$prerelease" == "true" ]] && status="(pre-release)"
    printf "%-4s %-20s %-12s %s\n" "$i)" "$tag" "$date" "$status"
    ((i++))
done <<< "$RELEASES"

if $LIST_ONLY; then
    exit 0
fi

# Ask user to select
echo ""
read -p "Select a release (1-${#VERSIONS[@]}) [1]: " SELECTION
SELECTION=${SELECTION:-1}

# Validate selection
if ! [[ "$SELECTION" =~ ^[0-9]+$ ]] || [[ "$SELECTION" -lt 1 ]] || [[ "$SELECTION" -gt ${#VERSIONS[@]} ]]; then
    echo -e "${RED}Error:${NC} Invalid selection"
    exit 1
fi

SELECTED_VERSION="${VERSIONS[$((SELECTION-1))]}"
echo ""
echo "Selected: $SELECTED_VERSION"

# Determine platform pattern
PLATFORM_PATTERN=$(get_platform_pattern) || exit 1
echo "Looking for platform: $PLATFORM_PATTERN"

# Get all releases with this version from all sources
# Normally prioritize Z3Prover, but for z3-4.12.5 on arm64-glibc, prefer Beneficial-AI-Foundation
# (Z3Prover's arm64-glibc-2.35 build has compatibility issues)
if [[ "$SELECTED_VERSION" == "z3-4.12.5" ]] && [[ "$PLATFORM_PATTERN" == "arm64-glibc" ]]; then
    MATCHING_RELEASES=$(echo "$ALL_RELEASES_JSON" | jq -r --arg tag "$SELECTED_VERSION" \
        '[.[] | select(.tag_name == $tag)] | sort_by(if .repo == "Beneficial-AI-Foundation/z3" then 0 else 1 end)')
else
    MATCHING_RELEASES=$(echo "$ALL_RELEASES_JSON" | jq -r --arg tag "$SELECTED_VERSION" \
        '[.[] | select(.tag_name == $tag)] | sort_by(if .repo == "Z3Prover/z3" then 0 else 1 end)')
fi

# Try each source in order until we find a matching asset
RELEASE_JSON=""
ASSET_JSON=""
for row in $(echo "$MATCHING_RELEASES" | jq -r '.[] | @base64'); do
    _release=$(echo "$row" | base64 -d)
    _repo=$(echo "$_release" | jq -r '.repo')
    _asset=$(echo "$_release" | jq -r --arg pattern "$PLATFORM_PATTERN" \
        '[.assets[] | select(.name | test($pattern; "i")) | select(.name | endswith(".zip"))][0]')

    if [[ -n "$_asset" ]] && [[ "$_asset" != "null" ]]; then
        RELEASE_JSON="$_release"
        ASSET_JSON="$_asset"
        echo "Found matching asset in: $_repo"
        break
    fi
done

if [[ -z "$ASSET_JSON" ]] || [[ "$ASSET_JSON" == "null" ]]; then
    echo -e "${RED}Error:${NC} No matching asset found for your platform in any source"
    echo ""
    echo "Available assets:"
    echo "$MATCHING_RELEASES" | jq -r '.[] | "[\(.repo)]", (.assets[] | select(.name | endswith(".zip")) | "  - \(.name)")'
    exit 1
fi

ASSET_NAME=$(echo "$ASSET_JSON" | jq -r '.name')
ASSET_SIZE=$(echo "$ASSET_JSON" | jq -r '.size')
DOWNLOAD_URL=$(echo "$ASSET_JSON" | jq -r '.browser_download_url')
SIZE_MB=$((ASSET_SIZE / 1048576))

echo "Found: $ASSET_NAME (${SIZE_MB} MB)"

# Create temp directory
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

DOWNLOAD_PATH="$TEMP_DIR/$ASSET_NAME"

# Download
echo ""
echo "Downloading..."
curl -L --progress-bar -o "$DOWNLOAD_PATH" "$DOWNLOAD_URL"
echo -e "${GREEN}✓${NC} Download completed"

# Extract
echo "Extracting..."
unzip -q "$DOWNLOAD_PATH" -d "$TEMP_DIR"

# Find z3 binary
Z3_BINARY=$(find "$TEMP_DIR" -name "z3" -type f -perm -u+x 2>/dev/null | head -1)
if [[ -z "$Z3_BINARY" ]]; then
    # Try without execute permission check (Windows builds)
    Z3_BINARY=$(find "$TEMP_DIR" -name "z3" -type f 2>/dev/null | head -1)
fi

if [[ -z "$Z3_BINARY" ]]; then
    echo -e "${RED}Error:${NC} Could not find z3 binary in archive"
    echo "Contents:"
    find "$TEMP_DIR" -type f | head -20
    exit 1
fi

echo "Found binary: $Z3_BINARY"

# Create install directory
mkdir -p "$INSTALL_DIR"

# Copy binary
INSTALLED_BINARY="$INSTALL_DIR/z3"
cp "$Z3_BINARY" "$INSTALLED_BINARY"
chmod +x "$INSTALLED_BINARY"

echo -e "${GREEN}✓${NC} Z3 installed to: $INSTALLED_BINARY"

# Verify installation
echo ""
echo "Verifying installation..."
if "$INSTALLED_BINARY" --version >/dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} Z3 is working!"
    "$INSTALLED_BINARY" --version
else
    echo -e "${YELLOW}⚠${NC} Z3 binary exists but may have issues running"
fi

# Ensure PATH
echo ""
ensure_path

echo ""
echo -e "${GREEN}Installation complete!${NC}"
