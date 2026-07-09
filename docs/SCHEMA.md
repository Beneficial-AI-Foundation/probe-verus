# probe-verus Data Schemas

Version: 6.10.3
Date: 2026-07-02

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
    "language": "rust",
    "is-public": true,
    "is-public-api": true,
    "has-body": true,
    "is-external": false,
    "is-cfg-gated": false
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
| `language` | string | yes | `"rust"` for `exec` atoms, `"verus"` for `proof`/`spec` atoms (derived from `kind`, not lexical scope; see [P20]) |
| `is-public` | boolean | no | Whether the function signature starts with unrestricted `pub` (absent for external stubs) |
| `is-public-api` | boolean | no | Whether the function is part of the crate's public API. Default: SCIP module-chain walk (`pub fn` + all ancestor modules `pub` + exec kind + library crate). With `--with-public-api`: overridden by `cargo public-api` ground truth via RQN matching. `spec fn` and `proof fn` always get `false` (erased at runtime). Absent for external stubs. |
| `has-body` | boolean | no | Whether the function has a body. `false` for bodiless trait method declarations; `true` otherwise. Absent for external stubs. |
| `is-external` | boolean | no | Whether `#[verifier::external]` is present (directly or via `#[cfg_attr(verus_keep_ghost, verifier::external)]`). Absent for external stubs. |
| `is-cfg-gated` | boolean | no | Whether the function or any enclosing item (impl block, module, `cfg_if!` branch, or the module's `mod` declaration) has `#[cfg(...)]`. Absent for external stubs. |

#### `--with-public-api`: ground-truth override

By default, `is-public-api` uses the SCIP module-chain walk (zero external
dependencies). This has a known limitation: trait implementation methods
(e.g., `impl Add for Point { fn add(...) }`) lack `pub` in their signature,
so they get `is-public: false` and are excluded from `is-public-api`.

The `--with-public-api` flag runs `cargo public-api` (requires
`cargo-public-api` installed) and overrides `is-public-api` for all atoms
whose `rust-qualified-name` matches a public API entry. This uses `rustdoc`
as the authority, correctly handling trait impls and re-exports.

Atoms without a `rust-qualified-name` (external stubs) keep their existing
SCIP-walk value. Blanket impls (`Into`, `TryFrom`, etc.) are filtered out
since they have no corresponding atoms.

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
`is-public`, `is-public-api`, `has-body`, `is-external`, and `is-cfg-gated` are absent on external stubs.

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

`data` is an object whose keys are probe code-names; each value is a
`SpecifyEntry` (a `FunctionInfo` flattened with optional taxonomy labels).
In addition, when the specify step finds Verus `assume_specification`
declarations, an optional sibling key `assume-specifications` holds an array
of those declarations (see below).  The key is omitted when the array is empty.

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
    "contains_admit": false,
    "is_external_body": false,
    "has_no_decreases_attr": false,
    "requires_text": "x > 0",
    "ensures_text": "result > x",
    "ensures-calls": ["helper"],
    "requires-calls": [],
    "spec-labels": ["safety-critical"]
  },
  "assume-specifications": [
    {
      "path-segments": ["MyType", "helper_spec"],
      "path-display": "MyType::helper_spec",
      "file": "src/module.rs",
      "line": 120,
      "has_requires": true,
      "has_ensures": false,
      "requires_text": "x > 0"
    }
  ]
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
| `contains_admit` | boolean | yes | Body contains `admit()` specifically (axiom — one of the extract `"trusted"` overrides; see section 5) |
| `is_external_body` | boolean | yes | Has `#[verifier::external_body]` (trusted without proof — one of the extract `"trusted"` overrides; see section 5) |
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

### `assume-specifications` (optional top-level key)

