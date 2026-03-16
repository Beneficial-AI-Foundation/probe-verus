# probe-verus Data Schemas

Version: 4.0
Date: 2026-03-16

This document specifies the concrete JSON `data` payloads produced by each
probe-verus subcommand.  It complements the language-agnostic
[envelope-rationale.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md)
which defines the envelope wrapper; this document defines what goes **inside**
the `data` field for each `schema` value.

---

## Common Types

These types appear across multiple schemas.

### CodeTextInfo

Line range of a function body (1-based, inclusive).

```json
{
  "lines-start": 42,
  "lines-end": 67
}
```

| Field | Type | Description |
|-------|------|-------------|
| `lines-start` | integer | First line of the function (1-based) |
| `lines-end` | integer | Last line of the function (1-based, inclusive) |

### DeclKind

Declaration kind, serialized as a lowercase string.

| Value | Meaning |
|-------|---------|
| `"exec"` | Executable code — compiled and verified |
| `"proof"` | Proof code — verified but erased at runtime |
| `"spec"` | Specification code — defines logical properties, erased at runtime |

### Code-Name Format

Atoms, specs, and proofs use **probe code-names** as dictionary keys.  The
format is:

```
probe:<crate>/<version>/<module-path>/<Type>#<Trait>#<method>()
```

Examples:
- `probe:curve25519-dalek/4.1.3/montgomery/MontgomeryPoint#mul()`
- `probe:curve25519-dalek/4.1.3/edwards/decompress()`
- `probe:vstd/0.0.0-2026-01-11-0057/arithmetic/mul/lemma_mul_is_commutative()`

For external (non-workspace) functions whose SCIP symbol references the
standard library:

```
probe:core/https://github.com/rust-lang/rust/library/core/option/impl#map()
```

---

## 1. `probe-verus/atoms` — Call Graph Atoms

**Produced by:** `atomize`
**Envelope schema:** `"probe-verus/atoms"`

### Data Shape

`data` is an object keyed by code-name.  Each value is an `AtomWithLines`:

```json
{
  "probe:my-crate/1.0.0/module/MyType#method()": {
    "display-name": "MyType::method",
    "dependencies": [
      "probe:my-crate/1.0.0/module/helper()",
      "probe:other-crate/2.0.0/foo/bar()"
    ],
    "dependencies-with-locations": [
      {
        "code-name": "probe:my-crate/1.0.0/module/helper()",
        "location": "inner",
        "line": 55
      }
    ],
    "code-module": "module",
    "code-path": "src/module.rs",
    "code-text": { "lines-start": 42, "lines-end": 67 },
    "kind": "exec",
    "language": "rust"
  }
}
```

### Field Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `display-name` | string | yes | Human-readable name (e.g. `"MyType::method"`) |
| `dependencies` | array of strings | yes | Sorted code-names of callees |
| `dependencies-with-locations` | array of objects | no | Present only when `--with-locations` is used |
| `code-module` | string | yes | Module path extracted from the code-name (may be empty for top-level functions) |
| `code-path` | string | yes | Relative source file path (empty string for external stubs) |
| `code-text` | CodeTextInfo | yes | Line range of the function body |
| `kind` | DeclKind | yes | `"exec"`, `"proof"`, or `"spec"` |
| `language` | string | yes | Source language; always `"rust"` for probe-verus (defaults to `"rust"` if absent for backward compat) |

### DependencyWithLocation

Only present when `--with-locations` is passed to `atomize`.

| Field | Type | Description |
|-------|------|-------------|
| `code-name` | string | Code-name of the callee |
| `location` | string | `"precondition"`, `"postcondition"`, or `"inner"` |
| `line` | integer | 1-based line number of the call site |

### External Stubs

Functions called as dependencies but defined outside the workspace get stub
entries with `code-path: ""` and `code-text: {"lines-start": 0, "lines-end": 0}`.

---

## 2. `probe-verus/proofs` — Verification Results (Per-Function)

**Produced by:** `run-verus --with-atoms` (or when atoms are auto-discovered), or by the `extract` unified pipeline
**Envelope schema:** `"probe-verus/proofs"`
**Envelope `tool.command`:** `"run-verus"`

