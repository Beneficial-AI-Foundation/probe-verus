#!/usr/bin/env python3
"""Run probe-verus on the support-target corpus and check the results.

For each project in corpus/support-target.toml:

1. Clone the repo (once) and check out the pinned commit.
2. Build an inventory of function declarations with `list-functions`. It
   uses only verus_syn, so it does not depend on the analyzer.
3. Run `extract`, always regenerating the SCIP index.
4. Match every declaration to a local atom: same file, same name, and
   overlapping line spans.

The script writes report.json and report.md to --out. It exits nonzero if a
step fails, or, with --gate, if a declaration has no atom and is not listed
in the known-limitations file.

To run a specific verus-analyzer release without touching
~/.probe-verus/tools, pass --analyzer-version. The script then installs that
release into its own HOME under the work directory and runs probe-verus
with that HOME. RUSTUP_HOME and CARGO_HOME keep pointing to the real ones.
"""

import argparse
import gzip
import json
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = REPO_ROOT / "corpus" / "support-target.toml"
DEFAULT_KNOWN = REPO_ROOT / "corpus" / "known-limitations.toml"
STEP_TIMEOUT = 3600
ANALYZER_ASSET = "verus-analyzer-x86_64-unknown-linux-gnu.gz"

# `list-functions -f detailed --show-kind` line:
#   name [kind] @ file:start:end in context
DETAILED_LINE = re.compile(
    r"^(?P<name>\S+) \[(?P<kind>[^\]]+)\] @ (?P<file>.+):(?P<start>\d+):(?P<end>\d+)"
    r"(?: in (?P<context>\S+))?$"
)


def run(cmd, cwd, env, log_path):
    """Run a command, write its output to log_path, return (rc, seconds)."""
    start = time.monotonic()
    with open(log_path, "w") as log:
        try:
            rc = subprocess.run(
                cmd, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT,
                timeout=STEP_TIMEOUT,
            ).returncode
        except subprocess.TimeoutExpired:
            log.write(f"\nTIMEOUT after {STEP_TIMEOUT} s\n")
            rc = 124
    return rc, round(time.monotonic() - start, 1)


def checkout(project, work_dir):
    repo_dir = work_dir / "repos" / project["name"]
    if not repo_dir.exists():
        subprocess.run(["git", "clone", "--quiet", project["repo"], str(repo_dir)], check=True)
    have = subprocess.run(
        ["git", "cat-file", "-e", project["commit"] + "^{commit}"], cwd=repo_dir,
    ).returncode == 0
    if not have:
        subprocess.run(["git", "fetch", "--quiet", "origin"], cwd=repo_dir, check=True)
    subprocess.run(["git", "checkout", "--quiet", "--detach", project["commit"]], cwd=repo_dir, check=True)
    subprocess.run(["git", "clean", "-fdxq"], cwd=repo_dir, check=True)
    return repo_dir


def analyzer_home(version, work_dir, scip):
    """Return a HOME whose ~/.probe-verus/tools holds the given analyzer."""
    if not re.fullmatch(r"[0-9A-Za-z._-]+", version):
        sys.exit(f"invalid analyzer version: {version}")
    home = work_dir / f"home-{version}"
    tools = home / ".probe-verus" / "tools"
    tools.mkdir(parents=True, exist_ok=True)
    binary = tools / "verus-analyzer"
    if not binary.exists():
        url = f"https://github.com/verus-lang/verus-analyzer/releases/download/{version}/{ANALYZER_ASSET}"
        tmp = binary.with_suffix(".tmp")
        with urllib.request.urlopen(url) as resp, open(tmp, "wb") as out:
            out.write(gzip.decompress(resp.read()))
        tmp.chmod(0o755)
        tmp.rename(binary)
    link = tools / "scip"
    if not link.exists():
        link.symlink_to(scip)
    return home


def find_scip(explicit):
    candidates = [explicit] if explicit else [
        str(Path.home() / ".probe-verus" / "tools" / "scip"), shutil.which("scip"),
    ]
    for c in candidates:
        if c and Path(c).exists():
            return Path(c).resolve()
    sys.exit("scip not found; pass --scip")


def parse_inventory(text):
    decls = []
    for line in text.splitlines():
        m = DETAILED_LINE.match(line.strip())
        if m:
            d = m.groupdict()
            d["start"], d["end"] = int(d["start"]), int(d["end"])
            decls.append(d)
    return decls


def atom_name(code_name, atom):
    display = atom.get("display-name") or ""
    if display:
        return display.split("::")[-1]
    m = re.search(r"([A-Za-z_][A-Za-z0-9_]*)\(\)(@.*)?$", code_name)
    return m.group(1) if m else code_name


def check(decls, atoms):
    """Match declarations to local atoms. Return (unmatched decls, extra atoms)."""
    by_key = {}
    for code_name, atom in atoms.items():
        path = atom.get("code-path")
        if not path:
            continue
        lines = atom.get("code-text") or {}
        entry = (code_name, lines.get("lines-start", 0), lines.get("lines-end", 0))
        by_key.setdefault((path, atom_name(code_name, atom)), []).append(entry)
    used = set()
    unmatched = []
    for d in decls:
        cands = by_key.get((d["file"], d["name"]), [])
        hit = next(
            (c for c in cands if c[0] not in used and c[1] <= d["end"] and d["start"] <= c[2]),
            None,
        )
        if hit:
            used.add(hit[0])
        else:
            unmatched.append(d)
    extra = sorted(
        k for k, a in atoms.items() if a.get("code-path") and k not in used
    )
    return unmatched, extra


def load_atoms(probes_dir):
    files = sorted(probes_dir.glob("*_atoms.json"))
    if not files:
        return None
    return json.load(open(files[0]))["data"]


