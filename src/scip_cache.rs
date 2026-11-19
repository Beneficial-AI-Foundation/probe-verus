//! SCIP index caching and generation module.
//!
//! This module handles the generation and caching of SCIP (Source Code Index Protocol)
//! indexes from verus-analyzer or rust-analyzer. SCIP generation can be slow for large
//! projects, so caching is important for developer experience.
//!
//! verus-analyzer is the release that [`analyzer_selection::select`] picks for
//! the project; scip and rust-analyzer come from the managed directory
//! (~/.probe-verus/tools/), then PATH. If `auto_install` is enabled, missing
//! tools are downloaded automatically. The cache is reused only if its
//! [`CacheKey`] matches the current run and no source file is newer.
//!
//! A [`ScipCache`] takes an exclusive lock on `data/index.scip.lock` at its
//! first use and holds it until it is dropped. Two runs on one project then
//! cannot mix one run's index with the other run's key, and a run does not
//! read an index that another run replaces.

use crate::analyzer_selection;
use crate::constants::{
    DATA_DIR, SCIP_INDEX_FILE, SCIP_INDEX_JSON_FILE, SCIP_INDEX_KEY_FILE, SCIP_INDEX_LOCK_FILE,
};
use crate::tool_manager::{self, Tool};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
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

/// First line of a command's stdout, or empty if it cannot be run.
fn command_first_line(program: &Path, args: &[&str], dir: &Path) -> String {
    Command::new(program)
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Problems to report before a (slow) `verus-analyzer scip` run, from the
/// analyzer's `--version` line (`verus-analyzer 0.3.269-standalone`) and the
/// project's `rustc --version` line (`rustc 1.92.0 (...)`).
fn analyzer_warnings(analyzer_version: &str, rustc_version: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(patch) = analyzer_version
        .split_whitespace()
        .nth(1)
        .and_then(crate::analyzer_patch_version)
    else {
        return warnings;
    };
    if patch < crate::FIRST_CURRENT_FORMAT_ANALYZER_PATCH {
        warnings.push(format!(
            "`{analyzer_version}` predates verus-analyzer 2026-08-22; its index will be \
             rejected. Install a newer release (`probe-verus setup`)."
        ));
    } else if rustc_version.starts_with("rustc 1.92.") {
        warnings.push(format!(
            "verus-analyzer 2026-08-22 and later index about 10x slower with `{rustc_version}`; \
             Rust 1.93 or later is not affected."
        ));
    }
    warnings
}

/// Error types for SCIP operations
#[derive(Debug)]
pub enum ScipError {
    /// Analyzer command not found (not in managed dir or PATH)
    AnalyzerNotFound(Analyzer, String),
    /// The verus-analyzer selection (environment or Cargo.toml pin) is invalid
    BadSelection(String),
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
    /// Failed to lock the cache
    LockFailed(std::io::Error),
}

impl std::fmt::Display for ScipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScipError::AnalyzerNotFound(a, detail) => {
                write!(f, "{a} not found. {detail}")
            }
            ScipError::BadSelection(detail) => {
                write!(f, "invalid verus-analyzer selection: {detail}")
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
            ScipError::LockFailed(e) => {
                write!(f, "failed to lock the SCIP cache: {}", e)
            }
        }
    }
}

impl std::error::Error for ScipError {}

/// The inputs that produced a cached index. If any of them differs from the
/// current run, the index is regenerated. Caches without a key (from
/// probe-verus 8.x) are regenerated too.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CacheKey {
    /// The analyzer and its selector, e.g. `verus-analyzer 2026-09-25`.
    pub analyzer: String,
    pub analyzer_path: String,
    pub analyzer_sha256: String,
    /// `rustc -vV` in the project directory.
    pub rustc: String,
    /// SHA-256 of the package and workspace `Cargo.toml` and of `Cargo.lock`,
    /// by canonical path. A missing file has no entry.
    pub manifests: BTreeMap<String, String>,
}

