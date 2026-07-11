# SCIP Range Format Bug (Historical Note)

## Summary

An earlier version of `probe-verus` (when it was named `scip-atoms`) had a bug in how it interprets SCIP range data for function definitions.

## The Bug

In `src/lib.rs` (lines 251-258):

```rust
// Extract line numbers from SCIP range [start_line, start_col, end_line, end_col]
let (lines_start, lines_end) = if node.range.len() >= 3 {
    let start = node.range[0] as usize + 1; // Convert to 1-based
    let end = node.range[2] as usize + 1;   // BUG: This is end_col, NOT end_line!
    (start, end)
} else {
    (0, 0)
};
```

## The Problem

SCIP ranges for function definitions use **3 elements**, not 4:

| Format | Elements | Meaning |
|--------|----------|---------|
| Single-line span | `[line, start_col, end_col]` | 3 elements - covers just the function NAME |
| Multi-line span | `[start_line, start_col, end_line, end_col]` | 4 elements - rarely used for function defs |

**Example from actual SCIP data:**
```json
{
  "range": [748, 7, 12],  // Line 749, columns 7-12 (just "part1")
  "symbol": "...Scalar52#part1().",
  "symbol_roles": 1
}
```

The code assumes `range[2]` is `end_line`, but it's actually `end_col` (12).

## Symptoms

This produces nonsensical output like:
```json
{
  "code-text": {
    "lines-start": 321,
    "lines-end": 41    // <-- end < start! This is the end COLUMN, not line.
  }
}
```

## Root Cause

**SCIP does not provide function body end lines.** It only provides the location of the function NAME definition.

## Solution

Use a proper parser like `verus_syn` to extract accurate function spans. This is implemented in `rust-atomizer` in the `verus_parser` module.

## Date Discovered

December 1, 2025