### Data Shape

`data` is an object keyed by code-name.  Each value is a
`FunctionVerificationEntry`:

```json
{
  "probe:my-crate/1.0.0/module/lemma_foo()": {
    "code-path": "src/module.rs",
    "code-line": 42,
    "verified": true,
    "status": "success"
  },
  "probe:my-crate/1.0.0/module/lemma_bar()": {
    "code-path": "src/module.rs",
    "code-line": 80,
    "verified": false,
    "status": "failure"
  }
}
```

### Field Reference

| Field | Type | Description |
|-------|------|-------------|
| `code-path` | string | Relative source file path |
| `code-line` | integer | 1-based line number of the function |
| `verified` | boolean | `true` if the function passed verification |
| `status` | string | `"success"`, `"failure"`, `"sorries"`, or `"warning"` |

### Status Values

| Value | Meaning |
|-------|---------|
| `"success"` | Passed verification without trusted assumptions |
| `"failure"` | Had verification errors |
| `"sorries"` | Contains `assume()` or `admit()` — not fully verified |
| `"warning"` | Verification passed with warnings |

---

## 3. `probe-verus/verification-report` — Verification Results (Aggregate)

**Produced by:** `run-verus` when no atoms file is available
**Envelope schema:** `"probe-verus/verification-report"`
**Envelope `tool.command`:** `"run-verus"`

### Data Shape

`data` is an `AnalysisResult` object:

```json
{
  "status": "verification_failed",
  "summary": {
    "total_functions": 25,
    "failed_functions": 2,
    "verified_functions": 20,
    "unverified_functions": 3,
    "verification_errors": 2,
    "compilation_errors": 0,
    "compilation_warnings": 1
  },
  "verification": {
    "failed_functions": [ ... ],
    "verified_functions": [ ... ],
    "unverified_functions": [ ... ],
    "errors": [ ... ]
  },
  "compilation": {
    "errors": [ ... ],
    "warnings": [ ... ]
  }
}
```

### Top-Level Fields

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `"success"`, `"verification_failed"`, `"compilation_failed"`, or `"functions_only"` |
| `summary` | AnalysisSummary | Counts |
| `verification` | VerificationResult | Per-function verification details |
| `compilation` | CompilationResult | Compilation errors and warnings |

### AnalysisSummary

| Field | Type | Description |
|-------|------|-------------|
| `total_functions` | integer | Total verifiable functions (those with requires/ensures) |
| `failed_functions` | integer | Count of functions with verification errors |
| `verified_functions` | integer | Count of functions that passed verification |
| `unverified_functions` | integer | Count of functions with `assume()`/`admit()` |
| `verification_errors` | integer | Total verification error count |
| `compilation_errors` | integer | Compilation error count |
| `compilation_warnings` | integer | Compilation warning count |

### VerificationResult

| Field | Type | Description |
|-------|------|-------------|
| `failed_functions` | array of FunctionLocation | Functions that failed verification |
| `verified_functions` | array of FunctionLocation | Functions that passed verification |
| `unverified_functions` | array of FunctionLocation | Functions with trusted assumptions |
| `errors` | array of VerificationFailure | Detailed error information |

### FunctionLocation

| Field | Type | Description |
|-------|------|-------------|
| `display-name` | string | Human-readable function name |
| `code-name` | string or null | Probe code-name (present only when enriched with atoms) |
| `code-path` | string | Relative source file path |
| `code-text` | CodeTextInfo | Line range |

### VerificationFailure

| Field | Type | Description |
|-------|------|-------------|
| `error_type` | string | e.g. `"assertion failed"`, `"postcondition not satisfied"` |
| `file` | string or null | Source file path |
| `line` | integer or null | 1-based line number |
| `column` | integer or null | 1-based column number |
| `message` | string | Error message text |
| `assertion_details` | array of strings | Context lines around the assertion |
| `full_error_text` | string | Complete error output |

### CompilationResult

| Field | Type | Description |
|-------|------|-------------|
| `errors` | array of CompilationError | Compilation errors |
| `warnings` | array of CompilationError | Compilation warnings |

