//! Tool manager for auto-downloading external dependencies.
//!
//! Manages verus-analyzer, scip, and verus binaries: resolves their location
//! (managed directory, then PATH), and downloads them on demand.
//!
//! Version resolution order:
//! 1. Environment variable override (`PROBE_VERUS_ANALYZER_VERSION`, `PROBE_SCIP_VERSION`,
//!    `PROBE_VERUS_VERSION`)
//! 2. For Verus: project Cargo.toml detection (via `detect_verus_version`)
//! 3. Latest stable release from GitHub API
//! 4. Known-good fallback version (compiled into the binary)

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::scip_cache::Analyzer;

// ---------------------------------------------------------------------------
// Known-good fallback versions (used when API is unreachable)
// ---------------------------------------------------------------------------

const VERUS_ANALYZER_FALLBACK_VERSION: &str = "2026-02-03";
const SCIP_FALLBACK_VERSION: &str = "v0.6.1";
const VERUS_FALLBACK_VERSION: &str = "0.2026.03.22.5e66329";

// ---------------------------------------------------------------------------
// Environment variable names for version overrides
// ---------------------------------------------------------------------------

const VERUS_ANALYZER_VERSION_ENV: &str = "PROBE_VERUS_ANALYZER_VERSION";
const SCIP_VERSION_ENV: &str = "PROBE_SCIP_VERSION";
pub const VERUS_VERSION_ENV: &str = "PROBE_VERUS_VERSION";

// ---------------------------------------------------------------------------
// GitHub repos
// ---------------------------------------------------------------------------

const VERUS_ANALYZER_REPO: &str = "verus-lang/verus-analyzer";
const SCIP_REPO: &str = "sourcegraph/scip";
const VERUS_REPO: &str = "verus-lang/verus";

// ---------------------------------------------------------------------------
// Tool enum
// ---------------------------------------------------------------------------

/// An external tool that probe-verus can manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    VerusAnalyzer,
    RustAnalyzer,
    Scip,
    Verus,
}

impl Tool {
    pub fn binary_name(&self) -> &'static str {
        match self {
            Tool::VerusAnalyzer => "verus-analyzer",
            Tool::RustAnalyzer => "rust-analyzer",
            Tool::Scip => "scip",
            Tool::Verus => "cargo-verus",
        }
    }

    /// The filename stored inside `~/.probe-verus/tools/`.
    /// For Verus this is a directory prefix (versioned dirs like `verus-{version}/`).
    fn managed_filename(&self) -> &'static str {
        match self {
            Tool::VerusAnalyzer => "verus-analyzer",
            Tool::RustAnalyzer => "rust-analyzer",
            Tool::Scip => "scip",
            Tool::Verus => "verus",
        }
    }

    fn fallback_version(&self) -> &'static str {
        match self {
            Tool::VerusAnalyzer | Tool::RustAnalyzer => VERUS_ANALYZER_FALLBACK_VERSION,
            Tool::Scip => SCIP_FALLBACK_VERSION,
            Tool::Verus => VERUS_FALLBACK_VERSION,
        }
    }

    fn version_env_var(&self) -> &'static str {
        match self {
            Tool::VerusAnalyzer | Tool::RustAnalyzer => VERUS_ANALYZER_VERSION_ENV,
            Tool::Scip => SCIP_VERSION_ENV,
            Tool::Verus => VERUS_VERSION_ENV,
        }
    }

    fn github_repo(&self) -> &'static str {
        match self {
            Tool::VerusAnalyzer | Tool::RustAnalyzer => VERUS_ANALYZER_REPO,
            Tool::Scip => SCIP_REPO,
            Tool::Verus => VERUS_REPO,
        }
    }

    pub fn from_analyzer(a: Analyzer) -> Self {
        match a {
            Analyzer::VerusAnalyzer => Tool::VerusAnalyzer,
            Analyzer::RustAnalyzer => Tool::RustAnalyzer,
        }
    }
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tool::Verus => f.write_str("verus"),
            _ => f.write_str(self.binary_name()),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ToolError {
    PlatformNotSupported(Tool, String),
    DownloadFailed(Tool, String),
    DecompressFailed(Tool, String),
    IoError(Tool, io::Error),
    NotInstalled(Tool),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::PlatformNotSupported(tool, detail) => {
                write!(
                    f,
                    "{tool}: platform not supported ({detail}). See https://github.com/{}/releases for available platforms.",
                    tool.github_repo()
                )
            }
            ToolError::DownloadFailed(tool, msg) => {
                write!(f, "{tool}: download failed: {msg}")
            }
            ToolError::DecompressFailed(tool, msg) => {
                write!(f, "{tool}: decompression failed: {msg}")
            }
            ToolError::IoError(tool, e) => {
                write!(f, "{tool}: I/O error: {e}")
            }
            ToolError::NotInstalled(tool) => match tool {
                Tool::RustAnalyzer => write!(
                    f,
                    "rust-analyzer not found. Install it with: rustup component add rust-analyzer"
                ),
                _ => write!(
                    f,
                    "{tool} not found. Install it with: probe-verus setup\n\
                     Or download manually: {}",
                    download_url(tool).unwrap_or_else(|_| "see upstream releases".into())
                ),
            },
        }
    }
}

