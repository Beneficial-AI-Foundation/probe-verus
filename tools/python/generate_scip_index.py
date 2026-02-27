#!/usr/bin/env python3
"""
SCIP Index Generator

Copies a project, runs an analyzer (verus-analyzer or rust-analyzer) to generate SCIP data,
and exports the SCIP index to JSON format.
"""

import argparse
import os
import sys
import shutil
import subprocess
from pathlib import Path
import tempfile


def copy_project(source_project, destination=None):
    """Copy a project to a temporary or specified destination."""
    source_path = Path(source_project).resolve()
    
    if not source_path.exists():
        raise FileNotFoundError(f"Source project not found: {source_path}")
    
    if not source_path.is_dir():
        raise NotADirectoryError(f"Source is not a directory: {source_path}")
    
    if destination:
        dest_path = Path(destination).resolve()
        if dest_path.exists():
            print(f"Removing existing destination: {dest_path}")
            shutil.rmtree(dest_path)
    else:
        # Create temporary directory
        temp_dir = tempfile.mkdtemp(prefix='scip_analysis_')
        dest_path = Path(temp_dir) / source_path.name
    
    print(f"Copying project from {source_path} to {dest_path}")
    
    # Copy the entire project
    shutil.copytree(source_path, dest_path)
    
    return dest_path


def check_analyzer_available(analyzer_name):
    """Check if the specified analyzer is available in PATH."""
    try:
        result = subprocess.run([analyzer_name, '--version'], 
                              capture_output=True, text=True, timeout=10)
        if result.returncode == 0:
            print(f"✓ {analyzer_name} is available")
            print(f"  Version: {result.stdout.strip()}")
            return True
        else:
            print(f"⚠ {analyzer_name} found but returned error: {result.stderr.strip()}")
            return False
    except FileNotFoundError:
        print(f"✗ {analyzer_name} not found in PATH")
        return False
    except subprocess.TimeoutExpired:
        print(f"⚠ {analyzer_name} version check timed out")
        return False
    except Exception as e:
        print(f"⚠ Error checking {analyzer_name}: {e}")
        return False


def check_scip_available():
    """Check if SCIP is available in PATH."""
    try:
        result = subprocess.run(['scip', '--version'], 
                              capture_output=True, text=True, timeout=10)
        if result.returncode == 0:
            print(f"✓ SCIP is available")
            print(f"  Version: {result.stdout.strip()}")
            return True
        else:
            # Try help command as fallback
            result = subprocess.run(['scip', '--help'], 
                                  capture_output=True, text=True, timeout=10)
            if result.returncode == 0:
                print(f"✓ SCIP is available")
                return True
            else:
                print(f"⚠ SCIP found but returned error: {result.stderr.strip()}")
                return False
    except FileNotFoundError:
        print(f"✗ SCIP not found in PATH")
        print(f"  Install SCIP using: ./scip_installer.py")
        return False
    except subprocess.TimeoutExpired:
        print(f"⚠ SCIP version check timed out")
        return False
    except Exception as e:
        print(f"⚠ Error checking SCIP: {e}")
        return False


def run_analyzer_scip(project_path, analyzer_name):
    """Run the analyzer to generate SCIP data."""
    project_path = Path(project_path)
    
    print(f"Running {analyzer_name} SCIP analysis in {project_path}")
    
    # Change to project directory
    original_cwd = os.getcwd()
    
    try:
        os.chdir(project_path)
        
        # Run analyzer SCIP command
        cmd = [analyzer_name, 'scip', '.']
        print(f"Executing: {' '.join(cmd)}")
        
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
        
        if result.returncode == 0:
            print(f"✓ {analyzer_name} SCIP analysis completed successfully")
            if result.stdout.strip():
                print(f"Output: {result.stdout.strip()}")
            
            # Check if index.scip was created
            scip_file = project_path / 'index.scip'
            if scip_file.exists():
                print(f"✓ SCIP index file created: {scip_file}")
                return scip_file
            else:
                print(f"⚠ SCIP index file not found at expected location: {scip_file}")
                # Look for SCIP files in common locations
                for pattern in ['*.scip', '**/*.scip']:
                    scip_files = list(project_path.glob(pattern))
                    if scip_files:
                        print(f"Found SCIP files: {scip_files}")
                        return scip_files[0]  # Return the first one found
                return None
        else:
            print(f"✗ {analyzer_name} SCIP analysis failed")
            print(f"Error: {result.stderr.strip()}")
            return None
            
    except subprocess.TimeoutExpired:
        print(f"⚠ {analyzer_name} SCIP analysis timed out (5 minutes)")
        return None
    except Exception as e:
        print(f"⚠ Error running {analyzer_name}: {e}")
        return None
    finally:
        os.chdir(original_cwd)


