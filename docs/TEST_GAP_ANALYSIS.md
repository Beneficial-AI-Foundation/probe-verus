# Test Gap Analysis: Why Verified Bugs Were Not Caught

This document analyzes why existing tests in probe-verus did not catch four verified bugs (S2, S3, C4, C5). For each bug, we explain: (1) what tests exist, (2) why they missed the bug, and (3) what test would have caught it.

---

## Bug S2: warning→verified mapping (extract.rs) — FIXED

### Location
`src/commands/extract.rs`, `map_verification_status()`:

**Status:** Fixed. Production code now maps `"warning" => "unverified"` and the
unit test `test_status_mapping_all_values` asserts this. The integration test
in `tests/unified_extract.rs` and `docs/SCHEMA.md` have also been updated.

### Original Bug
Verus's `"warning"` status was mapped to `"verified"`, misclassifying functions
with unverified assumptions as fully verified.

### Root Cause
The test `test_status_mapping_all_values` encoded the wrong specification.
Merge tests and fixtures only used `"success"` and `"failure"`, never exercising
the `"warning"` path.

---

## Bug S3: assume/admit string search in comments (verus_parser.rs)

### Location
`src/verus_parser.rs`, `has_trusted_assumption()` (lines 651–668):

```rust
if line.contains("assume(") || line.contains("admit(") {
    return true;
}
```

**Bug:** Uses plain string search over function body lines. Matches `assume(` and `admit(` in comments (e.g. `// This uses assume() for ...`) or doc strings, causing false positives for `has_trusted_assumption`.

### Existing Tests

| Test | Location | What it tests |
|------|----------|----------------|
| `test_parse_simple_function` | verus_parser.rs:1139–1162 | Parsing spans, function names, line ranges |
| `test_parse_file_for_functions` | verus_parser.rs:1165–1190 | Visibility (pub/private), impl methods |

### Why Tests Didn't Catch It

1. **No tests for `has_trusted_assumption`.** The tests focus on spans, names, and visibility.
2. **No tests with `assume`/`admit`.** Neither test uses functions that contain `assume()` or `admit()`.
3. **No tests with comments.** No test checks that `// assume(...)` or `/* admit() */` are ignored.

### Test That Would Have Caught It

```rust
#[test]
fn test_has_trusted_assumption_ignores_comments() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(
        file,
        r#"
        proof fn foo() {{
            // This function uses assume() for the tricky case
            assert(true);
        }}
        "#
    ).unwrap();

    let functions = parse_file_for_functions(file.path(), true, true, false, false, false).unwrap();
    let foo = functions.iter().find(|f| f.name == "foo").unwrap();
    // Should be false: assume() is only in a comment, not in code
    assert!(!foo.has_trusted_assumption);
}

#[test]
fn test_has_trusted_assumption_detects_actual_call() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(
        file,
        r#"
        proof fn bar() {{
            assume(true);
        }}
        "#
    ).unwrap();

    let functions = parse_file_for_functions(file.path(), true, true, false, false, false).unwrap();
    let bar = functions.iter().find(|f| f.name == "bar").unwrap();
    assert!(bar.has_trusted_assumption);
}
```

---

## Bug C4: Non-deterministic atom matching (verification.rs)

### Location
`src/verification.rs`, `find_code_name_in_atoms()` (lines 1050–1096):

```rust
for (code_name, atom) in atoms {  // HashMap iteration order is non-deterministic
    // ...
    if effective_diff < best_line_diff {
        best_match = Some(code_name);
        best_line_diff = effective_diff;
        // ...
    }
}
```

**Bug:** `atoms` is a `HashMap`. When multiple atoms tie on `effective_diff`, the chosen match depends on iteration order, which is non-deterministic. Output can vary between runs.

### Existing Tests

| Test | Location | What it tests |
|------|----------|----------------|
| `test_find_code_name_exact_match` | verification.rs:925–937 | Single matching atom |
| `test_find_code_name_suffix_match` | verification.rs:940–952 | Single matching atom (impl method) |
| `test_find_code_name_within_span_for_doc_comments` | verification.rs:955–968 | Single matching atom |
| `test_find_code_name_prefers_closest` | verification.rs:971–988 | Two atoms with *different* `effective_diff` |

### Why Tests Didn't Catch It

1. **No tie scenario.** `test_find_code_name_prefers_closest` uses atoms at lines 100+LINE_TOLERANCE and 101, so they have different `effective_diff`. There is no test where two atoms have the same `effective_diff`.
2. **Single-match tests** never exercise the “choose among equals” logic.
3. **HashMap order** is not controlled; with a tie, the result would be flaky, but no test creates a tie.

### Test That Would Have Caught It