impl CacheKey {
    /// Names of the fields that differ between `self` and `other`.
    fn changes(&self, other: &CacheKey) -> Vec<&'static str> {
        let mut changed = Vec::new();
        if self.analyzer != other.analyzer {
            changed.push("analyzer selection");
        }
        if self.analyzer_path != other.analyzer_path
            || self.analyzer_sha256 != other.analyzer_sha256
        {
            changed.push("analyzer binary");
        }
        if self.rustc != other.rustc {
            changed.push("rustc");
        }
        if self.manifests != other.manifests {
            changed.push("Cargo.toml or Cargo.lock");
        }
        changed
    }
}

/// Whether the cached index can be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheState {
    Current,
    Missing,
    /// The cache must be regenerated, for the given reason.
    Outdated(String),
}

impl CacheState {
    /// Why the index is generated, for progress output.
    pub fn reason(&self) -> String {
        match self {
            CacheState::Current => "(cached index is current)".into(),
            CacheState::Missing => "(no existing SCIP data found)".into(),
            CacheState::Outdated(why) => format!("({why})"),
        }
    }
}

/// SHA-256 of a file in hex, or `None` if it cannot be read.
fn sha256_file(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

/// Write `contents` to a temporary file next to `path`.
fn write_temp(path: &Path, contents: &[u8]) -> std::io::Result<PathBuf> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    Ok(tmp)
}

/// Manager for SCIP index caching.
///
/// SCIP indexes are stored in `<project>/data/` directory:
/// - `index.scip`: Binary SCIP index from verus-analyzer or rust-analyzer
/// - `index.scip.json`: JSON representation for parsing
/// - `index.scip.key.json`: the [`CacheKey`] of the index
/// - `index.scip.lock`: the lock that a `ScipCache` holds while it exists
pub struct ScipCache {
    project_path: PathBuf,
    analyzer: Analyzer,
    auto_install: bool,
    /// Resolved path to the analyzer binary (set by `resolve_analyzer`)
    analyzer_path: Option<PathBuf>,
    /// The key of the current run (set by `current_key`)
    key: Option<CacheKey>,
    /// The locked lock file (set by `lock`), released on drop
    lock: Option<std::fs::File>,
}