impl std::error::Error for ToolError {}

// ---------------------------------------------------------------------------
// Managed tools directory
// ---------------------------------------------------------------------------

/// `~/.probe-verus/tools`
pub fn tools_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".probe-verus").join("tools"))
}

fn managed_path(tool: &Tool) -> Option<PathBuf> {
    match tool {
        Tool::Verus => managed_verus_binary(),
        _ => tools_dir().map(|d| d.join(tool.managed_filename())),
    }
}

/// Find the most recently installed managed Verus binary (`cargo-verus`).
/// Scans `~/.probe-verus/tools/verus-*/cargo-verus` and returns the path if found.
fn managed_verus_binary() -> Option<PathBuf> {
    let dir = tools_dir()?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("verus-") && entry.path().is_dir() {
                let cargo_verus = entry.path().join("cargo-verus");
                if cargo_verus.exists() {
                    candidates.push(cargo_verus);
                }
            }
        }
    }
    // Sort ascending; pop() returns the lexicographically highest (newest) version
    candidates.sort();
    candidates.pop()
}

/// Return the versioned Verus directory for a specific version.
fn verus_version_dir(version: &str) -> Option<PathBuf> {
    tools_dir().map(|d| d.join(format!("verus-{version}")))
}

/// Return the `cargo-verus` binary path for a specific version.
pub fn verus_binary_for_version(version: &str) -> Option<PathBuf> {
    verus_version_dir(version).map(|d| d.join("cargo-verus"))
}

// ---------------------------------------------------------------------------
// Platform mapping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub os: &'static str,
    pub arch: &'static str,
}

pub fn current_platform() -> PlatformInfo {
    PlatformInfo {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    }
}

/// Map (os, arch) -> the verus-analyzer asset target triple.
fn verus_analyzer_target(p: &PlatformInfo) -> Result<&'static str, String> {
    match (p.os, p.arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        _ => Err(format!("{}-{}", p.os, p.arch)),
    }
}

/// Map (os, arch) -> the scip asset (os, arch) pair.
fn scip_target(p: &PlatformInfo) -> Result<(&'static str, &'static str), String> {
    match (p.os, p.arch) {
        ("linux", "x86_64") => Ok(("linux", "amd64")),
        ("linux", "aarch64") => Ok(("linux", "arm64")),
        ("macos", "x86_64") => Ok(("darwin", "amd64")),
        ("macos", "aarch64") => Ok(("darwin", "arm64")),
        _ => Err(format!("{}-{} (scip has no Windows binary)", p.os, p.arch)),
    }
}

/// Map (os, arch) -> Verus release asset target suffix.
/// Verus assets: `verus-{version}-x86-linux.zip`, `verus-{version}-arm64-macos.zip`, etc.
fn verus_target(p: &PlatformInfo) -> Result<&'static str, String> {
    match (p.os, p.arch) {
        ("linux", "x86_64") => Ok("x86-linux"),
        ("macos", "x86_64") => Ok("x86-macos"),
        ("macos", "aarch64") => Ok("arm64-macos"),
        ("windows", "x86_64") => Ok("x86-win"),
        _ => Err(format!(
            "{}-{} (Verus has no prebuilt binary for this platform)",
            p.os, p.arch
        )),
    }
}

// ---------------------------------------------------------------------------
// Version resolution: env var → GitHub API latest → fallback
// ---------------------------------------------------------------------------

