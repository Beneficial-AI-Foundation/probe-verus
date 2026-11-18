# Codex review of verus-analyzer-compat-plan.md

Reviewer: Codex via OpenRouter, read-only sandbox, 2026-11-17. Raw output: /tmp/codex-compat-plan.out.

## Codex's review (verbatim)

**Request changes.** The plan preserves one narrow contract—how a known legacy index is interpreted—but does not yet preserve project identity across runs, ensure that the selected analyzer actually executes, or make verification-result merging safe.

I inspected `main`, the local `origin/feat/latest-verus-analyzer` snapshot at `48b909e`, and both previous reviews. I did not run tests or independently verify the upstream release dates and minimum-toolchain claims.

## 1. Preserving an index’s names is not preserving a project’s names

**Plan §1:** “Legacy: run the 8.x symbol repair unchanged.”  
**Plan §3:** “installs the newest analyzer release whose minimum toolchain is at or below that toolchain.”

These policies conflict with the stated goal of preserving downstream identities.

Consider an existing project with:
- Rust 1.94;
- an old-format analyzer;
- stored verilib data keyed by legacy code-names.

Running the proposed `setup --from-project` selects a new-format analyzer. The analyzer-version cache key then triggers regeneration, and the project’s atoms are renamed. No source change or explicit identity migration was requested.

The scheme field makes this detectable **only if consumers implement and enforce a migration policy**. It does not preserve stored annotations or prevent duplicate records.

**Required:** Separate *compatibility selection* from *upgrade policy*. Pin an existing project’s analyzer/naming scheme until an explicit upgrade. Specify what downstream storage does when the scheme changes: reject ingestion, create a separate namespace, or perform a reviewed migration. A warning suggesting an analyzer upgrade must disclose the identity change.

## 2. Installing the right analyzer does not ensure it runs

**Plan §3:** “Analyzers install in versioned directories … as Verus does.”

The runtime handoff is missing. `ScipCache::check_prerequisites` calls a resolver without project context (`src/scip_cache.rs:235`). `resolve_tool` checks a managed binary and then `PATH`, not a project-specific version (`src/tool_manager.rs:541`).

A concrete failure:
1. Setup project A with an old analyzer.
2. Setup project B with a new analyzer.
3. Extract A.
4. A resolver choosing the newest installed analyzer runs B’s version.

A global “active analyzer” symlink would have the same problem, including races between concurrent projects.

Also, **the existing Verus resolver is not a safe model to copy unchanged**: if the explicitly requested version is missing, `managed_verus_binary` falls back to another installed version (`src/tool_manager.rs:190`).

**Required:** Resolve one immutable tool selection per invocation and pass it through installation, cache validation, execution, and output provenance. Define managed-directory versus `PATH` precedence. A missing explicitly pinned version must be installed or produce an error—not silently select another version. Cover `atomize`, `extract --auto-install`, setup, and status reporting.

## 3. The minimum-Rust table is not a compatibility specification

**Plan §3:** “newest analyzer release whose minimum toolchain is at or below that toolchain.”

Even accepting the supplied minimum versions, the inequality is not sufficient evidence that a particular analyzer supports a project. The plan needs to establish compatibility with the project’s Verus syntax, macro handling, cfg behavior, and indexing features—not merely a Rust version threshold.

The table also makes an unjustified prediction:

> “2026-08-22 and later | 1.94”

That assigns compatibility to every future release. If a later analyzer raises its minimum, this selector will still choose it.

“From the Verus release … or from the project's own `rust-toolchain.toml`” is also unresolved precedence, not an implementation detail. What happens when those disagree? What about inherited toolchain files, `RUSTUP_TOOLCHAIN`, `stable`, dated nightlies, custom toolchains, or workspace members with conflicting Verus requirements? The analyzer subprocess currently inherits its environment (`src/scip_cache.rs:284`).

**Required:** Use a finite, tested release catalog. Unknown releases must not inherit the last known compatibility range. Define the effective toolchain used by the indexing subprocess and report conflicts. Specify offline behavior, missing platform assets, and the no-compatible-release case.

A smaller, safer first implementation is an explicit analyzer pin plus known-good defaults, rather than automatic “newest compatible.”

## 4. `Unknown` cannot safely mean either legacy or current

**Plan §1:** “producer version first and symbol syntax second.”  
**Open question 1:** legacy or current default.

Both silent defaults can corrupt identity. A free-functions-only index can lack distinguishing syntax while still differing in module-path ordering. There is no information-theoretically sound inference from absent evidence.

