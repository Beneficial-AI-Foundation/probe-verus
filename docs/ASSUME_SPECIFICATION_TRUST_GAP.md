# Trust-base gap: `assume_specification` declarations

## Summary

`probe-verus extract` marks functions as `"trusted"` when they are part of
the verification trust base (axioms that Verus accepts without proof).
As of v6.4.0, this covers `admit()` calls. The planned v6.5.0 release
will add `#[verifier::external_body]` functions.

However, **`assume_specification` declarations** — another form of
Verus trust assumption — cannot be captured by the current pipeline.
This document describes the gap and proposes a path forward.

## What is `assume_specification`?

Verus lets a project declare specifications for external functions without
providing proofs:

```rust
pub assume_specification[ Choice::from ](u: u8) -> (c: Choice)
    ensures
        (u == 1) == choice_is_true(c),
;
```

This is an axiom: the project trusts that `Choice::from` satisfies the
postcondition. Verus uses the specification during verification of callers
but never checks the specification itself.

## Why current matching fails

The `specify` step matches parsed `FunctionInfo` entries to atoms using
three criteria (see `find_matching_atom` in `src/commands/specify.rs`):

1. **Path**: file suffix of the parsed function must match the atom's `code-path`
2. **Name**: function name must match the atom's `display-name`
3. **Line**: SCIP line must fall within the function's span or within tolerance of `fn_line`

`assume_specification` targets external functions (e.g., `Choice::from`
from the `subtle` crate). The corresponding atoms are external stubs
with **empty `code-path`** and **no source file** in the local project.
Consequently, path-based matching always fails: **0 of 191 external stubs
in the dalek project appear in specs.json**.

## Concrete impact (curve25519-dalek)

The dalek project contains 6 `assume_specification` declarations:

| Declaration | Target atom | Covered by `external_body`? |
|---|---|---|
| `Choice::from` | `probe:subtle/2.6.1/Choice#From#from()` | No |
| `Choice::unwrap_u8` | `probe:subtle/2.6.1/Choice#unwrap_u8()` | No |
| `<u64>::conditional_swap` | wrapper `conditional_swap_u64()` | Yes |
| `<u64>::conditional_assign` | wrapper `conditional_assign_u64()` | Yes |
| `Formatter::write_str` | No atom exists | N/A |
| `<[T;N] as Hash>::hash` | `probe:core/.../Hash#hash()` | No |

After v6.5.0 (external_body + admit), the trust base will show **114
trusted atoms**. The 3 unmatched `assume_specification` targets remain
as "absent" external stubs, indistinguishable from the 9,700+ other
absent stubs. This gives **97.4% trust-base coverage** (114/117).

## Why this matters

`assume_specification` declarations are the most explicit form of trust
assumption: the developer is consciously asserting a property about code
they did not write. A consumer auditing the trust base would want to see
these prominently — they are exactly the kind of axiom where a bug in
the specification could silently break the entire verification argument.

## Proposed solution

### Phase A: Collect `assume_specification` metadata (low risk)

1. Add `visit_assume_specification` to `FunctionInfoVisitor` in
   `verus_parser.rs`. The `verus_syn` crate already has the AST node
   (`ItemAssumeSpecification`) and visitor hook.

2. Store collected declarations in a new `Vec<AssumeSpecInfo>` alongside
   the existing `Vec<FunctionInfo>` in the parser output. Each entry
   captures:
   - **path**: the Verus path string (e.g., `Choice::from`)
   - **file** and **line**: source location of the declaration
   - **has_requires**, **has_ensures**: whether specs are present
   - **ensures_text**, **requires_text**: the raw specification text

3. In the `specify` step, output these as an `"assume-specifications"`
   list in `specs.json` (not keyed by atom code-name, since there is no
   reliable match).

This phase makes the data available for consumers without any fragile
matching logic.

### Phase B: Match to atoms (moderate complexity, fragile)

To mark the 3 unmatched atoms as `"trusted"` in the extract output,
we need name-based matching from Verus paths to SCIP symbols.

**The difficulty**: Verus paths use Rust syntax while SCIP uses its own
encoding:

| Verus path | SCIP code-name |
|---|---|
| `Choice::from` | `probe:subtle/2.6.1/Choice#From#from()` |
| `Choice::unwrap_u8` | `probe:subtle/2.6.1/Choice#unwrap_u8()` |
| `<u64 as ConditionallySelectable>::conditional_swap` | `probe:subtle/2.6.1/ConditionallySelectable#conditional_swap()` |

A matching heuristic would need to:
- Parse the Verus path into (type, optional trait, method) components
- Handle angle-bracket trait impls: `<T as Trait>::method`
- Handle inherent methods: `Type::method`
- Handle qualified paths with generics: `<[T; N] as Hash>::hash`
- Search atoms for code-names containing the matching segments

This is doable but introduces a fragile layer that must be tested against
each path format variant. False positives (e.g., matching the wrong `from()`
method) are possible for common method names.

### Recommendation

Ship Phase A with v6.5.0. It has zero matching risk, provides the raw
data, and enables consumers to manually audit `assume_specification`
declarations. Defer Phase B to a future release when more projects
provide test cases for the matching heuristic.