/// Describes how a version was determined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSource {
    EnvVar,
    CargoToml,
    GitHubLatest,
    Fallback,
}

impl std::fmt::Display for VersionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionSource::EnvVar => write!(f, "env"),
            VersionSource::CargoToml => write!(f, "Cargo.toml"),
            VersionSource::GitHubLatest => write!(f, "latest"),
            VersionSource::Fallback => write!(f, "fallback"),
        }
    }
}

/// A resolved version tag with its source.
#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    pub tag: String,
    pub source: VersionSource,
}

/// Resolve the version to install for a tool.
///
/// 1. If the env var is set (e.g. `PROBE_VERUS_ANALYZER_VERSION=2026-01-01`), use that.
/// 2. Otherwise, query GitHub `/releases/latest` for the newest stable tag.
/// 3. If the API call fails (offline, rate-limited), fall back to the compiled-in version.
pub fn resolve_version(tool: &Tool) -> ResolvedVersion {
    // 1. Env var override
    if let Ok(v) = std::env::var(tool.version_env_var()) {
        if !v.is_empty() {
            return ResolvedVersion {
                tag: v,
                source: VersionSource::EnvVar,
            };
        }
    }

    // 2. GitHub API latest
    if let Some(tag) = fetch_latest_release_tag(tool.github_repo()) {
        return ResolvedVersion {
            tag,
            source: VersionSource::GitHubLatest,
        };
    }

    // 3. Fallback
    ResolvedVersion {
        tag: tool.fallback_version().to_string(),
        source: VersionSource::Fallback,
    }
}

/// Fetch the tag_name of the latest release from GitHub.
/// Returns `None` on any error (network, rate limit, parse failure).
fn fetch_latest_release_tag(repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = github_api_get(&url).ok()?;
    let body_str = response.into_string().ok()?;
    let body: serde_json::Value = serde_json::from_str(&body_str).ok()?;
    body.get("tag_name")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Make a GitHub API GET request, using `GITHUB_TOKEN` env var for auth if available.
#[allow(clippy::result_large_err)]
fn github_api_get(url: &str) -> Result<ureq::Response, ureq::Error> {
    let mut req = ureq::get(url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "probe-verus");

    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
    }

    req.call()
}

/// Match a git commit rev to a Verus release tag by scanning GitHub releases.
/// Verus release tags end with the short commit hash (e.g., `release/0.2026.03.22.5e66329`).
/// Paginates up to 5 pages of 100 releases.
pub fn fetch_verus_release_by_rev(rev: &str) -> Option<String> {
    let short_rev = if rev.len() >= 7 { &rev[..7] } else { rev };
    let repo = VERUS_REPO;

    for page in 1..=5 {
        let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100&page={page}");
        let response = github_api_get(&url).ok()?;
        let body_str = response.into_string().ok()?;
        let releases: Vec<serde_json::Value> = serde_json::from_str(&body_str).ok()?;

        if releases.is_empty() {
            break;
        }

        for release in &releases {
            let prerelease = release
                .get("prerelease")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if prerelease {
                continue;
            }
            if let Some(tag) = release.get("tag_name").and_then(|v| v.as_str()) {
                if tag.ends_with(short_rev) {
                    // Strip the `release/` prefix to get the version
                    let version = tag.strip_prefix("release/").unwrap_or(tag);
                    return Some(version.to_string());
                }
            }
        }

        if releases.len() < 100 {
            break;
        }
    }

    None
}

/// Resolve the Verus version using project-aware detection.
///
/// Resolution order:
/// 1. `PROBE_VERUS_VERSION` env var
/// 2. Project Cargo.toml detection (if `project_path` is provided)
/// 3. GitHub `/releases/latest`
/// 4. Compiled-in fallback
pub fn resolve_verus_version(project_path: Option<&Path>) -> ResolvedVersion {
    // 1. Env var override
    if let Ok(v) = std::env::var(VERUS_VERSION_ENV) {
        if !v.is_empty() {
            return ResolvedVersion {
                tag: v,
                source: VersionSource::EnvVar,
            };
        }
    }

    // 2. Project Cargo.toml detection
    if let Some(path) = project_path {
        if let Some(version) = crate::metadata::detect_verus_version(path) {
            return ResolvedVersion {
                tag: version,
                source: VersionSource::CargoToml,
            };
        }
    }

    // 3. GitHub API latest
    if let Some(tag) = fetch_latest_release_tag(VERUS_REPO) {
        // Strip `release/` prefix from Verus tags
        let version = tag.strip_prefix("release/").unwrap_or(&tag).to_string();
        return ResolvedVersion {
            tag: version,
            source: VersionSource::GitHubLatest,
        };
    }

    // 4. Fallback
    ResolvedVersion {
        tag: VERUS_FALLBACK_VERSION.to_string(),
        source: VersionSource::Fallback,
    }
}

