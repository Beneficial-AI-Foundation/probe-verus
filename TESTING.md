# Testing

## Quick start

```bash
cargo test
```

## Test layers

| Layer | Count | Location | Requires |
|-------|-------|----------|----------|
| Unit tests | 138 | `src/**/*.rs` (`#[cfg(test)]` modules) | Nothing |
| Integration tests | 24 | `tests/*.rs` | Nothing |
| Backward compat test | 1 | `tests/extract_backward_compat.rs` | verus-analyzer, scip, `verus_micro` fixture |
| Live extract test | 1 (ignored) | `tests/extract_check.rs` | verus-analyzer, scip, verus |
| SCIP integration tests | varies | `tests/duplicate_symbols.rs`, `tests/function_coverage.rs` | `data/curve_top.json` (CI-generated) |

## Unit tests

138 tests across the library and commands modules, covering:

- Display-name enrichment (trait impls, inherent impls, free functions)
- SCIP symbol parsing and external function detection
- Code-name normalization and envelope-aware loading
- `rust-qualified-name` derivation
- Verus-specific kind classification (exec/proof/spec)
- Spec extraction (requires/ensures parsing)
- Taxonomy categorization
- Verification output analysis
- Tool management and caching

Run only unit tests: `cargo test --lib`

## Integration tests

24 tests across six test files:

| File | Tests | What they cover |
|------|-------|-----------------|
| `tests/extract_check.rs` | 3 + 1 ignored | Loads `tests/fixtures/unified_test/atoms.json` as `AtomEnvelope`, validates structural integrity, `probe:` key prefixes, and Verus-specific kinds (exec/proof/spec) |
| `tests/extract_backward_compat.rs` | 1 | Runs extract on `verus_micro`, compares output against golden file for backward compatibility |
| `tests/unified_extract.rs` | 7 | Merges atoms + specs + proofs fixtures into unified output; verifies `primary-spec` text, `verification-status` mapping, `spec-labels` flow, external stub handling, and JSON serialization format |
| `tests/merge_atoms.rs` | 3 | Stub replacement by real atoms, cross-project edge preservation, merged output matches expected fixture |
| `tests/duplicate_symbols.rs` | -- | Trait impl disambiguation (requires `data/curve_top.json`) |
| `tests/function_coverage.rs` | -- | Critical function presence checks (requires `data/curve_top.json`) |

The `duplicate_symbols` and `function_coverage` tests require a SCIP index
(`data/curve_top.json`) generated from dalek-lite. In CI, this is produced by
the separate `integration-test` job. Locally, these tests are skipped unless
the data file exists.

## Backward compatibility test

1 test that runs `cmd_extract` (with `skip_verify`) on the `verus_micro` fixture
and compares the output against a committed golden JSON file.

| Test | What it checks |
|------|---------------|
| `extract_backward_compat` | All fields in the golden file still exist and have the same values. New fields (additive changes) are printed as info but do not fail. |

**Prerequisites:** `verus-analyzer` and `scip` must be installed (or available via
`~/.probe-verus/tools/`). The `verus_micro` fixture must exist at
`../probe/probe-extract-check/tests/fixtures/verus_micro`. The test skips
gracefully if either prerequisite is missing.

**Golden file:** `tests/fixtures/extract_golden/golden.json`

Run the test:

```bash
cargo test --test extract_backward_compat -- --nocapture
```

Update the golden file after intentional output changes:

```bash
BLESS=1 cargo test --test extract_backward_compat -- --nocapture
```

Review the diff and commit with an explanation of what changed and why.

## Live extract test

1 ignored test that calls `cmd_extract` via the library API:

| Test | What it checks |
|------|---------------|
| `live_extract_structural_check` | Runs the full extract pipeline (atomize + specify + verify) on the `verus_micro` fixture, then validates the unified output with `check_all`. |

**Prerequisites:** `verus-analyzer` (or `rust-analyzer`), `scip`, and `verus` must be installed.

Run with:

```bash
cargo test -- --include-ignored
```

## CI

`.github/workflows/ci.yml` runs on push/PR to `main`:

1. **Format** -- `cargo fmt --all -- --check`
2. **Clippy** -- `cargo clippy --all-targets -- -D warnings`
3. **Test** -- `cargo test --verbose` (all tests except `#[ignore]` and SCIP-dependent)
4. **Integration Test** (separate job) -- clones dalek-lite, generates SCIP index, runs `duplicate_symbols`, `function_coverage`, and `extract_backward_compat` tests

The CI checks out the sibling `probe` repo alongside for the
`probe-extract-check` dev-dependency.

## Adding tests

- **Unit tests:** add to the `#[cfg(test)] mod tests` block in the relevant `src/` module.
- **Integration tests:** add to the appropriate `tests/*.rs` file, or create a new one. Use `probe_extract_check::{check_all, load_extract_json}` for structural validation.
- **New fixtures:** place in `tests/fixtures/` with a descriptive subdirectory name.
