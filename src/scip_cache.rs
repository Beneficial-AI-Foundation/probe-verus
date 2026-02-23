//! SCIP index caching and generation module.
//!
//! This module handles the generation and caching of SCIP (Source Code Index Protocol)
//! indexes from verus-analyzer or rust-analyzer. SCIP generation can be slow for large
//! projects, so caching is important for developer experience.

use crate::constants::{DATA_DIR, SCIP_INDEX_FILE, SCIP_INDEX_JSON_FILE};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Which language server to use for SCIP index generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Analyzer {
    VerusAnalyzer,
    RustAnalyzer,
}

impl Analyzer {
    pub fn command_name(&self) -> &'static str {
        match self {
            Analyzer::VerusAnalyzer => "verus-analyzer",
            Analyzer::RustAnalyzer => "rust-analyzer",
        }
    }
}

impl std::fmt::Display for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.command_name())
    }
}

/// Error types for SCIP operations
#[derive(Debug)]
pub enum ScipError {
    /// Analyzer command not found in PATH
    AnalyzerNotFound(Analyzer),
    /// scip CLI command not found in PATH
    ScipCliNotFound,
    /// Analyzer scip command failed
    AnalyzerFailed(Analyzer, String),
    /// scip print command failed
    ScipPrintFailed(String),
    /// index.scip file not generated
    IndexNotGenerated(Analyzer),
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
            ScipError::AnalyzerNotFound(a) => {
                write!(f, "{} not found in PATH", a)
            }
            ScipError::ScipCliNotFound => {
                write!(f, "scip not found in PATH")
            }
            ScipError::AnalyzerFailed(a, msg) => {
                write!(f, "{} scip failed: {}", a, msg)
            }
            ScipError::ScipPrintFailed(msg) => {
                write!(f, "scip print failed: {}", msg)
            }
            ScipError::IndexNotGenerated(a) => {
                write!(
                    f,
                    "index.scip not generated ({} may have failed silently)",
                    a
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
/// - `index.scip`: Binary SCIP index from verus-analyzer or rust-analyzer
/// - `index.scip.json`: JSON representation for parsing
pub struct ScipCache {
    project_path: PathBuf,
    analyzer: Analyzer,
}

impl ScipCache {
    /// Create a new ScipCache for the given project using the default verus-analyzer.
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
            analyzer: Analyzer::VerusAnalyzer,
        }
    }

    /// Create a new ScipCache with a specific analyzer choice.
    pub fn with_analyzer(project_path: impl Into<PathBuf>, analyzer: Analyzer) -> Self {
        Self {
            project_path: project_path.into(),
            analyzer,
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
        if !command_exists(self.analyzer.command_name()) {
            return Err(ScipError::AnalyzerNotFound(self.analyzer));
        }
        if !command_exists("scip") {
            return Err(ScipError::ScipCliNotFound);
        }
        Ok(())
    }

    /// Generate the SCIP index using the configured analyzer.
    fn generate_scip_index(&self, verbose: bool) -> Result<(), ScipError> {
        if verbose {
            println!(
                "Generating SCIP index for {} (using {})...",
                self.project_path.display(),
                self.analyzer
            );
        }

        let status = Command::new(self.analyzer.command_name())
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
                return Err(ScipError::AnalyzerFailed(
                    self.analyzer,
                    format!("exit status: {}", s),
                ));
            }
            Err(e) => {
                return Err(ScipError::AnalyzerFailed(self.analyzer, e.to_string()));
            }
        }

        // Check that index.scip was generated
        let generated_path = self.project_path.join("index.scip");
        if !generated_path.exists() {
            return Err(ScipError::IndexNotGenerated(self.analyzer));
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
        let err = ScipError::AnalyzerNotFound(Analyzer::VerusAnalyzer);
        assert_eq!(err.to_string(), "verus-analyzer not found in PATH");

        let err = ScipError::AnalyzerNotFound(Analyzer::RustAnalyzer);
        assert_eq!(err.to_string(), "rust-analyzer not found in PATH");

        let err = ScipError::ScipCliNotFound;
        assert_eq!(err.to_string(), "scip not found in PATH");
    }

    #[test]
    fn test_scip_cache_with_analyzer() {
        let cache = ScipCache::with_analyzer("/path/to/project", Analyzer::RustAnalyzer);
        assert_eq!(cache.analyzer, Analyzer::RustAnalyzer);
        assert_eq!(cache.data_dir(), PathBuf::from("/path/to/project/data"));
    }
}