### CompilationError

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | Error or warning message |
| `file` | string or null | Source file path |
| `line` | integer or null | 1-based line number |
| `column` | integer or null | 1-based column number |
| `full_message` | array of strings | All output lines for this error |

---

## 4. `probe-verus/specs` — Function Specifications

**Produced by:** `specify`
**Envelope schema:** `"probe-verus/specs"`

### Data Shape

`data` is an object keyed by code-name.  Each value is a `SpecifyEntry`
(a `FunctionInfo` flattened with optional taxonomy labels):

```json
{
  "probe:my-crate/1.0.0/module/MyType#method()": {
    "code-path": "src/module.rs",
    "spec-text": { "lines-start": 42, "lines-end": 67 },
    "kind": "exec",
    "specified": true,
    "has_requires": true,
    "has_ensures": true,
    "has_decreases": false,
    "has_trusted_assumption": false,
    "is_external_body": false,
    "has_no_decreases_attr": false,
    "requires_text": "x > 0",
    "ensures_text": "result > x",
    "ensures-calls": ["helper"],
    "requires-calls": [],
    "spec-labels": ["safety-critical"]
  }
}
```

### Field Reference

All fields from `FunctionInfo` are flattened into the entry.  The `name` field
is **not** serialized (the code-name key serves as the identifier).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `code-path` | string | no | Relative source file path |
| `spec-text` | object | yes | `{"lines-start": N, "lines-end": N}` — line range of the function (including attributes/doc comments) |
| `kind` | DeclKind | yes | `"exec"`, `"proof"`, or `"spec"` |
| `kind_display` | string | no | Human-readable kind (present when `--show-kind` was used) |
| `visibility` | string | no | e.g. `"pub"`, `"pub(crate)"` (present when `--show-visibility` was used) |
| `context` | string | no | `"impl"`, `"trait"`, or `"standalone"` |
| `specified` | boolean | yes | Whether the function has any spec (requires or ensures) |
| `has_requires` | boolean | yes | Has a `requires` clause |
| `has_ensures` | boolean | yes | Has an `ensures` clause |
| `has_decreases` | boolean | yes | Has a `decreases` clause |
| `has_trusted_assumption` | boolean | yes | Body contains `assume()` or `admit()` |
| `is_external_body` | boolean | yes | Has `#[verifier::external_body]` |
| `has_no_decreases_attr` | boolean | yes | Has `#[verifier::exec_allows_no_decreases_clause]` |
| `requires_text` | string | no | Raw text of the requires clause (only with `--with-spec-text`) |
| `ensures_text` | string | no | Raw text of the ensures clause (only with `--with-spec-text`) |
| `ensures-calls` | array of strings | no | Short names of functions called in ensures (omitted if empty) |
| `requires-calls` | array of strings | no | Short names of functions called in requires (omitted if empty) |
| `ensures-calls-full` | array of strings | no | Fully qualified paths of function calls in ensures |
| `requires-calls-full` | array of strings | no | Fully qualified paths of function calls in requires |
| `ensures-fn-calls` | array of strings | no | Non-method function calls in ensures |
| `ensures-method-calls` | array of strings | no | Method calls in ensures |
| `requires-fn-calls` | array of strings | no | Non-method function calls in requires |
| `requires-method-calls` | array of strings | no | Method calls in requires |
| `display-name` | string | no | Display name including impl type |
| `impl-type` | string | no | The impl block type name, if a method |
| `doc-comment` | string | no | Extracted `///` doc comments |
| `signature-text` | string | no | Function signature text |
| `body-text` | string | no | Full function body text (for spec functions) |
| `module-path` | string | no | Module path derived from file path |
| `spec-labels` | array of strings | no | Taxonomy classification labels (omitted if empty) |

---

## 5. `probe-verus/extract` — Unified Extract Output

**Produced by:** `extract` (unified pipeline)
**Envelope schema:** `"probe-verus/extract"`
**Envelope `tool.command`:** `"extract"`

### Overview

The primary output of the `extract` command.  Each entry is an atom enriched
with optional `verification-status` and structured `specs` fields, matching the
`probe-lean/verify` output structure.

