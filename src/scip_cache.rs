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
use std::path::{Path, PathBuf};
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

    /// Whether a cached SCIP JSON exists **and** is still current — i.e. no `.rs`
    /// source file under the project is newer than it. Callers that decide whether
    /// to reuse the cache should use this rather than [`Self::has_cached_json`], so
    /// an edited project does not silently reuse a stale index.
    pub fn has_current_cached_json(&self) -> bool {
        let json = self.json_path();
        json.exists() && !self.cache_is_stale(&json)
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

        if json_path.exists() && !regenerate && !self.cache_is_stale(&json_path) {
            return Ok(json_path);
        }
        if json_path.exists() && !regenerate && verbose {
            println!("  SCIP cache is stale (source newer than index); regenerating...");
        }

        self.check_prerequisites()?;
        self.generate_scip_index(verbose)?;
        self.convert_to_json(verbose)?;

        Ok(json_path)
    }

    /// Whether the cached SCIP JSON is older than any `.rs` source file under the
    /// project, in which case it must be regenerated. The cache is keyed only on
    /// file existence, so without this check an edited project silently reuses a
    /// stale index — atom line numbers then diverge from the current source,
    /// causing span-matching and backfill failures downstream.
    ///
    /// Conservative: if mtimes can't be read, returns `false` (keep the cache)
    /// rather than forcing an expensive regeneration on a metadata hiccup. The
    /// `data/` and `target/` directories are skipped.
    fn cache_is_stale(&self, json_path: &Path) -> bool {
        let Ok(cached_mtime) = std::fs::metadata(json_path).and_then(|m| m.modified()) else {
            return false;
        };
        for entry in walkdir::WalkDir::new(&self.project_path)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                name != DATA_DIR && name != "target"
            })
            .filter_map(Result::ok)
        {
            if entry.path().extension().is_some_and(|ext| ext == "rs") {
                if let Some(src_mtime) = entry.metadata().ok().and_then(|m| m.modified().ok()) {
                    if src_mtime > cached_mtime {
                        return true;
                    }
                }
            }
        }
        false
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

    /// Write a temporary verus-analyzer config that enables `verus_keep_ghost`.
    ///
    /// Verus projects gate specification-bearing variants of functions behind
    /// `#[cfg(verus_keep_ghost)]`.  Without this cfg, SCIP indexes the plain
    /// (non-spec) variants, whose line numbers diverge from what the Verus
    /// parser sees — causing atom-to-proof matching failures later.
    fn write_verus_cfg_config(&self) -> Option<PathBuf> {
        if self.analyzer != Analyzer::VerusAnalyzer {
            return None;
        }
        let path = self.data_dir().join(".va_scip_config.json");
        std::fs::create_dir_all(self.data_dir()).ok()?;
        std::fs::write(&path, r#"{"cargo":{"cfgs":{"verus_keep_ghost":null}}}"#).ok()?;
        // Canonicalize to an absolute path so it remains valid when the child
        // process runs with a different CWD (current_dir set to project_path).
        std::fs::canonicalize(&path).ok().or(Some(path))
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

        let config_file = self.write_verus_cfg_config();

        let mut cmd = Command::new(analyzer_bin);
        cmd.args(["scip", "."]);
        if let Some(ref cfg_path) = config_file {
            cmd.arg("--config-path").arg(cfg_path);
        }
        let status = cmd
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
            .args([
                "print",
                "--json",
                scip_index_path.to_string_lossy().as_ref(),
            ])
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
        } else if self.has_cached_json() {
            "(cached index is stale — source changed)"
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
    fn test_cache_is_stale_on_source_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();

        // Source written first, cache JSON after → cache is current.
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let json = root.join("data/index.scip.json");
        std::fs::write(&json, "{}").unwrap();

        let cache = ScipCache::new(root);
        assert!(
            !cache.cache_is_stale(&json),
            "cache newer than every source file must not be stale"
        );

        // Edit a source file after the cache → cache is stale.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("src/lib.rs"), "fn a() { /* edited */ }").unwrap();
        assert!(
            cache.cache_is_stale(&json),
            "a source file newer than the cache must mark it stale"
        );
    }

    #[test]
    fn test_cache_not_stale_when_only_data_dir_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let json = root.join("data/index.scip.json");
        std::fs::write(&json, "{}").unwrap();
        // A newer .rs under data/ (e.g. a generated artifact) must be ignored.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("data/generated.rs"), "fn g() {}").unwrap();

        let cache = ScipCache::new(root);
        assert!(
            !cache.cache_is_stale(&json),
            "changes under data/ must not invalidate the cache"
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
