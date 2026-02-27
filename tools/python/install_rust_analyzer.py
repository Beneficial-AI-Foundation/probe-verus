#!/usr/bin/env python3
# /// script
# dependencies = ["requests"]
# ///
"""
Rust Analyzer Latest Release Downloader

Downloads the latest release of Rust Analyzer from GitHub releases.
Supports latest stable release, latest pre-release, or most recent release.
"""

import requests
import os
import sys
import json
import platform
import gzip
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
    
    patterns = {
        'linux': {
            'x86_64': 'x86_64-unknown-linux-gnu',
            'amd64': 'x86_64-unknown-linux-gnu',
            'aarch64': 'aarch64-unknown-linux-gnu',
            'arm64': 'aarch64-unknown-linux-gnu',
            'armv7l': 'arm-unknown-linux-gnueabihf',
            'arm': 'arm-unknown-linux-gnueabihf',
        },
        'darwin': {
            'x86_64': 'x86_64-apple-darwin',
            'amd64': 'x86_64-apple-darwin',
            'arm64': 'aarch64-apple-darwin',
            'aarch64': 'aarch64-apple-darwin',
        },
        'windows': {
            'x86_64': 'x86_64-pc-windows-msvc',
            'amd64': 'x86_64-pc-windows-msvc',
            'aarch64': 'aarch64-pc-windows-msvc',
            'arm64': 'aarch64-pc-windows-msvc',
            'i686': 'i686-pc-windows-msvc',
            'i386': 'i686-pc-windows-msvc',
        }
    }
    
    if system in patterns and machine in patterns[system]:
        return patterns[system][machine]
    
    print(f"Warning: Unknown platform {system}-{machine}")
    return None


def get_latest_release(include_prerelease=False):
    """Fetch the latest release information from GitHub API.
    
    Args:
        include_prerelease: If True, fetch the latest pre-release version.
                           If False, fetch the latest stable release.
    """
    if include_prerelease:
        # Get all releases and find the most recent pre-release
        url = "https://api.github.com/repos/rust-lang/rust-analyzer/releases"
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
        url = "https://api.github.com/repos/rust-lang/rust-analyzer/releases/latest"
        response = requests.get(url)
        response.raise_for_status()
        return response.json()


def find_asset_for_platform(assets, platform_pattern, is_vsix=False):
    """Find the appropriate asset for the current platform."""
    if not platform_pattern and not is_vsix:
        return None
    
    if is_vsix:
        # Look for VS Code extension
        for asset in assets:
            asset_name = asset['name'].lower()
            if asset_name.endswith('.vsix'):
                return asset
        return None
    
    # Look for binary asset (not vsix extension)
    for asset in assets:
        asset_name = asset['name'].lower()
        if (platform_pattern in asset_name and 
            asset_name.endswith('.gz') and 
            'rust-analyzer-' in asset_name and
            not asset_name.endswith('.vsix')):
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


def extract_gzip(gzip_path, extract_to):
    """Extract a gzipped file."""
    gzip_path = Path(gzip_path)
    extract_to = Path(extract_to)
    
    print(f"Extracting {gzip_path.name}...")
    
    # The extracted filename should be the original name without .gz
    if gzip_path.name.endswith('.gz'):
        extracted_name = gzip_path.name[:-3]  # Remove .gz extension
    else:
        extracted_name = gzip_path.name + '_extracted'
    
    extracted_path = extract_to / extracted_name
    
    # Create extract directory if it doesn't exist
    extract_to.mkdir(parents=True, exist_ok=True)
    
    with gzip.open(gzip_path, 'rb') as gz_file:
        with open(extracted_path, 'wb') as out_file:
            shutil.copyfileobj(gz_file, out_file)
    
    print(f"Extracted to: {extracted_path}")
    return extracted_path


def make_executable(file_path):
    """Make a file executable on Unix-like systems."""
    if platform.system() != 'Windows':
        current_permissions = os.stat(file_path).st_mode
        os.chmod(file_path, current_permissions | stat.S_IEXEC)


def setup_rust_analyzer_installation(binary_path, install_dir=None):
    """Set up Rust Analyzer installation in a clean directory."""
    binary_path = Path(binary_path)
    
    # Default install directory
    if install_dir is None:
        home_dir = Path.home()
        install_dir = home_dir / '.local' / 'bin'
    else:
        install_dir = Path(install_dir)
    
    print(f"Found rust-analyzer binary: {binary_path}")

    # Create install directory if it doesn't exist
    print(f"Installing Rust Analyzer to: {install_dir}")
    install_dir.mkdir(parents=True, exist_ok=True)

    # Copy binary to install directory with standard name
    installed_binary = install_dir / 'rust-analyzer'
    if platform.system() == 'Windows':
        installed_binary = install_dir / 'rust-analyzer.exe'

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
    path_line = 'export PATH="$HOME/.local/bin:$PATH"  # Added by Rust Analyzer installer'

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
    """Create a batch script for Windows to add Rust Analyzer to PATH."""
    install_dir = Path(install_dir)
    batch_file = install_dir.parent / 'add_rust_analyzer_to_path.bat'
    
    batch_content = f'''@echo off
echo Adding Rust Analyzer to PATH...
setx PATH "%PATH%;{install_dir}"
echo Rust Analyzer has been added to your PATH.
echo Please restart your command prompt or PowerShell for changes to take effect.
pause
'''
    
    with open(batch_file, 'w') as f:
        f.write(batch_content)
    
    print(f"Created Windows batch script: {batch_file}")
    print("Run this script as Administrator to add Rust Analyzer to your system PATH")
    return batch_file


