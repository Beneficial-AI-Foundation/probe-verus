//! Selection of the verus-analyzer release that indexes a project.
//!
//! The release is resolved once per run, in this order:
//! 1. `PROBE_VERUS_ANALYZER_VERSION`.
//! 2. A `verus-analyzer` pin in `[package.metadata.probe-verus]` of the
//!    package manifest, then in `[workspace.metadata.probe-verus]` of the
//!    workspace manifest.
//! 3. [`DEFAULT_VERUS_ANALYZER_VERSION`].
//!
//! A value is a release tag (for example `2026-09-25`) or `path`, which uses
//! the `verus-analyzer` on `PATH`. `setup` and every indexing command use
//! [`select`], so they agree on the release.

use std::path::{Path, PathBuf};

/// The release used when neither the environment nor the project selects one.
/// It must be 2026-08-22 or later, the first release whose SCIP symbols match
/// rust-analyzer's (`impl#[SelfType][Trait]method()`). A newer release becomes
/// the default only in a probe-verus release, after the corpus check passes
/// with it.
pub const DEFAULT_VERUS_ANALYZER_VERSION: &str = "2026-09-25";

/// Environment variable that overrides every other selection.
pub const VERUS_ANALYZER_VERSION_ENV: &str = "PROBE_VERUS_ANALYZER_VERSION";

/// Key of the pin in `[package.metadata.probe-verus]` and
/// `[workspace.metadata.probe-verus]`.
const PIN_KEY: &str = "verus-analyzer";

/// Pin value that selects the `verus-analyzer` on `PATH`.
const PATH_SELECTOR: &str = "path";

/// The verus-analyzer to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// A release tag, installed in `~/.probe-verus/tools/verus-analyzer-{tag}/`.
    Release(String),
    /// The `verus-analyzer` found on `PATH`.
    Path,
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Selector::Release(tag) => f.write_str(tag),
            Selector::Path => f.write_str(PATH_SELECTOR),
        }
    }
}

/// Where a [`Selector`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionSource {
    Env,
    PackagePin(PathBuf),
    WorkspacePin(PathBuf),
    Default,
}

impl std::fmt::Display for SelectionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectionSource::Env => write!(f, "{VERUS_ANALYZER_VERSION_ENV}"),
            SelectionSource::PackagePin(p) => {
                write!(f, "[package.metadata.probe-verus] in {}", p.display())
            }
            SelectionSource::WorkspacePin(p) => {
                write!(f, "[workspace.metadata.probe-verus] in {}", p.display())
            }
            SelectionSource::Default => f.write_str("default"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub selector: Selector,
    pub source: SelectionSource,
}

/// Select the verus-analyzer for the package in `package_dir` (the directory
/// that `resolve_workspace_root` returns). If `package_dir` is a virtual
/// workspace root, only the workspace pin applies.
pub fn select(package_dir: &Path) -> Result<Selection, String> {
    if let Some(selection) = select_from_env()? {
        return Ok(selection);
    }
    let package_manifest = package_dir.join("Cargo.toml");
    if let Some(value) = read_pin(&package_manifest, "package")? {
        return Ok(Selection {
            selector: parse_selector(&value, &package_manifest.display().to_string())?,
            source: SelectionSource::PackagePin(package_manifest),
        });
    }
    if let Some(workspace_manifest) = workspace_manifest(package_dir) {
        if let Some(value) = read_pin(&workspace_manifest, "workspace")? {
            return Ok(Selection {
                selector: parse_selector(&value, &workspace_manifest.display().to_string())?,
                source: SelectionSource::WorkspacePin(workspace_manifest),
            });
        }
    }
    Ok(default_selection())
}

/// Select the verus-analyzer without a project: the environment, then the
/// default.
pub fn select_without_project() -> Result<Selection, String> {
    Ok(select_from_env()?.unwrap_or_else(default_selection))
}

fn default_selection() -> Selection {
    Selection {
        selector: Selector::Release(DEFAULT_VERUS_ANALYZER_VERSION.to_string()),
        source: SelectionSource::Default,
    }
}

fn select_from_env() -> Result<Option<Selection>, String> {
    match std::env::var(VERUS_ANALYZER_VERSION_ENV) {
        Ok(v) if !v.is_empty() => Ok(Some(Selection {
            selector: parse_selector(&v, VERUS_ANALYZER_VERSION_ENV)?,
            source: SelectionSource::Env,
        })),
        _ => Ok(None),
    }
}

