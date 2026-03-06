//! Schema 2.0 metadata gathering and envelope construction.
//!
//! Reads git info and Cargo.toml to populate the envelope fields.
//! Provides envelope wrapping for output and unwrapping for input.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

const TOOL_NAME: &str = "probe-verus";
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

// =============================================================================
// Envelope types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ToolInfo {
    pub name: String,
    pub version: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SourceInfo {
    pub repo: String,
    pub commit: String,
    pub language: String,
    pub package: String,
    pub package_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Envelope<T> {
    pub schema: String,
    pub schema_version: String,
    pub tool: ToolInfo,
    pub source: SourceInfo,
    pub timestamp: String,
    pub data: T,
}

/// Envelope variant for merged-atoms output (no single `source`; uses `inputs` array instead).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MergedEnvelope<T> {
    pub schema: String,
    pub schema_version: String,
    pub tool: ToolInfo,
    pub inputs: Vec<MergedInput>,
    pub timestamp: String,
    pub data: T,
}

/// One input entry in a merged-atoms envelope, recording provenance of each source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MergedInput {
    pub schema: String,
    pub source: SourceInfo,
}

// =============================================================================
// Project metadata
// =============================================================================

#[derive(Debug, Clone)]
pub struct ProjectMetadata {
    pub commit: String,
    pub repo: String,
    pub timestamp: String,
    pub pkg_name: String,
    pub pkg_version: String,
}

/// Walk up the directory tree from `starting_path` looking for `Cargo.toml`.
/// Returns the directory containing `Cargo.toml`, or `None` if not found.
pub fn find_project_root(starting_path: &Path) -> Option<PathBuf> {
    let mut current = if starting_path.is_file() {
        starting_path.parent()?.to_path_buf()
    } else {
        starting_path.to_path_buf()
    };
    loop {
        if current.join("Cargo.toml").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Gather all project metadata in one pass.
pub fn gather_metadata(project_path: &Path) -> ProjectMetadata {
    let commit = run_cmd_or_default("git", &["rev-parse", "HEAD"], Some(project_path), "");
    let repo = run_cmd_or_default(
        "git",
        &["remote", "get-url", "origin"],
        Some(project_path),
        "",
    );
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let (pkg_name, pkg_version) = read_cargo_package_info(project_path, &commit);

    ProjectMetadata {
        commit,
        repo,
        timestamp,
        pkg_name,
        pkg_version,
    }
}

/// Configuration for `atomize_internal`, replacing a long parameter list.
pub struct AtomizeInternalConfig<'a> {
    pub project_path: &'a Path,
    pub output: &'a Path,
    pub regenerate_scip: bool,
    pub verbose: bool,
    pub use_rust_analyzer: bool,
    pub allow_duplicates: bool,
    pub auto_install: bool,
    pub metadata: &'a ProjectMetadata,
}

/// Configuration for `verify_internal`, replacing a long parameter list.
pub struct VerifyInternalConfig<'a> {
    pub project_path: &'a Path,
    pub output: &'a Path,
    pub package: Option<&'a str>,
    pub atoms_path: Option<&'a Path>,
    pub verbose: bool,
    pub verus_args: &'a [String],
    pub metadata: &'a ProjectMetadata,
}

// =============================================================================
// Envelope wrapping / unwrapping
// =============================================================================

/// Wrap data in a Schema 2.0 envelope.
pub fn wrap_in_envelope<T: Serialize>(
    schema: &str,
    command: &str,
    data: T,
    metadata: &ProjectMetadata,
) -> Envelope<T> {
    Envelope {
        schema: schema.to_string(),
        schema_version: "2.0".to_string(),
        tool: ToolInfo {
            name: TOOL_NAME.to_string(),
            version: TOOL_VERSION.to_string(),
            command: command.to_string(),
        },
        source: SourceInfo {
            repo: metadata.repo.clone(),
            commit: metadata.commit.clone(),
            language: "rust".to_string(),
            package: metadata.pkg_name.clone(),
            package_version: metadata.pkg_version.clone(),
        },
        timestamp: metadata.timestamp.clone(),
        data,
    }
}

/// Wrap merged data in a Schema 2.0 merged-atoms envelope.
///
/// Per the spec, merged output uses `schema: "probe/merged-atoms"` and an `inputs`
/// array instead of a single `source` field.  `tool.name` is `"probe"` (the
/// merge operation is cross-tool by nature) and `tool.version` is the producing
/// tool's semver version, matching the spec's plain-version convention.
pub fn wrap_merged_envelope<T: Serialize>(
    data: T,
    inputs: Vec<MergedInput>,
    timestamp: &str,
) -> MergedEnvelope<T> {
    MergedEnvelope {
        schema: "probe/merged-atoms".to_string(),
        schema_version: "2.0".to_string(),
        tool: ToolInfo {
            name: "probe".to_string(),
            version: TOOL_VERSION.to_string(),
            command: "merge-atoms".to_string(),
        },
        inputs,
        timestamp: timestamp.to_string(),
        data,
    }
}