Sibling of the per-function entries inside `data`.  Each element describes one
`assume_specification[path]` declaration (declared spec for an external
function without a proof).  Omitted when there are no such declarations.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path-segments` | array of strings | yes | Path segments used for matching (e.g. type and method name) |
| `path-display` | string | yes | Human-readable Verus path for the target |
| `file` | string | no | Relative source file path (omitted when not available) |
| `line` | integer | yes | 1-based line number of the declaration |
| `has_requires` | boolean | yes | Whether a `requires` clause is present |
| `has_ensures` | boolean | yes | Whether an `ensures` clause is present |
| `requires_text` | string | no | Raw `requires` text (when specify ran with spec text enabled) |
| `ensures_text` | string | no | Raw `ensures` text (when specify ran with spec text enabled) |

---

## 5. `probe-verus/extract` — Unified Extract Output

**Produced by:** `extract` (unified pipeline)
**Envelope schema:** `"probe-verus/extract"`
**Envelope `tool.command`:** `"extract"`

### Overview

The primary output of the `extract` command.  Each entry is an atom enriched
with optional `primary-spec`, `is-disabled`, `verification-status`, and categorized
dependency fields, aligning with the `probe-lean/extract` output structure.

Dependencies are categorized into three subsets (analogous to probe-lean's
`type-dependencies` and `term-dependencies`):

- `requires-dependencies` — functions called in `requires` clauses
- `ensures-dependencies` — functions called in `ensures` clauses
- `body-dependencies` — functions called in the function body

The existing `dependencies` field is the union of all three.

In addition to this unified file, the individual atoms, specs, and proofs files
are always written alongside it in `<project>/.verilib/probes/`.

### Data Shape

`data` is an object keyed by code-name.  Each value is a `UnifiedAtom`
(an `AtomWithLines` with additional optional fields):

```json
{
  "probe:my-crate/1.0.0/module/MyType#method()": {
    "display-name": "MyType::method",
    "dependencies": [
      "probe:my-crate/1.0.0/module/helper()",
      "probe:my-crate/1.0.0/specs/is_valid()",
      "probe:my-crate/1.0.0/specs/helper_spec()"
    ],
    "requires-dependencies": [
      "probe:my-crate/1.0.0/specs/is_valid()"
    ],
    "ensures-dependencies": [
      "probe:my-crate/1.0.0/specs/helper_spec()"
    ],
    "body-dependencies": [
      "probe:my-crate/1.0.0/module/helper()"
    ],
    "code-module": "module",
    "code-path": "src/module.rs",
    "code-text": { "lines-start": 42, "lines-end": 67 },
    "kind": "exec",
    "language": "rust",
    "is-public": true,
    "is-public-api": true,
    "has-body": true,
    "is-external": false,
    "is-cfg-gated": false,
    "primary-spec": "requires\n    x > 0,\n    y < 100\nensures\n    result > x",
    "is-disabled": false,
    "verification-status": "verified",
    "spec-labels": ["safety-critical"]
  },
  "probe:my-crate/1.0.0/module/unspecified_fn()": {
    "display-name": "unspecified_fn",
    "dependencies": ["probe:my-crate/1.0.0/module/other()"],
    "body-dependencies": ["probe:my-crate/1.0.0/module/other()"],
    "code-module": "module",
    "code-path": "src/module.rs",
    "code-text": { "lines-start": 80, "lines-end": 90 },
    "kind": "exec",
    "language": "rust",
    "is-public": false,
    "is-public-api": false,
    "has-body": true,
    "is-external": false,
    "is-cfg-gated": false,
    "primary-spec": "",
    "is-disabled": true
  },
  "probe:my-crate/1.0.0/module/axiom_foo()": {
    "display-name": "axiom_foo",
    "dependencies": [],
    "code-module": "module",
    "code-path": "src/module.rs",
    "code-text": { "lines-start": 100, "lines-end": 110 },
    "kind": "proof",
    "language": "verus",
    "is-public": true,
    "is-public-api": false,
    "has-body": true,
    "is-external": false,
    "is-cfg-gated": false,
    "primary-spec": "ensures\n    foo_property(x)",
    "is-disabled": false,
    "verification-status": "trusted",
    "trusted-reason": "admit"
  },
  "probe:external/1.0.0/other/func()": {
    "display-name": "func",
    "dependencies": [],
    "code-module": "other",
    "code-path": "",
    "code-text": { "lines-start": 0, "lines-end": 0 },
    "kind": "exec",
    "language": "rust",
    "verification-status": "trusted",
    "trusted-reason": "assume-specification",
    "primary-spec": "ensures\n    result == expected"
  }
}
```

### Field Reference

All fields from `AtomWithLines` (section 1) are present.  The following
optional fields are added:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `requires-dependencies` | array of strings | no | Subset of `dependencies` called in `requires` clauses (omitted when empty) |
| `ensures-dependencies` | array of strings | no | Subset of `dependencies` called in `ensures` clauses (omitted when empty) |
| `body-dependencies` | array of strings | no | Subset of `dependencies` called in the function body (omitted when empty) |
| `primary-spec` | string | no | Full spec text (requires + ensures concatenated). Empty string = analyzed, no spec. Absent = not analyzed. |
| `is-disabled` | bool | no | `true` = in scope but no spec yet (the verification backlog); `false` = has a spec, or is trusted/excluded. Always `false` when a `verification-status` of `"trusted"` or `"excluded"` is present — such a state is a deliberate human act that puts the atom in analysis scope, so it is never in the backlog (KB P25). Absent for non-trusted external stubs or when `--skip-specify`. |
| `verification-status` | string | no | `"transitively-verified"`, `"verified"`, `"failed"`, `"unverified"`, `"trusted"`, or `"excluded"`.  After enrichment (default, skippable via `--skip-enrich`): `"transitively-verified"` = all transitive deps verified/trusted; `"verified"` = locally verified only.  `"trusted"` is set by the merge step when a trust-base trigger applies; `"excluded"` marks `#[verifier::external]` functions that are deliberately outside the verification scope (TCB-neutral — does not imply the proofs depend on the function).  See Verification Status Mapping. |
| `trusted-reason` | string | no | Present only when `verification-status` is `"trusted"`.  Values: `"admit"` (function uses `admit()`), `"external-body"` (has `#[verifier::external_body]`), or `"assume-specification"` (matched by an `assume_specification` declaration).  Enables automated trust-base classification without consulting specs.json. (v6.5.1) |
| `spec-labels` | array of strings | no | Taxonomy classification labels from `--taxonomy-config` (omitted when empty or when `--skip-specify`) |