def export_scip_to_json(scip_file, output_file=None):
    """Export SCIP index to JSON format."""
    scip_file = Path(scip_file)
    
    if not scip_file.exists():
        raise FileNotFoundError(f"SCIP file not found: {scip_file}")
    
    if output_file is None:
        output_file = scip_file.parent / 'index_scip.json'
    else:
        output_file = Path(output_file)
    
    print(f"Exporting SCIP index to JSON: {output_file}")
    
    # Change to the directory containing the SCIP file
    original_cwd = os.getcwd()
    
    try:
        os.chdir(scip_file.parent)
        
        # Run SCIP print command
        cmd = ['scip', 'print', '--json', scip_file.name]
        print(f"Executing: {' '.join(cmd)}")
        
        with open(output_file, 'w') as f:
            result = subprocess.run(cmd, stdout=f, stderr=subprocess.PIPE, 
                                  text=True, timeout=120)
        
        if result.returncode == 0:
            print(f"✓ SCIP JSON export completed successfully")
            print(f"✓ Output file: {output_file}")
            
            # Check file size
            file_size = output_file.stat().st_size
            if file_size > 0:
                size_mb = file_size / (1024 * 1024)
                print(f"  File size: {size_mb:.2f} MB")
                return output_file
            else:
                print(f"⚠ Output file is empty")
                return None
        else:
            print(f"✗ SCIP JSON export failed")
            print(f"Error: {result.stderr.strip()}")
            # Remove empty output file
            if output_file.exists() and output_file.stat().st_size == 0:
                output_file.unlink()
            return None
            
    except subprocess.TimeoutExpired:
        print(f"⚠ SCIP JSON export timed out (2 minutes)")
        return None
    except Exception as e:
        print(f"⚠ Error exporting SCIP to JSON: {e}")
        return None
    finally:
        os.chdir(original_cwd)


def main():
    parser = argparse.ArgumentParser(
        description='Copy a project and generate SCIP index in JSON format',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  %(prog)s /path/to/project
  %(prog)s /path/to/project --analyzer rust-analyzer
  %(prog)s /path/to/project --output-dir /tmp/analysis
  %(prog)s /path/to/project --keep-copy --output-dir ./analysis
        """
    )
    
    parser.add_argument('project', 
                       help='Path to the project to analyze')
    parser.add_argument('--analyzer', '-a', default='verus-analyzer',
                       choices=['verus-analyzer', 'rust-analyzer'],
                       help='Analyzer to use (default: verus-analyzer)')
    parser.add_argument('--output-dir', '-o',
                       help='Directory to copy project to (default: temporary directory)')
    parser.add_argument('--json-output', '-j',
                       help='Output file for JSON export (default: index_scip.json in project)')
    parser.add_argument('--keep-copy', action='store_true',
                       help='Keep the copied project after analysis')
    parser.add_argument('--check-tools', action='store_true',
                       help='Check if required tools are available and exit')
    
    args = parser.parse_args()
    
    try:
        # Check tool availability
        print("Checking required tools...")
        analyzer_available = check_analyzer_available(args.analyzer)
        scip_available = check_scip_available()
        
        if args.check_tools:
            if analyzer_available and scip_available:
                print("\n✓ All required tools are available")
                sys.exit(0)
            else:
                print("\n✗ Some required tools are missing")
                sys.exit(1)
        
        if not analyzer_available:
            print(f"\nError: {args.analyzer} is not available")
            print(f"Install it using: ./verus_analyzer_installer.py")
            sys.exit(1)
        
        if not scip_available:
            print(f"\nError: SCIP is not available")
            print(f"Install it using: ./scip_installer.py")
            sys.exit(1)
        
        print("\n" + "="*60)
        print("SCIP INDEX GENERATION")
        print("="*60)
        
        # Step 1: Copy project
        print("\n1. Copying project...")
        project_copy = copy_project(args.project, args.output_dir)
        
        # Step 2: Run analyzer SCIP
        print(f"\n2. Running {args.analyzer} SCIP analysis...")
        scip_file = run_analyzer_scip(project_copy, args.analyzer)
        
        if not scip_file:
            print("Failed to generate SCIP index")
            sys.exit(1)
        
        # Step 3: Export to JSON
        print("\n3. Exporting SCIP index to JSON...")
        json_output = args.json_output
        if json_output:
            json_output = Path(json_output).resolve()
        
        json_file = export_scip_to_json(scip_file, json_output)
        
        if not json_file:
            print("Failed to export SCIP index to JSON")
            sys.exit(1)
        
        # Summary
        print("\n" + "="*60)
        print("ANALYSIS COMPLETE")
        print("="*60)
        print(f"✓ Project: {args.project}")
        print(f"✓ Analyzer: {args.analyzer}")
        print(f"✓ Project copy: {project_copy}")
        print(f"✓ SCIP file: {scip_file}")
        print(f"✓ JSON output: {json_file}")
        
        # Cleanup
        if not args.keep_copy and not args.output_dir:
            print(f"\nCleaning up temporary project copy...")
            shutil.rmtree(project_copy.parent)  # Remove temp directory
            print(f"✓ Temporary files cleaned up")
        elif not args.keep_copy and args.output_dir:
            print(f"\nNote: Project copy kept at {project_copy} (use --keep-copy to suppress this message)")
        
        print(f"\n📄 SCIP index JSON available at: {json_file}")
        
    except KeyboardInterrupt:
        print("\n\nOperation cancelled by user")
        sys.exit(1)
    except FileNotFoundError as e:
        print(f"\nError: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"\nUnexpected error: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