The detector also needs policies for:
- Producer metadata contradicting symbol syntax.
- Mixed legacy/current evidence.
- Unknown or custom producer versions.
- Upstream rust-analyzer versus verus-analyzer version namespaces.
- Evidence present only in occurrence references.
- Future formats that still contain familiar `impl#[` syntax.

Backtick-aware parsing is necessary but does not resolve these cases.

There is also a concrete implementation prerequisite: `ScipToolInfo.version` is currently a required `String`, so an omitted version fails deserialization before an `Unknown` detector can run (`src/lib.rs:81`).

**Required:** Distinguish unknown, conflicting, and unsupported inputs. For ambiguous imported indexes, require an explicit format override or documented trusted provenance. Do not serialize an inferred legacy/current scheme as established fact. Use producer-aware, bounded version knowledge rather than an open-ended numeric threshold.

## 5. Scheme metadata needs a complete compatibility contract

**Plan §2:** “Each output records which naming scheme produced its code-names.”

Placement is not the only unresolved issue.

**Historical files:** Existing files have no scheme field. Defaulting them to legacy is unsafe because, by the plan’s own admission, 8.x also processed current-format indexes into malformed names. “Written by 8.x” therefore does not identify a canonical legacy naming domain.

**Propagation:** Current extract loaders discard the envelope (`src/commands/extract.rs:832`). `merge-atoms` likewise unwraps payloads, and merged provenance retains only schema and source—not naming scheme (`src/commands/merge_atoms.rs:58`, `src/metadata.rs:63`). Scheme checks must survive nested merges and specs/proofs round trips.

**Payload shape:** Putting `"code-name-scheme": "legacy"` directly alongside atoms in `data` breaks readers expecting every value to be an atom. This includes the current enrichment path (`src/commands/extract.rs:1035`).

**Mixed dependencies:** A new-format project may depend on a library independently indexed with a legacy analyzer. Blanket rejection makes that graph impossible to assemble. Allowing it without a mapping leaves incompatible dependency identities. Either outcome needs an explicit product decision.

**Required:** Define a versioned naming contract, such as `legacy-v1`, separately from analyzer provenance. Specify absent-field behavior, validation, propagation, and cross-scheme composition before implementation. An agreed envelope extension is safer than inserting metadata into an atom dictionary.

## 6. Equal schemes do not make verification artifacts safe to merge

**Plan §2:** “the `extract` merge refuse[s] to combine atoms of different schemes.”  
**Plan §5.3:** “`extract` stops after a failed atomize.”

These checks do not address stale proofs within the same scheme.

Concrete failure:
1. A previous run writes successful verification results.
2. A function body changes without changing its code-name.
3. Atomization succeeds, but the new verification step fails before replacing its output.
4. Extraction merges the old successful proof into the new atom.

Current merge-path selection depends on skip flags rather than step success (`src/commands/extract.rs:236`), and `run_unified_merge` accepts files because they exist (`src/commands/extract.rs:950`). Envelope metadata is discarded before merging.

This is **pre-existing**, but the proposed scheme check must not be presented as a provenance or verification-safety guarantee.

**Required:** Merge only successful artifacts from the current run, or explicitly validate reusable artifacts against a source/configuration fingerprint and atom-set identity. A commit alone is insufficient for dirty working trees. Publish outputs atomically and do not let failed stages reuse old files implicitly.

## 7. The PR’s “independent fixes” are not all established as safe

**Plan §5.3:** “`assume_specification` matching fixes, and graph nodes keyed by definition range.”

The current PR snapshot still contains the substantive trust-matching problem from round 2:

- `normalize_type` removes qualification (`src/lib.rs:1238` on the PR branch).
- Omitted trait arguments are matched by bare trait name (`src/lib.rs:1270` on the PR branch).
- The extract matcher attaches a specification when exactly one indexed stub matches (`src/commands/extract.rs:593` on the PR branch).

Thus a specification for `<good::Token as good::Check>::check` can still match an unrelated singleton stub with the same type and trait basenames. “Only one candidate was indexed” is not proof of target identity. Attachment can lead to a `trusted` status.

The graph change also still pairs records and sorted definition occurrences by position when their counts match (`src/lib.rs:683` on the PR branch). Equal counts do not establish correspondence.

**Required:** Do not import these changes under a blanket “fixes” label. Require negative end-to-end tests for wrong-package targets and default generic arguments. Leave ambiguous assumptions unattached unless authoritative identity is available. Test record-order permutations through final output.

## 8. The compatibility promise contradicts the planned changes and test

**Plan §1:** “Output must be byte-identical to 8.0.1.”  
**Plan §5:** add scheme metadata and apply backfill/graph fixes to both schemes.

Literal byte identity is impossible with an added field, changed tool version, and timestamps. More importantly, preserving previously dropped definitions or changing backfill decisions can change legacy atom payloads.

