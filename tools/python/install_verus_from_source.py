#!/usr/bin/env python3
# /// script
# dependencies = ["requests"]
# ///
"""
Verus Builder from Release

Builds Verus from source for a specific release/tag.
Useful for platforms without pre-built binaries (e.g., ARM64 Linux).

Prerequisites:
  - Rust toolchain (rustup): https://rustup.rs
  - Z3 theorem prover (4.12.5+) installed and in PATH, or VERUS_Z3_PATH set
  - git
"""

import requests
import os
import sys
import platform
import shutil
import tempfile
import subprocess
from pathlib import Path
import argparse
import re


GITHUB_REPO = "verus-lang/verus"
DEFAULT_NUM_RELEASES = 20
CARGO_BIN_DIR = Path.home() / '.cargo' / 'bin'


def get_shell_config_file():
    """Determine the appropriate shell configuration file."""
    shell = os.environ.get('SHELL', '').lower()
    home = Path.home()

    if 'zsh' in shell:
        config_files = [home / '.zshrc', home / '.zprofile']
    elif 'bash' in shell:
        config_files = [home / '.bashrc', home / '.bash_profile', home / '.profile']
    else:
        config_files = [
            home / '.zshrc',
            home / '.bashrc',
            home / '.bash_profile',
            home / '.profile'
        ]

    for config_file in config_files:
        if config_file.exists():
            return config_file

    return home / '.bashrc'


def ensure_path_in_shell_config():
    """Ensure ~/.cargo/bin is in PATH in shell configuration."""
    config_file = get_shell_config_file()
    cargo_bin = str(CARGO_BIN_DIR)

    # Check if already in PATH
    path_env = os.environ.get('PATH', '')
    if cargo_bin in path_env:
        print(f".cargo/bin is already in PATH")
        return config_file

    # Check if configured in shell config
    if config_file.exists():
        content = config_file.read_text()
        if '.cargo/bin' in content:
            print(f".cargo/bin is configured in {config_file} (will be available after restart)")
            return config_file

    # Add to config file
    print(f"Adding .cargo/bin to PATH in {config_file}")
    with open(config_file, 'a') as f:
        f.write('\n# Cargo bin directory\n')
        f.write('export PATH="$HOME/.cargo/bin:$PATH"\n')

    return config_file


def check_prerequisites():
    """Check that required tools are available."""
    missing = []

    for cmd in ['git', 'rustup', 'cargo']:
        try:
            subprocess.run([cmd, '--version'], capture_output=True, check=True)
        except (subprocess.CalledProcessError, FileNotFoundError):
            missing.append(cmd)

    if missing:
        print(f"Error: Missing required tools: {', '.join(missing)}")
        if 'rustup' in missing or 'cargo' in missing:
            print("Install Rust toolchain from: https://rustup.rs")
        sys.exit(1)

    # Check for Z3
    z3_path = os.environ.get('VERUS_Z3_PATH')
    if not z3_path:
        try:
            result = subprocess.run(['which', 'z3'], capture_output=True, text=True)
            if result.returncode == 0:
                z3_path = result.stdout.strip()
                os.environ['VERUS_Z3_PATH'] = z3_path
                print(f"Found Z3 at: {z3_path}")
        except Exception:
            pass

    if not z3_path:
        print("Warning: Z3 not found in PATH and VERUS_Z3_PATH not set")
        print("Verus requires Z3. Either:")
        print("  - Install Z3 and ensure it's in PATH")
        print("  - Set VERUS_Z3_PATH to point to the z3 binary")
        print("")
        response = input("Continue anyway? (y/N): ").strip().lower()
        if response != 'y':
            sys.exit(1)
    else:
        if not os.path.isfile(z3_path) or not os.access(z3_path, os.X_OK):
            print(f"Error: VERUS_Z3_PATH is set but file not found or not executable: {z3_path}")
            sys.exit(1)
        print(f"Using Z3 from VERUS_Z3_PATH: {z3_path}")


