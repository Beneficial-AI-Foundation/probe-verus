# rust-analyzer vs verus-analyzer: From Trait Symbol Comparison

This document compares how rust-analyzer and verus-analyzer generate SCIP symbols for `From` trait implementations.

## Summary

| Analyzer | Symbol Format | Duplicates? |
|----------|--------------|-------------|
| **rust-analyzer** | `impl#[\`TargetType<Generic>\`][\`From<&'a SourceType>\`]from()` | ❌ No |
| **verus-analyzer** | `TargetType#From#from()` | ✅ Yes |

## rust-analyzer Symbol Format

rust-analyzer uses a detailed format that includes:
1. The `impl#` prefix
2. The implementing (target) type with full generics: `[`NafLookupTable5<ProjectiveNielsPoint>`]`
3. The trait with full type parameters: `[`From<&'a EdwardsPoint>`]`
4. The method name: `from()`

### Examples from `curve_ra.json`

```
window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&'a EdwardsPoint>`]from().
window/impl#[`NafLookupTable5<AffineNielsPoint>`][`From<&'a EdwardsPoint>`]from().
window/impl#[`NafLookupTable8<ProjectiveNielsPoint>`][`From<&'a EdwardsPoint>`]from().
window/impl#[`NafLookupTable8<AffineNielsPoint>`][`From<&'a EdwardsPoint>`]from().
window/impl#[`LookupTable<ProjectiveNielsPoint>`][`From<&'a EdwardsPoint>`]from().
window/impl#[`LookupTable<AffineNielsPoint>`][`From<&'a EdwardsPoint>`]from().
```

Each implementation has a **unique symbol** because:
- The target type includes the generic parameter (`ProjectiveNielsPoint` vs `AffineNielsPoint`)
- The source type is preserved (`From<&'a EdwardsPoint>`)

## verus-analyzer Symbol Format

verus-analyzer uses a simplified format that loses type information:

### Examples from `curve_top.json`

```
window/LookupTable#From#from().
window/LookupTable#From#from().
window/NafLookupTable5#From#from().
window/NafLookupTable5#From#from().
window/NafLookupTable8#From#from().
window/NafLookupTable8#From#from().
```

**Problem**: Multiple implementations produce the same symbol:
- `impl From<&EdwardsPoint> for LookupTable<AffineNielsPoint>` → `LookupTable#From#from()`
- `impl From<&EdwardsPoint> for LookupTable<ProjectiveNielsPoint>` → `LookupTable#From#from()`

## Information Lost in verus-analyzer

| Information | rust-analyzer | verus-analyzer |
|------------|---------------|----------------|
| Source type (`From<T>`) | ✅ `From<&'a EdwardsPoint>` | ❌ Just `From` |
| Target generic params | ✅ `LookupTable<ProjectiveNielsPoint>` | ❌ Just `LookupTable` |
| Lifetime annotations | ✅ `&'a EdwardsPoint` | ❌ Not present |

## Mul Trait Comparison (for reference)

The same pattern applies to `Mul` and other binary traits:

### rust-analyzer (unique symbols)
```
montgomery/impl#[`&MontgomeryPoint`][`Mul<&Scalar>`]mul().
montgomery/impl#[`&Scalar`][`Mul<&MontgomeryPoint>`]mul().
```

### verus-analyzer (duplicate symbols)
```
montgomery/Mul#mul().
montgomery/Mul#mul().
```

## File Statistics

| File | Size | Symbol Count |
|------|------|--------------|
| `curve_ra.json` (rust-analyzer) | 11MB | 28,549 |
| `curve_top.json` (verus-analyzer) | 19MB | 62,014 |

The verus-analyzer file is larger despite having fewer unique symbols, likely due to:
1. Duplicate symbol entries
2. More verbose occurrence data

## Implications for probe-verus

Because verus-analyzer loses type information, probe-verus must:

1. **Extract type parameters** from the `signature_documentation.text` field
2. **Preserve references** (`&`) to distinguish `From<&T>` from `From<T>`
3. **Add line numbers** as last resort when symbol+signature are identical

### probe-verus Repaired Format

```
window/LookupTable#From<&EdwardsPoint>#from()@345
window/LookupTable#From<&EdwardsPoint>#from()@436
```

This approximates rust-analyzer's format but cannot recover the full generic type information (e.g., `LookupTable<ProjectiveNielsPoint>` vs `LookupTable<AffineNielsPoint>`).

## Conclusion

**rust-analyzer does not have the duplicate symbol problem** for `From` trait implementations. The issue is specific to verus-analyzer's simplified symbol format.

This should be addressed upstream in verus-analyzer by adopting rust-analyzer's `impl#[Type][Trait]method()` format. See `VERUS_ANALYZER_ISSUE_DRAFT.md` for the proposed issue.

## Data Sources

- rust-analyzer SCIP: `data/curve_ra.json` (generated from curve25519-dalek)
- verus-analyzer SCIP: `data/curve_top.json` (generated from curve25519-dalek with Verus annotations)
