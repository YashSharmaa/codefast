# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, family R: carry a rename through every call site, checked by the compiler.

A method with a common name (several other types have a method of the same name) is renamed at
its definition only. The agent has to update every call site, tests included, until
`go build ./...` and `go vet ./...` (which also compiles the tests) pass again. Renaming the wrong `Add` does not compile, so a clean
build means the right call sites were changed and no others.

Candidates come from the compiler-built call graph in `bench/oracles/syft` (methods with 6 to 30
static call sites in 3 or more files, whose name at least two other types also use), not from
codefast. A task is kept when a fixer driven by the compiler's own error messages reaches a clean
build, which also tells how many places have to change.

usage: uv run bench/tier2/make_r_tasks.py [--n 10]
"""
import argparse, collections, gzip, json, os, re, shutil, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab/r"
REPO = ROOT / "bench/repos/syft"
ORACLE = ROOT / "bench/oracles/syft"
NEW = {"Add": "AddEntry", "add": "addEntry", "ToSlice": "ToSortedSlice", "Get": "GetValue", "Empty": "IsEmpty", "Remove": "RemoveEntry", "Contains": "ContainsEntry"}
SKIP = {"String", "Name", "ID", "Error", "Len", "Less", "Swap", "MarshalJSON", "UnmarshalJSON"}  # used through interfaces of the standard library


def sh(cmd, cwd, timeout=1800):
    p = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout + p.stderr


def candidates():
    E = [json.loads(l) for l in gzip.open(ORACLE / "edges.jsonl.gz", "rt")]
    F = [json.loads(l) for l in gzip.open(ORACLE / "functions.jsonl.gz", "rt")]
    byloc = {(f["file"], f["line"]): f for f in F}
    byname = collections.defaultdict(set)
    for f in F:
        if f.get("receiver_or_class"):
            byname[f["name"]].add((f["receiver_or_class"], f["file"], f["line"]))
    calls = collections.defaultdict(list)
    for e in E:
        c = e["callee"]
        fr = byloc.get((c["file"], c["line"]))
        if fr and fr.get("receiver_or_class") and e.get("kind") == "static":
            calls[(fr["receiver_or_class"], c["name"], c["file"], c["line"])].append((e["caller"]["file"], e["site_line"]))
    out = []
    for k, sites in calls.items():
        rivals = len(byname[k[1]]) - 1
        if 6 <= len(sites) <= 30 and len(set(f for f, _ in sites)) >= 3 and rivals >= 2 and k[1] not in SKIP and k[1] in NEW:
            out.append(dict(receiver=k[0], name=k[1], file=k[2], line=k[3], oracle_sites=len(sites), rivals=rivals))
    out.sort(key=lambda c: (c["file"], c["line"]))
    return out


def rename_definition(repo, c):
    p = repo / c["file"]
    lines = p.read_text().split("\n")
    i = c["line"] - 1
    recv = c["receiver"].lstrip("*")
    m = re.match(r"^(func \([^)]*\b" + re.escape(recv) + r"(?:\[[^\]]*\])?\) )" + re.escape(c["name"]) + r"(\(|\[)", lines[i])
    if not m:
        return False
    lines[i] = lines[i][: m.end(1)] + NEW[c["name"]] + lines[i][m.end(1) + len(c["name"]):]
    p.write_text("\n".join(lines))
    return True


ERR = re.compile(r"^(?:vet: )?(\S+\.go):(\d+):(\d+): (.*)$", re.M)
# The unmodified project has findings of these two analyzers in its tests: they are switched
# off, so that the check only fails for code that does not compile.
VET = "go vet -composites=false -unreachable=false ./..."


def fix_by_compiler(repo, c):
    """Rename call sites the compiler points at until it is satisfied. Returns (ok, sites, files, log)."""
    old, new = c["name"], NEW[c["name"]]
    changed = set()
    for _ in range(14):
        rc, out = sh(f"go build ./... 2>&1 && {VET} 2>&1", repo)
        if rc != 0 and not ERR.search(out):
            return False, len(changed), 0, out[-300:]
        errs = []
        for line in out.split("\n"):
            m = ERR.match(line.strip())
            if m and "undefined" in m.group(4) and re.search(r"\b" + re.escape(old) + r"\b", m.group(4)):
                errs.append((m.group(1), int(m.group(2)), int(m.group(3))))
        if not errs:
            other = [l for l in out.split("\n") if ERR.match(l.strip())]
            return (not other and rc == 0), len(changed), len(set(f for f, _ in changed)), "\n".join(other[:6])
        progressed = False
        for f, ln, col in sorted(set(errs)):
            p = repo / f
            if not p.exists():
                continue
            lines = p.read_text().split("\n")
            text = lines[ln - 1]
            # The column points at the selector expression: the method name follows a dot at or after it.
            m = re.compile(r"\." + re.escape(old) + r"\b").search(text, max(col - 2, 0))
            if not m:
                m = re.compile(r"\." + re.escape(old) + r"\b").search(text)
            if not m:
                continue
            lines[ln - 1] = text[: m.start()] + "." + new + text[m.end():]
            p.write_text("\n".join(lines))
            changed.add((f, ln))
            progressed = True
        if not progressed:
            return False, len(changed), 0, "no progress on: " + "; ".join(f"{f}:{l}" for f, l, _ in errs[:5])
    return False, len(changed), 0, "did not converge"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=10)
    a = ap.parse_args()
    AB.mkdir(parents=True, exist_ok=True)
    tasks = []
    for i, c in enumerate(candidates()):
        if len(tasks) >= a.n:
            break
        tid = f"R{len(tasks) + 1:02d}"
        base, gold = AB / tid / "base", AB / tid / "gold"
        for d in (base, gold):
            if d.exists():
                shutil.rmtree(d)
        shutil.copytree(REPO, base, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast"))
        if not rename_definition(base, c):
            print("skip (definition line not as expected):", c)
            shutil.rmtree(base.parent)
            continue
        shutil.copytree(base, gold, symlinks=True)
        ok, sites, files, log = fix_by_compiler(gold, c)
        shutil.rmtree(gold)
        print(tid, c["receiver"], c["name"], "->", NEW[c["name"]], "valid" if ok else "INVALID", f"sites={sites} files={files} rivals={c['rivals']}", log[:300].replace("\n", " | "), flush=True)
        if not ok or sites < 5:
            shutil.rmtree(base.parent)
            continue
        tasks.append(dict(id=tid, family="R", repo_name="anchore/syft", base=str(base), receiver=c["receiver"], name=c["name"], new_name=NEW[c["name"]],
                          file=c["file"], line=c["line"], sites=sites, files=files, rivals=c["rivals"],
                          check_cmd="cd {repo} && go build ./... && " + VET))
    json.dump(tasks, open(AB / "tasks.json", "w"), indent=1)
    print(len(tasks), "tasks written to", AB / "tasks.json")


if __name__ == "__main__":
    main()
