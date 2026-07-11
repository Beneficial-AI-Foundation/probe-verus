# Missing `serialize` Functions in SCIP Output

## Investigation Summary

### Finding 1: The functions exist in source code

The `serialize` functions **do exist** in the curve25519-dalek source code:

```
curve25519-dalek/src/edwards.rs:    fn serialize<S>(...) - impl Serialize for EdwardsPoint
curve25519-dalek/src/edwards.rs:    fn serialize<S>(...) - impl Serialize for CompressedEdwardsY
curve25519-dalek/src/scalar.rs:     fn serialize<S>(...) - impl Serialize for Scalar
curve25519-dalek/src/ristretto.rs:  fn serialize<S>(...) - impl Serialize for RistrettoPoint
curve25519-dalek/src/ristretto.rs:  fn serialize<S>(...) - impl Serialize for CompressedRistretto
```

### Finding 2: They are conditionally compiled

All these functions are behind feature flags:

```rust
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl Serialize for Scalar {
    #[verifier::external_body]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        // ...
    }
}
```

### Finding 3: `serde` is not a default feature

In `curve25519-dalek/Cargo.toml`:

```toml
[features]
default = ["alloc", "precomputed-tables", "zeroize", "lizard"]

# serde is optional, not in default features
serde = { version = "1.0", default-features = false, optional = true, features = ["derive"] }
```

### Root Cause

When `verus-analyzer scip` generates the SCIP index, it compiles the project with default features only. Since `serde` is not a default feature, the `#[cfg(feature = "serde")]` blocks are not compiled, and therefore the `serialize` functions are not indexed.

## The SCIP Index Does Contain

The SCIP index correctly contains other serialization-related functions that are NOT behind feature flags:

- `to_bytes()` methods
- `from_bytes()` methods  
- `as_bytes()` methods

These appear in the atoms output:

```
"display-name": "to_bytes"
"display-name": "from_bytes_mod_order"
"display-name": "from_bytes_mod_order_wide"
"display-name": "spec_as_bytes"
"display-name": "lemma_as_bytes_52"
...
```

## Potential Solutions

### Option 1: Enable features during SCIP generation

Modify how `verus-analyzer scip` is invoked to include the `serde` feature. This might require:

1. Setting `CARGO_FEATURES` environment variable
2. Modifying `.cargo/config.toml` in the project
3. Using a rust-analyzer configuration file (`.rust-analyzer.json` or `rust-analyzer.toml`)

Example `rust-analyzer.toml`:
```toml
[cargo]
features = ["serde"]
```

### Option 2: Make serde a default feature

Modify `Cargo.toml` to include `serde` in default features (not recommended if it changes the library's public API contract).

### Option 3: Document the limitation

Accept that conditionally-compiled code won't be indexed unless the features are enabled, and document this behavior.

## Related Notes

- The functions are also marked with `#[verifier::external_body]`, which tells Verus to skip verification of the function body. This might also affect how verus-analyzer processes them.
- This issue affects any code behind `#[cfg(...)]` attributes, not just serde.

## Date

Investigated: December 2024

