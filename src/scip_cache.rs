//! SCIP index caching and generation module.
//!
//! This module handles the generation and caching of SCIP (Source Code Index Protocol)
//! indexes from verus-analyzer or rust-analyzer. SCIP generation can be slow for large
//! projects, so caching is important for developer experience.
//!
//! Tool resolution uses the tool manager: managed directory (~/.probe-verus/tools/)
//! is checked first, then PATH. If `auto_install` is enabled, missing tools are
//! downloaded automatically.

use crate::constants::{DATA_DIR, SCIP_INDEX_FILE, SCIP_INDEX_JSON_FILE};
use crate::tool_manager::{self, Tool};
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
    /// Analyzer command not found (not in managed dir or PATH)
    AnalyzerNotFound(Analyzer, String),
    /// scip CLI command not found (not in managed dir or PATH)
    ScipCliNotFound(String),
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
            ScipError::AnalyzerNotFound(a, detail) => {
                write!(f, "{a} not found. {detail}")
            }
            ScipError::ScipCliNotFound(detail) => {
                write!(f, "scip not found. {detail}")
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
    auto_install: bool,
    /// Resolved path to the analyzer binary (set during check_prerequisites)
    analyzer_path: Option<PathBuf>,
    /// Resolved path to the scip binary (set during check_prerequisites)
    scip_path_resolved: Option<PathBuf>,
}

impl ScipCache {
    /// Create a new ScipCache for the given project using the default verus-analyzer.
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
            analyzer: Analyzer::VerusAnalyzer,
            auto_install: false,
            analyzer_path: None,
            scip_path_resolved: None,
        }
    }

    /// Create a new ScipCache with a specific analyzer choice.
    pub fn with_analyzer(project_path: impl Into<PathBuf>, analyzer: Analyzer) -> Self {
        Self {
            project_path: project_path.into(),
            analyzer,
            auto_install: false,
            analyzer_path: None,
            scip_path_resolved: None,
        }
    }

    /// Enable auto-install: download missing tools automatically.
    pub fn with_auto_install(mut self, auto_install: bool) -> Self {
        self.auto_install = auto_install;
        self
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
    pub fn get_or_generate(
        &mut self,
        regenerate: bool,
        verbose: bool,
    ) -> Result<PathBuf, ScipError> {
        let json_path = self.json_path();

        if json_path.exists() && !regenerate {
            return Ok(json_path);
        }

        self.check_prerequisites()?;
        self.generate_scip_index(verbose)?;
        self.convert_to_json(verbose)?;

        Ok(json_path)
    }

    /// Resolve external tools via the tool manager (managed dir -> PATH -> auto-download).
    fn check_prerequisites(&mut self) -> Result<(), ScipError> {
        let analyzer_tool = Tool::from_analyzer(self.analyzer);
        let analyzer_path = tool_manager::resolve_or_install(analyzer_tool, self.auto_install)
            .map_err(|e| ScipError::AnalyzerNotFound(self.analyzer, e.to_string()))?;
        self.analyzer_path = Some(analyzer_path);

        let scip_path = tool_manager::resolve_or_install(Tool::Scip, self.auto_install)
            .map_err(|e| ScipError::ScipCliNotFound(e.to_string()))?;
        self.scip_path_resolved = Some(scip_path);

        Ok(())
    }

    /// Generate the SCIP index using the configured analyzer.
    fn generate_scip_index(&self, verbose: bool) -> Result<(), ScipError> {
        let analyzer_bin = self
            .analyzer_path
            .as_ref()
            .expect("check_prerequisites must be called first");

        if verbose {
            println!(
                "Generating SCIP index for {} (using {})...",
                self.project_path.display(),
                self.analyzer
            );
        }

        let status = Command::new(analyzer_bin)
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

        let generated_path = self.project_path.join("index.scip");
        if !generated_path.exists() {
            return Err(ScipError::IndexNotGenerated(self.analyzer));
        }

        let data_dir = self.data_dir();
        if !data_dir.exists() {
            std::fs::create_dir_all(&data_dir).map_err(ScipError::CreateDirFailed)?;
        }

        let cached_path = self.scip_path();
        std::fs::rename(&generated_path, &cached_path).map_err(ScipError::MoveFileFailed)?;

        if verbose {
            println!("  Saved index.scip to {}", cached_path.display());
        }

        Ok(())
    }

    /// Convert the SCIP index to JSON format.
    fn convert_to_json(&self, verbose: bool) -> Result<(), ScipError> {
        let scip_bin = self
            .scip_path_resolved
            .as_ref()
            .expect("check_prerequisites must be called first");

        if verbose {
            println!("Converting index.scip to JSON...");
        }

        let scip_index_path = self.scip_path();
        let output = Command::new(scip_bin)
            .args(["print", "--json", scip_index_path.to_string_lossy().as_ref()])
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
        let err = ScipError::AnalyzerNotFound(Analyzer::VerusAnalyzer, "not installed".into());
        assert!(err.to_string().contains("verus-analyzer not found"));

        let err = ScipError::AnalyzerNotFound(Analyzer::RustAnalyzer, "not installed".into());
        assert!(err.to_string().contains("rust-analyzer not found"));

        let err = ScipError::ScipCliNotFound("not installed".into());
        assert!(err.to_string().contains("scip not found"));
    }

    #[test]
    fn test_scip_cache_with_analyzer() {
        let cache = ScipCache::with_analyzer("/path/to/project", Analyzer::RustAnalyzer);
        assert_eq!(cache.analyzer, Analyzer::RustAnalyzer);
        assert_eq!(cache.data_dir(), PathBuf::from("/path/to/project/data"));
    }

    #[test]
    fn test_scip_cache_auto_install() {
        let cache = ScipCache::new("/path/to/project").with_auto_install(true);
        assert!(cache.auto_install);

        let cache = ScipCache::new("/path/to/project").with_auto_install(false);
        assert!(!cache.auto_install);
    }
}
