# GitHub Issue Comment: From Trait Symbol Comparison

Comment for https://github.com/verus-lang/verus-analyzer/issues/68

---

**Additional example: `From` trait implementations**

The same pattern applies to `From` trait implementations, which are commonly used for type conversions:

### Example: `From` trait - duplicate symbols

| Implementation | verus-analyzer | rust-analyzer |
|----------------|----------------|---------------|
| `impl From<&EdwardsPoint> for LookupTable<ProjectiveNielsPoint>` | `LookupTable#From#from().` | `impl#[LookupTable<ProjectiveNielsPoint>][From<&'a EdwardsPoint>]from().` |
| `impl From<&EdwardsPoint> for LookupTable<AffineNielsPoint>` | `LookupTable#From#from().` | `impl#[LookupTable<AffineNielsPoint>][From<&'a EdwardsPoint>]from().` |
| `impl From<&EdwardsPoint> for NafLookupTable5<ProjectiveNielsPoint>` | `NafLookupTable5#From#from().` | `impl#[NafLookupTable5<ProjectiveNielsPoint>][From<&'a EdwardsPoint>]from().` |
| `impl From<&EdwardsPoint> for NafLookupTable5<AffineNielsPoint>` | `NafLookupTable5#From#from().` | `impl#[NafLookupTable5<AffineNielsPoint>][From<&'a EdwardsPoint>]from().` |

All implementations for the same base type (e.g., `LookupTable`) produce identical symbols in verus-analyzer, while rust-analyzer distinguishes them by including the full generic type parameter.

### Information lost

| Information | rust-analyzer | verus-analyzer |
|-------------|---------------|----------------|
| Source type (`From<T>`) | ✅ `From<&'a EdwardsPoint>` | ❌ Just `From` |
| Target generic params | ✅ `LookupTable<ProjectiveNielsPoint>` | ❌ Just `LookupTable` |

This causes 6 `From` implementations in curve25519-dalek to collapse into 3 duplicate symbol pairs.