def fetch_releases(num_releases):
    """Fetch releases from GitHub."""
    url = f"https://api.github.com/repos/{GITHUB_REPO}/releases?per_page={num_releases}"
    response = requests.get(url)
    response.raise_for_status()
    return response.json()


def display_releases(releases):
    """Display available releases in a formatted table."""
    print("")
    print("Available Verus releases:")
    print("─" * 55)
    print(f"{'#':<4} {'Version':<30} {'Date':<12} Status")
    print("─" * 55)

    for i, release in enumerate(releases, 1):
        tag = release['tag_name']
        date = release['published_at'].split('T')[0]
        status = "(pre-release)" if release['prerelease'] else ""
        print(f"{i}){'':<3} {tag:<30} {date:<12} {status}")


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


def get_release_by_version(releases, version):
    """Get a specific release by version string."""
    # Try exact match first
    for release in releases:
        if release['tag_name'] == version:
            return release

    # Try partial match
    for release in releases:
        if version in release['tag_name']:
            return release

    print(f"Error: Version '{version}' not found")
    print("Available versions:")
    for release in releases[:10]:
        print(f"  - {release['tag_name']}")
    sys.exit(1)


def get_latest_prerelease(releases):
    """Get the latest pre-release version."""
    for release in releases:
        if release['prerelease']:
            return release

    print("Error: No pre-release versions found")
    sys.exit(1)


def extract_version_number(tag_name):
    """
    Extract version number from tag name.
    e.g., "release/0.2026.01.14.88f7396" -> "0.2026.01.14"
    """
    match = re.search(r'(\d+\.\d+\.\d+\.\d+)', tag_name)
    if match:
        return match.group(1)

    # Fallback: replace slashes with dashes
    return tag_name.replace('/', '-')


def run_command(cmd, cwd=None, env=None):
    """Run a command and print output in real-time."""
    print(f"Running: {' '.join(cmd)}")
    process = subprocess.Popen(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True
    )

    for line in process.stdout:
        print(line, end='')

    process.wait()
    return process.returncode


def build_verus(source_dir, jobs=None):
    """Build Verus using vargo."""
    source_path = source_dir / 'source'

    # Check for rust-toolchain.toml and install toolchain
    rust_toolchain = source_dir / 'rust-toolchain.toml'
    if not rust_toolchain.exists():
        rust_toolchain = source_path / 'rust-toolchain.toml'

    if rust_toolchain.exists():
        print("Installing Rust toolchain from rust-toolchain.toml...")
        subprocess.run(['rustup', 'show', 'active-toolchain'], cwd=source_path)

    # Build vargo if needed
    vargo_path = source_dir / 'tools' / 'vargo' / 'target' / 'release' / 'vargo'
    if not vargo_path.exists():
        print("Building vargo first...")
        vargo_src = source_dir / 'tools' / 'vargo'
        if vargo_src.exists():
            returncode = run_command(['cargo', 'build', '--release'], cwd=vargo_src)
            if returncode != 0:
                print("Error: Failed to build vargo")
                sys.exit(1)
        else:
            print("Error: Cannot find vargo source directory")
            sys.exit(1)

    # Build Verus with vargo
    vargo_args = ['build', '--release']
    if jobs:
        vargo_args.extend(['-j', str(jobs)])

    # Set up environment
    env = os.environ.copy()

    # Source activate script behavior - add tools to path
    tools_bin = source_dir / 'tools' / 'vargo' / 'target' / 'release'
    env['PATH'] = f"{tools_bin}:{env.get('PATH', '')}"

    print(f"Running: vargo {' '.join(vargo_args)}")
    returncode = run_command([str(vargo_path)] + vargo_args, cwd=source_path, env=env)

    if returncode != 0:
        print("Error: Verus build failed")
        sys.exit(1)

    print("Build completed successfully!")


def find_target_dir(source_dir):
    """Find the build output directory."""
    source_path = source_dir / 'source'

    candidates = [
        source_path / 'target-verus' / 'release',
        source_path / 'target' / 'release',
        source_dir / 'target-verus' / 'release',
        source_dir / 'target' / 'release',
    ]

    for candidate in candidates:
        if candidate.exists():
            return candidate

    print("Error: Could not find build output directory")
    print("Searched for: target-verus/release, target/release")
    sys.exit(1)