// ---------------------------------------------------------------------------
// Download URL construction
// ---------------------------------------------------------------------------

/// Build the download URL for a tool using a specific version tag.
pub fn download_url_with_version(
    tool: &Tool,
    version: &str,
    platform: &PlatformInfo,
) -> Result<String, ToolError> {
    match tool {
        Tool::VerusAnalyzer => {
            let target = verus_analyzer_target(platform)
                .map_err(|d| ToolError::PlatformNotSupported(*tool, d))?;
            let ext = if platform.os == "windows" {
                "zip"
            } else {
                "gz"
            };
            Ok(format!(
                "https://github.com/{}/releases/download/{version}/verus-analyzer-{target}.{ext}",
                tool.github_repo()
            ))
        }
        Tool::RustAnalyzer => Err(ToolError::PlatformNotSupported(
            *tool,
            "rust-analyzer should be installed via rustup: `rustup component add rust-analyzer`"
                .into(),
        )),
        Tool::Scip => {
            let (os, arch) =
                scip_target(platform).map_err(|d| ToolError::PlatformNotSupported(*tool, d))?;
            Ok(format!(
                "https://github.com/{}/releases/download/{version}/scip-{os}-{arch}.tar.gz",
                tool.github_repo()
            ))
        }
        Tool::Verus => {
            let target =
                verus_target(platform).map_err(|d| ToolError::PlatformNotSupported(*tool, d))?;
            // Release tags use `release/0.2026.03.22.xxx`; the slash must be URL-encoded.
            Ok(format!(
                "https://github.com/{}/releases/download/release%2F{version}/verus-{version}-{target}.zip",
                tool.github_repo()
            ))
        }
    }
}

/// Convenience: build the download URL for the current platform using resolved version.
pub fn download_url(tool: &Tool) -> Result<String, ToolError> {
    let version = resolve_version(tool);
    download_url_with_version(tool, &version.tag, &current_platform())
}

// ---------------------------------------------------------------------------
// Resolution: managed dir → PATH → not found
// ---------------------------------------------------------------------------

/// Resolve a tool to an absolute path. Checks managed dir first, then PATH.
/// Returns `Err(ToolError::NotInstalled)` if not found anywhere.
pub fn resolve_tool(tool: Tool) -> Result<PathBuf, ToolError> {
    // 1. Check managed directory
    if let Some(p) = managed_path(&tool) {
        if p.exists() {
            return Ok(p);
        }
    }

    // 2. Check PATH
    if let Some(p) = find_in_path(tool.binary_name()) {
        return Ok(p);
    }

    Err(ToolError::NotInstalled(tool))
}

/// Resolve, or auto-download if `auto_install` is true.
pub fn resolve_or_install(tool: Tool, auto_install: bool) -> Result<PathBuf, ToolError> {
    match resolve_tool(tool) {
        Ok(p) => Ok(p),
        Err(ToolError::NotInstalled(_)) if auto_install => {
            eprintln!("{tool} not found, downloading...");
            download_tool(tool)?;
            resolve_tool(tool)
        }
        Err(e) => Err(e),
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    // Use `which` on unix, `where` on windows
    let cmd = if cfg!(windows) { "where" } else { "which" };
    Command::new(cmd)
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            let line = s.lines().next()?.trim().to_string();
            if line.is_empty() {
                None
            } else {
                Some(PathBuf::from(line))
            }
        })
}

// ---------------------------------------------------------------------------
// Download + decompress
// ---------------------------------------------------------------------------

/// Download and install a tool into the managed directory.
pub fn download_tool(tool: Tool) -> Result<PathBuf, ToolError> {
    let resolved = match tool {
        Tool::Verus => resolve_verus_version(None),
        _ => resolve_version(&tool),
    };
    download_tool_version(tool, &resolved)
}