/// Parse a selector value. `origin` names its source in the error.
pub fn parse_selector(value: &str, origin: &str) -> Result<Selector, String> {
    if value == PATH_SELECTOR {
        return Ok(Selector::Path);
    }
    if value == "latest" {
        return Err(format!(
            "{origin}: verus-analyzer `latest` is not supported. \
             Give a release tag (for example `{DEFAULT_VERUS_ANALYZER_VERSION}`) or `path`."
        ));
    }
    if !is_safe_version(value) {
        return Err(format!(
            "{origin}: `{value}` is not a verus-analyzer release tag or `path`. \
             A tag contains only letters, digits, `.`, `_` and `-`."
        ));
    }
    Ok(Selector::Release(value.to_string()))
}

/// Whether `version` can be used as part of a directory name: ASCII letters,
/// digits, `.`, `_` and `-`, not starting with `.`, at most 64 characters.
pub fn is_safe_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && !version.starts_with('.')
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn read_manifest(manifest: &Path) -> Result<Option<toml::Table>, String> {
    let Ok(contents) = std::fs::read_to_string(manifest) else {
        return Ok(None);
    };
    contents
        .parse::<toml::Table>()
        .map(Some)
        .map_err(|e| format!("cannot parse {}: {e}", manifest.display()))
}

/// The `verus-analyzer` value in `[<section>.metadata.probe-verus]` of
/// `manifest`, if the manifest has that section.
fn read_pin(manifest: &Path, section: &str) -> Result<Option<String>, String> {
    let Some(table) = read_manifest(manifest)? else {
        return Ok(None);
    };
    let Some(value) = table
        .get(section)
        .and_then(|s| s.get("metadata"))
        .and_then(|m| m.get("probe-verus"))
        .and_then(|p| p.get(PIN_KEY))
    else {
        return Ok(None);
    };
    value.as_str().map(|s| Some(s.to_string())).ok_or_else(|| {
        format!(
            "{}: [{section}.metadata.probe-verus] {PIN_KEY} must be a string",
            manifest.display()
        )
    })
}

/// The workspace manifest of the package in `package_dir`: its own manifest
/// if that has a `[workspace]` table, the manifest that `package.workspace`
/// names, or else the nearest ancestor manifest with a `[workspace]` table
/// that does not exclude the package. As in Cargo, an ancestor excludes the
/// package if a `workspace.exclude` entry contains it and no
/// `workspace.members` entry does.
pub fn workspace_manifest(package_dir: &Path) -> Option<PathBuf> {
    let own = package_dir.join("Cargo.toml");
    let table = read_manifest(&own).ok().flatten();
    if let Some(table) = &table {
        if table.contains_key("workspace") {
            return Some(own);
        }
        if let Some(rel) = table
            .get("package")
            .and_then(|p| p.get("workspace"))
            .and_then(|w| w.as_str())
        {
            return Some(package_dir.join(rel).join("Cargo.toml"));
        }
    }
    let package_dir = std::fs::canonicalize(package_dir).unwrap_or(package_dir.to_path_buf());
    package_dir.ancestors().skip(1).find_map(|dir| {
        let manifest = dir.join("Cargo.toml");
        let table = read_manifest(&manifest).ok().flatten()?;
        let workspace = table.get("workspace")?;
        (!excludes(workspace, dir, &package_dir)).then_some(manifest)
    })
}

/// Whether the `[workspace]` table of the manifest in `root` excludes the
/// package in `package_dir`, by Cargo's rule: an `exclude` entry contains it
/// and no `members` entry does (entries are compared as path prefixes).
fn excludes(workspace: &toml::Value, root: &Path, package_dir: &Path) -> bool {
    let contains = |key: &str| {
        workspace
            .get(key)
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .any(|entry| package_dir.starts_with(root.join(entry)))
    };
    contains("exclude") && !contains("members")
}

