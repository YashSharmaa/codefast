"""Offline check of `codefast rename` on a random sample of definitions of a Go repository:
types, struct fields and methods. Each is renamed in a fresh copy and the compiler is asked.

A rename passes when every compiler error left is on a line the command itself listed as not
changed (a line where the name stands for two things, a use on a receiver of unknown type).
An error anywhere else is a wrong edit or a reference the command neither changed nor listed.
No model is involved.

usage: python3 bench/tier2/rename_sample.py [--repo syft] [--per-kind 8] [--seed 1] [--out FILE]
Needs bench/repos/REPO and Go. CODEFAST_BIN names the binary (default target/release/codefast).
"""
import argparse, json, os, random, re, subprocess, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def facts(binary, repo, path):
    out = subprocess.run([binary, "debug-facts", str(repo / path)], capture_output=True, text=True).stdout
    for line in out.splitlines():
        m = re.match(r"\s*\d+\s+(\w+)\s+(\S+)\s+L(\d+)-(\d+)\s+parent=(\S+)", line)
        if m:
            yield m.group(1), m.group(2), int(m.group(3)), m.group(5)


def candidates(binary, repo):
    files = subprocess.run(["git", "ls-files", "*.go"], cwd=repo, capture_output=True, text=True).stdout.split()
    out = {"type": [], "field": [], "method": []}
    for f in files:
        if f.endswith("_test.go") or "/test/" in f or f.startswith("test/"):
            continue
        for kind, name, line, parent in facts(binary, repo, f):
            if not re.fullmatch(r"[A-Z][A-Za-z0-9]{3,}", name):
                continue
            if kind in ("struct", "type", "iface", "interface") and parent == "-":
                out["type"].append((f, line, name))
            elif kind == "field" and parent != "-":
                out["field"].append((f, line, name))
            elif kind == "method" and parent != "-":
                out["method"].append((f, line, name))
    return out


def complaints(repo):
    """The places `go build` and `go vet` complain about."""
    out = set()
    for cmd in ("go build ./...", "go vet -composites=false -unreachable=false ./..."):
        c = subprocess.run(cmd, shell=True, cwd=repo, capture_output=True, text=True)
        for l in (c.stdout + c.stderr).splitlines():
            m = re.match(r"(?:vet: )?(?:\./)?(\S+\.go):(\d+):\d+: ", l)
            if m:
                out.add((m.group(1), int(m.group(2))))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default="syft")
    ap.add_argument("--per-kind", type=int, default=8)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", default=str(ROOT / "bench/results/rename-sample.jsonl"))
    a = ap.parse_args()
    binary = os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast"))
    src = ROOT / "bench/repos" / a.repo
    cands = candidates(binary, src)
    rng = random.Random(a.seed)
    work = Path(tempfile.mkdtemp(prefix="codefast-rename-sample-", dir=ROOT / "target"))
    # What the compiler and vet say about the repository as it is: not the rename's doing.
    baseline = complaints(src)
    print(f"{a.repo}: {len(baseline)} complaints before any rename", flush=True)
    rows = []
    for kind, pool in cands.items():
        rng.shuffle(pool)
        done = 0
        for f, line, name in pool:
            if done >= a.per_kind:
                break
            repo = work / f"{kind}-{name}-{line}"
            subprocess.run(["rsync", "-a", "--exclude", ".git", "--exclude", ".codefast", f"{src}/", f"{repo}/"], check=True)
            env = dict(os.environ, CODEFAST_INDEX_DIR=str(work / f"index-{kind}-{name}-{line}"), CODEFAST_RENAME_ANY="1")
            new = name + "Renamed"
            p = subprocess.run([binary, "--root", str(repo), "rename", f"{f}:{line}", new], capture_output=True, text=True, env=env)
            said = p.stdout.splitlines()
            head = said[0] if said else ""
            if not head.startswith("renamed "):
                # Refused (ambiguous, taken, reached through an interface): nothing to judge.
                rows.append(dict(repo=a.repo, kind=kind, target=f"{f}:{line} {name}", refused=head))
                subprocess.run(["rm", "-rf", str(repo)])
                print(f"{kind:6} {name:28} refused: {head[:90]}", flush=True)
                continue
            done += 1
            listed = set()
            for l in said:
                m = re.match(r"\s+(\S+\.go):(\d+): ", l)
                if m:
                    listed.add((m.group(1), int(m.group(2))))
                    continue
                # The rest of a long list: `  path: 12,15,90`.
                m = re.match(r"\s+(\S+\.go): ([\d,]+)$", l)
                if m:
                    listed.update((m.group(1), int(n)) for n in m.group(2).split(","))
            errs = complaints(repo) - baseline
            outside = sorted(e for e in errs if e not in listed)
            refs = re.search(r"and ([\d,]+) references? in ([\d,]+) files?", head)
            row = dict(repo=a.repo, kind=kind, target=f"{f}:{line} {name}", references=int(refs.group(1).replace(",", "")) if refs else 0, files=int(refs.group(2).replace(",", "")) if refs else 0,
                       listed_as_not_changed=len(listed), compiler_errors=len(errs), errors_outside_the_list=[f"{x}:{y}" for x, y in outside[:10]], passed=not outside)
            rows.append(row)
            print(f"{kind:6} {name:28} refs={row['references']:4} listed={len(listed):3} errors={len(errs):3} outside={len(outside):3} {'ok' if not outside else 'FAIL ' + str(outside[:3])}", flush=True)
            subprocess.run(["rm", "-rf", str(repo)])
    with open(a.out, "w") as fo:
        for r in rows:
            fo.write(json.dumps(r) + "\n")
    judged = [r for r in rows if "passed" in r]
    print(f"{sum(r['passed'] for r in judged)} of {len(judged)} renames leave no compiler error outside the lines the command listed ({sum(r['references'] for r in judged)} references changed).")


main()
