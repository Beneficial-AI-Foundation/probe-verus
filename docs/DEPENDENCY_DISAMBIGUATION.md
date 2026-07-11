# Dependency Disambiguation Using SCIP Type Hints

This document describes how `probe-verus` resolves ambiguous dependencies to the correct trait implementation using type information from the SCIP index.

## The Problem

When verus-analyzer generates SCIP data, multiple trait implementations can have **identical symbols**:

```
rust-analyzer cargo curve25519-dalek 4.1.3 window/NafLookupTable5#From#from().
rust-analyzer cargo curve25519-dalek 4.1.3 window/NafLookupTable5#From#from().
```

These correspond to two different implementations in the source:

```rust
impl<'a> From<&'a EdwardsPoint> for NafLookupTable5<ProjectiveNielsPoint> { ... }  // line 529
impl<'a> From<&'a EdwardsPoint> for NafLookupTable5<AffineNielsPoint> { ... }      // line 541
```

When a function calls `NafLookupTable5::from()`, which implementation should appear in its dependencies?

## The Discovery

We investigated the SCIP index and found that **the type information IS present** — just not attached directly to the method symbol. Instead, it appears as **separate type occurrences on the same line**.

### At Call Sites (References)

When the source code has:
```rust
let table = NafLookupTable5::<ProjectiveNielsPoint>::from(A);
```

The SCIP index records multiple occurrences on line 40:
```
Line 40: [REF] window/NafLookupTable5#
Line 40: [REF] curve_models/serial/backend/ProjectiveNielsPoint#    ← TYPE PARAMETER!
Line 40: [REF] window/NafLookupTable5#From#from().
```

The turbofish type parameter (`::<ProjectiveNielsPoint>`) is recorded as a separate type reference!

### At Definition Sites

Near each `fn from()` definition, there are type references indicating which impl it belongs to:

```
Definition at line 538 (0-indexed 537):
  Line 537: ProjectiveNielsPoint, EdwardsPoint, NafLookupTable5, From

Definition at line 550 (0-indexed 549):
  Line 549: AffineNielsPoint, EdwardsPoint, NafLookupTable5, From
```

## The Solution

We implemented a three-phase disambiguation strategy:

### Phase 1: Capture Definition Type Context

When building the call graph, for each function definition, we collect type symbols (symbols ending with `#`) from nearby lines (within 5 lines before the definition):

```rust
pub struct FunctionNode {
    // ... other fields ...
    /// Type context from the definition site (nearby type references)
    pub definition_type_context: Vec<String>,
}
```

### Phase 2: Capture Call-Site Type Hints

When recording function calls, we also capture type references on the same line:

```rust
pub struct CalleeInfo {
    /// The raw SCIP symbol of the callee
    pub symbol: String,
    /// Type hints found on the same line as the call (turbofish type parameters)
    pub type_hints: Vec<String>,
}
```

### Phase 3: Smart Matching

When resolving dependencies, we:

1. Find type hints that are **discriminating** — they appear in SOME implementations but not ALL
2. Match call-site hints to definition-site contexts using these discriminating types
3. Select the single matching implementation

```rust
// Find discriminating hints (types that don't appear in ALL impls)
let discriminating_hints: Vec<_> = callee.type_hints.iter()
    .filter(|hint| {
        let matching_count = scip_name_contexts.iter()
            .filter(|ctx| ctx.type_context.contains(hint))
            .count();
        // Keep hints that match some but not all impls
        matching_count > 0 && matching_count < scip_name_contexts.len()
    })
    .collect();
```

## Results

### scip_name Format (Definition Names)

**verus-analyzer raw output (ambiguous):**
```
window/NafLookupTable5#From#from().
window/NafLookupTable5#From#from().
```

**probe-verus enhanced output (disambiguated):**
```
window/NafLookupTable5<ProjectiveNielsPoint>#From<&EdwardsPoint>#from()
window/NafLookupTable5<AffineNielsPoint>#From<&EdwardsPoint>#from()
```

This is aligned with rust-analyzer's format:
```
window/impl#[`NafLookupTable5<ProjectiveNielsPoint>`][`From<&'a EdwardsPoint>`]from()
```

### Dependency Resolution

**Without disambiguation (both impls included):**
```
vartime_double_base/mul() dependencies:
  - window/NafLookupTable5#From#from()   ← Which one?
```

**With disambiguation (correct impl resolved):**
```
vartime_double_base/mul() dependencies:
  - window/NafLookupTable5<ProjectiveNielsPoint>#From<&EdwardsPoint>#from()
```

| Call Site | Type Hint from Turbofish | Resolved Implementation |
|-----------|--------------------------|------------------------|
| `vartime_double_base` | `ProjectiveNielsPoint` | `NafLookupTable5<ProjectiveNielsPoint>` |
| `precomputed_straus` | `AffineNielsPoint` | `NafLookupTable8<AffineNielsPoint>` |
| `variable_base` | `ProjectiveNielsPoint` | `LookupTable<ProjectiveNielsPoint>` |

## Key Insight

The SCIP index from verus-analyzer **does contain sufficient information** to disambiguate trait implementations — it's just structured differently than expected:

- Type parameters are recorded as **separate symbol occurrences** on the same line
- By correlating these type occurrences with method references, we can determine which specific implementation is being called

This approach works for any case where the call site uses explicit type syntax (turbofish `::< >` or type annotations), which is required by Rust when the compiler cannot infer the type.

## Limitations

This disambiguation relies on:
1. Explicit type annotations at call sites (turbofish syntax or type ascription)
2. Type symbols appearing near definitions in the source code

Cases where type inference alone determines the implementation (no explicit type in source) cannot be disambiguated from SCIP data alone.