/// Guards tests that read or change `PROBE_VERUS_ANALYZER_VERSION`.
#[cfg(test)]
pub(crate) static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn release(tag: &str) -> Selector {
        Selector::Release(tag.to_string())
    }

    /// A workspace at `root` with member `crates/a`.
    fn workspace(root: &Path, workspace_pin: Option<&str>, package_pin: Option<&str>) -> PathBuf {
        let mut ws = "[workspace]\nmembers = [\"crates/a\"]\n".to_string();
        if let Some(pin) = workspace_pin {
            ws.push_str(&format!(
                "[workspace.metadata.probe-verus]\nverus-analyzer = \"{pin}\"\n"
            ));
        }
        write(&root.join("Cargo.toml"), &ws);
        let mut pkg = "[package]\nname = \"a\"\nversion = \"0.1.0\"\n".to_string();
        if let Some(pin) = package_pin {
            pkg.push_str(&format!(
                "[package.metadata.probe-verus]\nverus-analyzer = \"{pin}\"\n"
            ));
        }
        let member = root.join("crates/a");
        write(&member.join("Cargo.toml"), &pkg);
        member
    }

    fn select_no_env(dir: &Path) -> Result<Selection, String> {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var(VERUS_ANALYZER_VERSION_ENV) };
        select(dir)
    }

    #[test]
    fn package_pin_wins_over_workspace_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let member = workspace(tmp.path(), Some("2026-10-01"), Some("2026-11-01"));
        let s = select_no_env(&member).unwrap();
        assert_eq!(s.selector, release("2026-11-01"));
        assert!(matches!(s.source, SelectionSource::PackagePin(_)));
    }

    #[test]
    fn workspace_pin_applies_to_member() {
        let tmp = tempfile::tempdir().unwrap();
        let member = workspace(tmp.path(), Some("2026-10-01"), None);
        let s = select_no_env(&member).unwrap();
        assert_eq!(s.selector, release("2026-10-01"));
        assert!(matches!(s.source, SelectionSource::WorkspacePin(_)));
    }

    #[test]
    fn workspace_pin_applies_at_virtual_root() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path(), Some("path"), Some("2026-11-01"));
        let s = select_no_env(tmp.path()).unwrap();
        assert_eq!(s.selector, Selector::Path);
    }

    #[test]
    fn default_without_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let member = workspace(tmp.path(), None, None);
        let s = select_no_env(&member).unwrap();
        assert_eq!(s.selector, release(DEFAULT_VERUS_ANALYZER_VERSION));
        assert_eq!(s.source, SelectionSource::Default);
    }

    #[test]
    fn package_workspace_key_names_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("ws/Cargo.toml"),
            "[workspace]\n[workspace.metadata.probe-verus]\nverus-analyzer = \"2026-10-01\"\n",
        );
        write(
            &tmp.path().join("pkg/Cargo.toml"),
            "[package]\nname = \"p\"\nworkspace = \"../ws\"\n",
        );
        let s = select_no_env(&tmp.path().join("pkg")).unwrap();
        assert_eq!(s.selector, release("2026-10-01"));
    }

    #[test]
    fn env_wins_over_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let member = workspace(tmp.path(), Some("2026-10-01"), Some("2026-11-01"));
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var(VERUS_ANALYZER_VERSION_ENV, "2026-12-01") };
        let s = select(&member);
        unsafe { std::env::remove_var(VERUS_ANALYZER_VERSION_ENV) };
        let s = s.unwrap();
        assert_eq!(s.selector, release("2026-12-01"));
        assert_eq!(s.source, SelectionSource::Env);
    }

    #[test]
    fn bad_pins_are_errors() {
        for pin in ["latest", "../x", "a/b", ".hidden", ""] {
            let tmp = tempfile::tempdir().unwrap();
            let member = workspace(tmp.path(), None, Some(pin));
            assert!(select_no_env(&member).is_err(), "pin {pin:?}");
        }
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("Cargo.toml"),
            "[package]\nname = \"p\"\n[package.metadata.probe-verus]\nverus-analyzer = 3\n",
        );
        assert!(select_no_env(tmp.path()).is_err());
    }

    #[test]
    fn excluded_package_skips_the_ancestor_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\"]\nexclude = [\"fuzz\"]\n\
             [workspace.metadata.probe-verus]\nverus-analyzer = \"2026-10-01\"\n",
        );
        write(&root.join("a/Cargo.toml"), "[package]\nname = \"a\"\n");
        write(
            &root.join("fuzz/Cargo.toml"),
            "[package]\nname = \"fuzz\"\n",
        );

        assert_eq!(
            workspace_manifest(&root.join("a")),
            Some(root.join("Cargo.toml"))
        );
        assert_eq!(workspace_manifest(&root.join("fuzz")), None);
        let s = select_no_env(&root.join("fuzz")).unwrap();
        assert_eq!(s.source, SelectionSource::Default);
    }

    #[test]
    fn safe_versions() {
        assert!(is_safe_version("2026-09-25"));
        assert!(is_safe_version("0.2026.03.22.5e66329"));
        assert!(!is_safe_version(".."));
        assert!(!is_safe_version("a b"));
        assert!(!is_safe_version(&"x".repeat(65)));
    }
}
