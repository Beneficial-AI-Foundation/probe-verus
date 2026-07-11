# probe-verus Knowledge Base

Local knowledge base for probe-verus. For cross-cutting ecosystem concerns
(schema, properties, architecture, glossary), the shared
[probe KB](https://github.com/Beneficial-AI-Foundation/probe/tree/main/kb)
is authoritative. This index organizes probe-verus-specific documentation.

## Shared KB (authoritative for cross-cutting concerns)

Located at `../probe/kb/` (sibling repo). Start with
[kb/index.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/index.md).

| File | What it governs |
|------|-----------------|
| [engineering/properties.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/properties.md) | Invariants P1-P19 all probe tools must satisfy |
| [engineering/schema.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/schema.md) | Schema 2.0 envelope and atom field definitions |
| [engineering/architecture.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/architecture.md) | Five-tool separation, data flow, per-tool roles |
| [engineering/glossary.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/glossary.md) | Precise domain terminology |
| [tools/probe-verus.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-verus.md) | probe-verus-specific page in the shared KB |

## Auditor skills (in probe repo)

Located at `../probe/.claude/skills/`. Run these after significant changes
(the "Ralph Loop" — see probe's CLAUDE.md).

| Skill | Purpose |
|-------|---------|
| [code-quality-auditor.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/.claude/skills/code-quality-auditor.md) | Check implementation against KB properties, architecture, documentation staleness |
| [test-quality-auditor.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/.claude/skills/test-quality-auditor.md) | Verify test coverage and quality |
| [ambiguity-auditor.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/.claude/skills/ambiguity-auditor.md) | Find specification ambiguities and contradictions |

## Local documentation

### User reference

| File | Description |
|------|-------------|
| [docs/USAGE.md](../docs/USAGE.md) | Full command reference with all options and examples |
| [docs/SCHEMA.md](../docs/SCHEMA.md) | JSON output schema specification for all commands |
| [docs/format.md](../docs/format.md) | Atoms format spec with parsing examples (TypeScript, Python, Rust) |

### Design and architecture

| File | Description |
|------|-------------|
| [docs/HOW_IT_WORKS.md](../docs/HOW_IT_WORKS.md) | SCIP pipeline, verus_syn spans, trait disambiguation, verification parsing |
| [docs/VERIFICATION_ARCHITECTURE.md](../docs/VERIFICATION_ARCHITECTURE.md) | Verification analysis architecture and known inefficiencies |
| [docs/SPEC_TAXONOMY_DESIGN.md](../docs/SPEC_TAXONOMY_DESIGN.md) | Spec taxonomy: AST extraction + TOML rule engine design analysis |

### Investigation notes (bug fixes and symbol resolution)

| File | Description |
|------|-------------|
| [docs/DUPLICATE_SYMBOL_BUG.md](../docs/DUPLICATE_SYMBOL_BUG.md) | Duplicate SCIP symbol analysis and resolution |
| [docs/IMPL_OVERWRITE_BUG_FIX.md](../docs/IMPL_OVERWRITE_BUG_FIX.md) | Impl block overwrite bug investigation |
| [docs/TRAIT_IMPL_SYMBOL_PATTERNS.md](../docs/TRAIT_IMPL_SYMBOL_PATTERNS.md) | SCIP symbol patterns for trait implementations |
| [docs/SCIP_SYMBOL_FORMAT_COMPARISON.md](../docs/SCIP_SYMBOL_FORMAT_COMPARISON.md) | verus-analyzer vs rust-analyzer SCIP symbol format comparison |
| [docs/FROM_DEPENDENCY_RESOLUTION.md](../docs/FROM_DEPENDENCY_RESOLUTION.md) | Dependency resolution for From trait implementations |

### Planning and discussion

| File | Description |
|------|-------------|
| [docs/PROBE_RUST_SPLIT_BRAINSTORM.md](../docs/PROBE_RUST_SPLIT_BRAINSTORM.md) | Brainstorm on splitting Rust-specific logic into probe-rust |
| [docs/TOOL_SEPARATION_DISCUSSION.md](../docs/TOOL_SEPARATION_DISCUSSION.md) | Discussion on tool separation boundaries |
| [docs/TEST_GAP_ANALYSIS.md](../docs/TEST_GAP_ANALYSIS.md) | Test coverage gap analysis |