The existing golden test cannot establish the stronger claim:
- It permits additive fields and ignores volatile metadata (`tests/extract_backward_compat.rs:1`).
- It skips when fixtures or tools are unavailable (`tests/extract_backward_compat.rs:68`).
- It regenerates the index with the installed analyzer and skips verification (`tests/extract_backward_compat.rs:92`).

**Required:** Replace “byte-identical” with a precise compatibility contract. Freeze legacy/current SCIP fixtures and their source snapshots, and make those regression tests independent of installed tools. Test final keys, dependencies, spans, and metadata—not just intermediate node counts.

A bug-fix justification explains an output change; it does not make the compatibility change disappear. The 9.0.0 breaking notice cannot promise that only current-format indexes are affected if legacy corrections or stricter merge requirements also change behavior.

## 9. Versioned directories and cache keys do not solve races

**Plan §3:** “The SCIP cache key includes the analyzer version.”

Current caching checks source mtimes before resolving tools (`src/scip_cache.rs:177`). Adding a version string requires deciding whether that is the requested release, actual executable version, or executable identity. Those can differ, especially with `PATH` binaries and custom builds.

Other concrete gaps:
- Manifest, lockfile, toolchain, feature, and cfg changes can alter indexing without a newer `.rs` file; the current freshness check only examines `.rs` mtimes (`src/scip_cache.rs:210`).
- Concurrent analyzers write the same project-root `index.scip` (`src/scip_cache.rs:316`).
- Concurrent installs can write the same executable; decompression currently creates the destination directly (`src/tool_manager.rs:695`).
- A crash can leave a partial file that later existence checks treat as installed or cached.

**Required:** Add per-install and per-project coordination, staged writes, validation, and atomic publication. Persist the actual resolved tool identity and indexing configuration with the cache. Validate release identifiers before using them in directory paths.

## Safer scope and rollout

I would first ship explicit analyzer pinning, deterministic runtime resolution, and two tested format interpreters. Make ambiguous format selection explicit. Add scheme metadata only with an agreed historical-input and consumer-ingestion policy. Defer automatic compatibility selection until the release matrix is demonstrated.

Treat legacy removal as a separate migration decision. Moving consumers to Rust 1.94 does not migrate archived indexes, stored code-name references, or annotations.

## Prioritized concerns

1. **False trust attribution:** the proposed PR imports still allow lossy matching to attach assumptions to unrelated atoms.
2. **Stale verification results:** same-scheme artifacts from different runs can falsely mark changed code verified.
3. **Unrequested identity migration:** automatic analyzer upgrades still rename existing projects’ atoms.
4. **Wrong analyzer execution:** project-aware installation lacks a project-aware runtime selection contract.
5. **Unsound format and compatibility inference:** ambiguous indexes and unknown releases lack safe policies.
6. **Incomplete metadata contract:** historical files, nested merges, consumers, and mixed-scheme dependencies are unresolved.
7. **Race and partial-write risks:** versioned paths alone do not make installation or extraction transactional.
8. **Unproven compatibility promise:** the golden test and “byte-identical” requirement do not match the proposed behavior.

# Round 2 (plan revision 2)

Reviewer: Codex via OpenRouter, read-only sandbox, 2026-11-17. Raw output: /tmp/codex-compat-plan-r2.out.

**Request changes.** Revision 2 resolves several architectural omissions, but the new guard is not conservative as claimed, filesystem-based detection can produce false evidence, and the compatibility contract is still internally inconsistent.

I inspected `main` at `7599440`, the local PR #50 snapshot at `48b909e`, and the prior reviews. This is a static review; I did not run tests or independently verify the supplied upstream release history. I am not reopening the explicitly deferred locking, matching, stale-artifact, ingestion, or cross-scheme-mapping work.

## Round-1 disposition

| Round-1 concern | Revision-2 status |
|---|---|
| 1. Unrequested identity migration | **Partly resolved.** The guard adds explicit consent, but its treatment of historical outputs defeats that protection in one direction. |
| 2. Selected analyzer actually executes | **Largely resolved in the design.** Immutable selection and no fallback address the core issue. `path` identity and command coverage remain underspecified. |
| 3. Compatibility inferred from minimum Rust | **Partly resolved.** Analyzer releases are bounded; claimed toolchain compatibility remains unbounded. |
| 4. Unknown/conflicting formats | **Partly resolved.** Explicit states, optional version, and overrides help. Filesystem evidence introduces a new misclassification mechanism. |
| 5. Scheme metadata contract | **Partly resolved.** Placement and versioned names are addressed. Unknown-input merging and standalone propagation are not. |
| 6. Stale verification artifacts | **Explicitly deferred**, with an appropriate disclaimer. |
| 7. Unsafe matching/record-pairing imports | **Removed from this work**, appropriately. Backfill changes still conflict with the compatibility promise. |
| 8. Compatibility tests | **Improved, not resolved.** Frozen fixtures address the old test’s weakness; “every output” still exceeds what these fixtures establish. |
| 9. Cache identity, races, partial writes | Directory-name validation and selection-aware caching address part of this. **Executable identity remains open**; coordination is explicitly deferred. |

