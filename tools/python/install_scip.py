#!/usr/bin/env python3
# /// script
# dependencies = ["requests"]
# ///
"""
SCIP (Source Code Intelligence Protocol) Latest Release Downloader

Downloads the latest release of SCIP from GitHub releases.
Supports latest stable release, latest pre-release, or most recent release.
"""

import requests
import os
import sys
import json
import platform
import tarfile
import shutil
import stat
from pathlib import Path
from urllib.parse import urlparse
import argparse
import subprocess


def get_platform_asset_pattern():
    """Determine the appropriate asset pattern for the current platform."""
    system = platform.system().lower()
    machine = platform.machine().lower()
    
    # Map Python's machine names to SCIP's expected architecture names
    arch_mapping = {
        'x86_64': 'amd64',
        'amd64': 'amd64',
        'aarch64': 'arm64',
        'arm64': 'arm64',
        'armv7l': 'arm',
        'arm': 'arm',
    }
    
    # Map Python's system names to SCIP's expected OS names
    os_mapping = {
        'linux': 'linux',
        'darwin': 'darwin',
        'windows': 'windows',
    }
    
    if system not in os_mapping:
        print(f"Warning: Unknown operating system {system}")
        return None
    
    if machine not in arch_mapping:
        print(f"Warning: Unknown architecture {machine}")
        return None
    
    os_name = os_mapping[system]
    arch_name = arch_mapping[machine]
    
    return f"scip-{os_name}-{arch_name}"


def get_latest_release(include_prerelease=False):
    """Fetch the latest release information from GitHub API.
    
    Args:
        include_prerelease: If True, fetch the latest pre-release version.
                           If False, fetch the latest stable release.
    """
    if include_prerelease:
        # Get all releases and find the most recent pre-release
        url = "https://api.github.com/repos/sourcegraph/scip/releases"
        response = requests.get(url)
        response.raise_for_status()
        releases = response.json()
        
        if not releases:
            raise Exception("No releases found")
        
        # Find the most recent pre-release
        for release in releases:
            if release['prerelease']:
                return release
        raise Exception("No pre-release versions found")
    else:
        # Get the latest stable release
        url = "https://api.github.com/repos/sourcegraph/scip/releases/latest"
        response = requests.get(url)
        response.raise_for_status()
        return response.json()


def find_asset_for_platform(assets, platform_pattern):
    """Find the appropriate asset for the current platform."""
    if not platform_pattern:
        return None
    
    # Look for tar.gz asset matching the platform pattern
    for asset in assets:
        asset_name = asset['name'].lower()
        if (platform_pattern.lower() in asset_name and 
            asset_name.endswith('.tar.gz')):
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


def extract_tar_gz(tar_path, extract_to, binary_name='scip'):
    """Extract a tar.gz file and find the binary."""
    tar_path = Path(tar_path)
    extract_to = Path(extract_to)
    
    print(f"Extracting {tar_path.name}...")
    
    # Create extract directory if it doesn't exist
    extract_to.mkdir(parents=True, exist_ok=True)
    
    with tarfile.open(tar_path, 'r:gz') as tar:
        # Extract all contents
        tar.extractall(extract_to)
        
        # Find the binary in extracted contents
        for member in tar.getmembers():
            if member.isfile() and member.name.endswith(binary_name):
                binary_path = extract_to / member.name
                print(f"Found binary: {binary_path}")
                return binary_path
            # Sometimes the binary might be just named 'scip' without path
            elif member.isfile() and Path(member.name).name == binary_name:
                binary_path = extract_to / member.name
                print(f"Found binary: {binary_path}")
                return binary_path
    
    # If not found by name, look for any executable file
    for file_path in extract_to.rglob('*'):
        if file_path.is_file() and os.access(file_path, os.X_OK):
            print(f"Found executable: {file_path}")
            return file_path
    
    raise Exception(f"Could not find {binary_name} binary in extracted archive")


def make_executable(file_path):
    """Make a file executable on Unix-like systems."""
    if platform.system() != 'Windows':
        current_permissions = os.stat(file_path).st_mode
        os.chmod(file_path, current_permissions | stat.S_IEXEC)


