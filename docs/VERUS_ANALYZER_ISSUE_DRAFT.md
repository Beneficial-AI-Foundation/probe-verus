# Draft Issue for verus-analyzer

**Repository**: [verus-lang/verus-analyzer](https://github.com/verus-lang/verus-analyzer)

---

## Title

**SCIP symbol format differs from rust-analyzer: missing Self type for reference implementations causes duplicates**

## Description

When generating SCIP indices, verus-analyzer uses a different symbol format than rust-analyzer for trait implementations. Specifically, verus-analyzer **omits the Self type from symbols when Self is a reference**, leading to:
1. Lost semantic information
2. Duplicate symbols for distinct implementations

rust-analyzer has a consistent format that doesn't have these issues.

## Symbol Format Comparison

| Tool | Format | Example |
|------|--------|---------|
| **rust-analyzer** | `impl#[SelfType][Trait]method()` | `` scalar/impl#[`&'a Scalar`][Neg]neg(). `` |
| **verus-analyzer** | `Type#Trait#method()` or `Trait#method()` | `scalar/Neg#neg().` |

## The Problem

verus-analyzer's symbol format varies based on whether `Self` is owned or a reference:

| Self Type | verus-analyzer Symbol |
|-----------|----------------------|
| **Owned** (`impl Trait for Type`) | `module/Type#Trait#method()` ✅ |
| **Reference** (`impl Trait for &Type`) | `module/Trait#method()` ❌ |

When `Self` is a reference, the implementor type is **omitted entirely**.

## Concrete Examples from curve25519-dalek

### Example 1: `Neg` trait - inconsistent structure

| Implementation | verus-analyzer | rust-analyzer |
|---------------|----------------|---------------|
| `impl Neg for Scalar` | `scalar/Scalar#Neg#neg().` | `scalar/impl#[Scalar][Neg]neg().` |
| `impl<'a> Neg for &'a Scalar` | `scalar/Neg#neg().` | `` scalar/impl#[`&'a Scalar`][Neg]neg(). `` |

Note: verus-analyzer omits `Scalar` for the reference impl, but rust-analyzer includes `` `&'a Scalar` ``.

### Example 2: `Mul` trait - duplicate symbols

| Implementation | verus-analyzer | rust-analyzer |
|---------------|----------------|---------------|
| `impl Mul<&Scalar> for &MontgomeryPoint` | `montgomery/Mul#mul().` | `` montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul(). `` |
| `impl Mul<&MontgomeryPoint> for &Scalar` | `montgomery/Mul#mul().` | `` montgomery/impl#[`&Scalar`][`Mul<&MontgomeryPoint>`]mul(). `` |

**Both implementations produce the identical symbol in verus-analyzer** (`montgomery/Mul#mul().`), despite being semantically different (point × scalar vs scalar × point). rust-analyzer produces unique symbols.

### Example 3: Owned Self works correctly

| Implementation | verus-analyzer | rust-analyzer |
|---------------|----------------|---------------|
| `impl ConstantTimeEq for MontgomeryPoint` | `montgomery/MontgomeryPoint#ConstantTimeEq#ct_eq().` | `montgomery/impl#[MontgomeryPoint][ConstantTimeEq]ct_eq().` |

Owned Self types are handled correctly in both tools.

## Impact

1. **Symbol collisions**: Multiple distinct trait implementations produce identical SCIP symbols, breaking tools that expect unique symbols
2. **Lost type information**: Cannot determine which type implements a trait when Self is a reference
3. **Inconsistency with rust-analyzer**: Tools consuming SCIP data may expect rust-analyzer's format

## Suggested Fix

Adopt rust-analyzer's symbol format for trait implementations:

```
module/impl#[SelfType][Trait]method()
```

This format:
- Always includes the Self type (including references like `` `&'a Type` ``)
- Includes the full trait with type parameters (e.g., `` `Mul<&Scalar>` ``)
- Produces unique symbols for all implementations

## Minimal Reproducible Example

A minimal Rust project demonstrating this issue is available at:
[github.com/Beneficial-AI-Foundation/minimal-scip-issue](https://github.com/Beneficial-AI-Foundation/minimal-scip-issue)

```bash
git clone https://github.com/Beneficial-AI-Foundation/minimal-scip-issue
cd minimal-scip-issue

# Generate rust-analyzer SCIP (correct behavior)
rust-analyzer scip . --output index-ra.scip
scip print --json index-ra.scip > index-ra.json

# Generate verus-analyzer SCIP (shows the bug)
verus-analyzer scip . --output index-va.scip
scip print --json index-va.scip > index-va.json

# Compare impl symbols
python extract_impl_symbols.py index-ra.json index-va.json
```

**rust-analyzer output** (4 unique symbols):
```
impl#[Scalar][Neg]neg().
impl#[`&Scalar`][Neg]neg().
impl#[`&Point`][`Mul<&Scalar>`]mul().
impl#[`&Scalar`][`Mul<&Point>`]mul().
```

**verus-analyzer output** (expected: 2-3 symbols with duplicates):
```
Scalar#Neg#neg().
Neg#neg().           <- missing Self type
Mul#mul().           <- duplicate (appears twice)
```

## Environment

- verus-analyzer version: (latest)
- Minimal example: `examples/minimal-scip-issue/`
- Also tested on: curve25519-dalek with Verus annotations

## Data Sources

- Minimal example: [minimal-scip-issue](https://github.com/Beneficial-AI-Foundation/minimal-scip-issue) `index-ra.json` vs `index-va.json`
- Large crate example:
  - verus-analyzer SCIP: `data/curve_top.json` (lines 522639 vs 523205 show duplicate `montgomery/Mul#mul().`)
  - rust-analyzer SCIP: `data/curve_ra.json` (lines 265512 vs 265833 show unique symbols)

## Workaround

We have implemented a workaround in [probe-verus](https://github.com/Beneficial-AI-Foundation/probe-verus) that repairs verus-analyzer's symbols by:
1. Extracting the Self type from the `method().(self)` parameter symbol's signature
2. Preserving the `&` for reference types to distinguish owned vs reference implementations
3. Inserting the Self type into symbols that are missing it

This produces repaired symbols like:
- `montgomery/&MontgomeryPoint#Mul<Scalar>#mul()` (was duplicate `montgomery/Mul#mul().`)
- `montgomery/&Scalar#Mul<MontgomeryPoint>#mul()` (was duplicate `montgomery/Mul#mul().`)
- `scalar/&Scalar#Neg#neg()` (was `scalar/Neg#neg().` with missing Self type)

See `docs/SCIP_SYMBOL_FORMAT_COMPARISON.md` for full details.

## Related

- [rust-analyzer #18772](https://github.com/rust-lang/rust-analyzer/issues/18772) - SCIP symbols for inherent `impl` declarations are ambiguous (similar category of issue)
- Local documentation: `docs/SCIP_SYMBOL_FORMAT_COMPARISON.md`
                    