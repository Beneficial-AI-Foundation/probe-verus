# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

See the [Versioning Policy section in CLAUDE.md](CLAUDE.md#versioning-policy) for
what constitutes a breaking change.

## [Unreleased]

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

[Unreleased]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/compare/v0.1.0...v1.0.0
[0.1.0]: https://github.com/Beneficial-AI-Foundation/probe-verus/releases/tag/v0.1.0