def setup_scip_installation(binary_path, install_dir=None):
    """Set up SCIP installation in a clean directory."""
    binary_path = Path(binary_path)

    # Default install directory
    if install_dir is None:
        home_dir = Path.home()
        install_dir = home_dir / '.local' / 'bin'
    else:
        install_dir = Path(install_dir)

    print(f"Found SCIP binary: {binary_path}")

    # Create install directory if it doesn't exist
    print(f"Installing SCIP to: {install_dir}")
    install_dir.mkdir(parents=True, exist_ok=True)

    # Copy binary to install directory with standard name
    installed_binary = install_dir / 'scip'
    if platform.system() == 'Windows':
        installed_binary = install_dir / 'scip.exe'

    # Remove existing binary if present
    if installed_binary.exists():
        installed_binary.unlink()

    shutil.copy2(binary_path, installed_binary)
    
    # Make binary executable
    make_executable(installed_binary)
    
    return install_dir, installed_binary


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


def ensure_path_in_shell_config():
    """Ensure ~/.local/bin is in PATH in shell configuration."""
    config_file = get_shell_config_file()
    local_bin = Path.home() / '.local' / 'bin'

    # Path export line to add
    path_line = 'export PATH="$HOME/.local/bin:$PATH"  # Added by SCIP installer'

    # Check if ~/.local/bin is already in PATH environment
    path_env = os.environ.get('PATH', '')
    if str(local_bin) in path_env:
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


def create_windows_batch_script(install_dir):
    """Create a batch script for Windows to add SCIP to PATH."""
    install_dir = Path(install_dir)
    batch_file = install_dir.parent / 'add_scip_to_path.bat'
    
    batch_content = f'''@echo off
echo Adding SCIP to PATH...
setx PATH "%PATH%;{install_dir}"
echo SCIP has been added to your PATH.
echo Please restart your command prompt or PowerShell for changes to take effect.
pause
'''
    
    with open(batch_file, 'w') as f:
        f.write(batch_content)
    
    print(f"Created Windows batch script: {batch_file}")
    print("Run this script as Administrator to add SCIP to your system PATH")
    return batch_file


def setup_path_configuration(install_dir):
    """Set up PATH configuration for the current platform."""
    if platform.system() == 'Windows':
        return create_windows_batch_script(install_dir)
    else:
        return ensure_path_in_shell_config()


def verify_installation(binary_path):
    """Verify that SCIP is working correctly."""
    print("Verifying SCIP installation...")
    
    try:
        result = subprocess.run([str(binary_path), '--version'], 
                              capture_output=True, text=True, timeout=30)
        
        if result.returncode == 0:
            print(f"✓ SCIP is working! Version info:")
            print(result.stdout.strip())
            return True
        else:
            # Try without --version flag, some binaries use different flags
            result = subprocess.run([str(binary_path), '--help'], 
                                  capture_output=True, text=True, timeout=30)
            if result.returncode == 0:
                print(f"✓ SCIP is working! Help output:")
                print(result.stdout.strip()[:200] + "..." if len(result.stdout) > 200 else result.stdout.strip())
                return True
            else:
                print(f"⚠ SCIP binary exists but returned error:")
                print(result.stderr.strip() if result.stderr else "Unknown error")
                return False
    except subprocess.TimeoutExpired:
        print("⚠ SCIP version check timed out")
        return False
    except Exception as e:
        print(f"⚠ Could not verify installation: {e}")
        return False


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