/// Download and install a specific version of a tool.
pub fn download_tool_version(tool: Tool, resolved: &ResolvedVersion) -> Result<PathBuf, ToolError> {
    let platform = current_platform();
    let url = download_url_with_version(&tool, &resolved.tag, &platform)?;

    let dest_dir = tools_dir().ok_or_else(|| {
        ToolError::IoError(
            tool,
            io::Error::new(io::ErrorKind::NotFound, "cannot determine home directory"),
        )
    })?;
    fs::create_dir_all(&dest_dir).map_err(|e| ToolError::IoError(tool, e))?;

    eprintln!(
        "Downloading {tool} {} ({}) from:",
        resolved.tag, resolved.source
    );
    eprintln!("  {url}");

    let response = ureq::get(&url)
        .call()
        .map_err(|e| ToolError::DownloadFailed(tool, e.to_string()))?;

    let mut compressed_bytes: Vec<u8> = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut compressed_bytes)
        .map_err(|e| ToolError::DownloadFailed(tool, e.to_string()))?;

    let dest_path = match tool {
        Tool::Verus => {
            let verus_dir = verus_version_dir(&resolved.tag).ok_or_else(|| {
                ToolError::IoError(
                    tool,
                    io::Error::new(io::ErrorKind::NotFound, "cannot determine home directory"),
                )
            })?;
            extract_verus_zip(&compressed_bytes, &verus_dir, tool)?;
            let cargo_verus = verus_dir.join("cargo-verus");
            if !cargo_verus.exists() {
                return Err(ToolError::DecompressFailed(
                    tool,
                    format!(
                        "cargo-verus not found in extracted archive at {}",
                        verus_dir.display()
                    ),
                ));
            }
            eprintln!(
                "Installed {tool} {} to {}",
                resolved.tag,
                verus_dir.display()
            );
            return Ok(cargo_verus);
        }
        Tool::VerusAnalyzer if platform.os == "windows" => {
            let dest_path = dest_dir.join(tool.managed_filename());
            extract_zip(&compressed_bytes, &dest_path, tool)?;
            dest_path
        }
        Tool::VerusAnalyzer => {
            let dest_path = dest_dir.join(tool.managed_filename());
            decompress_gzip(&compressed_bytes, &dest_path, tool)?;
            dest_path
        }
        Tool::Scip => {
            let dest_path = dest_dir.join(tool.managed_filename());
            extract_tar_gz(&compressed_bytes, &dest_path, tool)?;
            dest_path
        }
        Tool::RustAnalyzer => {
            return Err(ToolError::PlatformNotSupported(
                tool,
                "use `rustup component add rust-analyzer`".into(),
            ));
        }
    };

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&dest_path, perms).map_err(|e| ToolError::IoError(tool, e))?;
    }

    eprintln!("Installed {tool} to {}", dest_path.display());
    Ok(dest_path)
}

/// Decompress a .gz file (single file, not tar).
fn decompress_gzip(data: &[u8], dest: &Path, tool: Tool) -> Result<(), ToolError> {
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(data);
    let mut out = fs::File::create(dest).map_err(|e| ToolError::IoError(tool, e))?;
    io::copy(&mut decoder, &mut out)
        .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;
    Ok(())
}

/// Extract a .tar.gz and pull out the tool binary.
fn extract_tar_gz(data: &[u8], dest: &Path, tool: Tool) -> Result<(), ToolError> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let decoder = GzDecoder::new(data);
    let mut archive = Archive::new(decoder);

    let binary_name = tool.binary_name();
    let mut found = false;

    for entry_result in archive
        .entries()
        .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?
    {
        let mut entry =
            entry_result.map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name == binary_name {
            let mut out = fs::File::create(dest).map_err(|e| ToolError::IoError(tool, e))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;
            found = true;
            break;
        }
    }

    if !found {
        return Err(ToolError::DecompressFailed(
            tool,
            format!("binary '{binary_name}' not found in archive"),
        ));
    }

    Ok(())
}

