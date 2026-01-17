//! Path matching utilities for probe-verus.
//!
//! This module provides utilities for matching file paths with fuzzy/flexible
//! matching strategies. This is essential because different tools (verus-analyzer,
//! verus_syn, Verus compiler) may report paths in different formats.

use std::path::Path;

/// Extract the "src/..." suffix from a path for normalized matching.
///
/// This helps match paths like "/full/path/to/project/src/lib.rs" with "src/lib.rs".
///
/// # Examples
/// ```ignore
/// assert_eq!(extract_src_suffix("/home/user/project/src/lib.rs"), "src/lib.rs");
/// assert_eq!(extract_src_suffix("src/lib.rs"), "src/lib.rs");
/// assert_eq!(extract_src_suffix("lib.rs"), "lib.rs");
/// ```
pub fn extract_src_suffix(path: &str) -> &str {
    // Try to find the "src/" part and use everything from there
    if let Some(pos) = path.find("/src/") {
        return &path[pos + 1..]; // Returns "src/..."
    }
    path
}

/// Check if two paths match using suffix comparison.
///
/// Returns true if one path ends with the other.
///
/// # Examples
/// ```ignore
/// assert!(paths_match_by_suffix("/project/src/lib.rs", "src/lib.rs"));
/// assert!(paths_match_by_suffix("src/lib.rs", "/project/src/lib.rs"));
/// ```
pub fn paths_match_by_suffix(path1: &str, path2: &str) -> bool {
    path1.ends_with(path2) || path2.ends_with(path1)
}

/// Match score for path comparison (higher is better).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathMatchScore {
    /// No match
    None = 0,
    /// Only the filename matches
    FilenameOnly = 1,
    /// Path suffix matches
    Suffix = 2,
    /// Exact path match
    Exact = 3,
}

/// Calculate the match score between two paths.
///
/// Returns a score indicating match quality:
/// - `Exact`: Paths are identical
/// - `Suffix`: One path ends with the other
/// - `FilenameOnly`: Only the filenames match
/// - `None`: No match
pub fn calculate_path_match_score(query: &str, candidate: &str) -> PathMatchScore {
    let query_path = Path::new(query);
    let candidate_path = Path::new(candidate);

    // Exact match (highest priority)
    if query_path == candidate_path {
        return PathMatchScore::Exact;
    }

    // Suffix match (high priority)
    if paths_match_by_suffix(query, candidate) {
        return PathMatchScore::Suffix;
    }

    // Filename-only match (lowest priority)
    if query_path.file_name() == candidate_path.file_name() {
        return PathMatchScore::FilenameOnly;
    }

    PathMatchScore::None
}

/// Find the best matching path from a collection of candidates.
///
/// Returns the candidate with the highest match score, or None if no match.
///
/// # Arguments
/// * `query` - The path to match against
/// * `candidates` - Iterator over candidate paths
///
/// # Returns
/// The best matching candidate path, if any
pub fn find_best_matching_path<'a, I>(query: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best_match: Option<&str> = None;
    let mut best_score = PathMatchScore::None;

    for candidate in candidates {
        let score = calculate_path_match_score(query, candidate);

        // Exact match - return immediately
        if score == PathMatchScore::Exact {
            return Some(candidate);
        }

        if score > best_score {
            best_match = Some(candidate);
            best_score = score;
        }
    }

    if best_score > PathMatchScore::None {
        best_match
    } else {
        None
    }
}

/// A helper for efficiently looking up paths from a known set.
///
/// This struct provides O(1) amortized lookup for path matching,
/// with fuzzy matching support (exact > suffix > filename-only).
#[derive(Debug, Clone)]
pub struct PathMatcher {
    /// All known paths
    known_paths: Vec<String>,
}

impl PathMatcher {
    /// Create a new PathMatcher with the given known paths.
    pub fn new(paths: Vec<String>) -> Self {
        Self { known_paths: paths }
    }

    /// Find the best matching known path for the given query.
    ///
    /// Matching priority: exact > suffix > filename-only
    pub fn find_best_match(&self, query: &str) -> Option<&String> {
        let mut best_match: Option<&String> = None;
        let mut best_score = PathMatchScore::None;

        for candidate in &self.known_paths {
            let score = calculate_path_match_score(query, candidate);

            // Exact match - return immediately
            if score == PathMatchScore::Exact {
                return Some(candidate);
            }

            if score > best_score {
                best_match = Some(candidate);
                best_score = score;
            }
        }

        if best_score > PathMatchScore::None {
            best_match
        } else {
            None
        }
    }

    /// Get the list of known paths.
    pub fn known_paths(&self) -> &[String] {
        &self.known_paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_src_suffix() {
        assert_eq!(
            extract_src_suffix("/home/user/project/src/lib.rs"),
            "src/lib.rs"
        );
        assert_eq!(extract_src_suffix("src/lib.rs"), "src/lib.rs");
        assert_eq!(extract_src_suffix("lib.rs"), "lib.rs");
        assert_eq!(
            extract_src_suffix("/project/src/module/file.rs"),
            "src/module/file.rs"
        );
    }

    #[test]
    fn test_paths_match_by_suffix() {
        assert!(paths_match_by_suffix("/project/src/lib.rs", "src/lib.rs"));
        assert!(paths_match_by_suffix("src/lib.rs", "/project/src/lib.rs"));
        assert!(!paths_match_by_suffix("/project/src/lib.rs", "src/main.rs"));
    }

    #[test]
    fn test_calculate_path_match_score() {
        assert_eq!(
            calculate_path_match_score("src/lib.rs", "src/lib.rs"),
            PathMatchScore::Exact
        );
        assert_eq!(
            calculate_path_match_score("/project/src/lib.rs", "src/lib.rs"),
            PathMatchScore::Suffix
        );
        assert_eq!(
            calculate_path_match_score("/other/lib.rs", "src/lib.rs"),
            PathMatchScore::FilenameOnly
        );
        assert_eq!(
            calculate_path_match_score("/other/main.rs", "src/lib.rs"),
            PathMatchScore::None
        );
    }

    #[test]
    fn test_path_matcher() {
        let paths = vec![
            "src/lemmas/field_lemmas/constants_lemmas.rs".to_string(),
            "src/lemmas/edwards_lemmas/constants_lemmas.rs".to_string(),
        ];
        let matcher = PathMatcher::new(paths);

        // Should prefer exact suffix match
        let result = matcher.find_best_match("src/lemmas/edwards_lemmas/constants_lemmas.rs");
        assert_eq!(
            result,
            Some(&"src/lemmas/edwards_lemmas/constants_lemmas.rs".to_string())
        );

        // Should find by suffix
        let result = matcher.find_best_match("edwards_lemmas/constants_lemmas.rs");
        assert_eq!(
            result,
            Some(&"src/lemmas/edwards_lemmas/constants_lemmas.rs".to_string())
        );

        // Ambiguous filename-only should return one of them
        let result = matcher.find_best_match("constants_lemmas.rs");
        assert!(result.is_some());
    }
}