def setup_path_configuration(install_dir):
    """Set up PATH configuration for the current platform."""
    if platform.system() == 'Windows':
        return create_windows_batch_script(install_dir)
    else:
        return ensure_path_in_shell_config()


def verify_installation(binary_path):
    """Verify that Rust Analyzer is working correctly."""
    print("Verifying Rust Analyzer installation...")
    
    try:
        result = subprocess.run([str(binary_path), '--version'], 
                              capture_output=True, text=True, timeout=30)
        
        if result.returncode == 0:
            print(f"✓ Rust Analyzer is working! Version info:")
            print(result.stdout.strip())
            return True
        else:
            print(f"⚠ Rust Analyzer binary exists but returned error:")
            print(result.stderr.strip() if result.stderr else "Unknown error")
            return False
    except subprocess.TimeoutExpired:
        print("⚠ Rust Analyzer version check timed out")
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
    parser = argparse.ArgumentParser(description='Download and install the latest Rust Analyzer release')
    
    parser.add_argument('--pre-release', '--prerelease', action='store_true', 
                       help='Download the latest pre-release version instead of stable')
    parser.add_argument('--output-dir', '-o', default='.', 
                       help='Download directory (default: current directory)')
    parser.add_argument('--install-dir', '-i',
                       help='Installation directory (default: ~/.local/bin)')
    parser.add_argument('--platform', 
                       help='Platform pattern to search for (e.g., x86_64-unknown-linux-gnu)')
    parser.add_argument('--list-assets', action='store_true',
                       help='List all available assets without downloading')
    parser.add_argument('--no-extract', action='store_true',
                       help='Download only, do not extract or install')
    parser.add_argument('--no-path', action='store_true',
                       help='Do not modify PATH configuration')
    parser.add_argument('--vsix', action='store_true',
                       help='Download VS Code extension (.vsix) instead of binary')
    
    args = parser.parse_args()
    
    try:
        # Determine release type based on arguments
        if args.pre_release:
            print("Fetching latest Rust Analyzer pre-release...")
        else:
            print("Fetching latest stable Rust Analyzer release...")
            
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
                asset_type = "VS Code Extension" if asset['name'].endswith('.vsix') else "Binary"
                print(f"  - {asset['name']} ({size_mb:.1f} MB) - {asset_type}")
            return
        
        if not assets:
            print("No assets found in this release")
            return
        
        # Handle VS Code extension download
        if args.vsix:
            vsix_asset = find_asset_for_platform(assets, None, is_vsix=True)
            
            if not vsix_asset:
                print(f"No VS Code extension found")
                print("Available VS Code extensions:")
                for a in assets:
                    if a['name'].endswith('.vsix'):
                        print(f"  - {a['name']}")
                return
            
            asset = vsix_asset
        else:
            # Handle binary download
            # Determine platform
            platform_pattern = args.platform or get_platform_asset_pattern()
            
            if platform_pattern:
                asset = find_asset_for_platform(assets, platform_pattern)
                if not asset:
                    print(f"No binary asset found for platform pattern: {platform_pattern}")
                    print("Available binary assets:")
                    for a in assets:
                        if (a['name'].endswith('.gz') and 
                            'rust-analyzer-' in a['name'].lower() and 
                            not a['name'].endswith('.vsix')):
                            print(f"  - {a['name']}")
                    return
            else:
                binary_assets = [a for a in assets if (a['name'].endswith('.gz') and 
                                                     'rust-analyzer-' in a['name'].lower() and 
                                                     not a['name'].endswith('.vsix'))]
                if len(binary_assets) == 1:
                    asset = binary_assets[0]
                else:
                    print("Multiple binary assets available, please specify platform:")
                    for i, a in enumerate(binary_assets):
                        print(f"  {i}: {a['name']}")
                    
                    choice = input("Enter asset number: ")
                    try:
                        asset = binary_assets[int(choice)]
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
        
        # Handle VS Code extension
        if args.vsix:
            print(f"\n📦 VS Code extension downloaded: {filename}")
            print(f"To install the extension in VS Code, run:")
            print(f"   code --install-extension {filename}")
            return
        
        # Extract and install binary if not disabled
        if not args.no_extract:
            if filename.name.endswith('.gz'):
                try:
                    # Extract to temporary directory
                    temp_extract_dir = output_dir / 'temp_extract'
                    temp_extract_dir.mkdir(exist_ok=True)
                    
                    extracted_binary = extract_gzip(filename, temp_extract_dir)
                    
                    # Set up installation
                    install_dir, installed_binary = setup_rust_analyzer_installation(
                        extracted_binary, args.install_dir
                    )
                    
                    print(f"✓ Rust Analyzer installed to: {install_dir}")
                    print(f"✓ Rust Analyzer binary: {installed_binary}")
                    
                    # Verify installation
                    if verify_installation(installed_binary):
                        print("✓ Installation verified successfully!")
                    
                    # Set up PATH
                    if not args.no_path:
                        config_file = setup_path_configuration(install_dir)
                        
                        if platform.system() == 'Windows':
                            print(f"\n📝 Next steps for Windows:")
                            print(f"   1. Run {config_file} as Administrator to add Rust Analyzer to PATH")
                            print(f"   2. Restart your command prompt/PowerShell")
                            print(f"   3. Type 'rust-analyzer --version' to verify")
                        else:
                            print(f"\n📝 Next steps:")
                            print(f"   1. Restart your terminal or run: source {config_file}")
                            print(f"   2. Type 'rust-analyzer --version' to verify")
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
                print(f"Downloaded file is not a gzipped archive: {filename}")
                print("Manual installation may be required.")
        else:
            print(f"\nTo manually extract and install:")
            print(f"  gunzip '{filename}'")
            print(f"  chmod +x '{filename.with_suffix('')}'")
        
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