impl ScipCache {
    /// Create a new ScipCache for the given project using the default verus-analyzer.
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        Self::with_analyzer(project_path, Analyzer::VerusAnalyzer)
    }

    /// Create a new ScipCache with a specific analyzer choice.
    pub fn with_analyzer(project_path: impl Into<PathBuf>, analyzer: Analyzer) -> Self {
        Self {
            project_path: project_path.into(),
            analyzer,
            auto_install: false,
            analyzer_path: None,
            key: None,
            lock: None,
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

    /// Get the path of the cache key.
    pub fn key_path(&self) -> PathBuf {
        self.data_dir().join(SCIP_INDEX_KEY_FILE)
    }

    /// Path of the lock file.
    pub fn lock_path(&self) -> PathBuf {
        self.data_dir().join(SCIP_INDEX_LOCK_FILE)
    }

    /// Take the exclusive cache lock, once. It is held until `self` is
    /// dropped, so it also covers reads of the index after `state` or
    /// `generate` returns. If another run holds it, wait.
    fn lock(&mut self) -> Result<(), ScipError> {
        if self.lock.is_some() {
            return Ok(());
        }
        std::fs::create_dir_all(self.data_dir()).map_err(ScipError::CreateDirFailed)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.lock_path())
            .map_err(ScipError::LockFailed)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                eprintln!(
                    "  Waiting for another probe-verus run to release {}",
                    self.lock_path().display()
                );
                file.lock().map_err(ScipError::LockFailed)?;
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(ScipError::LockFailed(e)),
        }
        self.lock = Some(file);
        Ok(())
    }

    /// Path of the analyzer's stderr from the last index generation.
    pub fn stderr_path(&self) -> PathBuf {
        self.data_dir().join("index.scip.stderr")
    }

    /// Error lines (containing "error", any case) of the analyzer's stderr from
    /// the last index generation, at most `limit` of them.
    pub fn analyzer_errors(&self, limit: usize) -> Vec<String> {
        std::fs::read_to_string(self.stderr_path())
            .unwrap_or_default()
            .lines()
            .filter(|l| l.to_lowercase().contains("error"))
            .take(limit)
            .map(str::to_string)
            .collect()
    }

    /// Delete the cached index, so that the next run regenerates it.
    pub fn discard(&self) {
        let _ = std::fs::remove_file(self.key_path());
        let _ = std::fs::remove_file(self.scip_path());
        let _ = std::fs::remove_file(self.json_path());
    }

    /// The analyzer and its selector of the current run, e.g.
    /// `verus-analyzer 2026-09-25`. Set after [`Self::state`] or
    /// [`Self::generate`] succeeds.
    #[must_use]
    pub fn analyzer_selection(&self) -> Option<&str> {
        self.key.as_ref().map(|k| k.analyzer.as_str())
    }

    /// Whether the cached index can be reused. This takes the cache lock
    /// and resolves the analyzer
    /// (and installs it with auto-install), because the key holds the hash of
    /// its binary.
    pub fn state(&mut self) -> Result<CacheState, ScipError> {
        self.lock()?;
        let json_path = self.json_path();
        if !json_path.exists() {
            return Ok(CacheState::Missing);
        }
        let key = self.current_key()?.clone();
        let stored: Option<CacheKey> = std::fs::read_to_string(self.key_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let Some(stored) = stored else {
            return Ok(CacheState::Outdated(
                "cached index has no key, e.g. from probe-verus 8.x".into(),
            ));
        };
        let changes = stored.changes(&key);
        if !changes.is_empty() {
            return Ok(CacheState::Outdated(format!(
                "cached index is outdated: {} changed",
                changes.join(", ")
            )));
        }
        if self.cache_is_stale(&json_path) {
            return Ok(CacheState::Outdated(
                "cached index is stale: source changed".into(),
            ));
        }
        Ok(CacheState::Current)
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
        let state = self.state()?;
        if state == CacheState::Current && !regenerate {
            return Ok(self.json_path());
        }
        if verbose && !regenerate && state != CacheState::Missing {
            println!("  Regenerating SCIP index {}", state.reason());
        }
        self.generate(verbose)
    }

    /// Generate the index, its JSON and its key, and return the JSON path.
    pub fn generate(&mut self, verbose: bool) -> Result<PathBuf, ScipError> {
        self.lock()?;
        let key = self.current_key()?.clone();
        let scip_bin = tool_manager::resolve_or_install(Tool::Scip, self.auto_install)
            .map_err(|e| ScipError::ScipCliNotFound(e.to_string()))?;

        let data_dir = self.data_dir();
        std::fs::create_dir_all(&data_dir).map_err(ScipError::CreateDirFailed)?;
        // Remove the old key first: an interrupted run then leaves no key, and
        // the next run regenerates.
        let _ = std::fs::remove_file(self.key_path());

        let generated = self.generate_scip_index(&key, verbose)?;
        let json = self.convert_to_json(&scip_bin, &generated, verbose)?;

        let key_json = serde_json::to_vec_pretty(&key).expect("cache key serializes");
        let json_tmp = write_temp(&self.json_path(), &json).map_err(ScipError::WriteJsonFailed)?;
        let key_tmp =
            write_temp(&self.key_path(), &key_json).map_err(ScipError::WriteJsonFailed)?;
        std::fs::rename(&generated, self.scip_path()).map_err(ScipError::MoveFileFailed)?;
        std::fs::rename(&json_tmp, self.json_path()).map_err(ScipError::WriteJsonFailed)?;
        std::fs::rename(&key_tmp, self.key_path()).map_err(ScipError::WriteJsonFailed)?;
        if verbose {
            println!("  Saved SCIP JSON to {}", self.json_path().display());
        }
        Ok(self.json_path())
    }

    /// Whether the cached SCIP JSON is older than any `.rs` source file under the
    /// project, in which case it must be regenerated. Without this check an
    /// edited project silently reuses a stale index — atom line numbers then
    /// diverge from the current source, causing span-matching and backfill
    /// failures downstream.
    ///
    /// Conservative: if mtimes can't be read, returns `false` (keep the cache)
    /// rather than forcing an expensive regeneration on a metadata hiccup. The
    /// project-root `data/`, `target/`, and `.git/` directories are skipped (only
    /// at the root — a nested `src/data/` with real sources is still scanned).
    /// Skipping `.git/` avoids walking a potentially huge object store that can
    /// never hold a `.rs` source.
    fn cache_is_stale(&self, json_path: &Path) -> bool {
        let Ok(cached_mtime) = std::fs::metadata(json_path).and_then(|m| m.modified()) else {
            return false;
        };
        let skip_data = self.project_path.join(DATA_DIR);
        let skip_target = self.project_path.join("target");
        let skip_git = self.project_path.join(".git");
        for entry in walkdir::WalkDir::new(&self.project_path)
            .into_iter()
            .filter_entry(|e| {
                e.path() != skip_data && e.path() != skip_target && e.path() != skip_git
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

    /// Resolve the analyzer once: for verus-analyzer, the release selected for
    /// this project ([`analyzer_selection::select`]); for rust-analyzer, the
    /// managed directory, then `PATH`. Returns the binary and the selector.
    fn resolve_analyzer(&mut self) -> Result<(PathBuf, String), ScipError> {
        let not_found = |e: String| ScipError::AnalyzerNotFound(self.analyzer, e);
        let (path, selector) = match self.analyzer {
            Analyzer::VerusAnalyzer => {
                let selection = analyzer_selection::select(&self.project_path)
                    .map_err(ScipError::BadSelection)?;
                let path = match &self.analyzer_path {
                    Some(p) => p.clone(),
                    None => tool_manager::resolve_analyzer(&selection, self.auto_install)
                        .map_err(|e| not_found(e.to_string()))?,
                };
                (path, format!("{} {}", self.analyzer, selection.selector))
            }
            Analyzer::RustAnalyzer => {
                let path = match &self.analyzer_path {
                    Some(p) => p.clone(),
                    None => tool_manager::resolve_or_install(Tool::RustAnalyzer, self.auto_install)
                        .map_err(|e| not_found(e.to_string()))?,
                };
                (path, self.analyzer.to_string())
            }
        };
        self.analyzer_path = Some(path.clone());
        Ok((path, selector))
    }

    /// The key of the current run, computed once.
    fn current_key(&mut self) -> Result<&CacheKey, ScipError> {
        if self.key.is_none() {
            let (path, analyzer) = self.resolve_analyzer()?;
            let rustc = Command::new("rustc")
                .arg("-vV")
                .current_dir(&self.project_path)
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let package_manifest = self.project_path.join("Cargo.toml");
            let workspace_manifest = analyzer_selection::workspace_manifest(&self.project_path);
            let lockfile = workspace_manifest
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(&self.project_path)
                .join("Cargo.lock");
            let manifests = [Some(package_manifest), workspace_manifest, Some(lockfile)]
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let hash = sha256_file(&p)?;
                    // Canonical, so that `.` and an absolute project path give
                    // the same key.
                    let p = std::fs::canonicalize(&p).unwrap_or(p);
                    Some((p.display().to_string(), hash))
                })
                .collect();
            self.key = Some(CacheKey {
                analyzer,
                analyzer_sha256: sha256_file(&path).unwrap_or_default(),
                analyzer_path: path.display().to_string(),
                rustc,
                manifests,
            });
        }
        Ok(self.key.as_ref().expect("key is set"))
    }

    /// Run the analyzer. Returns the path of the index it wrote, in the
    /// project directory.
    fn generate_scip_index(&self, key: &CacheKey, verbose: bool) -> Result<PathBuf, ScipError> {
        let analyzer_bin = Path::new(&key.analyzer_path);

        if verbose {
            println!(
                "Generating SCIP index for {} (using {} at {})...",
                self.project_path.display(),
                key.analyzer,
                key.analyzer_path
            );
        }

        if self.analyzer == Analyzer::VerusAnalyzer {
            for warning in analyzer_warnings(
                &command_first_line(analyzer_bin, &["--version"], &self.project_path),
                key.rustc.lines().next().unwrap_or_default(),
            ) {
                eprintln!("  Warning: {warning}");
            }
        }

        // verus-analyzer (2026-08-22 and later) enables `verus_keep_ghost` itself,
        // so `#[cfg(verus_keep_ghost)]` items are indexed without extra config.
        // stderr is kept in a file, so that a failed index check can show it.
        let status = Command::new(analyzer_bin)
            .args(["scip", "."])
            .current_dir(&self.project_path)
            .stdout(if verbose {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                let stderr = child.stderr.take().expect("stderr is piped");
                let mut log = String::new();
                for line in BufReader::new(stderr).lines() {
                    let line = line?;
                    if verbose {
                        eprintln!("{line}");
                    }
                    log.push_str(&line);
                    log.push('\n');
                }
                // A missing log only loses diagnostics, so a write error is ignored.
                let _ = std::fs::write(self.stderr_path(), &log);
                child.wait()
            });

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return Err(ScipError::AnalyzerFailed(self.analyzer, s.to_string()));
            }
            Err(e) => {
                return Err(ScipError::AnalyzerFailed(self.analyzer, e.to_string()));
            }
        }

        let generated_path = self.project_path.join("index.scip");
        if !generated_path.exists() {
            return Err(ScipError::IndexNotGenerated(self.analyzer));
        }
        Ok(generated_path)
    }

    /// Convert the SCIP index at `index` to JSON.
    fn convert_to_json(
        &self,
        scip_bin: &Path,
        index: &Path,
        verbose: bool,
    ) -> Result<Vec<u8>, ScipError> {
        if verbose {
            println!("Converting index.scip to JSON...");
        }

        let output = Command::new(scip_bin)
            .args(["print", "--json", index.to_string_lossy().as_ref()])
            .output();

        match output {
            Ok(o) if o.status.success() => Ok(o.stdout),
            Ok(o) => Err(ScipError::ScipPrintFailed(o.status.to_string())),
            Err(e) => Err(ScipError::ScipPrintFailed(e.to_string())),
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

    /// Set a file's mtime deterministically (avoids sleep-based test flakiness
    /// on coarse-granularity filesystems).
    fn set_mtime(path: &Path, secs: u64) {
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn test_cache_is_stale_on_source_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        let json = root.join("data/index.scip.json");
        std::fs::write(&json, "{}").unwrap();
        let cache = ScipCache::new(root);

        // Cache newer than every source file → not stale.
        set_mtime(&root.join("src/lib.rs"), 1_000);
        set_mtime(&json, 2_000);
        assert!(
            !cache.cache_is_stale(&json),
            "cache newer than every source file must not be stale"
        );

        // A source file newer than the cache → stale.
        set_mtime(&root.join("src/lib.rs"), 3_000);
        assert!(
            cache.cache_is_stale(&json),
            "a source file newer than the cache must mark it stale"
        );
    }

    #[test]
    fn test_cache_not_stale_when_only_root_data_dir_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        let json = root.join("data/index.scip.json");
        std::fs::write(&json, "{}").unwrap();
        std::fs::write(root.join("data/generated.rs"), "fn g() {}").unwrap();

        let cache = ScipCache::new(root);
        set_mtime(&root.join("src/lib.rs"), 1_000);
        set_mtime(&json, 2_000);
        // A newer .rs under the root data/ (a generated artifact) is ignored.
        set_mtime(&root.join("data/generated.rs"), 3_000);
        assert!(
            !cache.cache_is_stale(&json),
            "changes under the root data/ must not invalidate the cache"
        );
    }

    #[test]
    fn test_cache_stale_for_nested_data_dir_sources() {
        // A nested `src/data/` holds real sources and must still be scanned
        // (only the project-root data/ is skipped).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("src/data")).unwrap();
        std::fs::write(root.join("src/data/real.rs"), "fn r() {}").unwrap();
        let json = root.join("data/index.scip.json");
        std::fs::write(&json, "{}").unwrap();

        let cache = ScipCache::new(root);
        set_mtime(&json, 2_000);
        set_mtime(&root.join("src/data/real.rs"), 3_000);
        assert!(
            cache.cache_is_stale(&json),
            "a newer source under a nested src/data/ must mark the cache stale"
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

    fn key() -> CacheKey {
        CacheKey {
            analyzer: "verus-analyzer 2026-09-25".into(),
            analyzer_path: "/a".into(),
            analyzer_sha256: "00".into(),
            rustc: "rustc 1.93.0".into(),
            manifests: BTreeMap::from([("Cargo.toml".into(), "11".into())]),
        }
    }

    #[test]
    fn test_cache_key_changes() {
        let base = key();
        assert!(base.changes(&key()).is_empty());
        let mut other = key();
        other.analyzer = "verus-analyzer path".into();
        other.analyzer_sha256 = "01".into();
        assert_eq!(
            base.changes(&other),
            vec!["analyzer selection", "analyzer binary"]
        );
        let mut other = key();
        other.rustc = "rustc 1.92.0".into();
        other.manifests.insert("Cargo.lock".into(), "22".into());
        assert_eq!(
            base.changes(&other),
            vec!["rustc", "Cargo.toml or Cargo.lock"]
        );
    }

    /// A cache whose analyzer is already resolved to `analyzer`, so that
    /// `state` needs no installed verus-analyzer.
    fn cache_with_binary(root: &Path, analyzer: &Path) -> ScipCache {
        let mut cache = ScipCache::with_analyzer(root, Analyzer::RustAnalyzer);
        cache.analyzer_path = Some(analyzer.to_path_buf());
        cache
    }

    #[test]
    fn test_cache_state_follows_key() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        let bin = root.join("analyzer-bin");
        std::fs::write(&bin, "v1").unwrap();

        let mut cache = cache_with_binary(root, &bin);
        assert_eq!(cache.state().unwrap(), CacheState::Missing);

        // An index without a key (8.x) is outdated.
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(cache.json_path(), "{}").unwrap();
        assert!(matches!(cache.state().unwrap(), CacheState::Outdated(r) if r.contains("no key")));

        // With the current key, it is current.
        let current = cache.current_key().unwrap().clone();
        std::fs::write(cache.key_path(), serde_json::to_vec(&current).unwrap()).unwrap();
        assert_eq!(cache.state().unwrap(), CacheState::Current);

        // A new analyzer binary or manifest outdates it.
        std::fs::write(&bin, "v2").unwrap();
        // Each cache holds the lock until it is dropped.
        drop(cache);
        let mut cache = cache_with_binary(root, &bin);
        assert!(
            matches!(cache.state().unwrap(), CacheState::Outdated(r) if r.contains("analyzer binary"))
        );
        std::fs::write(&bin, "v1").unwrap();
        std::fs::write(root.join("Cargo.lock"), "lock").unwrap();
        drop(cache);
        let mut cache = cache_with_binary(root, &bin);
        assert!(
            matches!(cache.state().unwrap(), CacheState::Outdated(r) if r.contains("Cargo.lock"))
        );

        // discard removes the key too.
        cache.discard();
        assert!(!cache.key_path().exists());
    }

    #[test]
    fn test_cache_lock_is_exclusive_until_drop() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        let bin = root.join("analyzer-bin");
        std::fs::write(&bin, "v1").unwrap();

        let mut cache = cache_with_binary(root, &bin);
        cache.state().unwrap();
        let other = std::fs::File::open(cache.lock_path()).unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(cache);
        other.try_lock().unwrap();
    }

    #[test]
    fn test_cache_key_ignores_path_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        std::fs::write(root.join("Cargo.lock"), "lock").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let bin = root.join("analyzer-bin");
        std::fs::write(&bin, "v1").unwrap();

        let plain = cache_with_binary(root, &bin).current_key().unwrap().clone();
        let other = cache_with_binary(&root.join("sub/.."), &bin)
            .current_key()
            .unwrap()
            .clone();
        assert_eq!(plain.manifests.len(), 2);
        assert_eq!(plain.manifests, other.manifests);
    }

    #[test]
    fn test_analyzer_warnings() {
        let old = analyzer_warnings("rust-analyzer 0.3.264-standalone", "rustc 1.93.0 (x)");
        assert!(old[0].contains("will be rejected"), "{old:?}");
        let slow = analyzer_warnings("verus-analyzer 0.3.269-standalone", "rustc 1.92.0 (x)");
        assert!(slow[0].contains("10x slower"), "{slow:?}");
        assert!(
            analyzer_warnings("verus-analyzer 0.3.269-standalone", "rustc 1.93.0 (x)").is_empty()
        );
        assert!(analyzer_warnings("", "").is_empty());
    }
}
