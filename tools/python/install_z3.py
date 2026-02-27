#!/usr/bin/env python3
# /// script
# dependencies = ["requests"]
# ///
"""
Z3 SMT Solver Installer

Downloads Z3 from GitHub releases, allowing the user to select a version.
Supports releases from both Z3Prover/z3 and Beneficial-AI-Foundation/z3.
Installs to ~/.local/bin and ensures it's in PATH.
"""

import requests
import os
import sys
import platform
import shutil
import stat
import zipfile
import tempfile
from pathlib import Path
import argparse
import subprocess


GITHUB_REPOS = ["Z3Prover/z3", "Beneficial-AI-Foundation/z3"]
DEFAULT_NUM_RELEASES = 30


def get_platform_pattern():
    """Determine the appropriate asset pattern for the current platform."""
    system = platform.system().lower()
    machine = platform.machine().lower()

    # Map OS names to Z3 naming convention
    # Linux builds use "glibc", macOS uses "osx", Windows uses "win"
    os_mapping = {
        'linux': 'glibc',
        'darwin': 'osx',
        'windows': 'win',
    }

    # Map architecture names for Z3 naming convention
    arch_mapping = {
        'x86_64': 'x64',
        'amd64': 'x64',
        'aarch64': 'arm64',
        'arm64': 'arm64',
    }

    if system not in os_mapping:
        print(f"Warning: Unknown operating system {system}")
        return None

    if machine not in arch_mapping:
        print(f"Warning: Unknown architecture {machine}")
        return None

    os_name = os_mapping[system]
    arch_name = arch_mapping[machine]

    # Pattern: arch-os (e.g., x64-glibc, arm64-osx, x64-win)
    return f"{arch_name}-{os_name}"


def get_shell_config_file():
    """Determine the appropriate shell configuration file."""
    shell = os.environ.get('SHELL', '').lower()
    home = Path.home()

    # Check for common shell config files
    config_files = []

    if 'zsh' in shell:
        config_files = [home / '.zshrc', home / '.zprofile']
    elif 'bash' in shell:
        config_files = [home / '.bashrc', home / '.bash_profile', home / '.profile']
    else:
        # Default order of preference
        config_files = [
            home / '.zshrc',
            home / '.bashrc',
            home / '.bash_profile',
            home / '.profile'
        ]

    # Return the first existing file, or .bashrc as default
    for config_file in config_files:
        if config_file.exists():
            return config_file

    return home / '.bashrc'


def ensure_path_in_shell_config(install_dir):
    """Ensure install directory is in PATH in shell configuration."""
    config_file = get_shell_config_file()
    install_dir = Path(install_dir)

    # Path export line to add
    path_line = f'export PATH="$HOME/.local/bin:$PATH"  # Added by Z3 installer'

    # Check if install_dir is already in PATH environment
    path_env = os.environ.get('PATH', '')
    if str(install_dir) in path_env:
        print(f"~/.local/bin is already in PATH")
        return config_file

    # Check if path is already configured in shell config
    if config_file.exists():
        content = config_file.read_text()
        if '.local/bin' in content:
            print(f"~/.local/bin is configured in {config_file} but not in current session")
            print(f"   Run: source {config_file}")
            return config_file

    # Add path to config file
    print(f"Adding ~/.local/bin to PATH in {config_file}")

    with open(config_file, 'a') as f:
        f.write(f'\n# Local binaries\n{path_line}\n')

    print(f"PATH updated in {config_file}")
    print(f"   Run: source {config_file}")
    return config_file


def fetch_releases_from_repos(num_releases):
    """Fetch releases from all GitHub repos and combine them."""
    combined_releases = []

    for repo in GITHUB_REPOS:
        print(f"  - {repo}")
        try:
            url = f"https://api.github.com/repos/{repo}/releases?per_page={num_releases}"
            response = requests.get(url)
            response.raise_for_status()
            releases = response.json()

            # Add repo field to each release
            for release in releases:
                release['repo'] = repo
            combined_releases.extend(releases)
        except requests.exceptions.RequestException as e:
            print(f"    Warning: Failed to fetch from {repo}: {e}")

    return combined_releases


