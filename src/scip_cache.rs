//! SCIP index caching and generation module.
//!
//! This module handles the generation and caching of SCIP (Source Code Index Protocol)
//! indexes from verus-analyzer. SCIP generation can be slow for large projects,
//! so caching is important for developer experience.

use crate::constants::{DATA_DIR, SCIP_INDEX_FILE, SCIP_INDEX_JSON_FILE};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Error types for SCIP operations
#[derive(Debug)]
pub enum ScipError {
    /// verus-analyzer command not found in PATH
    VerusAnalyzerNotFound,
    /// scip CLI command not found in PATH
    ScipCliNotFound,
    /// verus-analyzer scip command failed
    VerusAnalyzerFailed(String),
    /// scip print command failed
    ScipPrintFailed(String),
    /// index.scip file not generated
    IndexNotGenerated,
    /// Failed to create data directory
    CreateDirFailed(std::io::Error),
    /// Failed to move index file
    MoveFileFailed(std::io::Error),
    /// Failed to write JSON file
    WriteJsonFailed(std::io::Error),
}

impl std::fmt::Display for ScipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScipError::VerusAnalyzerNotFound => {
                write!(f, "verus-analyzer not found in PATH")
            }
            ScipError::ScipCliNotFound => {
                write!(f, "scip not found in PATH")
            }
            ScipError::VerusAnalyzerFailed(msg) => {
                write!(f, "verus-analyzer scip failed: {}", msg)
            }
            ScipError::ScipPrintFailed(msg) => {
                write!(f, "scip print failed: {}", msg)
            }
            ScipError::IndexNotGenerated => {
                write!(
                    f,
                    "index.scip not generated (verus-analyzer may have failed silently)"
                )
            }
            ScipError::CreateDirFailed(e) => {
                write!(f, "failed to create data directory: {}", e)
            }
            ScipError::MoveFileFailed(e) => {
                write!(f, "failed to move index.scip: {}", e)
            }
            ScipError::WriteJsonFailed(e) => {
                write!(f, "failed to write SCIP JSON: {}", e)
            }
        }
    }
}

impl std::error::Error for ScipError {}

/// Manager for SCIP index caching.
///
/// SCIP indexes are stored in `<project>/data/` directory:
/// - `index.scip`: Binary SCIP index from verus-analyzer
/// - `index.scip.json`: JSON representation for parsing
pub struct ScipCache {
    project_path: PathBuf,
}

impl ScipCache {
    /// Create a new ScipCache for the given project.
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
        }
    }

    /// Get the data directory path.
    pub fn data_dir(&self) -> PathBuf {
        self.project_path.join(DATA_DIR)
    }

    /// Get the cached SCIP binary index path.
    pub fn scip_path(&self) -> PathBuf {
        self.data_dir().join(SCIP_INDEX_FILE)
    }

    /// Get the cached SCIP JSON path.
    pub fn json_path(&self) -> PathBuf {
        self.data_dir().join(SCIP_INDEX_JSON_FILE)
    }

    /// Check if cached SCIP JSON exists.
    pub fn has_cached_json(&self) -> bool {
        self.json_path().exists()
    }

    /// Get the path to the SCIP JSON, generating it if necessary.
    ///
    /// # Arguments
    /// * `regenerate` - If true, regenerate even if cached version exists
    /// * `verbose` - If true, show progress output
    ///
    /// # Returns
    /// Path to the SCIP JSON file
    pub fn get_or_generate(&self, regenerate: bool, verbose: bool) -> Result<PathBuf, ScipError> {
        let json_path = self.json_path();

        // Use cache if available and not regenerating
        if json_path.exists() && !regenerate {
            return Ok(json_path);
        }

        // Need to generate - check prerequisites
        self.check_prerequisites()?;

        // Generate SCIP index
        self.generate_scip_index(verbose)?;

        // Convert to JSON
        self.convert_to_json(verbose)?;

        Ok(json_path)
    }

    /// Check that required external tools are available.
    fn check_prerequisites(&self) -> Result<(), ScipError> {
        if !command_exists("verus-analyzer") {
            return Err(ScipError::VerusAnalyzerNotFound);
        }
        if !command_exists("scip") {
            return Err(ScipError::ScipCliNotFound);
        }
        Ok(())
    }

    /// Generate the SCIP index using verus-analyzer.
    fn generate_scip_index(&self, verbose: bool) -> Result<(), ScipError> {
        if verbose {
            println!(
                "Generating SCIP index for {}...",
                self.project_path.display()
            );
        }

        let status = Command::new("verus-analyzer")
            .args(["scip", "."])
            .current_dir(&self.project_path)
            .stdout(if verbose {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .stderr(if verbose {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return Err(ScipError::VerusAnalyzerFailed(format!(
                    "exit status: {}",
                    s
                )));
            }
            Err(e) => {
                return Err(ScipError::VerusAnalyzerFailed(e.to_string()));
            }
        }

        // Check that index.scip was generated
        let generated_path = self.project_path.join("index.scip");
        if !generated_path.exists() {
            return Err(ScipError::IndexNotGenerated);
        }

        // Ensure data directory exists
        let data_dir = self.data_dir();
        if !data_dir.exists() {
            std::fs::create_dir_all(&data_dir).map_err(ScipError::CreateDirFailed)?;
        }

        // Move to data directory
        let cached_path = self.scip_path();
        std::fs::rename(&generated_path, &cached_path).map_err(ScipError::MoveFileFailed)?;

        if verbose {
            println!("  Saved index.scip to {}", cached_path.display());
        }

        Ok(())
    }

    /// Convert the SCIP index to JSON format.
    fn convert_to_json(&self, verbose: bool) -> Result<(), ScipError> {
        if verbose {
            println!("Converting index.scip to JSON...");
        }

        let scip_path = self.scip_path();
        let output = Command::new("scip")
            .args(["print", "--json", scip_path.to_str().unwrap()])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let json_path = self.json_path();
                std::fs::write(&json_path, o.stdout).map_err(ScipError::WriteJsonFailed)?;

                if verbose {
                    println!("  Saved SCIP JSON to {}", json_path.display());
                }

                Ok(())
            }
            Ok(o) => Err(ScipError::ScipPrintFailed(format!(
                "exit status: {}",
                o.status
            ))),
            Err(e) => Err(ScipError::ScipPrintFailed(e.to_string())),
        }
    }

    /// Get the reason string for why generation is happening.
    pub fn generation_reason(&self, regenerate: bool) -> &'static str {
        if regenerate {
            "(regeneration requested)"
        } else {
            "(no existing SCIP data found)"
        }
    }
}

/// Check if a command exists in PATH.
fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scip_cache_paths() {
        let cache = ScipCache::new("/path/to/project");
        assert_eq!(cache.data_dir(), PathBuf::from("/path/to/project/data"));
        assert_eq!(
            cache.scip_path(),
            PathBuf::from("/path/to/project/data/index.scip")
        );
        assert_eq!(
            cache.json_path(),
            PathBuf::from("/path/to/project/data/index.scip.json")
        );
    }

    #[test]
    fn test_scip_error_display() {
        let err = ScipError::VerusAnalyzerNotFound;
        assert_eq!(err.to_string(), "verus-analyzer not found in PATH");

        let err = ScipError::ScipCliNotFound;
        assert_eq!(err.to_string(), "scip not found in PATH");
    }
}