## Findings

### 1. The historical-output guard does not “only block”

**§3:** “Existing outputs without a scheme field count as `legacy-v1` … That is safe because the check blocks, it never renames.”

That reasoning is false. The check can **permit an overwrite**:

1. An existing untagged output contains names produced by 8.x processing a current-format index—the historical case identified in round 1.
2. A new run selects a genuine legacy analyzer.
3. The guard treats the old file as `legacy-v1`.
4. The new scheme is also `legacy-v1`, so replacement proceeds without consent.

The guard need not perform the renaming itself; allowing the subsequent write is sufficient. More generally, absent metadata does not establish that the existing keys belong to canonical `legacy-v1`.

**Required:** Unknown historical outputs must remain unknown for authorization purposes. Require explicit acceptance or an explicit historical-scheme assertion before replacing them. Do not silently promote absence to legacy.

Test both directions: untagged → legacy and untagged → current. The former is the missing case.

### 2. Document paths are not reliable evidence of module ordering

**§1, evidence 3:** Compare module order with the document path.

A valid source layout can make a current symbol look exactly like the proposed legacy ordering:

```rust
// src/lib.rs
#[path = "b/a.rs"]
mod a;

// src/b/a.rs
pub mod b {
    pub fn f() {}
}
```

The logical module path is `a/b`, while the file-derived path is `b/a`. Your heuristic sees the current logical path as the reverse of the physical path.

Consequences:

- Known current producer metadata becomes falsely `Conflicting`.
- An unknown producer can be falsely classified as `Legacy`.

Inline modules, `mod.rs`, custom crate-root paths, and reused source files also invalidate a direct path-to-module correspondence. These are not necessarily malformed or unsupported projects.

Furthermore, “reads definitions and occurrences” needs an evidence-specific restriction: a reference occurrence’s document is the **caller’s location**, not the referenced symbol’s defining module.

**Required:** Either remove filesystem ordering as decisive evidence, or establish the actual logical module mapping before using it. Only use defining occurrences for this comparison. A matching physical path is not proof that the logical module mapping is conventional.

Also specify that equal forward/reverse paths—including single-component and palindromic paths—provide **no distinguishing evidence**, not evidence for both formats.

### 3. Terminal failure is a prerequisite for the guard, not a later PR

**§8:** PR 1 introduces the guard; PR 3 makes `extract` stop after failed atomization.

Current `run_atomize_step` records an error rather than returning it (`src/commands/extract.rs:303`). The caller then continues to later stages and eventually writes a summary (`src/commands/extract.rs:175`, `src/commands/extract.rs:271`).

If PR 1 implements rejection inside atomization and relies on that error propagating, the guard will **not stop extraction**. This is a direct interaction with the new protection, not a request to solve the deferred stale-artifact issue.

**Required:** Make guard rejection terminal at the `extract` boundary in PR 1, or move PR 3 ahead of it. Likewise, detector failures must stop the pipeline.

Acceptance test: cause a guard rejection, assert a nonzero result, assert no later stage executes, and compare all pre-existing output contents before and after.

### 4. “Existing outputs” needs a precise scope and lifecycle

**§3:** Inspect existing outputs in `.verilib/probes/`.

Scanning the entire directory and inspecting only intended destination files both have problems:

- **Entire directory:** Run legacy extraction, then accept a current-scheme `atomize`. Old specs, proofs, and unified outputs remain. Does every subsequent current-scheme atomization now require acceptance again?
- **Destination files only:** Changing package version changes the default filename, potentially avoiding comparison with the previous output. Filenames include package and version (`src/metadata.rs:281`).
- **Directory only:** `atomize --output` supports an arbitrary destination. Does an existing file outside `.verilib/probes/` receive no protection? See `src/commands/atomize.rs:163`.
- **Multiple packages or archived outputs:** Does an unrelated output veto the current package’s run?

This ambiguity also affects `--skip-atomize` and `--with-atoms`: the scheme must come from the artifact actually used, not merely the analyzer that would have been selected.

**Required:** Define the protected identity, active output set, destination checks, and acceptance lifecycle. If acceptance is intentionally per invocation, say so. Do not make old auxiliary files silently become a permanent migration veto.

