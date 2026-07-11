# Spec Taxonomy — Design Analysis

## What It Does

Given a Verus-verified codebase, the spec taxonomy automatically answers: **"what kind of thing does each function prove?"** — not just "this function is verified" but "this function proves crash safety" or "this function proves the output matches a mathematical model."

It turns a flat list of verified functions into a **structured, human-readable classification** that makes verification results meaningful to non-expert stakeholders.

## Architecture

A two-layer system: **AST extraction** (in Rust) + **rule matching** (in TOML config).

```
Source files --> verus_syn AST --> FunctionInfo --> Rule Engine --> spec-labels[]
                                                        ^
                                                        |
                                                 taxonomy.toml
```

### Layer 1: Structured metadata from the AST

`CallNameCollector` (in `verus_parser.rs`) walks `verus_syn` `Expr` nodes inside `requires` and `ensures` clauses to extract **function call names** — the "vocabulary" of the spec. For each call it captures:

- **Short name** (last path segment): e.g., `is_canonical` from `crate::spec::is_canonical`
- **Full qualified path**: e.g., `crate::spec::is_canonical` (for disambiguation)
- **Call kind**: function call (`ExprCall`) vs. method call (`ExprMethodCall`)

Combined with `mode` (exec/proof/spec), boolean flags (`has_ensures`, `has_decreases`, `ensures_calls_empty`), and context (impl/trait/standalone), this gives a rich feature vector per function without regex or raw text matching.

### Layer 2: External TOML rules

Classification logic lives outside the binary. Rules are AND-of-OR predicates over the feature vector. Multiple rules can fire, giving multi-label output. No recompilation needed to tune or extend.

Available match criteria:

| Criterion | Type | Description |
|-----------|------|-------------|
| `mode` | `["exec", "proof", "spec"]` | Function's Verus mode |
| `context` | `["impl", "trait", "standalone"]` | Function context |
| `ensures_calls_contain` | `["substring", ...]` | Any ensures call name contains any substring |
| `requires_calls_contain` | `["substring", ...]` | Same for requires |
| `ensures_calls_full_contain` | `["substring", ...]` | Match against full qualified paths |
| `requires_calls_full_contain` | `["substring", ...]` | Same for requires |
| `ensures_fn_calls_contain` | `["substring", ...]` | Match only function calls (not method calls) |
| `ensures_method_calls_contain` | `["substring", ...]` | Match only method calls |
| `requires_fn_calls_contain` | `["substring", ...]` | Same for requires |
| `requires_method_calls_contain` | `["substring", ...]` | Same for requires |
| `name_contains` | `["substring", ...]` | Function name contains any substring |
| `path_contains` | `["substring", ...]` | Source path contains any substring |
| `has_ensures` | `true / false` | Has ensures clause |
| `has_requires` | `true / false` | Has requires clause |
| `has_decreases` | `true / false` | Has decreases clause |
| `has_trusted_assumption` | `true / false` | Has assume/admit |
| `ensures_calls_empty` | `true / false` | Ensures has zero function calls |
| `requires_calls_empty` | `true / false` | Requires has zero function calls |

Additional config features:

- **`stop_words`**: A list of utility function names (e.g., `len`, `old`, `unwrap`) to filter from ensures/requires calls before rule evaluation. Keeps rules clean and focused on domain-specific signals.

### Debugging tools

- **`--taxonomy-explain`**: Prints per-function MATCHED/missed explanations with the specific failing criteria, sent to stderr so it doesn't mix with JSON output.
- **Coverage summary**: When taxonomy is active, the CLI reports `Taxonomy: N/M specified functions classified (X%), L/T overall` alongside the normal output line.

## Results

| Project | Domain | Specified | Classified | Categories |
|---------|--------|-----------|------------|------------|
| curve25519-dalek | Crypto/ECC | 295 | 295 (100%) | 8 |
| pmemlog | Persistent memory | 70 | 70 (100%) | 9 |

Two very different domains, same engine, different TOML configs.

