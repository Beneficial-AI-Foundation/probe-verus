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
| [engineering/schema.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/schema.md) | Schema 3.0 envelope and atom field definitions |
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