/// Extract provenance entries from a JSON value that might be an envelope.
///
/// - Single-source envelope (has `source`): returns one `MergedInput`.
/// - Merged envelope (has `inputs` array): returns all nested `MergedInput` entries,
///   preserving provenance on recursive merge.
/// - Bare dict (no `schema`): returns empty vec.
pub fn extract_envelope_inputs(json: &serde_json::Value) -> Vec<MergedInput> {
    let Some(map) = json.as_object() else {
        return vec![];
    };
    let Some(serde_json::Value::String(schema)) = map.get("schema") else {
        return vec![];
    };
    if !schema.contains('/') {
        return vec![];
    }

    if let Some(source_val) = map.get("source") {
        if let Ok(source) = serde_json::from_value::<SourceInfo>(source_val.clone()) {
            return vec![MergedInput {
                schema: schema.clone(),
                source,
            }];
        }
    }

    if let Some(serde_json::Value::Array(inputs_arr)) = map.get("inputs") {
        return inputs_arr
            .iter()
            .filter_map(|v| serde_json::from_value::<MergedInput>(v.clone()).ok())
            .collect();
    }

    vec![]
}

/// Extract the data payload from JSON, unwrapping the Schema 2.0 envelope if present.
///
/// Accepts any envelope that has a `"schema"` string containing `'/'` (e.g.
/// `"probe-verus/atoms"`, `"probe-lean/atoms"`) and a `"data"` field.
/// Returns the original JSON unchanged if no envelope is detected.
pub fn unwrap_envelope(json: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(mut map) = json {
        let is_envelope = matches!(
            map.get("schema"),
            Some(serde_json::Value::String(s)) if s.contains('/')
        );
        if is_envelope {
            if let Some(data) = map.remove("data") {
                return data;
            }
        }
        serde_json::Value::Object(map)
    } else {
        json
    }
}

// =============================================================================
// Default output paths
// =============================================================================

/// Compute the default output path: `.verilib/probes/verus_<pkg>_<ver>[_<suffix>].json`
///
/// `suffix` is empty for atoms, or `"specs"`, `"proofs"`, `"stubs"`, etc.
pub fn get_default_output_path(
    project_root: &Path,
    metadata: &ProjectMetadata,
    suffix: &str,
) -> PathBuf {
    let pkg = if metadata.pkg_name.is_empty() {
        "unknown"
    } else {
        &metadata.pkg_name
    };
    let ver = if metadata.pkg_version.is_empty() {
        "unknown"
    } else {
        &metadata.pkg_version
    };

    let filename = if suffix.is_empty() {
        format!("verus_{}_{}.json", pkg, ver)
    } else {
        format!("verus_{}_{}_{}.json", pkg, ver, suffix)
    };

    project_root.join(".verilib").join("probes").join(filename)
}

/// Find the default atoms file under `.verilib/probes/`, tolerating version mismatches.
///
/// 1. Try exact path: `verus_<pkg>_<ver>.json`
/// 2. If not found, scan for `verus_<pkg>_*.json` and pick the most recently modified
/// 3. Warn on fallback
pub fn find_default_atoms_path(project_root: &Path, metadata: &ProjectMetadata) -> Option<PathBuf> {
    let exact = get_default_output_path(project_root, metadata, "");
    if exact.exists() {
        return Some(exact);
    }

    let probes_dir = project_root.join(".verilib").join("probes");
    if !probes_dir.is_dir() {
        return None;
    }

    let pkg = if metadata.pkg_name.is_empty() {
        "unknown"
    } else {
        &metadata.pkg_name
    };
    let prefix = format!("verus_{}_", pkg);

    let suffixes = ["_specs", "_proofs", "_stubs", "_specs-data", "_run-summary"];
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    if let Ok(entries) = std::fs::read_dir(&probes_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(&prefix) && name_str.ends_with(".json") {
                let after_prefix = &name_str[prefix.len()..name_str.len() - ".json".len()];
                if suffixes.iter().any(|s| after_prefix.ends_with(s)) {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(_, t)| modified > *t) {
                    best = Some((entry.path(), modified));
                }
            }
        }
    }

    if let Some((path, _)) = best {
        eprintln!(
            "Warning: exact atoms path not found ({}), falling back to {}",
            exact.display(),
            path.display()
        );
        Some(path)
    } else {
        None
    }
}