## Strengths

- **AST-first, not regex**: Function call names come from parsed AST, not brittle text matching. No false positives from comments or string literals.
- **Separation of concerns**: The engine is domain-agnostic; all domain knowledge lives in the TOML. A new project just needs a new `.toml` file.
- **Multi-label**: A function can be both `crash-safety` and `data-invariant`, reflecting reality.
- **Deterministic**: BTreeMap output + sorted file traversal = byte-identical JSON across runs.
- **Incremental workflow**: Run once without taxonomy to see ensures-calls, then write rules against those patterns.
- **Rich call data**: Full qualified paths and fn/method split available for fine-grained rules.
- **Stop-word filtering**: Noisy utility calls can be removed at the config level.
- **Debuggable**: `--taxonomy-explain` shows exactly why each rule matched or missed.

## Weaknesses and Limitations

### W1: Shallow call extraction — names only, no types

`CallNameCollector` extracts function names but not types. `x.len()` is just `"len"` whether `x` is a `Seq`, `Vec`, or `Slice`. Full paths help with function calls but method calls still lack receiver type information.

### W2: No semantic depth — one level of call, no transitive analysis

We extract what functions are **directly called** in ensures/requires. We don't look at what those spec functions themselves assert. If `spec_foo(x)` internally checks `is_canonical(x.field)`, we don't see `is_canonical` in the caller's ensures-calls.

### W3: "Utility" calls pollute the signal

Calls like `len`, `subrange`, `old`, `unwrap`, `Some` appear in nearly every spec clause. The `stop_words` config mitigates this, but stop-word lists must be maintained per-project.

### W4: Empty ensures-calls is a coarse signal

Functions whose specs use only operators/literals/quantifiers have empty ensures-calls. The `ensures_calls_empty` criterion catches them but can't distinguish between very different kinds of assertions.

### W5: Rule authoring requires domain expertise

Writing a good TOML requires understanding both the project's spec function vocabulary AND the taxonomy categories. The workflow (run once, inspect ensures-calls, write rules) works but is manual.

## Future Improvements

### Medium Effort

**Auto-suggest taxonomy rules**: After running specify without taxonomy, cluster ensures-calls patterns and propose candidate rules. "12 exec functions all call `recover` — consider a `crash-safety` label."

**Transitive spec analysis**: For spec functions in the same project, resolve what they call in their body and propagate one hop. Addresses W2.

**Weighted matching / confidence score**: Rules contribute a confidence score instead of binary match. Match on `mode` alone = low confidence; `mode` + `ensures_calls_contain` + `context` = high confidence.

### Longer Term

**LLM-assisted classification**: For unmatched functions, send signature + spec text to an LLM with taxonomy definitions. Use LLM output to suggest new TOML rules. Rule engine handles the 95%; LLM handles the 5%.

**Cross-project taxonomy normalization**: As per-project TOMLs accumulate, extract universal categories with project-specific refinements. Formal inheritance/override mechanism.

**Integration with verification results**: Combine taxonomy labels with `probe-verus verify` output: "100% of crash-safety specs verify, but 3 functional-correctness specs are failing." This is the ultimate payoff for stakeholder reporting.

## File Map

| File | Purpose |
|------|---------|
| `src/taxonomy.rs` | Rule engine: load config, classify, explain, stop-word filtering |
| `src/verus_parser.rs` | AST extraction: `CallNameCollector`, `FunctionInfo`, fn/method split, full paths |
| `src/commands/specify.rs` | CLI command: classify, explain output, coverage summary |
| `src/main.rs` | CLI args: `--taxonomy-config`, `--taxonomy-explain` |
| `spec_taxonomy_examples/spec-taxonomy-default.toml` | Starter template for new projects |
| `spec_taxonomy_examples/spec-taxonomy-curve25519-dalek.toml` | Full config for crypto/ECC |
| `spec_taxonomy_examples/spec-taxonomy-pmemlog.toml` | Full config for persistent memory |
