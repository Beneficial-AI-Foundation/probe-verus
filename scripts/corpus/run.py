#!/usr/bin/env python3
"""Run probe-verus on the support-target corpus and check the results.

For each project in corpus/support-target.toml:

1. Clone the repo (once) and check out the pinned commit.
2. Build an inventory of function declarations with `list-functions`. It
   uses only verus_syn, so it does not depend on the analyzer.
3. Run `extract`, always regenerating the SCIP index.
4. Match every declaration to a local atom: same file, same name, and
   overlapping line spans.

The script writes report.json and report.md to --out after each project. It
exits nonzero if a step fails, if the inventory is empty or incomplete, or,
with --gate, if a declaration has no atom and is not listed in the
known-limitations file.

The clones under --work-dir belong to the script: it resets them to the
pinned commit and deletes untracked files on every run. Do not run two
instances with the same --work-dir at the same time.

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
import signal
import subprocess
import sys
import time
import tomllib
import traceback
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = REPO_ROOT / "corpus" / "support-target.toml"
DEFAULT_KNOWN = REPO_ROOT / "corpus" / "known-limitations.toml"
STEP_TIMEOUT = 3600
GIT_TIMEOUT = 600
DOWNLOAD_TIMEOUT = 300
ANALYZER_ASSET = "verus-analyzer-x86_64-unknown-linux-gnu.gz"

# `list-functions -f detailed --show-kind` line:
#   name [kind] @ file:start:end in context
DETAILED_LINE = re.compile(
    r"^(?P<name>\S+) \[(?P<kind>[^\]]+)\] @ (?P<file>.+):(?P<start>\d+):(?P<end>\d+)"
    r"(?: in (?P<context>\S+))?$"
)
SUMMARY_LINE = re.compile(r"^Summary: (?P<count>\d+) functions in \d+ files$", re.M)
# list-functions prints these on stderr for files it cannot parse, and goes on.
PARSE_ERROR = re.compile(r"^(Warning|Error parsing file): ", re.M)


def run(cmd, cwd, env, log_path):
    """Run a command, write its output to log_path, return (rc, seconds)."""
    start = time.monotonic()
    with open(log_path, "w") as log:
        # A new session, so that a timeout kills the whole process group
        # (cargo, rustc, the analyzer), not only the direct child.
        proc = subprocess.Popen(
            cmd, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            rc = proc.wait(timeout=STEP_TIMEOUT)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
            log.write(f"\nTIMEOUT after {STEP_TIMEOUT} s\n")
            rc = 124
    return rc, round(time.monotonic() - start, 1)


def git(args, cwd, **kw):
    return subprocess.run(["git", *args], cwd=cwd, timeout=GIT_TIMEOUT, **kw)


def checkout(project, work_dir):
    """Return a clean clone of the project at its pinned commit."""
    repo_dir = work_dir / "repos" / project["name"]
    if not repo_dir.exists():
        repo_dir.parent.mkdir(parents=True, exist_ok=True)
        git(["clone", "--quiet", project["repo"], str(repo_dir)], None, check=True)
    # Only reset and clean a clone of the expected repo.
    origin = git(["remote", "get-url", "origin"], repo_dir, capture_output=True, text=True)
    if repo_dir.is_symlink() or origin.returncode != 0 or origin.stdout.strip() != project["repo"]:
        raise RuntimeError(f"{repo_dir} is not a clone of {project['repo']}; remove it")
    have = git(["cat-file", "-e", project["commit"] + "^{commit}"], repo_dir).returncode == 0
    if not have:
        git(["fetch", "--quiet", "origin"], repo_dir, check=True)
    git(["checkout", "--quiet", "--force", "--detach", project["commit"]], repo_dir, check=True)
    git(["reset", "--quiet", "--hard", project["commit"]], repo_dir, check=True)
    git(["clean", "-ffdxq"], repo_dir, check=True)
    git(["submodule", "update", "--quiet", "--init", "--recursive", "--force"], repo_dir, check=True)
    status = git(["status", "--porcelain", "--ignore-submodules=none"], repo_dir,
                 capture_output=True, text=True, check=True).stdout
    if status:
        raise RuntimeError(f"{repo_dir} is not clean after reset:\n{status}")
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
        tmp = tools / f"verus-analyzer.{os.getpid()}.tmp"
        with urllib.request.urlopen(url, timeout=DOWNLOAD_TIMEOUT) as resp, open(tmp, "wb") as out:
            out.write(gzip.decompress(resp.read()))
        tmp.chmod(0o755)
        tmp.rename(binary)
    # Point the link at the requested scip on every run.
    link = tools / "scip"
    if link.is_symlink() or link.exists():
        link.unlink()
    link.symlink_to(scip)
    return home


def tool_version(cmd, env=None):
    """Return the first line of `cmd --version`, or an error string."""
    try:
        out = subprocess.run([*cmd, "--version"], env=env, capture_output=True, text=True,
                             timeout=60)
    except (OSError, subprocess.TimeoutExpired) as e:
        return f"error: {e}"
    text = (out.stdout or out.stderr).strip()
    return text.splitlines()[0] if out.returncode == 0 and text else f"error: rc {out.returncode}"


def find_scip(explicit):
    candidates = [explicit] if explicit else [
        str(Path.home() / ".probe-verus" / "tools" / "scip"), shutil.which("scip"),
    ]
    for c in candidates:
        if c and Path(c).exists():
            return Path(c).resolve()
    sys.exit("scip not found; pass --scip")


def parse_inventory(text):
    """Return the declarations in `list-functions` output.

    Raise ValueError if the output has parse errors, has no summary line, or
    if the number of parsed lines differs from the summary count.
    """
    errors = PARSE_ERROR.findall(text)
    if errors:
        raise ValueError(f"list-functions could not parse {len(errors)} file(s); see list-functions.log")
    summary = SUMMARY_LINE.search(text)
    if not summary:
        raise ValueError("list-functions printed no summary line")
    decls = []
    for line in text.splitlines():
        m = DETAILED_LINE.match(line.strip())
        if m:
            d = m.groupdict()
            d["start"], d["end"] = int(d["start"]), int(d["end"])
            decls.append(d)
    expected = int(summary["count"])
    if len(decls) != expected:
        raise ValueError(f"parsed {len(decls)} declarations, list-functions reports {expected}")
    if not decls:
        raise ValueError("empty inventory")
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


def no_duplicate_keys(pairs):
    obj = {}
    for k, v in pairs:
        if k in obj:
            raise ValueError(f"duplicate key in atoms file: {k}")
        obj[k] = v
    return obj


def load_fresh(probes_dir, pattern, since, schema):
    """Load the one file matching `pattern` that was written at or after `since`.

    Check its envelope schema and return (file name, data).
    """
    files = [f for f in probes_dir.glob(pattern) if f.stat().st_mtime >= since]
    if len(files) != 1:
        raise ValueError(f"expected one new {pattern} in {probes_dir}, found {len(files)}")
    env = json.loads(files[0].read_text(), object_pairs_hook=no_duplicate_keys)
    if env.get("schema") != schema or not isinstance(env.get("data"), dict):
        raise ValueError(f"{files[0].name}: expected schema {schema} with a data object")
    return files[0].name, env["data"]


def check_extract(probes_dir, since, verify):
    """Check the extract summary and the unified output. Return the atoms data.

    extract can exit 0 when it skips verification (no cargo-verus) or cannot
    write the unified output, so the exit code alone is not enough.
    """
    summary_file, summary = load_fresh(probes_dir, "*_extract_summary.json", since,
                                       "probe-verus/extract-summary")
    status = summary.get("status")
    if status not in ("success", "verification_failed"):
        raise ValueError(f"extract status {status}")
    for step in ("atomize", "specify") + (("verify",) if verify else ()):
        if not (summary.get(step) or {}).get("success"):
            raise ValueError(f"extract step {step} did not run or failed; see extract.log")
    unified = summary_file.removesuffix("_extract_summary.json") + ".json"
    load_fresh(probes_dir, unified, since, "probe-verus/extract")
    atoms_file, atoms = load_fresh(probes_dir, "*_atoms.json", since, "probe-verus/atoms")
    return atoms_file, atoms, status


def load_known(path, project_names):
    if not path.exists():
        return set()
    data = tomllib.load(open(path, "rb"))
    known = set()
    for k in data.get("limitation", []):
        missing = [f for f in ("project", "file", "name", "start", "reason") if not k.get(f)]
        if missing:
            sys.exit(f"{path}: limitation {k} lacks {', '.join(missing)}")
        if k["project"] not in project_names:
            sys.exit(f"{path}: limitation for unknown project {k['project']}")
        known.add((k["project"], k["file"], k["name"], k["start"]))
    return known


def run_project(project, args, env_base, out_dir, result):
    """Run one project and fill `result`. Raise on any failure."""
    name = project["name"]
    pdir = out_dir / name
    if pdir.exists():
        shutil.rmtree(pdir)
    pdir.mkdir(parents=True)
    repo_dir = checkout(project, args.work_dir)
    crate = (repo_dir / project["crate"]).resolve()
    env = dict(env_base)
    # Without a toolchain in the manifest, rustup must use the project's
    # rust-toolchain.toml, so drop any toolchain override from the caller.
    env.pop("RUSTUP_TOOLCHAIN", None)
    if project.get("toolchain"):
        env["RUSTUP_TOOLCHAIN"] = project["toolchain"]
    result["commit"] = git(["rev-parse", "HEAD"], repo_dir, capture_output=True, text=True,
                           check=True).stdout.strip()
    rustc = subprocess.run(["rustc", "-vV"], cwd=crate, env=env, capture_output=True,
                           text=True, timeout=600)
    if rustc.returncode != 0:
        raise RuntimeError(f"rustc -vV failed in {crate}:\n{rustc.stderr}")
    result["rustc"] = rustc.stdout.strip()

    rc, secs = run([args.probe_verus, "list-functions", "-f", "detailed", "--show-kind", str(crate)],
                   crate, env, pdir / "list-functions.log")
    result["steps"]["list-functions"] = {"rc": rc, "seconds": secs}
    if rc != 0:
        raise RuntimeError("list-functions failed; see list-functions.log")
    decls = parse_inventory((pdir / "list-functions.log").read_text())

    cmd = [args.probe_verus, "extract", str(crate), "--regenerate-scip"]
    verify = args.verify and project["name"] not in args.no_verify
    if not verify:
        cmd.append("--skip-verify")
    started = time.time()
    rc, secs = run(cmd, crate, env, pdir / "extract.log")
    result["steps"]["extract"] = {"rc": rc, "seconds": secs}

    probes = crate / ".verilib" / "probes"
    if probes.exists():
        shutil.copytree(probes, pdir / "probes")
    if rc != 0:
        raise RuntimeError("extract failed; see extract.log")
    result["atoms_file"], atoms, result["extract_status"] = check_extract(probes, started, verify)

    local = {k: a for k, a in atoms.items() if a.get("code-path")}
    unmatched, extra = check(decls, atoms)
    result.update({
        "declarations": len(decls),
        "local_atoms": len(local),
        "edges": sum(len(a.get("dependencies", [])) for a in local.values()),
        "unmatched": unmatched,
        "atoms_without_declaration": len(extra),
    })
    return unmatched


def write_report(results, tools, out_dir, known):
    report = {"tools": tools, "projects": results}
    (out_dir / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    lines = [", ".join(f"{k}: {v}" for k, v in tools.items()), "",
        "| Project | Steps | Seconds | Declarations | Local atoms | Edges | Unmatched (new) |",
        "|---------|-------|---------|--------------|-------------|-------|-----------------|",
    ]
    for r in results:
        steps = ", ".join(f"{k} {v['rc']}" for k, v in r["steps"].items())
        secs = sum(v["seconds"] for v in r["steps"].values())
        um = r.get("unmatched", [])
        new = [u for u in um if (r["name"], u["file"], u["name"], u["start"]) not in known]
        counts = "error" if r.get("error") else f"{len(um)} ({len(new)})"
        lines.append(
            f"| {r['name']} | {steps} | {secs:.0f} | {r.get('declarations', '-')} | "
            f"{r.get('local_atoms', '-')} | {r.get('edges', '-')} | {counts} |"
        )
    lines.append("")
    for r in results:
        if r.get("error"):
            lines.append(f"- {r['name']}: ERROR {r['error'].splitlines()[0]}")
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
    if not os.access(args.probe_verus, os.X_OK):
        sys.exit(f"--probe-verus: {args.probe_verus} is not an executable file")
    args.no_verify = {"verismo"}
    args.work_dir = args.work_dir.resolve()
    args.out = args.out.resolve()
    args.work_dir.mkdir(parents=True, exist_ok=True)
    args.out.mkdir(parents=True, exist_ok=True)

    projects = tomllib.load(open(args.manifest, "rb")).get("project", [])
    names = [p["name"] for p in projects]
    if not projects or len(set(names)) != len(names):
        sys.exit(f"{args.manifest}: no projects, or duplicate project names")
    if args.project:
        unknown = sorted(set(args.project) - set(names))
        if unknown:
            sys.exit(f"unknown project(s): {', '.join(unknown)}; known: {', '.join(names)}")
        projects = [p for p in projects if p["name"] in args.project]
    known = load_known(args.known, set(names))

    env = dict(os.environ)
    if args.analyzer_version:
        env["RUSTUP_HOME"] = env.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        env["CARGO_HOME"] = env.get("CARGO_HOME", str(Path.home() / ".cargo"))
        env["HOME"] = str(analyzer_home(args.analyzer_version, args.work_dir, find_scip(args.scip)))
    tools_dir = Path(env["HOME"]) / ".probe-verus" / "tools"
    analyzer = tools_dir / "verus-analyzer"
    scip = tools_dir / "scip"
    tools = {
        "probe-verus": tool_version([args.probe_verus]),
        "verus-analyzer": tool_version([str(analyzer)]) if analyzer.exists() else "from PATH",
        "scip": tool_version([str(scip)]) if scip.exists() else "from PATH",
    }

    results, failed = [], False
    for project in projects:
        print(f"== {project['name']}", flush=True)
        result = {"name": project["name"], "steps": {}}
        results.append(result)
        unmatched = []
        try:
            unmatched = run_project(project, args, env, args.out, result)
        except Exception as e:
            result["error"] = f"{type(e).__name__}: {e}"
            (args.out / project["name"]).mkdir(parents=True, exist_ok=True)
            (args.out / project["name"] / "error.log").write_text(traceback.format_exc())
            failed = True
        new = [u for u in unmatched if (project["name"], u["file"], u["name"], u["start"]) not in known]
        if args.gate and new:
            failed = True
        print(f"   steps {result['steps']}  unmatched {len(unmatched)} (new {len(new)})"
              + (f"  ERROR {result['error'].splitlines()[0]}" if "error" in result else ""),
              flush=True)
        write_report(results, tools, args.out, known)
    print(f"report: {args.out / 'report.md'}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