def install_verus(target_dir, install_dir):
    """Install Verus to the specified directory."""
    # Remove existing installation
    if install_dir.exists():
        print(f"Removing existing installation at {install_dir}")
        shutil.rmtree(install_dir)

    # Copy entire release directory
    print(f"Copying release directory to {install_dir}...")
    shutil.copytree(target_dir, install_dir)

    # Create symlinks
    CARGO_BIN_DIR.mkdir(parents=True, exist_ok=True)

    # Symlink for verus
    verus_link = CARGO_BIN_DIR / 'verus'
    verus_binary = install_dir / 'verus'

    if verus_link.exists() or verus_link.is_symlink():
        verus_link.unlink()

    if verus_binary.exists():
        verus_link.symlink_to(verus_binary)
        print(f"  Created symlink: {verus_link} -> {verus_binary}")

    # Symlink for cargo-verus
    cargo_verus_link = CARGO_BIN_DIR / 'cargo-verus'
    cargo_verus_binary = install_dir / 'cargo-verus'

    if cargo_verus_link.exists() or cargo_verus_link.is_symlink():
        cargo_verus_link.unlink()

    if cargo_verus_binary.exists():
        cargo_verus_link.symlink_to(cargo_verus_binary)
        print(f"  Created symlink: {cargo_verus_link} -> {cargo_verus_binary}")

    return verus_binary


def verify_installation(binary_path):
    """Verify that Verus is working correctly."""
    print("Verifying Verus installation...")

    if not binary_path.exists():
        # Try rust_verify as fallback
        binary_path = binary_path.parent / 'rust_verify'

    if not binary_path.exists():
        print("Warning: Could not find verus binary for verification")
        return False

    try:
        result = subprocess.run([str(binary_path), '--version'],
                              capture_output=True, text=True, timeout=30)

        if result.returncode == 0:
            print("Verus is working!")
            print(result.stdout.strip() if result.stdout.strip() else result.stderr.strip())
            return True
        else:
            print("Verus binary exists but version check failed")
            print("This may be normal - try running it manually")
            return False
    except subprocess.TimeoutExpired:
        print("Verus version check timed out")
        return False
    except Exception as e:
        print(f"Could not verify installation: {e}")
        return False


