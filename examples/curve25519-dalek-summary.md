# Verification Statistics: curve25519-dalek 4.1.3

Generated from `examples/verus_curve25519-dalek_4.1.3.json` produced by
`probe-verus extract`.

All queries below use `jq` on the extract JSON:

```bash
FILE=examples/verus_curve25519-dalek_4.1.3.json
```

Functions that Verus processes receive a `verification-status` field
(`verified`, `trusted`, `unverified`, or `failed`). Functions outside Verus's scope
(feature-gated ecosystem trait impls, `#[verifier::external]` originals,
test helpers, bodyless trait declarations) have no `verification-status`.
The ratios below use `verification-status != null` as the denominator so
they reflect only functions Verus actually processed.

---

## 1. Verified exec functions (all)

**363** exec functions were processed by Verus: **281 verified + 82 trusted,
0 unverified** (100%).

```bash
# In-scope exec = those with a verification-status
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."verification-status" != null)] | length' "$FILE"
# => 363

# Verified
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."verification-status" == "verified")] | length' "$FILE"
# => 281

# Trusted
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."verification-status" == "trusted")] | length' "$FILE"
# => 82

# List all in-scope exec by status
jq -r '.data | to_entries[] | select(.value.kind == "exec" and .value."verification-status" != null) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 2. Verified pub exec functions

**167** `pub fn` exec functions were processed by Verus: **102 verified +
65 trusted, 0 unverified** (100%).

Note: 246 total `pub fn` exec functions exist in the output, but 79 have no
`verification-status` because they are outside Verus's scope (feature-gated
trait impls for `group`/`serde`/`ff`, `#[verifier::external]` originals with
`_verus` companions, `#[cfg(test)]` helpers, trait declarations).

```bash
# In-scope pub exec = pub + exec + has status
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and .value."verification-status" != null)] | length' "$FILE"
# => 167

# Verified
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and .value."verification-status" == "verified")] | length' "$FILE"
# => 102

# Trusted
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and .value."verification-status" == "trusted")] | length' "$FILE"
# => 65

# List in-scope pub exec by status
jq -r '.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and .value."verification-status" != null) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 3. Verified public API functions

**122** public API exec functions were processed by Verus: **93 verified +
29 trusted, 0 unverified** (100%).

Public API = `pub fn` + all ancestor modules are `pub` + exec kind + library
crate. `spec fn` and `proof fn` are excluded (erased at runtime).

```bash
# In-scope public API = is-public-api + has status
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" != null)] | length' "$FILE"
# => 122

# Verified
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" == "verified")] | length' "$FILE"
# => 93

# Trusted
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" == "trusted")] | length' "$FILE"
# => 29

# List in-scope public API by status
jq -r '.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" != null) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 4. Axioms (functions using `admit()`)

**48** functions use `admit()` in their body.

`admit()` is the Verus analogue of an axiom: the solver accepts the proof
without checking. These are marked `verification-status: "trusted"` with
`trusted-reason: "admit"`.

```bash
# Count
jq '[.data | to_entries[] | select(.value."trusted-reason" == "admit")] | length' "$FILE"
# => 48

# List axioms
jq -r '.data | to_entries[] | select(.value."trusted-reason" == "admit") | "\(.value."display-name")\t\(.value.kind)\t\(.value."code-path")"' "$FILE" | sort
```

---

## 5. External functions assumed correct

**82** external functions are assumed correct without proof:

- **77** have `#[verifier::external_body]` — the function body is trusted
  without a checked proof.
- **5** are matched by `assume_specification` declarations — an external
  (non-workspace) function whose spec is declared but not proved.

```bash
# Count external-body
jq '[.data | to_entries[] | select(.value."trusted-reason" == "external-body")] | length' "$FILE"
# => 77

# List external-body
jq -r '.data | to_entries[] | select(.value."trusted-reason" == "external-body") | "\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort

# Count assume-specification
jq '[.data | to_entries[] | select(.value."trusted-reason" == "assume-specification")] | length' "$FILE"
# => 5

# List assume-specification
jq -r '.data | to_entries[] | select(.value."trusted-reason" == "assume-specification") | "\(.key)\t\(.value."primary-spec" // "(no spec)")"' "$FILE"

# All trusted (axioms + external-body + assume-specification)
jq '[.data | to_entries[] | select(.value."verification-status" == "trusted")] | group_by(.value."trusted-reason") | map({reason: .[0].value."trusted-reason", count: length})' "$FILE"
```

---

## 6. Out-of-scope pub exec functions (4-category breakdown)

**79** `pub fn` exec functions have no `verification-status`. Using the new
`has-body`, `is-external`, and `is-cfg-gated` fields (added in v6.6.0), these
are categorized as follows:

| Category | Count | Description |
|----------|------:|-------------|
| Bodiless trait declarations | 5 | Trait methods with no default body (`has-body: false`) |
| `#[verifier::external]` | 8 | Explicitly excluded from verification |
| Feature/cfg-gated | 62 | Under `#[cfg(...)]` — not compiled during verification |
| Other (trait default methods) | 4 | Trait default methods in `src/traits.rs` that delegate to other methods |
| **Total** | **79** | |

The 4 "other" functions are all thin delegation methods defined as trait
defaults in `src/traits.rs` (`BasepointTable::mul_base_clamped`,
`VartimeMultiscalarMul::vartime_multiscalar_mul`,
`VartimePrecomputedMultiscalarMul::vartime_multiscalar_mul`,
`VartimePrecomputedMultiscalarMul::vartime_mixed_multiscalar_mul`).
They are not processed by Verus because they are trait default methods,
not `impl` methods.

```bash
# Total pub exec without verification-status
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."is-public" == true and .value."verification-status" == null)] | length' "$FILE"
# => 79

# Cat 1: Bodiless trait declarations
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."is-public" == true and .value."verification-status" == null and .value."has-body" == false)] | length' "$FILE"
# => 5

# Cat 2: External (verifier::external), excluding bodiless
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."is-public" == true and .value."verification-status" == null and .value."has-body" != false and .value."is-external" == true)] | length' "$FILE"
# => 8

# Cat 3: Cfg-gated (not external, not bodiless)
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."is-public" == true and .value."verification-status" == null and .value."has-body" != false and .value."is-external" != true and .value."is-cfg-gated" == true)] | length' "$FILE"
# => 62

# Cat 4: Other (none of the above)
jq '[.data | to_entries[] | select(.value.kind == "exec" and .value."is-public" == true and .value."verification-status" == null and .value."has-body" != false and .value."is-external" != true and .value."is-cfg-gated" != true)] | length' "$FILE"
# => 4

# List the "other" functions
jq -r '.data | to_entries[] | select(.value.kind == "exec" and .value."is-public" == true and .value."verification-status" == null and .value."has-body" != false and .value."is-external" != true and .value."is-cfg-gated" != true) | "\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## Summary

| Metric | Verified | Trusted | In-scope | % (v+t) |
|--------|--------:|---------:|---------:|--------:|
| All exec functions | 281 | 82 | 363 | 100% |
| `pub fn` exec | 102 | 65 | 167 | 100% |
| Public API exec | 93 | 29 | 122 | 100% |

"In-scope" = functions with a `verification-status` (i.e., Verus processed
them). The 79 pub exec functions outside scope break down as:
5 bodiless trait declarations, 8 `#[verifier::external]`, 62 feature/cfg-gated,
and 4 trait default methods (see section 6).

| Trust base | Count |
|------------|------:|
| Axioms (`admit()`) | 48 |
| External-body (trusted) | 77 |
| Assume-specification (trusted) | 5 |
| **Total trust base** | **130** |
