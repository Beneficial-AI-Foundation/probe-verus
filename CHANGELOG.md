# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

See the [Versioning Policy section in CLAUDE.md](CLAUDE.md#versioning-policy) for
what constitutes a breaking change.

## [3.0.0] - 2026-03-10

### Breaking
- **Command rename**: `verify` -> `run-verus` (standalone Verus verification runner)
- **Command rename**: `run` -> `verify` (unified pipeline: atomize + specify + run-verus)
- **Command removed**: `run` no longer exists as a standalone command
- `verify` now runs a 3-step pipeline (atomize, specify, run-verus) instead of just cargo verus
- Old `--atomize-only`/`--verify-only` flags replaced by `--skip-atomize`/`--skip-specify`/`--skip-verify`
- Proofs envelope `tool.command` changed from `"verify"` to `"run-verus"`
- Verification-report envelope `tool.command` changed from `"verify"` to `"run-verus"`
- Summary file renamed from `run_summary.json` to `verify_summary.json`
- Summary envelope schema changed from `probe-verus/run-summary` to `probe-verus/verify-summary`
- Docker entrypoint changed from `probe-verus run` to `probe-verus verify`
- Atomize default output filename changed from `verus_<pkg>_<ver>.json` to `verus_<pkg>_<ver>_atoms.json`
- Unified verify output uses the unsuffixed name `verus_<pkg>_<ver>.json` (previously used by atomize)

### Added
- New unified `verify` command combining atomize + specify + run-verus in a single pipeline
- `verify` produces a single unified JSON (schema `probe-verus/verify`) where each atom entry includes optional `verification-status` and `specified` fields, matching `probe-lean/verify` output structure
- `--separate-outputs` flag on `verify` to also write individual atoms, specs, and proofs files
- `--skip-atomize`, `--skip-specify`, `--skip-verify` flags on `verify` to selectively skip steps
- `--with-atoms`, `--with-spec-text`, `--taxonomy-config` flags on `verify` for specify step configuration
- `--verus-args` flag on `verify` to pass extra arguments to cargo verus
- `UnifiedAtom` type composing `AtomWithLines` with optional `verification-status` and `specified`
- `specify_internal` function and `SpecifyInternalConfig` struct for pipeline integration

### Changed
- `verify_internal` renamed to `run_verus_internal` (internal API, not user-facing)
- `VerifyInternalConfig` remains unchanged (used by `run_verus_internal`)

## [2.1.0] - 2026-03-09

### Added
- `specs-data`: include `external_body` functions in output with `category: "external"` (previously silently skipped)

### Fixed
- `specs-data`: remove duplicate cross-references caused by redundant text-based scanning on top of AST-extracted calls
- `specs-data`: fix contract text duplication where `requires`/`ensures` clauses appeared twice (once from signature, once from dedicated fields)
- `verus_parser`: skip block comments (`/* ... */`) when extracting function signature text
- `verus_parser`: preserve relative indentation in multi-line signature text instead of fully trimming each line

## [2.0.0] - 2026-03-06

### Breaking
- Rename `FunctionMode` enum to `DeclKind` and JSON field `"mode"` to `"kind"` across all output formats. This unifies the declaration classification field name with `probe-lean`, enabling a single web viewer to handle both Verus and Lean atom graphs.
- All JSON outputs now wrapped in Schema 2.0 metadata envelope (structured `tool`, `source`, `timestamp` fields). Consumers must use `data` key or the `unwrap_envelope` function to access the payload.
- Default output paths changed from flat files (e.g. `atoms.json`) to `.verilib/probes/verus_<pkg>_<ver>[_suffix].json`. The `--output` flag still overrides this.
- `run` command now writes atoms/proofs to `.verilib/probes/` instead of the `--output` directory (summary file still goes to `--output`).
- `run_summary.json` now wrapped in Schema 2.0 envelope (`probe-verus/run-summary`).
- Merged-atoms envelope `tool.name` is now `"probe"` (not `"probe-verus"`) per the canonical envelope spec.
- `verify` command: enriched output (with atoms) uses `probe-verus/proofs` schema; unenriched output (without atoms) uses `probe-verus/verification-report` schema.
- `verify -a` changed from `Option<Option<PathBuf>>` to `Option<PathBuf>` -- auto-discovers atoms in `.verilib/probes/` when omitted.

### Added
- Schema 2.0 metadata envelope for all JSON outputs (`src/metadata.rs`): includes `tool` (name, version, command), `source` (repo, commit, language, package, package-version), and `timestamp` fields
- `language` field on `AtomWithLines` (defaults to `"rust"`) for cross-language merge compatibility
- `--project-path` flag on `stubify`, `specify`, and `specs-data` subcommands for explicit project root when the input path is outside the project tree
- `find_default_atoms_path` function for version-mismatch resilient atoms lookup, wired into `verify`'s auto-discovery
- `AtomizeInternalConfig` and `VerifyInternalConfig` structs to replace long parameter lists and `#[allow(clippy::too_many_arguments)]`
- `unwrap_envelope` accepts any envelope with a schema containing `/` and a `data` field, providing backward compatibility for bare JSON and cross-tool interop
- `extract_envelope_inputs` (plural) propagates provenance from nested merged envelopes on recursive merge
- `specs-data` command output now wrapped in Schema 2.0 envelope (`probe-verus/specs-data`)
- `#[serde(rename_all = "kebab-case")]` on all envelope structs, future-proofing against snake_case serialization bugs
- Unit tests for `extract_envelope_inputs`, `wrap_merged_envelope`, merged-envelope-through-unwrap roundtrip, and recursive merge provenance

### Changed
- Rename helper functions: `convert_mode` -> `convert_kind`, `mode_to_string` -> `kind_to_string`
- Package version fallback uses 7-char git short hash instead of `"unknown"`, matching probe-lean and the envelope-rationale spec
- `chrono` dependency simplified (removed unnecessary `serde` feature)
- Eliminated `RunContext` struct; `run` command now passes shared metadata via config structs
- Fixed `&PathBuf` anti-pattern in internal APIs (now uses `&Path`)

### Fixed
- `verify_internal` (used by `run` command) now produces `probe-verus/proofs` schema with `ProofsOutput` format when atoms are available, matching `cmd_verify` behavior (previously always used `verification-report` schema, creating a filename/schema mismatch)
- GitHub Action (`action/action.yml`): `jq` commands now unwrap the Schema 2.0 envelope via `.data` so atom counts and verification result parsing work correctly
- Merged-atoms envelope `tool.version` now uses plain semver (`"2.0.0"`) instead of compound `"probe-verus/2.0.0"`, matching the envelope-rationale spec
- Updated `action/README.md` and `docker/README.md` output format docs for Schema 2.0 envelope and new file paths

### Note
- Schema values `probe-verus/specs-data` and `probe-verus/run-summary` need to be registered in the upstream [envelope-rationale.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md) known values list

## [1.5.0] - 2026-03-02

### Added
- `setup` subcommand to install and manage external tool dependencies (verus-analyzer, scip)
- Auto-download tool manager: probe-verus can fetch the latest stable verus-analyzer and scip to `~/.probe-verus/tools/` on demand, with env var overrides (`PROBE_VERUS_ANALYZER_VERSION`, `PROBE_SCIP_VERSION`) and compiled-in fallback versions
- `--auto-install` flag on `atomize` and `run` subcommands for non-interactive CI tool download
- Pre-built binary releases via cargo-dist for Linux (x86_64, aarch64), macOS (Intel, Apple Silicon), and Windows
- Shell and PowerShell installer scripts for one-line installation

### Changed
- Tool resolution now checks `~/.probe-verus/tools/` (managed) before PATH (user-installed), falling back to helpful error messages with install instructions

### Fixed
- `PlatformNotSupported` error messages now link to the correct upstream repo per tool (not always verus-analyzer)
- `NotInstalled` error for rust-analyzer now recommends `rustup component add rust-analyzer` instead of `probe-verus setup`
- Windows verus-analyzer installs now correctly handle `.zip` archives (previously assumed gzip)
- `probe-verus setup` now skips tools unsupported on the current platform instead of failing
- `setup` subcommand help text now accurately describes the version resolution strategy
- Env var tests use a mutex guard to prevent parallel test races

## [1.4.0] - 2026-03-02

### Added
- `callee-crates` subcommand to find which crates a function's callees belong to at a given depth, with corresponding README documentation for its usage, options, and JSON output format

## [1.3.0] - 2026-02-28

### Added
- `merge-atoms` subcommand to combine independently-indexed atoms.json files ([#11](https://github.com/Beneficial-AI-Foundation/probe-verus/issues/11))
- `normalize_code_name` public utility for consistent code_name formatting
- Manual integration test script for libsignal + libcrux-ml-kem equivalence (`scripts/`)

### Changed
- Deterministic `atoms.json` output: sorted keys (`BTreeMap`) and sorted dependencies (`BTreeSet`)

### Fixed
- Strip trailing `.` from external function code_names in `symbol_to_code_name` fallback path
- Allow deserialization of atoms.json files missing optional `dependencies-with-locations` field

## [1.2.0] - 2026-02-28

### Added
- Track calls to external (non-workspace) functions in call graph
- Stub atoms for external function dependencies in `atoms.json` output
- Installation scripts for Verus development tools (`tools/`)
- `CHANGELOG.md` and semver versioning policy ([#7](https://github.com/Beneficial-AI-Foundation/probe-verus/issues/7))

### Fixed
- Strip reference/lifetime prefix from impl Self type in parser
- Resolve enriched display name mismatch in span map lookup

## [1.1.0] - 2026-02-24

### Added
- `tracked-csv` subcommand for generating dashboard CSV files
- `specs-data` subcommand for generating specs browser JSON
- `--verus-args` flag on `verify` to forward extra arguments to Verus
- `--rust-analyzer` flag on `atomize` to use rust-analyzer instead of verus-analyzer
- `--allow-duplicates` flag on `atomize` to continue on duplicate code_names
- Enriched `display_name` with impl type for method nodes (e.g., `Type::method`)
- Spec taxonomy classification via `--taxonomy-config` on `specify`
- `sub_module` field in `specs-data` output for backend sub-grouping
- `external_body`/`no_decreases` attribute detection

### Changed
- Docker builds now track `Cargo.lock` for reproducible builds

### Fixed
- Disambiguation fallback for duplicate code_names in atomize
- Handle enriched display_name in code_name suffix check
- Docker: avoid GitHub API rate limiting for Verus install
- CSV writer properly quotes fields with commas
- `tracked-csv`: use `fn` keyword line instead of declaration span start
- `specify`: match trait impl methods to atoms via suffix name matching
- `verify`: span-based line matching for trait impl methods

## [1.0.0] - 2026-01-27

### Breaking
- Renamed tool to `probe-verus` (from `rust-analyzer-test`)
- Subcommands renamed to `atomize` and `list-functions`
- Renamed `scip-name` to `code-name` throughout output schema
- Output format changed from JSON array to dictionary keyed by `code-name`
- `proofs.json` now uses `code-name` keys with new schema
- `specify` output schema restructured
- CLI options standardized across subcommands

### Added
- `atomize` subcommand (replaces old default behavior)
- `list-functions` subcommand
- `verify` subcommand with Verus integration and function-level analysis
- `specify` subcommand for extracting function specifications
- `stubify` subcommand for converting .md files with YAML frontmatter to JSON
- `run` subcommand for Docker/CI usage (atomize + verify pipeline)
- `--with-locations` flag on `atomize` for per-call location tracking
- `--with-spec-text` flag on `specify` for raw specification text
- Verus function mode (`exec`, `proof`, `spec`) in atomize output
- `code-module` field in atom output
- Docker support with configurable Verus version
- GitHub Action for Verus verification
- Interval tree for O(log n) error-to-function mapping in `verify`
- SCIP data caching in `<project>/data/`
- MIT license

### Fixed
- Handle duplicate SCIP symbols for trait implementations (4-component unique key)
- Repair verus-analyzer SCIP symbols to match rust-analyzer format
- Use span containment instead of tolerance for function end line matching
- Add `cfg_if!` macro support in verus_syn parser
- Disambiguate trait impls using SCIP type hints and definition-site type context
- Function matching for large doc comment blocks in `specify`

## [0.1.0] - 2026-01-15

Initial release. SCIP-based call graph generation for Rust/Verus projects.

[Unreleased]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.5.0...v2.0.0
[1.5.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v0.1.0...v1.0.0
[0.1.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/releases/tag/v0.1.0