### Dependency Categorization

The `extract` pipeline internally computes call location data (the same data
available via `--with-locations` on `atomize`).  Each dependency is tagged as
`"precondition"`, `"postcondition"`, or `"inner"` based on whether it appears
in a `requires` clause, `ensures` clause, or the function body.

The `dependencies` field is the **union** of all categories (unchanged from
the atomize step).  The three subcategory fields partition this union:

| probe-lean analogy | probe-verus field | Source |
|--------------------|-------------------|--------|
| `type-dependencies` | `requires-dependencies` + `ensures-dependencies` | Spec clauses |
| `term-dependencies` | `body-dependencies` | Function body |
| `dependencies` | `dependencies` | Union of all |

### Verification Status Mapping

| Verus status | Unified value | Meaning |
|-------------|---------------|---------|
| `success` | `"verified"` | Passed verification |
| `failure` | `"failed"` | Verification errors |
| `sorries` | `"unverified"` | Contains `assume()` or `admit()` |
| `warning` | `"unverified"` | Passed with warnings (defensive: treated as unverified) |

**Trusted overrides (since v6.4.0 for `admit()`, extended in v6.5.0):** After
mapping proofs status as above, the merge step may override the unified atom to
`"trusted"` when any of the following holds (detected using specify output and
atoms):

1. **`admit()` in the body** — `contains_admit` is true (unchanged since v6.4.0).
   `admit()` is the Verus analogue of an axiom: the solver accepts the proof
   without checking.

2. **`#[verifier::external_body]`** — `is_external_body` is true (v6.5.0).  The
   function is treated as trusted without a checked body.

3. **`assume_specification` matched to an external stub** — An entry in
   `assume-specifications` is matched to an atom with empty `code-path`
   (external stub) using its path segments (v6.5.0).  That stub atom is marked
   `"trusted"` even though it has no local body or proofs entry.

Functions with only `assume()` (no `admit()`, not `external_body`, and not a
matched `assume_specification` stub) remain `"unverified"` when proofs report
`sorries`.