### 5. Workspace pin resolution can disagree between setup and extraction

**§4:** A pin may appear under workspace or package metadata.

The plan does not define:

- Which wins when both exist.
- Whether a member inherits the workspace pin.
- What happens when members disagree.
- Whether lookup starts from the invocation path, selected package, or workspace root.

This is consequential in this repository. `extract` replaces its project path using `resolve_workspace_root` before gathering metadata (`src/commands/extract.rs:127`). Despite its name, that helper can return a selected member directory (`src/lib.rs:1861`) or automatically choose the sole member (`src/lib.rs:1895`). Setup examines the supplied project path (`src/commands/setup.rs:82`).

A naïve implementation can therefore install the workspace-pinned analyzer during setup, then ignore that pin during extraction.

**Required:** Resolve an explicit workspace/package context once and share it across setup, extraction, atomization, and status reporting. Specify conflict and inheritance rules. Invalid pin values must error rather than silently select the catalog default.

The guard’s suggested remedy also needs correction: adding a Cargo pin does nothing while `PROBE_VERUS_ANALYZER_VERSION` overrides it. Diagnostics must identify the winning configuration source.

### 6. The catalog still makes unsupported compatibility claims

**§4:** “The catalog lists only tested releases,” followed by `<1.94` and `>=1.94`.

Testing an analyzer release is not equivalent to testing those entire ranges.

Accepting the plan’s own minimum-version account:

- `<1.94` includes versions below the old analyzer’s stated 1.78 minimum.
- `>=1.94` includes arbitrarily future toolchains.
- Neither range establishes compatibility with all Verus releases using that Rust version.

The project-toolchain override is especially questionable. An old Verus dependency does not become compatible with the new analyzer merely because the project’s own toolchain file names Rust 1.94. The opening problem is about indexing old Verus projects, not just choosing an installed compiler.

**Required:** Separate supported combinations from best-effort defaults. Record tested Verus/toolchain/analyzer combinations and explicit exclusions. Unknown combinations should require a pin or clearly acknowledged best-effort behavior—not inherit an unlimited compatibility assertion.

A simpler first release is explicit pinning plus a small set of demonstrated defaults, rather than universal two-bucket selection.

### 7. Toolchain discovery is not yet tied to execution or offline operation

**§4:** The project toolchain file wins; offline runs use the installed selected analyzer.

Two missing contracts:

**Effective toolchain.** Reading a file does not ensure the analyzer’s subprocesses use that toolchain. The current analyzer command inherits its environment (`src/scip_cache.rs:284`). The plan needs a policy for `RUSTUP_TOOLCHAIN`, inherited toolchain files, directory overrides, floating channels, and custom toolchains. It also needs to reconcile the existing `PROBE_VERUS_VERSION` override with deriving the analyzer from the Verus version in Cargo metadata (`src/tool_manager.rs:442`).

**Offline resolution.** An installed analyzer does not help if the selector cannot determine which analyzer to choose. Fetching the Verus toolchain currently requires GitHub (`src/tool_manager.rs:857`). Setup can cache a channel marker (`src/tool_manager.rs:953`), but revision 2 does not specify using that marker, its absence, or its relationship to the requested Verus release.

**Required:** Define resolution and execution together. Specify local metadata sources and failure behavior before claiming offline support. Explicit analyzer pins should bypass unnecessary toolchain/network discovery.

Also explicitly cover the existing `--rust-analyzer` mode; a verus-analyzer release catalog cannot silently stand in for both tools.

### 8. `path` is a selection mode, not an analyzer version

**§§4–5:** Pin `path`; cache by analyzer version and binary path.

If the cache records `version = "path"` and `/usr/local/bin/verus-analyzer`, replacing that executable changes neither field. Even recording `--version` is insufficient for custom builds with identical reported versions.

This is a gap in the newly proposed selection identity, separate from general source/artifact freshness.

**Required:** Distinguish:

- Requested selector.
- Resolved absolute executable path.
- Reported executable identity.
- SCIP producer metadata.
- Detected or explicitly selected naming scheme.

For mutable `PATH` installations, fingerprint the executable or disable reuse when identity cannot be established. Resolve the path once rather than repeating a lookup.

Do not populate index provenance from the currently selected executable when consuming an existing index. That executable may not have produced it.

### 9. Unknown-input merging and standalone propagation remain undefined

**§2:** Missing scheme means unknown; inputs with different schemes are rejected.

What happens for:

- Unknown + unknown?
- Unknown + legacy?
- An unsupported value such as `legacy-v2`?
- An invalid field type?
- A nested merged envelope?

