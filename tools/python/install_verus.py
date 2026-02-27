#!/usr/bin/env python3
# /// script
# dependencies = ["requests"]
# ///
"""
Verus Latest Release Downloader

Downloads the latest release of Verus from GitHub releases.
Supports latest stable release, latest pre-release, or most recent release.
"""

import requests
import os
import sys
import json
import platform
import zipfile
import tarfile
import shutil
import stat
from pathlib import Path
from urllib.parse import urlparse
import argparse


def get_platform_asset_pattern():
    """Determine the appropriate asset pattern for the current platform."""
    system = platform.system().lower()
    machine = platform.machine().lower()
    
    patterns = {
        'linux': {
            'x86_64': 'x86-linux',
            'amd64': 'x86-linux',
        },
        'darwin': {
            'x86_64': 'x86-macos',
            'amd64': 'x86-macos',
            'arm64': 'arm64-macos',
        },
        'windows': {
            'x86_64': 'x86-win',
            'amd64': 'x86-win',
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
        url = "https://api.github.com/repos/verus-lang/verus/releases"
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
        url = "https://api.github.com/repos/verus-lang/verus/releases/latest"
        response = requests.get(url)
        response.raise_for_status()
        return response.json()


def get_specific_release(version):
    """Fetch a specific release by version from GitHub API.
    
    Args:
        version: The version string to search for (e.g., "0.2025.08.25.63ab0cb")
    """
    url = "https://api.github.com/repos/verus-lang/verus/releases"
    response = requests.get(url)
    response.raise_for_status()
    releases = response.json()
    
    if not releases:
        raise Exception("No releases found")
    
    # Search for releases that contain the version string
    matching_releases = []
    for release in releases:
        tag_name = release['tag_name']
        release_name = release.get('name', '')
        
        # Check if version appears in tag name or release name
        if (version in tag_name or 
            version in release_name or 
            tag_name.endswith(version) or
            release_name.endswith(version)):
            matching_releases.append(release)
    
    if not matching_releases:
        # Show available versions to help the user
        print(f"Version '{version}' not found. Available versions:")
        for release in releases[:10]:  # Show first 10 releases
            print(f"  - {release['tag_name']} ({release.get('name', 'No name')})")
        if len(releases) > 10:
            print(f"  ... and {len(releases) - 10} more releases")
        raise Exception(f"No release found matching version: {version}")
    
    if len(matching_releases) > 1:
        print(f"Multiple releases found matching '{version}':")
        for i, release in enumerate(matching_releases):
            print(f"  {i}: {release['tag_name']} - {release.get('name', 'No name')}")
        
        choice = input("Enter the number of the release to download: ")
        try:
            selected_index = int(choice)
            if 0 <= selected_index < len(matching_releases):
                return matching_releases[selected_index]
            else:
                raise ValueError("Invalid selection")
        except (ValueError, IndexError):
            raise Exception("Invalid choice. Please run the command again and select a valid option.")
    
    return matching_releases[0]


def find_asset_for_platform(assets, platform_pattern):
    """Find the appropriate asset for the current platform."""
    if not platform_pattern:
        return None
    
    for asset in assets:
        if platform_pattern in asset['name'].lower():
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


def extract_archive(archive_path, extract_to):
    """Extract zip or tar.gz archive."""
    archive_path = Path(archive_path)
    extract_to = Path(extract_to)
    
    print(f"Extracting {archive_path.name}...")
    
    if archive_path.suffix.lower() == '.zip':
        with zipfile.ZipFile(archive_path, 'r') as zip_ref:
            zip_ref.extractall(extract_to)
    elif archive_path.name.endswith(('.tar.gz', '.tgz')):
        with tarfile.open(archive_path, 'r:gz') as tar_ref:
            tar_ref.extractall(extract_to)
    else:
        raise Exception(f"Unsupported archive format: {archive_path.suffix}")
    
    print(f"Extracted to: {extract_to}")
    return extract_to


def find_verus_binary(extract_dir):
    """Find the verus binary in the extracted directory."""
    extract_dir = Path(extract_dir)
    
    # Common binary names
    binary_names = ['verus', 'verus.exe']
    
    # Search for verus binary
    for binary_name in binary_names:
        # Check root of extract directory
        binary_path = extract_dir / binary_name
        if binary_path.exists():
            return binary_path
        
        # Check subdirectories
        for root, dirs, files in os.walk(extract_dir):
            if binary_name in files:
                return Path(root) / binary_name
    
    return None


def make_executable(file_path):
    """Make a file executable on Unix-like systems."""
    if platform.system() != 'Windows':
        current_permissions = os.stat(file_path).st_mode
        os.chmod(file_path, current_permissions | stat.S_IEXEC)


def make_all_binaries_executable(install_dir):
    """Make all binary files in the installation directory executable."""
    if platform.system() == 'Windows':
        return  # Windows doesn't need execute permissions
    
    install_dir = Path(install_dir)
    
    # Common patterns for binary files
    binary_patterns = ['*', '*.exe']  # Include all files, but we'll filter below
    
    # Walk through all files in the installation directory
    for file_path in install_dir.rglob('*'):
        if file_path.is_file():
            # Check if it's likely a binary file
            if (file_path.suffix == '' or file_path.suffix == '.exe' or
                file_path.name.startswith('rust_') or
                file_path.name.startswith('verus') or
                'bin' in str(file_path.parent).lower()):
                
                # Try to determine if it's a binary by checking if it's executable-like
                try:
                    # Read first few bytes to check for executable markers
                    with open(file_path, 'rb') as f:
                        header = f.read(4)
                        # ELF magic number (Linux/Unix executables)
                        if header.startswith(b'\x7fELF'):
                            print(f"Making executable: {file_path}")
                            make_executable(file_path)
                        # Mach-O magic numbers (macOS executables)  
                        elif header.startswith((b'\xfe\xed\xfa\xce', b'\xfe\xed\xfa\xcf', 
                                              b'\xce\xfa\xed\xfe', b'\xcf\xfa\xed\xfe')):
                            print(f"Making executable: {file_path}")
                            make_executable(file_path)
                        # PE magic number (Windows executables) - though less relevant on Unix
                        elif header.startswith(b'MZ'):
                            print(f"Making executable: {file_path}")
                            make_executable(file_path)
                        # Also make files with specific names executable
                        elif (file_path.name in ['verus', 'rust_verify', 'z3'] or
                              file_path.name.startswith(('rust_', 'verus'))):
                            print(f"Making executable by name: {file_path}")
                            make_executable(file_path)
                except (IOError, OSError):
                    # If we can't read the file, skip it
                    pass


def setup_verus_installation(extract_dir, install_dir=None):
    """Set up Verus installation in a clean directory."""
    extract_dir = Path(extract_dir)
    
    # Default install directory
    if install_dir is None:
        home_dir = Path.home()
        install_dir = home_dir / 'verus'
    else:
        install_dir = Path(install_dir)
    
    # Find the verus binary
    verus_binary = find_verus_binary(extract_dir)
    if not verus_binary:
        raise Exception("Could not find verus binary in extracted files")
    
    print(f"Found verus binary: {verus_binary}")
    
    # Find the directory containing the binary (usually the root of extracted content)
    binary_dir = verus_binary.parent
    
    # Create install directory
    if install_dir.exists():
        print(f"Removing existing installation at {install_dir}")
        shutil.rmtree(install_dir)
    
    print(f"Installing Verus to: {install_dir}")
    
    # Copy all files from binary directory to install directory
    shutil.copytree(binary_dir, install_dir)
    
    # Make all binaries executable
    make_all_binaries_executable(install_dir)
    
    # Ensure the main verus binary is executable (redundant but safe)
    installed_binary = install_dir / verus_binary.name
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


def update_path_in_shell_config(install_dir):
    """Add Verus installation directory to PATH in shell configuration."""
    install_dir = Path(install_dir)
    config_file = get_shell_config_file()
    
    # Path export line to add
    path_line = f'export PATH="{install_dir}:$PATH"  # Added by Verus installer'
    
    # Check if path is already configured
    if config_file.exists():
        content = config_file.read_text()
        if str(install_dir) in content:
            print(f"PATH already configured in {config_file}")
            return config_file
    
    # Add path to config file
    print(f"Adding Verus to PATH in {config_file}")
    
    with open(config_file, 'a') as f:
        f.write(f'\n# Verus installation\n{path_line}\n')
    
    return config_file


def create_windows_batch_script(install_dir):
    """Create a batch script for Windows to add Verus to PATH."""
    install_dir = Path(install_dir)
    batch_file = install_dir.parent / 'add_verus_to_path.bat'
    
    batch_content = f'''@echo off
echo Adding Verus to PATH...
setx PATH "%PATH%;{install_dir}"
echo Verus has been added to your PATH.
echo Please restart your command prompt or PowerShell for changes to take effect.
pause
'''
    
    with open(batch_file, 'w') as f:
        f.write(batch_content)
    
    print(f"Created Windows batch script: {batch_file}")
    print("Run this script as Administrator to add Verus to your system PATH")
    return batch_file


def setup_path_configuration(install_dir):
    """Set up PATH configuration for the current platform."""
    if platform.system() == 'Windows':
        return create_windows_batch_script(install_dir)
    else:
        return update_path_in_shell_config(install_dir)


def verify_installation(verus_binary):
    """Verify that Verus is working correctly."""
    print("Verifying Verus installation...")
    
    try:
        import subprocess
        result = subprocess.run([str(verus_binary), '--version'], 
                              capture_output=True, text=True, timeout=30)
        
        if result.returncode == 0:
            print(f"✓ Verus is working! Version info:")
            print(result.stdout.strip())
            return True
        else:
            print(f"⚠ Verus binary exists but returned error:")
            print(result.stderr.strip() if result.stderr else "Unknown error")
            return False
    except subprocess.TimeoutExpired:
        print("⚠ Verus version check timed out")
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
    parser = argparse.ArgumentParser(description='Download and install the latest Verus release')
    
    parser.add_argument('--version', '-v',
                       help='Download a specific version (e.g., "0.2025.08.25.63ab0cb")')
    parser.add_argument('--pre-release', '--prerelease', action='store_true', 
                       help='Download the latest pre-release version instead of stable')
    parser.add_argument('--output-dir', '-o', default='.', 
                       help='Download directory (default: current directory)')
    parser.add_argument('--install-dir', '-i',
                       help='Installation directory (default: ~/verus)')
    parser.add_argument('--platform', 
                       help='Platform pattern to search for (e.g., linux-x86_64)')
    parser.add_argument('--list-assets', action='store_true',
                       help='List all available assets without downloading')
    parser.add_argument('--no-extract', action='store_true',
                       help='Download only, do not extract or install')
    parser.add_argument('--no-path', action='store_true',
                       help='Do not modify PATH configuration')
    
    args = parser.parse_args()
    
    try:
        # Validate arguments
        if args.version and args.pre_release:
            print("Error: Cannot specify both --version and --pre-release")
            sys.exit(1)
        
        # Determine release type and fetch release information
        if args.version:
            print(f"Fetching Verus release version: {args.version}...")
            release = get_specific_release(args.version)
        elif args.pre_release:
            print("Fetching latest Verus pre-release...")
            release = get_latest_release(include_prerelease=True)
        else:
            print("Fetching latest stable Verus release...")
            release = get_latest_release(include_prerelease=False)
        
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
                    print(f"  - {a['name']}")
                return
        else:
            if len(assets) == 1:
                asset = assets[0]
            else:
                print("Multiple assets available, please specify platform:")
                for i, a in enumerate(assets):
                    print(f"  {i}: {a['name']}")
                
                choice = input("Enter asset number: ")
                try:
                    asset = assets[int(choice)]
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
            if filename.suffix.lower() in ['.zip'] or filename.name.endswith(('.tar.gz', '.tgz')):
                try:
                    # Extract to temporary directory
                    temp_extract_dir = output_dir / 'temp_extract'
                    if temp_extract_dir.exists():
                        shutil.rmtree(temp_extract_dir)
                    
                    extract_archive(filename, temp_extract_dir)
                    
                    # Set up installation
                    install_dir, verus_binary = setup_verus_installation(
                        temp_extract_dir, args.install_dir
                    )
                    
                    print(f"✓ Verus installed to: {install_dir}")
                    print(f"✓ Verus binary: {verus_binary}")
                    
                    # Verify installation
                    if verify_installation(verus_binary):
                        print("✓ Installation verified successfully!")
                    
                    # Set up PATH
                    if not args.no_path:
                        config_file = setup_path_configuration(install_dir)
                        
                        if platform.system() == 'Windows':
                            print(f"\n📝 Next steps for Windows:")
                            print(f"   1. Run {config_file} as Administrator to add Verus to PATH")
                            print(f"   2. Restart your command prompt/PowerShell")
                            print(f"   3. Type 'verus --version' to verify")
                        else:
                            print(f"\n📝 Next steps:")
                            print(f"   1. Restart your terminal or run: source {config_file}")
                            print(f"   2. Type 'verus --version' to verify")
                            print(f"   Or run directly: {verus_binary} --version")
                    
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
                print(f"Downloaded file is not an archive: {filename}")
                print("Manual installation may be required.")
        else:
            print(f"\nTo manually extract and install:")
            if filename.suffix.lower() == '.zip':
                print(f"  unzip '{filename}'")
            else:
                print(f"  tar -xzf '{filename}'")
        
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