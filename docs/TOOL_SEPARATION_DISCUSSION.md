# Tool Separation Discussion

Should `probe-verus atomize` and `probe-verus extract` be separate tools?

**Decision (for now):** Keep together for simplicity and quick experimentation.

**Revisit later:** Once usage patterns become clear.

---

## Current State

```
probe-verus
├── atomize    → Call graph from SCIP index (uses verus-analyzer)
├── verify     → Verification analysis (uses cargo verus)
└── list-functions → List functions (shared utility)
```

---

## Arguments for Keeping Together

| Pro | Reasoning |
|-----|-----------|
| **Shared code** | Both use `verus_syn` for function span parsing |
| **Same workflow** | Both analyze Verus projects, same audience |
| **Single install** | One `cargo install probe-verus` gets everything |
| **Idiomatic Rust** | `cargo`, `rustup`, `git` all use subcommands |
| **Future synergy** | Could combine: "show call graph of failed functions" |

---

## Arguments for Separating

| Pro | Reasoning |
|-----|-----------|
| **Single responsibility** | Each tool does one thing well |
| **Clearer naming** | The `atomize` command is about SCIP; verification has nothing to do with SCIP |
| **Smaller binaries** | Don't pull in verification deps if only need call graph |
| **Independent evolution** | Could version/release separately |

---

## Rust Community Patterns

### Same tool, subcommands
When operations target the **same conceptual thing**:
- `cargo build/test/run` - all about a crate
- `git add/commit/push` - all about a repository

### Separate tools
When they serve **different conceptual purposes**:
- `rustfmt` vs `clippy` vs `miri` - formatting, linting, interpretation
- `cargo` vs `rustup` - project management vs toolchain management

---

## The Key Question

**What is the conceptual unity?**

| Command | Conceptual Purpose |
|---------|-------------------|
| `atomize` | Static code structure (SCIP-based dependency graph) |
| `extract` | Dynamic verification results (parsing Verus output) |

These are **different analytical lenses** on the same project, not the same operation.

---

## If We Decide to Separate Later

Proposed structure:

```
verus-tools/                    # Cargo workspace
├── Cargo.toml                  # Workspace manifest
├── probe-atomize/              # Call graph generation (SCIP-focused)
│   ├── Cargo.toml
│   └── src/
├── probe-verify/               # Verification analysis
│   ├── Cargo.toml
│   └── src/
└── probe-common/               # Shared library
    ├── Cargo.toml
    └── src/
        ├── verus_parser.rs     # verus_syn-based parsing
        └── function_info.rs    # FunctionInfo, FunctionLocation
```

**Naming options for verification tool:**
- `probe-verify`
- `probe-results`
- `probe-check`
- `probe-analyze`

**Benefits of separation:**
- `probe-atomize` stays true to its purpose (SCIP-based)
- `probe-verify` is clearly about verification
- Shared code lives in `probe-common` crate
- Each tool can be installed independently

---

## Decision Criteria for Future

Consider separating if:
1. Users only need one tool, not both
2. The tools evolve at different rates
3. Binary size becomes a concern
4. Conceptual confusion arises between commands

Consider keeping together if:
1. Users typically run both in the same workflow
2. We want to add combined features (e.g., call graph of verified functions)
3. Maintenance overhead of multiple crates is not worth it
