#!/usr/bin/env bash
#
# Manual integration test: verify merge-atoms equivalence for libsignal + libcrux-ml-kem
#
# This script tests that independently indexing two projects and merging
# produces equivalent results to indexing them as a combined workspace.
#
# Prerequisites:
#   - probe-verus built and installed (cargo install --path .)
#   - verus-analyzer available on PATH
#   - libsignal checked out at:  LIBSIGNAL_PATH (see below)
#   - libcrux-ml-kem checked out at: LIBCRUX_ML_KEM_PATH (see below)
#
# Approach B (combined workspace) setup instructions:
#   1. Clone/fork libsignal into a separate directory for the combined workspace
#   2. Add libcrux as a git submodule:
#        cd <combined-workspace>
#        git submodule add <libcrux-repo-url> deps/libcrux
#   3. Create a symlink at the workspace root:
#        ln -sfn deps/libcrux/libcrux-ml-kem libcrux-ml-kem
#   4. Edit Cargo.toml:
#        - Add "libcrux-ml-kem" to [workspace] members
#        - Add to [patch.crates-io]:
#            libcrux-ml-kem = { path = "libcrux-ml-kem" }
#   5. Verify it builds: cargo check
#   6. Set COMBINED_WORKSPACE_PATH below to point to this directory

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────────────
LIBSIGNAL_PATH="${LIBSIGNAL_PATH:-/home/lacra/git_repos/baif/libsignal_original/libsignal}"
LIBCRUX_ML_KEM_PATH="${LIBCRUX_ML_KEM_PATH:-/home/lacra/git_repos/baif/libcrux/libcrux-ml-kem}"
# Set this to a workspace that includes both libsignal and libcrux-ml-kem
COMBINED_WORKSPACE_PATH="${COMBINED_WORKSPACE_PATH:-}"

OUTDIR="${OUTDIR:-/tmp/merge_test_libsignal_libcrux}"
PROBE_VERUS="${PROBE_VERUS:-probe-verus}"
# Use --rust-analyzer by default; set to "" to use verus-analyzer
ANALYZER_FLAG="${ANALYZER_FLAG:---rust-analyzer}"

# ── Setup ──────────────────────────────────────────────────────────────
mkdir -p "$OUTDIR"

echo "═══════════════════════════════════════════════════════════"
echo "  Merge-atoms equivalence test: libsignal + libcrux-ml-kem"
echo "═══════════════════════════════════════════════════════════"
echo
echo "  libsignal:       $LIBSIGNAL_PATH"
echo "  libcrux-ml-kem:  $LIBCRUX_ML_KEM_PATH"
echo "  output dir:      $OUTDIR"
echo

# ── Approach A: Independent indexing + merge ───────────────────────────
echo "━━━ Approach A: Independent indexing + merge ━━━"
echo

echo "Step 1: Atomize libsignal..."
$PROBE_VERUS atomize "$LIBSIGNAL_PATH" $ANALYZER_FLAG -o "$OUTDIR/atoms_libsignal.json"
echo

echo "Step 2: Atomize libcrux-ml-kem..."
$PROBE_VERUS atomize "$LIBCRUX_ML_KEM_PATH" $ANALYZER_FLAG -o "$OUTDIR/atoms_libcrux_ml_kem.json"
echo

echo "Step 3: Merge..."
$PROBE_VERUS merge-atoms \
    "$OUTDIR/atoms_libsignal.json" \
    "$OUTDIR/atoms_libcrux_ml_kem.json" \
    -o "$OUTDIR/merged.json"
echo

# ── Approach B: Combined workspace (optional) ─────────────────────────
if [ -n "$COMBINED_WORKSPACE_PATH" ]; then
    echo "━━━ Approach B: Combined workspace ━━━"
    echo
    echo "Step 4: Atomize combined workspace..."
    $PROBE_VERUS atomize "$COMBINED_WORKSPACE_PATH" $ANALYZER_FLAG -o "$OUTDIR/atoms_combined.json"
    echo
else
    echo "━━━ Skipping Approach B (COMBINED_WORKSPACE_PATH not set) ━━━"
    echo "  Set COMBINED_WORKSPACE_PATH to a workspace containing both projects"
    echo "  to run the full equivalence comparison."
    echo
fi

# ── Comparison ─────────────────────────────────────────────────────────
echo "━━━ Analysis ━━━"
echo

# Count atoms in each file
count_atoms() {
    python3 -c "
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
print(len(data))
" "$1"
}

count_stubs() {
    python3 -c "
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
stubs = [k for k, v in data.items() if not v.get('code-path', '')]
print(len(stubs))
" "$1"
}

echo "Approach A results:"
echo "  atoms_libsignal.json:      $(count_atoms "$OUTDIR/atoms_libsignal.json") atoms ($(count_stubs "$OUTDIR/atoms_libsignal.json") stubs)"
echo "  atoms_libcrux_ml_kem.json: $(count_atoms "$OUTDIR/atoms_libcrux_ml_kem.json") atoms ($(count_stubs "$OUTDIR/atoms_libcrux_ml_kem.json") stubs)"
echo "  merged.json:               $(count_atoms "$OUTDIR/merged.json") atoms ($(count_stubs "$OUTDIR/merged.json") stubs)"
echo

if [ -n "$COMBINED_WORKSPACE_PATH" ] && [ -f "$OUTDIR/atoms_combined.json" ]; then
    echo "Approach B results:"
    echo "  atoms_combined.json:       $(count_atoms "$OUTDIR/atoms_combined.json") atoms ($(count_stubs "$OUTDIR/atoms_combined.json") stubs)"
    echo

    echo "━━━ Equivalence comparison ━━━"
    echo
    python3 -c "
import json, sys

with open('$OUTDIR/merged.json') as f:
    merged = json.load(f)
with open('$OUTDIR/atoms_combined.json') as f:
    combined = json.load(f)

merged_keys = set(merged.keys())
combined_keys = set(combined.keys())

shared = merged_keys & combined_keys
only_merged = merged_keys - combined_keys
only_combined = combined_keys - merged_keys

print(f'  Shared keys:          {len(shared)}')
print(f'  Only in merged:       {len(only_merged)}')
print(f'  Only in combined:     {len(only_combined)}')
print()

diffs = 0
for key in sorted(shared):
    m = merged[key]
    c = combined[key]
    issues = []
    if set(m.get('dependencies', [])) != set(c.get('dependencies', [])):
        issues.append('dependencies')
    if m.get('code-path', '') != c.get('code-path', ''):
        issues.append('code-path')
    if m.get('code-text', {}) != c.get('code-text', {}):
        issues.append('code-text')
    if m.get('mode', '') != c.get('mode', ''):
        issues.append('mode')
    if issues:
        diffs += 1
        if diffs <= 20:
            print(f'  DIFF {key}:')
            for issue in issues:
                print(f'    {issue}: merged={m.get(issue)} vs combined={c.get(issue)}')

if diffs == 0:
    print('  All shared atoms are equivalent!')
else:
    print(f'  {diffs} atoms differ (showing first 20)')

if only_merged:
    print()
    print('  Keys only in merged (first 10):')
    for k in sorted(only_merged)[:10]:
        print(f'    {k}')

if only_combined:
    print()
    print('  Keys only in combined (first 10):')
    for k in sorted(only_combined)[:10]:
        print(f'    {k}')
"
fi

echo
echo "═══════════════════════════════════════════════════════════"
echo "  Output files in: $OUTDIR"
echo "═══════════════════════════════════════════════════════════"