/// Extract a .zip archive and pull out the tool binary (used for Windows assets).
fn extract_zip(data: &[u8], dest: &Path, tool: Tool) -> Result<(), ToolError> {
    use std::io::Cursor;

    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;

    let binary_name = tool.binary_name();
    let exe_name = format!("{binary_name}.exe");

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;

        let name = file
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));

        if let Some(name) = name {
            if name == binary_name || name == exe_name {
                let mut out = fs::File::create(dest).map_err(|e| ToolError::IoError(tool, e))?;
                io::copy(&mut file, &mut out)
                    .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;
                return Ok(());
            }
        }
    }

    Err(ToolError::DecompressFailed(
        tool,
        format!("binary '{binary_name}' not found in zip archive"),
    ))
}

/// Extract a Verus release zip into a versioned directory.
///
/// Verus zips contain a top-level directory (e.g., `verus-x86-linux/`) with
/// `verus`, `cargo-verus`, libraries, etc. We extract all contents into `dest_dir`.
fn extract_verus_zip(data: &[u8], dest_dir: &Path, tool: Tool) -> Result<(), ToolError> {
    use std::io::Cursor;

    fs::create_dir_all(dest_dir).map_err(|e| ToolError::IoError(tool, e))?;

    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;

        let enclosed = match file.enclosed_name() {
            Some(p) => p.to_owned(),
            None => continue,
        };

        // Strip the top-level directory (e.g., `verus-x86-linux/cargo-verus` -> `cargo-verus`)
        let relative: PathBuf = enclosed.components().skip(1).collect();

        if relative.as_os_str().is_empty() {
            continue;
        }

        let dest_path = dest_dir.join(&relative);

        if file.is_dir() {
            fs::create_dir_all(&dest_path).map_err(|e| ToolError::IoError(tool, e))?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|e| ToolError::IoError(tool, e))?;
            }
            let mut out = fs::File::create(&dest_path).map_err(|e| ToolError::IoError(tool, e))?;
            io::copy(&mut file, &mut out)
                .map_err(|e| ToolError::DecompressFailed(tool, e.to_string()))?;

            // Make binaries executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = file.unix_mode() {
                    let perms = fs::Permissions::from_mode(mode);
                    let _ = fs::set_permissions(&dest_path, perms);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Status reporting
// ---------------------------------------------------------------------------

/// Information about a single tool's installation state.
#[derive(Debug)]
pub struct ToolStatus {
    pub tool: Tool,
    pub managed_path: Option<PathBuf>,
    pub path_location: Option<PathBuf>,
    pub install_version: ResolvedVersion,
}

impl ToolStatus {
    pub fn is_available(&self) -> bool {
        self.managed_path.is_some() || self.path_location.is_some()
    }

    pub fn resolved_path(&self) -> Option<&PathBuf> {
        self.managed_path.as_ref().or(self.path_location.as_ref())
    }
}

pub fn tool_status(tool: Tool) -> ToolStatus {
    let mp = managed_path(&tool).filter(|p| p.exists());
    let pp = find_in_path(tool.binary_name());
    let install_version = resolve_version(&tool);
    ToolStatus {
        tool,
        managed_path: mp,
        path_location: pp,
        install_version,
    }
}

/// Print a human-readable status table for all managed tools.
pub fn print_status() {
    let tools = [Tool::VerusAnalyzer, Tool::Scip, Tool::Verus];
    let dir_display = tools_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "<unknown>".into());

    println!("Managed tools directory: {dir_display}\n");
    println!(
        "{:<20} {:<18} {:<10} Location",
        "Tool", "Install version", "Status"
    );
    println!("{}", "-".repeat(78));

    for tool in &tools {
        let st = tool_status(*tool);
        let status = if st.managed_path.is_some() {
            "managed"
        } else if st.path_location.is_some() {
            "PATH"
        } else {
            "missing"
        };
        let location = st
            .resolved_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".into());
        let version_display = format!("{} ({})", st.install_version.tag, st.install_version.source);
        println!(
            "{:<20} {:<18} {:<10} {}",
            tool, version_display, status, location
        );
    }

    println!();
    println!("Override versions with environment variables:");
    println!("  {VERUS_ANALYZER_VERSION_ENV}=<tag>  (e.g. 2026-02-03)");
    println!("  {SCIP_VERSION_ENV}=<tag>             (e.g. v0.6.1)");
    println!("  {VERUS_VERSION_ENV}=<tag>            (e.g. {VERUS_FALLBACK_VERSION})");
}