def deduplicate_releases(releases, num_releases):
    """
    Deduplicate releases by tag_name, keeping the earliest published date.
    Sort by date descending.
    """
    # Group by tag_name
    groups = {}
    for release in releases:
        tag = release['tag_name']
        if tag not in groups:
            groups[tag] = []
        groups[tag].append(release)

    # For each group, take the one with earliest published_at
    deduplicated = []
    for tag, group in groups.items():
        group.sort(key=lambda r: r['published_at'])
        deduplicated.append(group[0])

    # Sort by date descending
    deduplicated.sort(key=lambda r: r['published_at'], reverse=True)

    return deduplicated[:num_releases]


def display_releases(releases):
    """Display available releases in a formatted table."""
    print("")
    print("Available Z3 releases:")
    print("─" * 50)
    print(f"{'#':<4} {'Version':<20} {'Date':<12} Status")
    print("─" * 50)

    for i, release in enumerate(releases, 1):
        tag = release['tag_name']
        date = release['published_at'].split('T')[0]
        status = "(pre-release)" if release['prerelease'] else ""
        print(f"{i}){'':<3} {tag:<20} {date:<12} {status}")


def select_release_interactive(releases):
    """Let user select a release interactively."""
    print("")
    try:
        selection = input(f"Select a release (1-{len(releases)}) [1]: ").strip()
        selection = int(selection) if selection else 1
    except ValueError:
        print("Error: Invalid selection")
        sys.exit(1)

    if selection < 1 or selection > len(releases):
        print("Error: Invalid selection")
        sys.exit(1)

    return releases[selection - 1]


def find_asset_for_platform(release, platform_pattern, all_releases):
    """
    Find matching asset for the platform.
    For z3-4.12.5 on arm64-glibc, prefer Beneficial-AI-Foundation version.
    Otherwise prefer Z3Prover.
    """
    tag = release['tag_name']

    # Get all releases with this tag
    matching_releases = [r for r in all_releases if r['tag_name'] == tag]

    # Sort by preference
    if tag == "z3-4.12.5" and platform_pattern == "arm64-glibc":
        # Prefer Beneficial-AI-Foundation for this specific case
        matching_releases.sort(key=lambda r: 0 if r['repo'] == "Beneficial-AI-Foundation/z3" else 1)
    else:
        # Normally prefer Z3Prover
        matching_releases.sort(key=lambda r: 0 if r['repo'] == "Z3Prover/z3" else 1)

    # Try each source in order
    for rel in matching_releases:
        for asset in rel.get('assets', []):
            asset_name = asset['name'].lower()
            if platform_pattern.lower() in asset_name and asset_name.endswith('.zip'):
                print(f"Found matching asset in: {rel['repo']}")
                return asset

    return None


def download_file(url, filename, progress_callback=None):
    """Download a file with progress indication."""
    response = requests.get(url, stream=True)
    response.raise_for_status()

    total_size = int(response.headers.get('content-length', 0))
    downloaded = 0

    with open(filename, 'wb') as f:
        for chunk in response.iter_content(chunk_size=8192):
            if chunk:
                f.write(chunk)
                downloaded += len(chunk)
                if progress_callback and total_size > 0:
                    progress_callback(downloaded, total_size)

    return filename


