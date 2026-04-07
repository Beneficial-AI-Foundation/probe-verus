# Verification Statistics: curve25519-dalek 4.1.3

Generated from `examples/verus_curve25519-dalek_4.1.3.json` produced by
`probe-verus extract`.

All queries below use `jq` on the extract JSON:

```bash
FILE=examples/verus_curve25519-dalek_4.1.3.json
```

---

## 1. Public functions verified

**167 / 246** `pub fn` exec functions are verified or trusted (67.9%):
102 verified + 65 trusted.

These are functions whose signature starts with unrestricted `pub` (not
`pub(crate)` or `pub(super)`) and whose kind is `exec`.

```bash
# Count
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec")] | length' "$FILE"
# => 246

# Count verified
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and .value."verification-status" == "verified")] | length' "$FILE"
# => 102

# Count verified or trusted
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and (.value."verification-status" == "verified" or .value."verification-status" == "trusted"))] | length' "$FILE"
# => 167

# List verified or trusted pub fn
jq -r '.data | to_entries[] | select(.value."is-public" == true and .value.kind == "exec" and (.value."verification-status" == "verified" or .value."verification-status" == "trusted")) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 2. Public API functions verified

**122 / 148** public API functions are verified or trusted (82.4%):
93 verified + 29 trusted.

Public API = `pub fn` + all ancestor modules are `pub` + exec kind + library
crate. `spec fn` and `proof fn` are excluded (erased at runtime).

```bash
# Count
jq '[.data | to_entries[] | select(.value."is-public-api" == true)] | length' "$FILE"
# => 148

# Count verified
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" == "verified")] | length' "$FILE"
# => 93

# Count verified or trusted
jq '[.data | to_entries[] | select(.value."is-public-api" == true and (.value."verification-status" == "verified" or .value."verification-status" == "trusted"))] | length' "$FILE"
# => 122

# List verified or trusted public API
jq -r '.data | to_entries[] | select(.value."is-public-api" == true and (.value."verification-status" == "verified" or .value."verification-status" == "trusted")) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort

# Full public API breakdown by verification status
jq '[.data | to_entries[] | select(.value."is-public-api" == true)] | group_by(.value."verification-status" // "absent") | map({status: .[0].value."verification-status" // "absent", count: length})' "$FILE"
```

---

## 3. Axioms (functions using `admit()`)

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

## 4. External functions assumed correct

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

## Summary

| Metric | Count | Denominator | % |
|--------|------:|------------:|---:|
| `pub fn` exec verified or trusted | 167 | 246 | 67.9% |
| — of which verified | 102 | 246 | 41.5% |
| — of which trusted | 65 | 246 | 26.4% |
| Public API verified or trusted | 122 | 148 | 82.4% |
| — of which verified | 93 | 148 | 62.8% |
| — of which trusted | 29 | 148 | 19.6% |
| Axioms (`admit()`) | 48 | — | — |
| External-body (trusted) | 77 | — | — |
| Assume-specification (trusted) | 5 | — | — |
| **Total trust base** | **130** | 2281 | 5.7% |