/// Install all manageable tools. Returns a list of errors for tools that failed.
/// Tools already available (managed or on PATH) are skipped.
/// Tools unsupported on the current platform are reported but not treated as errors.
pub fn install_all() -> Vec<ToolError> {
    let tools = [Tool::VerusAnalyzer, Tool::Scip, Tool::Verus];
    let platform = current_platform();
    let mut errors = Vec::new();

    for tool in &tools {
        if resolve_tool(*tool).is_ok() {
            eprintln!("{tool}: already available, skipping download.");
            continue;
        }

        let supported = match tool {
            Tool::VerusAnalyzer => verus_analyzer_target(&platform).is_ok(),
            Tool::Scip => scip_target(&platform).is_ok(),
            Tool::Verus => verus_target(&platform).is_ok(),
            Tool::RustAnalyzer => false,
        };

        if !supported {
            eprintln!(
                "{tool}: not available for {}-{}, skipping. See https://github.com/{}/releases",
                platform.os,
                platform.arch,
                tool.github_repo()
            );
            continue;
        }

        if let Err(e) = download_tool(*tool) {
            errors.push(e);
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Guards tests that mutate process-wide environment variables.
    /// Rust tests run in parallel; without this, `set_var`/`remove_var` calls race.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_platform_mapping_verus_analyzer() {
        let linux_x86 = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        assert_eq!(
            verus_analyzer_target(&linux_x86).unwrap(),
            "x86_64-unknown-linux-gnu"
        );

        let mac_arm = PlatformInfo {
            os: "macos",
            arch: "aarch64",
        };
        assert_eq!(
            verus_analyzer_target(&mac_arm).unwrap(),
            "aarch64-apple-darwin"
        );

        let win_x86 = PlatformInfo {
            os: "windows",
            arch: "x86_64",
        };
        assert_eq!(
            verus_analyzer_target(&win_x86).unwrap(),
            "x86_64-pc-windows-msvc"
        );

        let unsupported = PlatformInfo {
            os: "freebsd",
            arch: "x86_64",
        };
        assert!(verus_analyzer_target(&unsupported).is_err());
    }

    #[test]
    fn test_platform_mapping_scip() {
        let linux_x86 = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        assert_eq!(scip_target(&linux_x86).unwrap(), ("linux", "amd64"));

        let mac_arm = PlatformInfo {
            os: "macos",
            arch: "aarch64",
        };
        assert_eq!(scip_target(&mac_arm).unwrap(), ("darwin", "arm64"));

        let win = PlatformInfo {
            os: "windows",
            arch: "x86_64",
        };
        assert!(scip_target(&win).is_err());
    }

    #[test]
    fn test_download_url_verus_analyzer_linux() {
        let platform = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        let url = download_url_with_version(&Tool::VerusAnalyzer, "2026-02-03", &platform).unwrap();
        assert_eq!(
            url,
            "https://github.com/verus-lang/verus-analyzer/releases/download/2026-02-03/verus-analyzer-x86_64-unknown-linux-gnu.gz"
        );
    }

    #[test]
    fn test_download_url_verus_analyzer_windows() {
        let platform = PlatformInfo {
            os: "windows",
            arch: "x86_64",
        };
        let url = download_url_with_version(&Tool::VerusAnalyzer, "2026-02-03", &platform).unwrap();
        assert!(url.ends_with(".zip"));
    }

    #[test]
    fn test_download_url_scip_mac_arm() {
        let platform = PlatformInfo {
            os: "macos",
            arch: "aarch64",
        };
        let url = download_url_with_version(&Tool::Scip, "v0.6.1", &platform).unwrap();
        assert_eq!(
            url,
            "https://github.com/sourcegraph/scip/releases/download/v0.6.1/scip-darwin-arm64.tar.gz"
        );
    }

    #[test]
    fn test_download_url_rust_analyzer_rejected() {
        let platform = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        let result = download_url_with_version(&Tool::RustAnalyzer, "any", &platform);
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_binary_names() {
        assert_eq!(Tool::VerusAnalyzer.binary_name(), "verus-analyzer");
        assert_eq!(Tool::RustAnalyzer.binary_name(), "rust-analyzer");
        assert_eq!(Tool::Scip.binary_name(), "scip");
        assert_eq!(Tool::Verus.binary_name(), "cargo-verus");
    }

    #[test]
    fn test_tool_from_analyzer() {
        assert_eq!(
            Tool::from_analyzer(Analyzer::VerusAnalyzer),
            Tool::VerusAnalyzer
        );
        assert_eq!(
            Tool::from_analyzer(Analyzer::RustAnalyzer),
            Tool::RustAnalyzer
        );
    }

    #[test]
    fn test_tools_dir() {
        let dir = tools_dir();
        assert!(dir.is_some());
        let dir = dir.unwrap();
        assert!(dir.ends_with(".probe-verus/tools"));
    }

    #[test]
    fn test_resolve_version_env_override() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var(VERUS_ANALYZER_VERSION_ENV, "custom-version") };
        let resolved = resolve_version(&Tool::VerusAnalyzer);
        unsafe { std::env::remove_var(VERUS_ANALYZER_VERSION_ENV) };
        assert_eq!(resolved.tag, "custom-version");
        assert_eq!(resolved.source, VersionSource::EnvVar);
    }

    #[test]
    fn test_resolve_version_empty_env_ignored() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var(SCIP_VERSION_ENV, "") };
        let resolved = resolve_version(&Tool::Scip);
        unsafe { std::env::remove_var(SCIP_VERSION_ENV) };
        assert_ne!(resolved.source, VersionSource::EnvVar);
    }

    #[test]
    fn test_fallback_versions_are_valid() {
        assert!(!VERUS_ANALYZER_FALLBACK_VERSION.is_empty());
        assert!(!SCIP_FALLBACK_VERSION.is_empty());
        assert!(SCIP_FALLBACK_VERSION.starts_with('v'));
        assert!(!VERUS_FALLBACK_VERSION.is_empty());
    }

    #[test]
    fn test_version_env_var_names() {
        assert_eq!(
            Tool::VerusAnalyzer.version_env_var(),
            "PROBE_VERUS_ANALYZER_VERSION"
        );
        assert_eq!(Tool::Scip.version_env_var(), "PROBE_SCIP_VERSION");
        assert_eq!(Tool::Verus.version_env_var(), "PROBE_VERUS_VERSION");
    }

    #[test]
    fn test_platform_mapping_verus() {
        let linux_x86 = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        assert_eq!(verus_target(&linux_x86).unwrap(), "x86-linux");

        let mac_arm = PlatformInfo {
            os: "macos",
            arch: "aarch64",
        };
        assert_eq!(verus_target(&mac_arm).unwrap(), "arm64-macos");

        let mac_x86 = PlatformInfo {
            os: "macos",
            arch: "x86_64",
        };
        assert_eq!(verus_target(&mac_x86).unwrap(), "x86-macos");

        let win_x86 = PlatformInfo {
            os: "windows",
            arch: "x86_64",
        };
        assert_eq!(verus_target(&win_x86).unwrap(), "x86-win");

        let linux_arm = PlatformInfo {
            os: "linux",
            arch: "aarch64",
        };
        assert!(verus_target(&linux_arm).is_err());
    }

    #[test]
    fn test_download_url_verus_linux() {
        let platform = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        let url =
            download_url_with_version(&Tool::Verus, "0.2026.03.22.5e66329", &platform).unwrap();
        assert_eq!(
            url,
            "https://github.com/verus-lang/verus/releases/download/release%2F0.2026.03.22.5e66329/verus-0.2026.03.22.5e66329-x86-linux.zip"
        );
    }

    #[test]
    fn test_download_url_verus_mac_arm() {
        let platform = PlatformInfo {
            os: "macos",
            arch: "aarch64",
        };
        let url =
            download_url_with_version(&Tool::Verus, "0.2026.03.22.5e66329", &platform).unwrap();
        assert!(url.contains("arm64-macos.zip"));
    }

    #[test]
    fn test_resolve_verus_version_env_override() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var(VERUS_VERSION_ENV, "0.2026.01.01.abc1234") };
        let resolved = resolve_verus_version(None);
        unsafe { std::env::remove_var(VERUS_VERSION_ENV) };
        assert_eq!(resolved.tag, "0.2026.01.01.abc1234");
        assert_eq!(resolved.source, VersionSource::EnvVar);
    }

    #[test]
    fn test_verus_display_name() {
        assert_eq!(format!("{}", Tool::Verus), "verus");
        assert_eq!(format!("{}", Tool::VerusAnalyzer), "verus-analyzer");
    }
}