def load_known(path):
    if not path.exists():
        return set()
    data = tomllib.load(open(path, "rb"))
    return {
        (k["project"], k["file"], k["name"], k["start"]) for k in data.get("limitation", [])
    }


def run_project(project, args, env_base, out_dir):
    name = project["name"]
    pdir = out_dir / name
    pdir.mkdir(parents=True, exist_ok=True)
    repo_dir = checkout(project, args.work_dir)
    crate = (repo_dir / project["crate"]).resolve()
    env = dict(env_base)
    if project.get("toolchain"):
        env["RUSTUP_TOOLCHAIN"] = project["toolchain"]
    result = {
        "name": name,
        "commit": subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo_dir,
                                 capture_output=True, text=True).stdout.strip(),
        "rustc": subprocess.run(["rustc", "-vV"], cwd=crate, env=env,
                                capture_output=True, text=True).stdout.strip(),
        "steps": {},
    }

    rc, secs = run([args.probe_verus, "list-functions", "-f", "detailed", "--show-kind", str(crate)],
                   crate, env, pdir / "list-functions.log")
    result["steps"]["list-functions"] = {"rc": rc, "seconds": secs}
    decls = parse_inventory((pdir / "list-functions.log").read_text()) if rc == 0 else []

    cmd = [args.probe_verus, "extract", str(crate), "--regenerate-scip"]
    if not args.verify or project["name"] in args.no_verify:
        cmd.append("--skip-verify")
    rc, secs = run(cmd, crate, env, pdir / "extract.log")
    result["steps"]["extract"] = {"rc": rc, "seconds": secs}

    probes = crate / ".verilib" / "probes"
    if probes.exists():
        shutil.copytree(probes, pdir / "probes", dirs_exist_ok=True)
    atoms = load_atoms(probes) if probes.exists() else None
    if atoms is None:
        result["steps"]["extract"]["rc"] = result["steps"]["extract"]["rc"] or 1
        result["error"] = "no atoms file"
        return result, []

    local = {k: a for k, a in atoms.items() if a.get("code-path")}
    unmatched, extra = check(decls, atoms)
    result.update({
        "declarations": len(decls),
        "local_atoms": len(local),
        "edges": sum(len(a.get("dependencies", [])) for a in local.values()),
        "unmatched": unmatched,
        "atoms_without_declaration": len(extra),
    })
    return result, unmatched


def write_report(results, out_dir, known):
    (out_dir / "report.json").write_text(json.dumps(results, indent=2) + "\n")
    lines = [
        "| Project | Steps | Seconds | Declarations | Local atoms | Edges | Unmatched (new) |",
        "|---------|-------|---------|--------------|-------------|-------|-----------------|",
    ]
    for r in results:
        steps = ", ".join(f"{k} {v['rc']}" for k, v in r["steps"].items())
        secs = sum(v["seconds"] for v in r["steps"].values())
        um = r.get("unmatched", [])
        new = [u for u in um if (r["name"], u["file"], u["name"], u["start"]) not in known]
        lines.append(
            f"| {r['name']} | {steps} | {secs:.0f} | {r.get('declarations', '-')} | "
            f"{r.get('local_atoms', '-')} | {r.get('edges', '-')} | {len(um)} ({len(new)}) |"
        )
    lines.append("")
    for r in results:
        for u in r.get("unmatched", []):
            lines.append(f"- {r['name']}: `{u['name']}` [{u['kind']}] {u['file']}:{u['start']}-{u['end']} ({u.get('context')})")
    (out_dir / "report.md").write_text("\n".join(lines) + "\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--probe-verus", required=True, help="probe-verus binary to test")
    ap.add_argument("--out", required=True, type=Path, help="output directory")
    ap.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    ap.add_argument("--known", type=Path, default=DEFAULT_KNOWN, help="known-limitations file")
    ap.add_argument("--work-dir", type=Path, default=Path.home() / ".cache" / "probe-verus-corpus")
    ap.add_argument("--analyzer-version", help="run with this verus-analyzer release in an isolated HOME")
    ap.add_argument("--scip", help="scip binary for the isolated HOME (default: managed or PATH)")
    ap.add_argument("--project", action="append", help="only run this project (repeatable)")
    ap.add_argument("--verify", action="store_true", help="also run Verus verification")
    ap.add_argument("--gate", action="store_true", help="fail on unmatched declarations not in --known")
    args = ap.parse_args()
    args.probe_verus = str(Path(args.probe_verus).resolve())
    args.no_verify = {"verismo"}
    args.work_dir.mkdir(parents=True, exist_ok=True)
    args.out.mkdir(parents=True, exist_ok=True)

    env = dict(os.environ)
    if args.analyzer_version:
        env["RUSTUP_HOME"] = env.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        env["CARGO_HOME"] = env.get("CARGO_HOME", str(Path.home() / ".cargo"))
        env["HOME"] = str(analyzer_home(args.analyzer_version, args.work_dir, find_scip(args.scip)))

    projects = tomllib.load(open(args.manifest, "rb"))["project"]
    if args.project:
        projects = [p for p in projects if p["name"] in args.project]
    known = load_known(args.known)

    results, failed = [], False
    for project in projects:
        print(f"== {project['name']}", flush=True)
        result, unmatched = run_project(project, args, env, args.out)
        results.append(result)
        if any(s["rc"] != 0 for s in result["steps"].values()):
            failed = True
        new = [u for u in unmatched if (project["name"], u["file"], u["name"], u["start"]) not in known]
        if args.gate and new:
            failed = True
        print(f"   steps {result['steps']}  unmatched {len(unmatched)} (new {len(new)})", flush=True)
    write_report(results, args.out, known)
    print(f"report: {args.out / 'report.md'}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