```rust
#[test]
fn test_find_code_name_deterministic_when_tied() {
    // Two atoms with identical effective_diff (e.g., same line, same path, same name pattern)
    let loc = make_loc("foo", "src/lib.rs", 10, 20);
    let mut atoms = HashMap::new();
    atoms.insert("probe:pkg/1.0/mod/foo_a()", make_atom_entry("foo", "src/lib.rs", 10));
    atoms.insert("probe:pkg/1.0/mod/foo_b()", make_atom_entry("foo", "src/lib.rs", 10));

    let result1 = find_code_name_in_atoms(&loc, &atoms);
    let result2 = find_code_name_in_atoms(&loc, &atoms);
    // Must be deterministic: same result every time
    assert_eq!(result1, result2);
    // Ideally: assert result is one of the two valid code_names
}
```

Running this repeatedly would expose non-determinism when both atoms tie.

---

## Bug C5: Filename-only error attribution (verification.rs / path_utils.rs)

### Location
- `path_utils.rs`: `find_best_matching_path()` / `PathMatcher::find_best_match()` use `FilenameOnly` when only the filename matches.
- `verification.rs`: `FunctionIndex::find_at_line()` uses `PathMatcher` to resolve error paths to functions.

**Bug:** When Verus reports an error with only a filename (e.g. `constants_lemmas.rs`) and multiple files share that name (e.g. `field_lemmas/constants_lemmas.rs` and `edwards_lemmas/constants_lemmas.rs`), the matcher can pick the wrong file. Errors are then attributed to the wrong function.

### Existing Tests

| Test | Location | What it tests |
|------|----------|---------------|
| `test_path_matcher` | path_utils.rs:214–239 | With `"constants_lemmas.rs"`, only asserts `result.is_some()` |
| `test_find_function_at_line_prefers_suffix_match_over_filename` | verification.rs:991–1018 | Query has full path `"src/lemmas/edwards_lemmas/constants_lemmas.rs"` |
| `test_find_function_at_line_with_partial_path` | verification.rs:1021–1044 | Query has suffix `"curve25519-dalek/src/lemmas/edwards_lemmas/constants_lemmas.rs"` |

### Why Tests Didn't Catch It

1. **path_utils** (`test_path_matcher`): For `"constants_lemmas.rs"`, the test only checks `result.is_some()` and does not assert *which* of the two files is returned. The comment says “Ambiguous filename-only should return one of them,” which accepts incorrect attribution.
2. **verification** tests use query paths that allow suffix matching, so they never hit the filename-only fallback. They do not cover the case where the error path is *only* the filename.

### Test That Would Have Caught It

```rust
// In path_utils.rs
#[test]
fn test_filename_only_ambiguous_returns_none_or_deterministic() {
    let paths = vec![
        "src/lemmas/field_lemmas/constants_lemmas.rs".to_string(),
        "src/lemmas/edwards_lemmas/constants_lemmas.rs".to_string(),
    ];
    let matcher = PathMatcher::new(paths);

    // When ONLY filename is available, we cannot safely attribute - should return None
    // OR if we must return one, it must be deterministic (e.g., lexicographically first)
    let result = matcher.find_best_match("constants_lemmas.rs");
    // Option A: Reject ambiguous filename-only
    assert!(result.is_none(), "filename-only match with multiple candidates should be rejected");
    // Option B: If we allow it, verify determinism
    // let r2 = matcher.find_best_match("constants_lemmas.rs");
    // assert_eq!(result, r2);
}

// In verification.rs - integration with FunctionIndex
#[test]
fn test_error_attribution_rejects_filename_only_when_ambiguous() {
    // Setup: two files with same name, different paths, each with a function at same line
    // Error reported as "constants_lemmas.rs:52"
    // Should NOT attribute to wrong file's function
    // ...
}
```

---

## Summary Table

| Bug | Module | Root Cause | Test Gap |
|-----|--------|------------|----------|
| **S2** | extract.rs | Wrong spec for `"warning"` | Test asserts buggy mapping; no fixture with `"warning"` |
| **S3** | verus_parser.rs | String search in body text | No tests for `has_trusted_assumption`; no comments/assume/admit cases |
| **C4** | verification.rs | HashMap iteration for ties | No test with multiple atoms having same `effective_diff` |
| **C5** | path_utils.rs, verification.rs | Filename-only match when ambiguous | Tests accept any match; no check for correct file when only filename given |

---

## Recommendations

1. **S2:** Add a test that `"warning"` maps to `"unverified"`, and a merge test with a `"warning"` proof entry.
2. **S3:** Add tests for `has_trusted_assumption` with (a) `assume`/`admit` in comments (expect false) and (b) real `assume`/`admit` calls (expect true).
3. **C4:** Add a tie-breaking test with two atoms having the same `effective_diff`, and assert deterministic output (e.g., via `BTreeMap` or explicit tie-breaker).
4. **C5:** Either reject filename-only matches when multiple files share the name, or define and test a deterministic rule (e.g., lexicographic order), and add a verification test that checks error attribution in the ambiguous case.