Two unknown values being equal does not establish compatibility. Conversely, rejecting them is a deliberate historical-input breaking change that needs documentation and a recovery mechanism.

Standalone commands matter too. `specify` currently strips the input envelope (`src/commands/specify.rs:249`). `run-verus` can produce either code-name-keyed proofs or a verification report without atoms (`src/commands/run_verus.rs:81`).

**Required:** Define a merge decision table and propagate the scheme from the atoms actually consumed by standalone commands. A report without code-name-keyed atoms should not invent a naming scheme. Merged provenance must accommodate multiple producers within one valid scheme.

Keep `--accept-scheme-change` separate from permission to merge incompatible or unknown inputs.

### 10. Section 6 is not achievable as written

**§6:** “For a legacy index, the `data` payload of every output stays identical to 8.0.1.”

Three problems:

**Backfill contradicts the promise.** PR 4 explicitly changes legacy output. A separate changelog entry does not make the equality claim true. State whether the contract applies only before that PR, excludes enumerated changes, or preserves the old behavior behind a compatibility mode.

**An index is not the complete input.** Source snapshots help, but proofs and unified outputs also depend on verifier results, options, metadata, and auxiliary inputs. Extract-summary payloads contain output filenames (`src/commands/extract.rs:46`). Tool-free SCIP fixtures alone cannot establish equality of every output.

**Some legacy duplicate behavior has no unique reference result.** Conversion iterates `HashMap` values (`src/lib.rs:1330`), and `--allow-duplicates` keeps the first atom for a duplicate key (`src/commands/atomize.rs:117`). When duplicate candidates differ, “the 8.0.1 output” is not necessarily a single stable value. This needs a contract boundary, not an unrelated bug fix smuggled into compatibility work.

A testable replacement would be:

> For successfully processed legacy fixtures, given identical SCIP, sources, options, project metadata, and captured external-tool results, supported output schemas have structurally equal `data` values to the frozen 8.0.1 baseline, except for an explicit list of changes.

Define the supported schemas, duplicate-case policy, and exceptions. Use strict equality—not the existing comparator’s permission for additional fields (`tests/extract_backward_compat.rs:4`).

For the flat-free-function open item: the simple symbol conversion can coincide, but that does not identify the format or justify silently assigning a scheme. Any warning-only policy must specify the selected scheme and establish equality across the relevant pipeline, including backfill and dependencies.

## Prioritized concerns

1. **Historical unknown outputs can be overwritten without consent:** the guard’s central safety claim is false.
2. **Filesystem ordering can misclassify valid indexes:** remove it as decisive evidence unless logical module mapping is established.
3. **Guard and detector rejection must terminate extraction immediately:** this cannot depend on a later PR.
4. **Workspace pins and effective toolchains lack one consistent resolution contract:** setup and execution can disagree.
5. **The catalog overclaims compatibility:** finite analyzer releases do not justify unbounded Rust/Verus support.
6. **The legacy equality promise contradicts planned changes and nondeterministic cases:** narrow it before writing acceptance tests.
7. **Scheme scope, unknown merging, and mutable `path` identity remain underspecified:** these need explicit state tables and end-to-end tests, not just field additions.

# Round 3 (plan revision 4)

Reviewer: Codex via OpenRouter, read-only sandbox, 2026-11-17. Raw output: /tmp/codex-compat-plan-r3.out.

**The single-path direction is sound. Revision 4 is not yet ready for approval.** Accepting the owner’s decision removes the main reason to retain legacy symbol conversion. But the evidence does not establish the stated support guarantee, and several proposed safeguards are weaker than their descriptions.

I inspected the code at `48b909e`, the measurement scripts, and the JSON/log artifacts. I recomputed location comparisons; I did not rerun indexing.

## 1. The measurements do not test removal of `verus_keep_ghost`

**Sections 1 and 7; measurement 1.**

The corpus deliberately runs PR #50 with `noregen`, after main generates an index with the new analyzer. See `/tmp/vacompat/corpus-tc.sh:4` and `/tmp/vacompat/corpus.sh:7`.

That matters because main supplies the `verus_keep_ghost` configuration (`src/scip_cache.rs:255`), while the PR removes it.

Consequently, the measurements test:

> New analyzer, configured by main → PR conversion.

They do **not** test:

> PR’s new indexing configuration → PR conversion.

The removed configuration exists specifically to select specification-bearing variants. Parser backfill might restore function nodes without restoring their calls, or might attach specs to the wrong indexed variant.

**Required change:** independently generate fresh indexes with the candidate implementation on all five targets. Test cfg-dependent definitions, spec/body dependencies, and spec-to-atom matching—not just function counts. Keep the configuration until those tests demonstrate that removing it is safe.