def progress_bar(downloaded, total):
    """Simple progress bar for download."""
    if total == 0:
        return

    percent = (downloaded / total) * 100
    bar_length = 50
    filled_length = int(bar_length * downloaded // total)
    bar = '█' * filled_length + '-' * (bar_length - filled_length)

    print(f'\rDownloading: |{bar}| {percent:.1f}% ({downloaded}/{total} bytes)', end='')
    if downloaded == total:
        print()  # New line when complete


def make_executable(file_path):
    """Make a file executable on Unix-like systems."""
    if platform.system() != 'Windows':
        current_permissions = os.stat(file_path).st_mode
        os.chmod(file_path, current_permissions | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def verify_installation(binary_path):
    """Verify that Z3 is working correctly."""
    print("Verifying Z3 installation...")

    try:
        result = subprocess.run([str(binary_path), '--version'],
                              capture_output=True, text=True, timeout=30)

        if result.returncode == 0:
            print(f"Z3 is working!")
            print(result.stdout.strip())
            return True
        else:
            print(f"Z3 binary exists but returned error:")
            print(result.stderr.strip() if result.stderr else "Unknown error")
            return False
    except subprocess.TimeoutExpired:
        print("Z3 version check timed out")
        return False
    except Exception as e:
        print(f"Could not verify installation: {e}")
        return False


def main():
    parser = argparse.ArgumentParser(description='Download and install Z3 SMT solver from GitHub releases')

    parser.add_argument('-n', '--num-releases', type=int, default=DEFAULT_NUM_RELEASES,
                       help=f'Number of releases to show (default: {DEFAULT_NUM_RELEASES})')
    parser.add_argument('-l', '--list', action='store_true',
                       help='List releases without installing')
    parser.add_argument('--install-dir', '-i', default=None,
                       help='Installation directory (default: ~/.local/bin)')
    parser.add_argument('--platform',
                       help='Platform pattern to search for (e.g., x64-glibc)')
    parser.add_argument('--no-path', action='store_true',
                       help='Do not modify PATH configuration')

    args = parser.parse_args()

    # Set install directory
    if args.install_dir:
        install_dir = Path(args.install_dir)
    else:
        install_dir = Path.home() / '.local' / 'bin'

    try:
        # Fetch releases from all repos
        print("Fetching Z3 releases from GitHub...")
        all_releases = fetch_releases_from_repos(args.num_releases)

        if not all_releases:
            print("Error: No releases found")
            sys.exit(1)

        # Deduplicate and sort
        deduplicated = deduplicate_releases(all_releases, args.num_releases)

        # Display releases
        display_releases(deduplicated)

        if args.list:
            return

        # Select release
        selected = select_release_interactive(deduplicated)
        print("")
        print(f"Selected: {selected['tag_name']}")

        # Determine platform pattern
        platform_pattern = args.platform or get_platform_pattern()
        if not platform_pattern:
            print("Error: Could not determine platform")
            sys.exit(1)

        print(f"Looking for platform: {platform_pattern}")

        # Find matching asset
        asset = find_asset_for_platform(selected, platform_pattern, all_releases)

        if not asset:
            print(f"Error: No matching asset found for platform {platform_pattern}")
            print("")
            print("Available assets:")
            # Find all releases with this tag
            for rel in all_releases:
                if rel['tag_name'] == selected['tag_name']:
                    print(f"[{rel['repo']}]")
                    for a in rel.get('assets', []):
                        if a['name'].endswith('.zip'):
                            print(f"  - {a['name']}")
            sys.exit(1)

        asset_name = asset['name']
        asset_size = asset['size']
        download_url = asset['browser_download_url']
        size_mb = asset_size // (1024 * 1024)

        print(f"Found: {asset_name} ({size_mb} MB)")

        # Create temp directory
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_dir = Path(temp_dir)
            download_path = temp_dir / asset_name

            # Download
            print("")
            print("Downloading...")
            download_file(download_url, download_path, progress_bar)
            print("Download completed")

            # Extract
            print("Extracting...")
            with zipfile.ZipFile(download_path, 'r') as zip_ref:
                zip_ref.extractall(temp_dir)

            # Find z3 binary
            z3_binary = None
            for path in temp_dir.rglob('z3'):
                if path.is_file():
                    z3_binary = path
                    break

            if not z3_binary:
                # Try z3.exe for Windows
                for path in temp_dir.rglob('z3.exe'):
                    if path.is_file():
                        z3_binary = path
                        break

            if not z3_binary:
                print("Error: Could not find z3 binary in archive")
                print("Contents:")
                for p in list(temp_dir.rglob('*'))[:20]:
                    print(f"  {p}")
                sys.exit(1)

            print(f"Found binary: {z3_binary}")

            # Create install directory
            install_dir.mkdir(parents=True, exist_ok=True)

            # Copy binary
            installed_binary = install_dir / 'z3'
            if platform.system() == 'Windows':
                installed_binary = install_dir / 'z3.exe'

            # Remove existing binary if present
            if installed_binary.exists():
                installed_binary.unlink()

            shutil.copy2(z3_binary, installed_binary)
            make_executable(installed_binary)

            print(f"Z3 installed to: {installed_binary}")

        # Verify installation
        print("")
        if verify_installation(installed_binary):
            print("Installation verified successfully!")

        # Set up PATH
        if not args.no_path:
            print("")
            ensure_path_in_shell_config(install_dir)

        print("")
        print("Installation complete!")

    except requests.exceptions.RequestException as e:
        print(f"Network error: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