**Excluded override (v6.11.0):** independently of the trusted overrides, a
function marked `#[verifier::external]` (`is_external` in specify output) is set
to `verification-status: "excluded"` and `is-disabled: false`.  This means the
function is deliberately outside the verification scope (e.g. `Debug::fmt`, serde
impls).  It is **TCB-neutral**: unlike `"trusted"`, `"excluded"` does not enlarge
the trust base — Verus ignores the function entirely rather than assuming its
spec.  A trust reason takes precedence: an atom that is both `external` and (via
`external_body`/`admit`) trusted is reported as `"trusted"`.

### Notes

- External stubs (empty `code-path`) are not ordinary specify entries, so they
  usually lack `primary-spec`, `is-disabled`, and `spec-labels`, and proofs
  often omit them.  Merge may still set `verification-status` to `"trusted"`
  when an `assume_specification` declaration matches the stub (v6.5.0); in
  that case the stub also gets the declared spec text as `primary-spec` and
  `is-disabled: false`.
- When a pipeline step is skipped (`--skip-specify` or `--skip-verify`),
  the corresponding fields are absent from **all** entries.
- `spec-labels` is only populated when `--taxonomy-config` is provided to
  the `extract` command.

---

## 6. `probe-verus/extract-summary` — Extract Pipeline Summary

**Produced by:** `extract` (written alongside the unified extract JSON)
**Envelope schema:** `"probe-verus/extract-summary"`
**Envelope `tool.command`:** `"extract"`

### Data Shape

`data` records pipeline status and per-step results (`atomize`, `specify`,
`verify`).  When unified merge produced a map of `UnifiedAtom` values, an
optional `trust-base` object (v6.5.0) summarizes **post-override**
`verification-status` counts over those atoms (after trusted overrides from
`admit()`, `#[verifier::external_body]`, and matched `assume_specification`
stubs).  `trust-base` is absent when unified output was not produced (e.g. no
atoms file or merge failure).

```json
{
  "status": "success",
  "atomize": {
    "success": true,
    "output_file": "<project>/.verilib/probes/verus_<pkg>_<ver>_atoms.json",
    "total_functions": 42
  },
  "specify": {
    "success": true,
    "output_file": "<project>/.verilib/probes/verus_<pkg>_<ver>_specs.json",
    "total_functions": 42
  },
  "verify": {
    "success": true,
    "output_file": "<project>/.verilib/probes/verus_<pkg>_<ver>_proofs.json",
    "summary": {
      "total_functions": 42,
      "verified": 40,
      "failed": 0,
      "unverified": 2
    }
  },
  "trust-base": {
    "verified": 38,
    "trusted": 3,
    "unverified": 1,
    "failed": 0,
    "absent": 0
  }
}
```

### Top-Level `data` Fields

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | Overall pipeline status (e.g. `"success"`, `"specify_failed"`) |
| `atomize` | object | Result of the atomize step (omitted when `--skip-atomize`) |
| `specify` | object | Result of the specify step (omitted when `--skip-specify` or when specify did not run) |
| `verify` | object | Result of run-verus (omitted when `--skip-verify`) |
| `trust-base` | object | Optional post-override verification-status histogram (v6.5.0); see below |

### `trust-base` (optional)

Counts of atoms by final `verification-status` after merge overrides.  Keys:

| Field | Type | Description |
|-------|------|-------------|
| `verified` | integer | Atoms with `"verified"` |
| `trusted` | integer | Atoms with `"trusted"` |
| `excluded` | integer | Atoms with `"excluded"` (`#[verifier::external]`, out of scope). Omitted when zero (v6.11.0) |
| `unverified` | integer | Atoms with `"unverified"` |
| `failed` | integer | Atoms with `"failed"` |
| `absent` | integer | Atoms with no `verification-status` (e.g. stubs skipped by verify, or `--skip-verify`) |

---

## 7. `probe-verus/stubs` — Stub Frontmatter

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

## 8. `probe/merged-atoms` — Merged Call Graph

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

### 9. `list-functions` — Function Listing

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

### 10. `callee-crates` — Crate Dependencies at Call Depth

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