## 2. The claimed location preservation is contradicted by the artifacts

**Support target; measurement 1: “8.0.1 finds the same function locations with both analyzers.”**

Using precisely `(code-path, lines-start)` for local atoms, I obtain:

| Project | Baseline locations missing from 8.0.1 + new analyzer | Baseline locations missing from PR #50 |
|---|---:|---:|
| dalek-verus | 0 | 2 |
| pmemlog | 0 | 5 |
| vest | **31** | **40** |
| capybarakv | **3** | **55** |

These are **location mismatches, not necessarily missing functions**. Some are visibly span changes:

- Vest’s `ex_to_be_bytes` moves from line 896 to 897.
- Capybarakv’s `as_byte_slice` moves from line 131 to 132.

But that distinction is exactly the problem: the plan defines support by start-line preservation. Under that definition, the measurements fail.

The claim that lower PR counts come from bodiless trait declarations also does not establish complete coverage. For example, capybarakv has **55 baseline-only locations and 40 new-only locations**, not merely 15 missing declarations.

**Required change:** either preserve the promised location convention or explicitly revise the test to compare source declarations using canonical spans and owner identity. Account for every unmatched declaration. Do not replace this with aggregate counts or a loose line window.

## 3. The five-project support claim has important holes

**Sections 7 and 9; “All … `atomize` only.”**

Three gaps remain:

- **Verismo has no successful old-analyzer baseline.** Its coverage requirement is currently undefined, or vacuously passes against an empty baseline. Establish an explicit expected inventory. Main with the new analyzer can supply a provisional inventory, but that is not independent evidence of analyzer completeness.
- **No full `extract` run is measured.** These experiments do not establish spec attachment or verification integration on the older toolchains, especially verismo’s fork. This does not require fixing the deferred Verus fallback, but the support claim must distinguish indexing support from end-to-end extraction support.
- **Every measured run enables `--allow-duplicates`.** That does not demonstrate that the ordinary command succeeds. Test the documented/default invocation, or explicitly make that flag part of the support contract.

The current measurement script also captures the command’s failure code and then finishes with a successful `echo`; it does not propagate failure. See `/tmp/vacompat/corpus.sh:11`. Reusing that script as a release gate would be dangerous.

**Required change:** use isolated output directories, fail on any failed stage, verify actual commits/toolchains, and make the corpus job a required release dependency rather than merely a scheduled report.

## 4. Dependency references are not a reliable completeness test

**Section 2: “A missing crate means that the analyzer did not load the project metadata.”**

That implication is too strong.

A dependency can be present in the manifest but unused, optional and disabled, or target-specific. Conversely, an index can contain many `vstd` references while omitting another module, target, macro expansion, or dependency.

The manifest lookup is also underspecified for workspace-inherited dependencies, renamed packages, and build/dev dependencies.

The proposed error message introduces another problem: the plan says verdict produced a partial index “without an error,” but promises to stop “with the analyzer’s metadata error.” There may be no such error. Current subprocess handling also discards stderr when not verbose (`src/scip_cache.rs:282`).

**Required change:** describe this as a targeted heuristic, not a completeness validator. Preserve analyzer diagnostics and emit a probe-owned diagnostic when none exists. Check relevant resolved dependencies and index/document coverage separately. Do not publish a failed-validation index as a valid cache entry.

## 5. “Exactly one matching stub” still does not mean fail-closed

**Section 3.6.**

Consider an unqualified target:

```rust
assume_specification[Record::read](...)
```

Suppose `Record` is imported from dependency A, but the only available stub is dependency B’s `Record::read`. Matching Self, method, and the absence of a trait can yield exactly one candidate—and still select the wrong function.

Preserving “any crate path in the target” does not help when the target uses an import or alias. Explicit paths also need dependency identity and version handling; the implementing crate is not necessarily the defining crate of Self or the trait.

The PR currently discards preceding segments for ordinary paths (`/tmp/vacompat/pv-pr/src/commands/extract.rs:583`). Fixing that is necessary, but not sufficient.

**Required change:** distinguish *unique textual candidate* from *established target identity*. Until #53 supplies authoritative resolution, leave cases with unresolved qualification or inferred identity unattached. A smaller supported matching subset is safer than incorrectly marking a stub trusted.

This concern is about attribution within one run, not cross-run code-name stability.

## 6. The proposed deduplication identity remains too weak

**Sections 3.2–3.4.**

`Type::method` does not identify the owner completely:

```rust
impl FirstTrait for Record { fn read() {} }
impl SecondTrait for Record { fn read() {} }
```

Both display as `Record::read`. The PR’s proximity test can suppress one when it is absent from the index because it accepts matching display names within a line tolerance. See `/tmp/vacompat/pv-pr/src/lib.rs:1809`.

