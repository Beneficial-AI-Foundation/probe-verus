//! Stubify command - Convert .md files with YAML frontmatter to JSON.

use crate::metadata::{
    find_project_root, gather_metadata, get_default_output_path, wrap_in_envelope,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use walkdir::WalkDir;

/// YAML frontmatter structure from stub .md files.
#[derive(Debug, Deserialize, Serialize)]
pub struct StubFrontmatter {
    #[serde(rename = "code-line", default, skip_serializing_if = "Option::is_none")]
    pub code_line: Option<usize>,
    #[serde(rename = "code-path", default, skip_serializing_if = "Option::is_none")]
    pub code_path: Option<String>,
    #[serde(rename = "code-name", default, skip_serializing_if = "Option::is_none")]
    pub code_name: Option<String>,
}

/// Execute the stubify command.
///
/// Walks through a directory hierarchy of .md files with YAML frontmatter
/// and converts them to a JSON file where keys are file paths and values
/// are the frontmatter fields.
pub fn cmd_stubify(
    path: PathBuf,
    output: Option<PathBuf>,
    project_path_override: Option<PathBuf>,
) -> Result<(), String> {
    // Validate input path
    if !path.exists() {
        return Err(format!("Path does not exist: {}", path.display()));
    }

    if !path.is_dir() {
        return Err(format!("Path must be a directory: {}", path.display()));
    }

    // Walk directory and collect .md files
    let mut stubs: HashMap<String, StubFrontmatter> = HashMap::new();
    let mut processed = 0;
    let mut errors = 0;

    for entry in WalkDir::new(&path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let entry_path = entry.path();

        // Only process .md files
        if !entry_path.is_file() {
            continue;
        }
        if entry_path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        // Get relative path from input directory
        let relative_path = match entry_path.strip_prefix(&path) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => entry_path.to_string_lossy().to_string(),
        };

        // Read and parse the file
        match parse_frontmatter(entry_path) {
            Ok(frontmatter) => {
                stubs.insert(relative_path, frontmatter);
                processed += 1;
            }
            Err(e) => {
                eprintln!("Warning: Failed to parse {}: {}", entry_path.display(), e);
                errors += 1;
            }
        }
    }

    if processed == 0 {
        return Err(format!("No .md files found in {}", path.display()));
    }

    // Resolve project root: explicit flag > auto-detect from input path
    let project_root = project_path_override
        .unwrap_or_else(|| find_project_root(&path).unwrap_or_else(|| path.clone()));
    let metadata = gather_metadata(&project_root);
    let output =
        output.unwrap_or_else(|| get_default_output_path(&project_root, &metadata, "stubs"));

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
    }

    // Wrap in envelope and write
    let envelope = wrap_in_envelope("probe-verus/stubs", "stubify", &stubs, &metadata);
    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
    std::fs::write(&output, &json).map_err(|e| format!("Failed to write output file: {}", e))?;

    println!(
        "Wrote {} stubs to {} ({} errors)",
        processed,
        output.display(),
        errors
    );
    Ok(())
}

/// Parse YAML frontmatter from a markdown file.
///
/// Expects files in the format:
/// ```text
/// ---
/// code-line: 123
/// code-path: path/to/file.rs
/// code-name: scip:...
/// ---
/// ```
fn parse_frontmatter(path: &std::path::Path) -> Result<StubFrontmatter, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

    // Check for frontmatter delimiters
    if !content.starts_with("---") {
        return Err("File does not start with YAML frontmatter".to_string());
    }

    // Find the closing delimiter
    let rest = &content[3..];
    let end_pos = rest
        .find("\n---")
        .ok_or_else(|| "No closing frontmatter delimiter found".to_string())?;

    // Extract and parse the YAML
    let yaml_content = &rest[..end_pos].trim();

    serde_yaml::from_str(yaml_content).map_err(|e| format!("Failed to parse YAML: {}", e))
}
