# SCIP Symbol Format Comparison: rust-analyzer vs verus-analyzer

When generating SCIP indices, `verus-analyzer` seems to use a different symbol format than `rust-analyzer` for trait implementations. Specifically, `verus-analyzer` seems to omit the `Self` type from symbols when `Self` is a reference.

## rust-analyzer and verus-analyzer versions

```
$ rust-analyzer --version
rust-analyzer 0.3.2593-standalone
$ verus-analyzer --version
rust-analyzer 0.3.255-standalone
```

## Symbol Format Comparison

| Tool | Format | Example |
|------|--------|---------|
| **rust-analyzer** | `impl#[SelfType][Trait]method()` | `` scalar/impl#[`&'a Scalar`][Neg]neg(). `` |
| **verus-analyzer** | `Type#Trait#method()` or `Trait#method()` | `scalar/Neg#neg().` |

So `verus-analyzer`'s symbol format seems to vary based on whether `Self` is owned or a reference:

| Self Type | verus-analyzer Symbol |
|-----------|----------------------|
| **Owned** (`impl Trait for Type`) | `module/Type#Trait#method()` |
| **Reference** (`impl Trait for &Type`) | `module/Trait#method()` |

When `Self` is a reference, the implementor type seems to be omitted.

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

Both implementations produce the identical symbol in `verus-analyzer` (`montgomery/Mul#mul().`).

Would it be possible that `verus-analyzer` has the same format as `rust-analyzer`?

Thank you,
Lacramioara