There are two separate requirements:

1. Deduplication must not discard distinct definitions **before** final key generation.
2. Final key generation must not silently overwrite definitions that remain.

`@path:line:column` is not universally unique either: multiple macro-generated definitions can share an invocation location. Lifetime removal and other normalization can also collapse distinct raw symbols.

**Required change:** retain raw symbol identity internally; use exact source-definition identity where available, including trait/generic context where needed. Assert uniqueness after all conversion and backfill. For unresolved collisions, error or use an explicit additional discriminator—never silently overwrite.

## 7. The workspace pin lookup starts from the wrong abstraction

**Section 4: “The pin lookup starts at the directory that `resolve_workspace_root` returns.”**

Despite its name, this function resolves a virtual workspace **to a member package**. Its documentation explicitly says so (`src/lib.rs:1820`).

Therefore, reading metadata only at its result loses the root’s `[workspace.metadata.probe-verus]` pin. Walking arbitrary parent directories is not an adequate specification either.

**Required change:** keep the selected package manifest and actual workspace manifest as separate inputs. Define precedence using those two locations. Test:

- Invocation at the workspace root and directly inside a member.
- Package pin overriding workspace pin.
- Workspace pin with no package override.
- `setup --from-project` selecting the same analyzer as `extract`.

Also specify how the existing `--rust-analyzer` option interacts with this selection policy.

## 8. Binary identity alone does not make the cache valid

**Sections 4 and 5.**

Keeping the same analyzer binary while changing the toolchain, manifest, lockfile, target, features, or analyzer configuration can change the index. The existing freshness check only looks for newer `.rs` files (`src/scip_cache.rs:198`).

This matters especially because the design deliberately inherits the subprocess environment. Changing `RUSTUP_TOOLCHAIN` can leave the selector, binary path, and SHA-256 unchanged.

There is also a new consistency problem: the identity record must describe the exact index being reused. A crash or concurrent run can pair index A with identity B unless publication is coordinated. Similarly, two auto-installers can write the same versioned executable; a versioned directory does not prevent that.

**Required change:** include effective indexing configuration and relevant toolchain/manifest inputs in invalidation. Publish the validated index and identity as one coherent generation. Use staged installation and atomic publication for the selected analyzer.

This is narrower than solving all deferred output transactions in #54. It is necessary for the new cache/install guarantees themselves.

## 9. The five dalek diagnostics are not five ordinary functions to backfill

**Open item 1.**

The logged locations point to macro invocations and a derive site. For example:

- `define_add_variants!` at `/tmp/vacompat/dv-5b70dd73/curve25519-dalek/src/edwards.rs:2112`.
- `define_mul_variants!` at `/tmp/vacompat/dv-5b70dd73/curve25519-dalek/src/scalar.rs:645`.
- `#[derive(Copy, Clone)]` at `/tmp/vacompat/dv-5b70dd73/curve25519-dalek/src/backend/serial/u64/field.rs:112`.

The parser’s macro visitor handles `verus!` and `cfg_if!`, not general macro expansion (`src/verus_parser.rs:502`). “Make sure backfill covers these five functions” understates the problem: one diagnostic may concern generated definitions that parser backfill cannot reconstruct.

Likewise, restoring a local atom does not restore missing calls. The known edge omissions can affect downstream dependency/trust propagation even if the narrow location contract passes.

**Required change:** identify the actual missing generated symbols and affected edges. Add targeted assertions or document explicit limitations. Reporting upstream is useful, but is not a local correctness safeguard.

## Prioritized concerns

1. **Fresh candidate indexing is untested:** the corpus reuses indexes generated with the configuration the plan removes.
2. **The stated location-preservation claim is false for the supplied artifacts:** resolve all mismatches before claiming support.
3. **Spec attribution can still select the wrong singleton stub:** unresolved identity must remain unattached.
4. **Coverage and release gates are incomplete:** verismo lacks a baseline; full `extract` and default duplicate handling are unproven.
5. **Backfill and key generation can still lose distinct definitions:** full owner/source identity and final uniqueness checks are required.
6. **The partial-index check cannot establish completeness:** dependency presence is only a heuristic.
7. **Workspace selection and cache publication need precise semantics:** otherwise the wrong analyzer or stale index can be used.
8. **Macro-generated omissions and missing edges need explicit treatment:** atom counts cannot establish graph correctness.

**Recommendation:** keep the single-format design, but first land a corrected, fresh-index corpus gate. Preserve the ghost configuration unless that gate proves it unnecessary, and prefer refusing ambiguous attribution over adding more matching heuristics.
