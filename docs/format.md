# Output Format Specification

## Overview

`probe-verus atomize` generates a JSON dictionary of function metadata keyed by `code-name`
(a probe-style URI). Each entry contains the function's display name, dependencies, source
location, Verus mode, and optional per-call location data.

## JSON Structure

The top-level output is an **object** (dictionary), not an array. Keys are `code-name` URIs.

```json
{
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#Add<&Scalar>#add()": {
    "display-name": "Scalar::add",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/scalar/UnpackedScalar#add()"
    ],
    "code-module": "scalar",
    "code-path": "src/scalar.rs",
    "code-text": {
      "lines-start": 450,
      "lines-end": 475
    },
    "mode": "exec"
  }
}
```

## Fields

### Dictionary key: `code-name` (string)

A probe-style URI that uniquely identifies the function. Format:

```
probe:<crate>/<version>/<module>/<Type>#<Trait><TypeParam>#<method>()
```

Examples:
- Free function: `probe:curve25519-dalek/4.1.3/field/reduce()`
- Inherent method: `probe:curve25519-dalek/4.1.3/field/FieldElement51#square()`
- Trait impl: `probe:curve25519-dalek/4.1.3/scalar/Scalar#Add<&Scalar>#add()`

The `code-name` is not serialized inside the value object (it is the key).

### `display-name` (string)

Human-readable function name, enriched with the `Self` type for methods.

Examples: `"reduce"`, `"FieldElement51::square"`, `"Scalar::add"`

### `dependencies` (array of strings)

List of `code-name` URIs for functions called by this function.

```json
"dependencies": [
  "probe:curve25519-dalek/4.1.3/scalar/UnpackedScalar#add()",
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#unpack()"
]
```

### `dependencies-with-locations` (array of objects, optional)

Only present when `--with-locations` is passed. Each entry records where
in the function a call occurs.

```json
"dependencies-with-locations": [
  {
    "code-name": "probe:curve25519-dalek/4.1.3/scalar/UnpackedScalar#add()",
    "location": "inner",
    "line": 455
  },
  {
    "code-name": "probe:curve25519-dalek/4.1.3/field/reduce()",
    "location": "precondition",
    "line": 451
  }
]
```

The `location` field is one of:
- `"precondition"` -- call appears in a `requires` clause
- `"postcondition"` -- call appears in an `ensures` clause
- `"inner"` -- call appears in the function body

### `code-module` (string)

Module path extracted from the `code-name`, without the crate/version prefix or the
type/function suffix.

Examples: `"scalar"`, `"backend/serial/u64/field"`, `""` (top-level)

### `code-path` (string)

Relative path to the source file from the project root.

Examples: `"src/scalar.rs"`, `"src/backend/serial/u64/field.rs"`

For external function stubs (functions from non-workspace crates), this is an empty string.

### `code-text` (object)

Line range of the function definition in the source file.

- `lines-start` (number): First line of the function (1-based)
- `lines-end` (number): Last line of the function (1-based)

For external function stubs, both values are `0`.

```json
"code-text": {
  "lines-start": 679,
  "lines-end": 734
}
```

### `mode` (string)

Verus function mode. One of:
- `"exec"` -- executable code (compiled and verified)
- `"proof"` -- proof code (verified but erased at runtime)
- `"spec"` -- specification code (defines logical properties, erased at runtime)

For external function stubs, this defaults to `"exec"`.

## External Function Stubs

When `atomize` tracks calls to functions outside the workspace (e.g., standard library,
external crates), it creates lightweight stub entries. Stubs can be identified by:
- `code-path` is `""`
- `code-text` has `lines-start: 0` and `lines-end: 0`
- `dependencies` is empty

## Complete Example

```json
{
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#Add<&Scalar>#add()": {
    "display-name": "Scalar::add",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/scalar/UnpackedScalar#add()"
    ],
    "code-module": "scalar",
    "code-path": "src/scalar.rs",
    "code-text": {
      "lines-start": 450,
      "lines-end": 475
    },
    "mode": "exec"
  },
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#Mul<&Scalar>#mul()": {
    "display-name": "Scalar::mul",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/scalar/UnpackedScalar#mul()",
      "probe:curve25519-dalek/4.1.3/scalar/Scalar#unpack()"
    ],
    "code-module": "scalar",
    "code-path": "src/scalar.rs",
    "code-text": {
      "lines-start": 500,
      "lines-end": 525
    },
    "mode": "exec"
  }
}
```

## Parsing Examples

### TypeScript

```typescript
interface CodeText {
  "lines-start": number;
  "lines-end": number;
}

interface DependencyWithLocation {
  "code-name": string;
  location: "precondition" | "postcondition" | "inner";
  line: number;
}

interface Atom {
  "display-name": string;
  dependencies: string[];
  "dependencies-with-locations"?: DependencyWithLocation[];
  "code-module": string;
  "code-path": string;
  "code-text": CodeText;
  mode: "exec" | "proof" | "spec";
}

type AtomsDict = Record<string, Atom>;

const atoms: AtomsDict = JSON.parse(fileContent);
```

### Python

```python
import json
from typing import TypedDict

class CodeText(TypedDict):
    lines_start: int  # JSON key: "lines-start"
    lines_end: int    # JSON key: "lines-end"

class Atom(TypedDict):
    display_name: str         # JSON key: "display-name"
    dependencies: list[str]
    code_module: str          # JSON key: "code-module"
    code_path: str            # JSON key: "code-path"
    code_text: CodeText       # JSON key: "code-text"
    mode: str                 # "exec" | "proof" | "spec"

with open("atoms.json") as f:
    atoms: dict[str, Atom] = json.load(f)

for code_name, atom in atoms.items():
    print(f"{atom['display-name']} at {atom['code-path']}:{atom['code-text']['lines-start']}")
```

### Rust

```rust
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Serialize, Deserialize)]
struct Atom {
    #[serde(rename = "display-name")]
    display_name: String,
    dependencies: HashSet<String>,
    #[serde(rename = "code-module")]
    code_module: String,
    #[serde(rename = "code-path")]
    code_path: String,
    #[serde(rename = "code-text")]
    code_text: CodeText,
    mode: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CodeText {
    #[serde(rename = "lines-start")]
    lines_start: usize,
    #[serde(rename = "lines-end")]
    lines_end: usize,
}

let atoms: HashMap<String, Atom> = serde_json::from_str(&file_content)?;
```

## Version History

### v1.2.0 (2026-02-28)

- Added external function stub entries for non-workspace dependencies
- Existing atoms may now list external function dependencies

### v1.1.0 (2026-02-24)

- Added `mode` field for Verus function modes (`exec`, `proof`, `spec`)
- Added optional `dependencies-with-locations` array (with `--with-locations` flag)
- Added `code-module` field extracted from the code-name URI
- Enriched `display-name` with Self type for impl methods

### v1.0.0 (2026-01-27)

- Output changed from JSON array to dictionary keyed by `code-name`
- Renamed `scip-name` / `code-function` to `code-name` (used as dictionary key)
- Removed `visible` field
- Field naming uses kebab-case throughout

### v0.1.0 (2026-01-15)

- Initial format: JSON array with `visible`, `code-function`, line ranges