Specifications are separated from dependencies: function calls in
`requires`/`ensures` clauses are removed from `dependencies` (when location
data is available) and instead appear in the `specs` array.

By default, only this file is produced.  Pass `--separate-outputs` to also
write the individual atoms, specs, and proofs files.

### Data Shape

`data` is an object keyed by code-name.  Each value is a `UnifiedAtom`
(an `AtomWithLines` with two optional fields):

```json
{
  "probe:my-crate/1.0.0/module/MyType#method()": {
    "display-name": "MyType::method",
    "dependencies": [
      "probe:my-crate/1.0.0/module/helper()"
    ],
    "code-module": "module",
    "code-path": "src/module.rs",
    "code-text": { "lines-start": 42, "lines-end": 67 },
    "kind": "exec",
    "language": "rust",
    "verification-status": "verified",
    "specs": [
      {
        "kind": "precondition",
        "text": "requires\n    x > 0,\n    y < 100",
        "clauses": ["x > 0", "y < 100"],
        "calls": ["is_valid"],
        "calls-full": ["crate::specs::is_valid"]
      },
      {
        "kind": "postcondition",
        "text": "ensures\n    result > x",
        "clauses": ["result > x"],
        "calls": ["helper_spec"],
        "calls-full": ["crate::specs::helper_spec"]
      }
    ]
  },
  "probe:my-crate/1.0.0/module/unspecified_fn()": {
    "display-name": "unspecified_fn",
    "dependencies": [],
    "code-module": "module",
    "code-path": "src/module.rs",
    "code-text": { "lines-start": 80, "lines-end": 90 },
    "kind": "exec",
    "language": "rust",
    "specs": []
  },
  "probe:external/1.0.0/other/func()": {
    "display-name": "func",
    "dependencies": [],
    "code-module": "other",
    "code-path": "",
    "code-text": { "lines-start": 0, "lines-end": 0 },
    "kind": "exec",
    "language": "rust"
  }
}
```

### Field Reference

All fields from `AtomWithLines` (section 1) are present.  Two optional fields
are added:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `verification-status` | string | no | `"verified"`, `"failed"`, or `"unverified"` (absent when `--skip-verify`) |
| `specs` | array of SpecCondition | no | Specification conditions (absent when `--skip-specify` or for external stubs). Empty array = analyzed, no specs. |

To check whether a function has specs, test `specs != []`.  The `specs` field
is absent (not serialized) only for external stubs or when the specify step was
skipped entirely.

### SpecCondition

Each element in the `specs` array is a typed condition:

| Field | Type | Description |
|-------|------|-------------|
| `kind` | string | `"precondition"` or `"postcondition"` |
| `text` | string | Raw text of the condition block including keyword (e.g. `"requires\n    x > 0"`) |
| `clauses` | array of strings | Individual clauses split from the block (omitted if empty) |
| `calls` | array of strings | Short names of functions called in this condition (AST-extracted, omitted if empty) |
| `calls-full` | array of strings | Fully qualified Rust paths of function calls (omitted if empty) |

In Verus, a function has at most one `requires` block (precondition) and one
`ensures` block (postcondition), so the array has 0–2 elements.

### Verification Status Mapping

| Verus status | Unified value | Meaning |
|-------------|---------------|---------|
| `success` | `"verified"` | Passed verification |
| `failure` | `"failed"` | Verification errors |
| `sorries` | `"unverified"` | Contains `assume()`/`admit()` |
| `warning` | `"verified"` | Passed with warnings |

### Dependency Filtering

When the `extract` pipeline runs, it internally computes call location data
(the same data available via `--with-locations` on `atomize`).  During the
merge step, dependencies tagged as `"precondition"` or `"postcondition"` are
removed from the `dependencies` array.  These spec-related calls appear
exclusively in the corresponding `specs` array entries.

If location data is not available (e.g., when using pre-existing atoms without
location tags), all dependencies are preserved as-is.

### Notes

- External stubs (functions defined outside the workspace) will not have
  `verification-status` or `specs` fields since they are not parsed by
  specify or verified by run-verus.
- When a pipeline step is skipped (`--skip-specify` or `--skip-verify`),
  the corresponding field is absent from **all** entries.