// =============================================================================
// Internal helpers
// =============================================================================

fn run_cmd_or_default(cmd: &str, args: &[&str], cwd: Option<&Path>, default: &str) -> String {
    let mut command = Command::new(cmd);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    match command.output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => default.to_string(),
    }
}

/// For a workspace-only Cargo.toml (no `[package]`), try the single workspace member's
/// Cargo.toml to get a package name and version.
fn try_workspace_member_info(
    project_path: &Path,
    table: &toml::Table,
    version_fallback: &dyn Fn() -> String,
) -> Option<(String, String)> {
    let workspace = table.get("workspace")?.as_table()?;
    let members = workspace.get("members")?.as_array()?;
    if members.len() != 1 {
        return None;
    }
    let member_dir = members[0].as_str()?;
    let member_toml = project_path.join(member_dir).join("Cargo.toml");
    let member_content = std::fs::read_to_string(&member_toml).ok()?;
    let member_table: toml::Table = member_content.parse().ok()?;
    let pkg = member_table.get("package")?.as_table()?;
    let name = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(version_fallback);
    Some((name, version))
}

/// Read package name and version from Cargo.toml.
/// When the version field is absent, falls back to the 7-char git short hash
/// (matching probe-lean's `getPackageNameAndVersion` and the envelope-rationale spec).
/// For workspace-only roots (no `[package]`), tries the single member crate's Cargo.toml,
/// then falls back to the directory name.
fn read_cargo_package_info(project_path: &Path, commit: &str) -> (String, String) {
    let version_fallback = || -> String {
        if commit.len() >= 7 {
            commit[..7].to_string()
        } else {
            "unknown".to_string()
        }
    };

    let cargo_toml_path = project_path.join("Cargo.toml");
    let content = match std::fs::read_to_string(&cargo_toml_path) {
        Ok(c) => c,
        Err(_) => return ("unknown".to_string(), version_fallback()),
    };

    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(_) => return ("unknown".to_string(), version_fallback()),
    };

    let package = match table.get("package").and_then(|p| p.as_table()) {
        Some(p) => p,
        None => {
            if let Some((name, version)) =
                try_workspace_member_info(project_path, &table, &version_fallback)
            {
                return (name, version);
            }
            let dir_name = project_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            return (dir_name, version_fallback());
        }
    };

    let name = package
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let version = package
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(version_fallback);

    (name, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unwrap_envelope_with_envelope() {
        let json = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": { "name": "probe-verus", "version": "2.0.0", "command": "atomize" },
            "source": {
                "repo": "https://github.com/org/proj",
                "commit": "abc123",
                "language": "rust",
                "package": "my-crate",
                "package-version": "1.0.0"
            },
            "timestamp": "2026-03-06T12:00:00Z",
            "data": {
                "probe:my-crate/1.0.0/func()": {
                    "display-name": "func",
                    "dependencies": [],
                    "code-module": "",
                    "code-path": "src/lib.rs",
                    "code-text": { "lines-start": 1, "lines-end": 10 },
                    "kind": "exec",
                    "language": "rust"
                }
            }
        });

        let data = unwrap_envelope(json);
        assert!(data.is_object());
        assert!(data.get("probe:my-crate/1.0.0/func()").is_some());
        assert!(data.get("schema").is_none());
    }

    #[test]
    fn test_unwrap_envelope_bare_dict() {
        let json = serde_json::json!({
            "probe:my-crate/1.0.0/func()": {
                "display-name": "func",
                "dependencies": [],
                "code-module": "",
                "code-path": "src/lib.rs",
                "code-text": { "lines-start": 1, "lines-end": 10 },
                "kind": "exec"
            }
        });

        let data = unwrap_envelope(json.clone());
        assert_eq!(data, json);
    }

    #[test]
    fn test_unwrap_envelope_foreign_schema() {
        let json = serde_json::json!({
            "schema": "probe-lean/atoms",
            "schema-version": "2.0",
            "data": { "lean:Foo.bar": {} }
        });

        let data = unwrap_envelope(json);
        assert!(data.get("lean:Foo.bar").is_some());
    }

    #[test]
    fn test_unwrap_envelope_no_data_key() {
        let json = serde_json::json!({
            "schema": "probe-verus/atoms",
            "payload": { "key": "value" }
        });

        let data = unwrap_envelope(json.clone());
        assert_eq!(data, json);
    }

    #[test]
    fn test_get_default_output_path_atoms() {
        let meta = ProjectMetadata {
            commit: "abc".to_string(),
            repo: "".to_string(),
            timestamp: "".to_string(),
            pkg_name: "curve25519-dalek".to_string(),
            pkg_version: "4.1.3".to_string(),
        };
        let path = get_default_output_path(Path::new("/project"), &meta, "");
        assert_eq!(
            path,
            PathBuf::from("/project/.verilib/probes/verus_curve25519-dalek_4.1.3.json")
        );
    }

    #[test]
    fn test_get_default_output_path_specs() {
        let meta = ProjectMetadata {
            commit: "".to_string(),
            repo: "".to_string(),
            timestamp: "".to_string(),
            pkg_name: "my-crate".to_string(),
            pkg_version: "0.1.0".to_string(),
        };
        let path = get_default_output_path(Path::new("/project"), &meta, "specs");
        assert_eq!(
            path,
            PathBuf::from("/project/.verilib/probes/verus_my-crate_0.1.0_specs.json")
        );
    }

    #[test]
    fn test_get_default_output_path_unknown_fallback() {
        let meta = ProjectMetadata {
            commit: "".to_string(),
            repo: "".to_string(),
            timestamp: "".to_string(),
            pkg_name: "".to_string(),
            pkg_version: "".to_string(),
        };
        let path = get_default_output_path(Path::new("/project"), &meta, "proofs");
        assert_eq!(
            path,
            PathBuf::from("/project/.verilib/probes/verus_unknown_unknown_proofs.json")
        );
    }

    #[test]
    fn test_find_project_root_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        assert_eq!(
            find_project_root(tmp.path()),
            Some(tmp.path().to_path_buf())
        );
    }

    #[test]
    fn test_find_project_root_from_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let sub = tmp.path().join("src").join("commands");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_project_root(&sub), Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn test_find_project_root_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("no_cargo_here");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_project_root(&sub), None);
    }

    #[test]
    fn test_wrap_in_envelope_roundtrip() {
        let data = serde_json::json!({"key": "value"});
        let meta = ProjectMetadata {
            commit: "abc123".to_string(),
            repo: "https://github.com/org/proj".to_string(),
            timestamp: "2026-03-06T12:00:00Z".to_string(),
            pkg_name: "my-crate".to_string(),
            pkg_version: "1.0.0".to_string(),
        };

        let envelope = wrap_in_envelope("probe-verus/atoms", "atomize", data.clone(), &meta);
        assert_eq!(envelope.schema, "probe-verus/atoms");
        assert_eq!(envelope.tool.name, "probe-verus");
        assert_eq!(envelope.tool.command, "atomize");
        assert_eq!(envelope.source.package, "my-crate");

        let serialized = serde_json::to_value(&envelope).unwrap();
        let unwrapped = unwrap_envelope(serialized);
        assert_eq!(unwrapped, data);
    }

    #[test]
    fn test_extract_envelope_inputs_from_envelope() {
        let json = serde_json::json!({
            "schema": "probe-verus/atoms",
            "schema-version": "2.0",
            "tool": { "name": "probe-verus", "version": "2.0.0", "command": "atomize" },
            "source": {
                "repo": "https://github.com/org/proj",
                "commit": "abc123",
                "language": "rust",
                "package": "my-crate",
                "package-version": "1.0.0"
            },
            "timestamp": "2026-03-06T12:00:00Z",
            "data": {}
        });

        let inputs = extract_envelope_inputs(&json);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].schema, "probe-verus/atoms");
        assert_eq!(inputs[0].source.package, "my-crate");
        assert_eq!(inputs[0].source.commit, "abc123");
    }

    #[test]
    fn test_extract_envelope_inputs_from_bare_dict() {
        let json = serde_json::json!({
            "probe:my-crate/1.0.0/func()": {
                "display-name": "func",
                "dependencies": []
            }
        });

        assert!(extract_envelope_inputs(&json).is_empty());
    }

    #[test]
    fn test_extract_envelope_inputs_from_merged_envelope() {
        let json = serde_json::json!({
            "schema": "probe/merged-atoms",
            "schema-version": "2.0",
            "tool": { "name": "probe", "version": "2.0.0", "command": "merge-atoms" },
            "inputs": [
                {
                    "schema": "probe-verus/atoms",
                    "source": {
                        "repo": "https://github.com/org/a",
                        "commit": "aaa",
                        "language": "rust",
                        "package": "crate-a",
                        "package-version": "1.0.0"
                    }
                },
                {
                    "schema": "probe-lean/atoms",
                    "source": {
                        "repo": "https://github.com/org/b",
                        "commit": "bbb",
                        "language": "lean",
                        "package": "crate-b",
                        "package-version": "0.1.0"
                    }
                }
            ],
            "timestamp": "2026-03-06T12:00:00Z",
            "data": {}
        });

        let inputs = extract_envelope_inputs(&json);
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].source.package, "crate-a");
        assert_eq!(inputs[1].source.package, "crate-b");
    }

    #[test]
    fn test_wrap_merged_envelope_structure() {
        let data = serde_json::json!({"merged": true});
        let inputs = vec![MergedInput {
            schema: "probe-verus/atoms".to_string(),
            source: SourceInfo {
                repo: "https://github.com/org/a".to_string(),
                commit: "aaa".to_string(),
                language: "rust".to_string(),
                package: "crate-a".to_string(),
                package_version: "1.0.0".to_string(),
            },
        }];

        let envelope = wrap_merged_envelope(data, inputs, "2026-03-06T12:00:00Z");
        assert_eq!(envelope.schema, "probe/merged-atoms");
        assert_eq!(envelope.schema_version, "2.0");
        assert_eq!(envelope.tool.name, "probe");
        assert_eq!(envelope.tool.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(envelope.tool.command, "merge-atoms");
        assert_eq!(envelope.inputs.len(), 1);
        assert_eq!(envelope.inputs[0].source.package, "crate-a");
    }

    #[test]
    fn test_merged_envelope_unwrap_roundtrip() {
        let data = serde_json::json!({"key": "merged_value"});
        let inputs = vec![MergedInput {
            schema: "probe-lean/atoms".to_string(),
            source: SourceInfo {
                repo: "https://github.com/org/b".to_string(),
                commit: "bbb".to_string(),
                language: "lean".to_string(),
                package: "crate-b".to_string(),
                package_version: "0.1.0".to_string(),
            },
        }];

        let envelope = wrap_merged_envelope(data.clone(), inputs, "2026-03-06T12:00:00Z");
        let serialized = serde_json::to_value(&envelope).unwrap();
        let unwrapped = unwrap_envelope(serialized);
        assert_eq!(unwrapped, data);
    }

    #[test]
    fn test_find_default_atoms_path_excludes_suffixed_files_not_package_names() {
        let tmp = tempfile::tempdir().unwrap();
        let probes = tmp.path().join(".verilib").join("probes");
        std::fs::create_dir_all(&probes).unwrap();

        // Package whose name contains "_specs" -- atoms file should NOT be excluded
        std::fs::write(probes.join("verus_my_specs_tool_1.0.0.json"), "{}").unwrap();
        // A real specs file that SHOULD be excluded
        std::fs::write(probes.join("verus_my_specs_tool_1.0.0_specs.json"), "{}").unwrap();

        let meta = ProjectMetadata {
            commit: "abc1234".to_string(),
            repo: "".to_string(),
            timestamp: "".to_string(),
            pkg_name: "my_specs_tool".to_string(),
            pkg_version: "9.9.9".to_string(), // mismatch to force fallback scan
        };

        let found = find_default_atoms_path(tmp.path(), &meta);
        assert!(found.is_some());
        let found_name = found
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(found_name, "verus_my_specs_tool_1.0.0.json");
    }

    #[test]
    fn test_read_cargo_package_info_workspace_single_member() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"my-crate\"]\n",
        )
        .unwrap();

        let member = root.join("my-crate");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.3.0\"\n",
        )
        .unwrap();

        let (name, version) = read_cargo_package_info(root, "abcdef1234567890");
        assert_eq!(name, "my-crate");
        assert_eq!(version, "0.3.0");
    }

    #[test]
    fn test_read_cargo_package_info_workspace_single_member_no_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"my-crate\"]\n",
        )
        .unwrap();

        let member = root.join("my-crate");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\n",
        )
        .unwrap();

        let (name, version) = read_cargo_package_info(root, "abcdef1234567890");
        assert_eq!(name, "my-crate");
        assert_eq!(version, "abcdef1");
    }

    #[test]
    fn test_read_cargo_package_info_workspace_multiple_members_falls_back_to_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crate-a\", \"crate-b\"]\n",
        )
        .unwrap();

        let (name, version) = read_cargo_package_info(root, "abcdef1234567890");
        let expected_dir = root.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(name, expected_dir);
        assert_eq!(version, "abcdef1");
    }

    #[test]
    fn test_read_cargo_package_info_no_package_no_workspace_falls_back_to_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(root.join("Cargo.toml"), "# empty toml\n").unwrap();

        let (name, version) = read_cargo_package_info(root, "abcdef1234567890");
        let expected_dir = root.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(name, expected_dir);
        assert_eq!(version, "abcdef1");
    }
}