def main():
    parser = argparse.ArgumentParser(description='Download and install the latest SCIP release')
    
    parser.add_argument('--pre-release', '--prerelease', action='store_true', 
                       help='Download the latest pre-release version instead of stable')
    parser.add_argument('--output-dir', '-o', default='.', 
                       help='Download directory (default: current directory)')
    parser.add_argument('--install-dir', '-i',
                       help='Installation directory (default: ~/.local/bin)')
    parser.add_argument('--platform', 
                       help='Platform pattern to search for (e.g., scip-linux-amd64)')
    parser.add_argument('--list-assets', action='store_true',
                       help='List all available assets without downloading')
    parser.add_argument('--no-extract', action='store_true',
                       help='Download only, do not extract or install')
    parser.add_argument('--no-path', action='store_true',
                       help='Do not modify PATH configuration')
    
    args = parser.parse_args()
    
    try:
        # Determine release type based on arguments
        if args.pre_release:
            print("Fetching latest SCIP pre-release...")
        else:
            print("Fetching latest stable SCIP release...")
            
        release = get_latest_release(include_prerelease=args.pre_release)
        
        print(f"Found release: {release['tag_name']}")
        print(f"Published: {release['published_at']}")
        print(f"Pre-release: {release['prerelease']}")
        print(f"Description: {release['name']}")
        
        if release['body']:
            print(f"Release notes:\n{release['body'][:200]}...")
        
        assets = release['assets']
        
        if args.list_assets:
            print(f"\nAvailable assets ({len(assets)}):")
            for asset in assets:
                size_mb = asset['size'] / (1024 * 1024)
                print(f"  - {asset['name']} ({size_mb:.1f} MB)")
            return
        
        if not assets:
            print("No assets found in this release")
            return
        
        # Determine platform
        platform_pattern = args.platform or get_platform_asset_pattern()
        
        if platform_pattern:
            asset = find_asset_for_platform(assets, platform_pattern)
            if not asset:
                print(f"No asset found for platform pattern: {platform_pattern}")
                print("Available assets:")
                for a in assets:
                    if a['name'].endswith('.tar.gz'):
                        print(f"  - {a['name']}")
                return
        else:
            tar_assets = [a for a in assets if a['name'].endswith('.tar.gz')]
            if len(tar_assets) == 1:
                asset = tar_assets[0]
            else:
                print("Multiple assets available, please specify platform:")
                for i, a in enumerate(tar_assets):
                    print(f"  {i}: {a['name']}")
                
                choice = input("Enter asset number: ")
                try:
                    asset = tar_assets[int(choice)]
                except (ValueError, IndexError):
                    print("Invalid choice")
                    return
        
        # Create output directory
        output_dir = Path(args.output_dir)
        output_dir.mkdir(parents=True, exist_ok=True)
        
        # Download the asset
        filename = output_dir / asset['name']
        download_url = asset['browser_download_url']
        
        print(f"Downloading {asset['name']} ({asset['size'] / (1024*1024):.1f} MB)...")
        print(f"URL: {download_url}")
        print(f"Saving to: {filename}")
        
        download_file(download_url, filename, progress_bar)
        
        print(f"\n✓ Download completed: {filename}")
        
        # Extract and install if not disabled
        if not args.no_extract:
            if filename.name.endswith('.tar.gz'):
                try:
                    # Extract to temporary directory
                    temp_extract_dir = output_dir / 'temp_extract'
                    temp_extract_dir.mkdir(exist_ok=True)
                    
                    extracted_binary = extract_tar_gz(filename, temp_extract_dir)
                    
                    # Set up installation
                    install_dir, installed_binary = setup_scip_installation(
                        extracted_binary, args.install_dir
                    )
                    
                    print(f"✓ SCIP installed to: {install_dir}")
                    print(f"✓ SCIP binary: {installed_binary}")
                    
                    # Verify installation
                    if verify_installation(installed_binary):
                        print("✓ Installation verified successfully!")
                    
                    # Set up PATH
                    if not args.no_path:
                        config_file = setup_path_configuration(install_dir)
                        
                        if platform.system() == 'Windows':
                            print(f"\n📝 Next steps for Windows:")
                            print(f"   1. Run {config_file} as Administrator to add SCIP to PATH")
                            print(f"   2. Restart your command prompt/PowerShell")
                            print(f"   3. Type 'scip --version' to verify")
                        else:
                            print(f"\n📝 Next steps:")
                            print(f"   1. Restart your terminal or run: source {config_file}")
                            print(f"   2. Type 'scip --version' to verify")
                            print(f"   Or run directly: {installed_binary} --version")
                    
                    # Clean up temp directory
                    shutil.rmtree(temp_extract_dir)
                    
                    # Optionally remove downloaded archive
                    remove_archive = input(f"\nRemove downloaded archive {filename}? (y/N): ").lower()
                    if remove_archive == 'y':
                        filename.unlink()
                        print("✓ Archive removed")
                    
                except Exception as e:
                    print(f"Error during extraction/installation: {e}")
                    print("You can manually extract and install the downloaded archive.")
                    sys.exit(1)
            else:
                print(f"Downloaded file is not a tar.gz archive: {filename}")
                print("Manual installation may be required.")
        else:
            print(f"\nTo manually extract and install:")
            print(f"  tar -xzf '{filename}'")
            print(f"  chmod +x scip")
            print(f"  mv scip /usr/local/bin/  # (or your preferred location)")
        
    except requests.exceptions.RequestException as e:
        print(f"Network error: {e}")
        sys.exit(1)
    except KeyError as e:
        print(f"Unexpected API response format: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