def main():
    parser = argparse.ArgumentParser(
        description='Build and install Verus from source',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Prerequisites:
  - Rust toolchain (rustup): https://rustup.rs
  - Z3 theorem prover installed (set VERUS_Z3_PATH if not in PATH)
  - git

Examples:
  %(prog)s                              # Build latest stable
  %(prog)s --version v0.2025.08.25      # Build specific version
  %(prog)s --pre-release                # Build latest pre-release
  %(prog)s --install-dir /opt/verus     # Custom install location
""")

    parser.add_argument('-v', '--version',
                       help='Build a specific version/tag (e.g., "v0.2025.08.25")')
    parser.add_argument('-p', '--pre-release', '--prerelease', action='store_true',
                       help='Build the latest pre-release version')
    parser.add_argument('-i', '--install-dir',
                       help='Installation directory (default: ~/.cargo/bin/verus-<version>)')
    parser.add_argument('-b', '--build-dir',
                       help='Build directory (default: temporary directory)')
    parser.add_argument('-k', '--keep-build', action='store_true',
                       help='Keep the build directory after installation')
    parser.add_argument('-j', '--jobs', type=int,
                       help='Number of parallel jobs for cargo (default: auto)')
    parser.add_argument('-l', '--list-releases', action='store_true',
                       help='List available releases and exit')

    args = parser.parse_args()

    # Validate arguments
    if args.version and args.pre_release:
        print("Error: Cannot specify both --version and --pre-release")
        sys.exit(1)

    try:
        # List releases if requested
        if args.list_releases:
            print("Fetching available releases...")
            releases = fetch_releases(DEFAULT_NUM_RELEASES)
            display_releases(releases)
            return

        # Check prerequisites
        print("Checking prerequisites...")
        check_prerequisites()

        # Fetch releases
        print("")
        print("Fetching Verus releases from GitHub...")
        releases = fetch_releases(DEFAULT_NUM_RELEASES)

        if not releases:
            print("Error: No releases found")
            sys.exit(1)

        # Select release
        if args.version:
            print(f"Fetching release info for version: {args.version}...")
            release = get_release_by_version(releases, args.version)
        elif args.pre_release:
            print("Fetching latest pre-release...")
            release = get_latest_prerelease(releases)
        else:
            # Interactive selection
            display_releases(releases)
            release = select_release_interactive(releases)

        tag_name = release['tag_name']
        published = release['published_at']
        version_number = extract_version_number(tag_name)

        # Set install directory
        if args.install_dir:
            install_dir = Path(args.install_dir)
        else:
            install_dir = CARGO_BIN_DIR / f'verus-{version_number}'

        print("")
        print("=" * 60)
        print("VERUS BUILDER FROM RELEASE")
        print("=" * 60)
        print(f"Version: {tag_name}")
        print(f"Published: {published}")
        print(f"Install directory: {install_dir}")

        # Create or use build directory
        temp_build_dir = None
        if args.build_dir:
            build_dir = Path(args.build_dir)
            build_dir.mkdir(parents=True, exist_ok=True)
            print(f"Build directory: {build_dir}")
        else:
            temp_build_dir = tempfile.mkdtemp(prefix='verus_build_')
            build_dir = Path(temp_build_dir)
            print(f"Build directory: {build_dir} (temporary)")

        try:
            # Clone source
            print("")
            print("Step 1: Cloning source code...")
            verus_src = build_dir / 'verus'

            clone_cmd = [
                'git', 'clone', '--depth', '1', '--branch', tag_name,
                f'https://github.com/{GITHUB_REPO}.git', str(verus_src)
            ]
            returncode = run_command(clone_cmd)

            if returncode != 0:
                print("Error: Failed to clone repository")
                sys.exit(1)

            print(f"Source directory: {verus_src}")

            # Set up Rust toolchain
            print("")
            print("Step 2: Setting up Rust toolchain...")
            source_path = verus_src / 'source'
            subprocess.run(['rustup', 'show', 'active-toolchain'], cwd=source_path)

            # Build
            print("")
            print("Step 3: Building Verus (this may take a while)...")
            build_verus(verus_src, args.jobs)

            # Install
            print("")
            print(f"Step 4: Installing to {install_dir}...")
            target_dir = find_target_dir(verus_src)
            print(f"Found build output: {target_dir}")

            verus_binary = install_verus(target_dir, install_dir)
            print(f"Verus installed to: {install_dir}")

            # Configure PATH
            print("")
            print("Step 5: Configuring PATH...")
            config_file = ensure_path_in_shell_config()

            # Verify
            print("")
            print("Step 6: Verifying installation...")
            verify_installation(verus_binary)

        finally:
            # Cleanup
            print("")
            if not args.keep_build and temp_build_dir:
                print("Cleaning up build directory...")
                shutil.rmtree(temp_build_dir, ignore_errors=True)
                print("Build directory removed")
            elif args.build_dir or args.keep_build:
                print(f"Build directory kept at: {build_dir}")

        # Summary
        print("")
        print("=" * 60)
        print("BUILD COMPLETE")
        print("=" * 60)
        print(f"Version: {tag_name}")
        print(f"Installed to: {install_dir}")
        print("")
        print("Next steps:")
        print(f"   1. Restart your terminal or run: source {config_file}")
        print("   2. Ensure VERUS_Z3_PATH is set if Z3 is not in PATH")
        print("   3. Run 'verus --version' to verify")

    except requests.exceptions.RequestException as e:
        print(f"Network error: {e}")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nBuild cancelled")
        sys.exit(1)
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