---

## 6. `probe-verus/stubs` — Stub Frontmatter

**Produced by:** `stubify`
**Envelope schema:** `"probe-verus/stubs"`

### Data Shape

`data` is an object keyed by the relative path of the `.md` file:

```json
{
  "montgomery/MontgomeryPoint_mul.md": {
    "code-line": 42,
    "code-path": "src/montgomery.rs",
    "code-name": "probe:curve25519-dalek/4.1.3/montgomery/MontgomeryPoint#mul()"
  },
  "edwards/decompress.md": {
    "code-path": "src/edwards.rs"
  }
}
```

### Field Reference

All fields are optional.

| Field | Type | Description |
|-------|------|-------------|
| `code-line` | integer | Line number in the source file |
| `code-path` | string | Relative source file path |
| `code-name` | string | Probe code-name |

---

## 7. `probe/merged-atoms` — Merged Call Graph

**Produced by:** `merge-atoms`
**Envelope schema:** `"probe/merged-atoms"`

### Envelope Variant

Merged output uses a different envelope structure: `source` is replaced by
`inputs` (an array recording provenance of each input file).  See
[envelope-rationale.md § Merged-Atoms Envelope Variant](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md#merged-atoms-envelope-variant).

```json
{
  "schema": "probe/merged-atoms",
  "schema-version": "2.0",
  "tool": { "name": "probe", "version": "2.0.0", "command": "merge-atoms" },
  "inputs": [
    {
      "schema": "probe-verus/atoms",
      "source": { "repo": "...", "commit": "...", "language": "rust", "package": "...", "package-version": "..." }
    }
  ],
  "timestamp": "2026-03-06T12:00:00Z",
  "data": { ... }
}
```

### Data Shape

Same as `probe-verus/atoms` — an object keyed by code-name where each value
is an `AtomWithLines`.

---

## Commands Without Envelopes

The following commands produce raw JSON without a Schema 2.0 envelope.

### 8. `list-functions` — Function Listing

**Envelope:** None

```json
{
  "functions": [ ... ],
  "functions_by_file": { "src/lib.rs": [ ... ] },
  "summary": { "total_functions": 42, "total_files": 5 }
}
```

#### ParsedOutput

| Field | Type | Description |
|-------|------|-------------|
| `functions` | array of FunctionInfo | All discovered functions |
| `functions_by_file` | object | Functions grouped by file path |
| `summary` | object | `{"total_functions": N, "total_files": N}` |

Each `FunctionInfo` in the array has the same shape as the specs entry (section
4), except the `name` field is **not** serialized and there is no `spec-labels`
field.

### 9. `callee-crates` — Crate Dependencies at Call Depth

**Envelope:** None

```json
{
  "function": "probe:curve25519-dalek/4.1.3/montgomery/MontgomeryPoint#mul()",
  "depth": 2,
  "crates": [
    {
      "crate": "curve25519-dalek",
      "version": "4.1.3",
      "functions": [
        "probe:curve25519-dalek/4.1.3/field/FieldElement51#mul()"
      ]
    },
    {
      "crate": "vstd",
      "version": "0.0.0-2026-01-11-0057",
      "functions": [
        "probe:vstd/0.0.0-2026-01-11-0057/arithmetic/mul/lemma_mul_is_commutative()"
      ]
    }
  ]
}
```

#### CalleeCratesOutput

| Field | Type | Description |
|-------|------|-------------|
| `function` | string | Resolved code-name of the root function |
| `depth` | integer | BFS traversal depth |
| `crates` | array of CrateEntry | Callees grouped by crate |

#### CrateEntry

| Field | Type | Description |
|-------|------|-------------|
| `crate` | string | Crate name |
| `version` | string | Crate version (or `"stdlib"` for `core`/`alloc`/`std`) |
| `functions` | array of strings | Code-names of callees in this crate |

---

## Schema Evolution

When adding new optional fields, increment the minor version (`2.0` → `2.1`).
When changing required fields or their semantics, increment the major version
(`2.0` → `3.0`).

Consumers should check `schema-version` and reject files with an unsupported
major version.
