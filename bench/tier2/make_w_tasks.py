# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Code-writing tasks: a well-tested function's body is gone, and the agent writes it again.

A task takes one function of gitleaks whose statements the package's own tests cover, replaces its
body with `panic("not implemented")`, and asks for the body back. The outcome is the package's
tests: they pass or they do not. Nothing about the function is left in the tree except its
signature, its doc comment and the places that call it, so writing it means finding out from the
repository what it has to do. That is the case the navigation families do not cover.

The copy the task is set in keeps no git history, so the body cannot be recovered from it, and
`live.py` copies that copy again for every run.

The coverage profile says which statements the tests reach. Make one with
`go test -coverprofile=target/ab/w/<name>.cover ./<pkg>/...` in the repository, once.

usage: python3 -I bench/tier2/make_w_tasks.py [--n 8] [--repo bench/repos/gitleaks]
       [--pkgs config,detect,report] [--cover target/ab/w/gitleaks.cover] [--prefix W]
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab"
CHECK = "cd {repo} && go test ./%s/..."


def functions(binary, path):
    """Every function and method of one file, from the index's own facts."""
    out = subprocess.run([str(binary), "debug-facts", str(path)], capture_output=True, text=True).stdout
    for line in out.splitlines():
        m = re.match(r"\s*\d+\s+(fn|method)\s+(\S+)\s+L(\d+)-(\d+)\s+parent=(\S+)", line)
        if m:
            yield m.group(2), int(m.group(3)), int(m.group(4)), m.group(5).lstrip("~")


def coverage(profile, module):
    cov = {}
    for line in open(profile):
        m = re.match(r"(.+?):(\d+)\.\d+,(\d+)\.\d+ (\d+) (\d+)", line)
        if m:
            f = m.group(1).split(module, 1)[-1].lstrip("/")
            cov.setdefault(f, []).append((int(m.group(2)), int(m.group(3)), int(m.group(4)), int(m.group(5)) > 0))
    return cov


def covered_fraction(blocks, a, b):
    stm = hit = 0
    for (s, e, n, h) in blocks:
        if s >= a and e <= b:
            stm += n
            hit += n if h else 0
    return (hit / stm if stm else 0.0), stm


def mutate(path, start, end):
    """Replace one function's body with a panic, and return how many lines went away."""
    lines = Path(path).read_text().split("\n")
    open_i = next(i for i in range(start - 1, end) if lines[i].rstrip().endswith("{"))
    close_i = end - 1
    assert lines[close_i].strip() == "}", (path, lines[close_i])
    body = lines[open_i + 1:close_i]
    lines[open_i + 1:close_i] = ['\tpanic("not implemented")']
    Path(path).write_text("\n".join(lines))
    return len(body)


def test_hash(repo):
    """The tests and their data, so a run that edited them can be told apart."""
    h = hashlib.sha256()
    for p in sorted(Path(repo).rglob("*")):
        rel = str(p.relative_to(repo))
        if p.is_file() and (rel.endswith("_test.go") or rel.startswith("testdata/") or "/testdata/" in rel):
            h.update(rel.encode())
            h.update(p.read_bytes())
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=8)
    ap.add_argument("--repo", default="bench/repos/gitleaks")
    ap.add_argument("--repo-name", default="zricethezav/gitleaks")
    ap.add_argument("--module", default="gitleaks/v8")
    ap.add_argument("--binary", default="target/release/codefast")
    ap.add_argument("--cover", default="target/ab/w/gitleaks.cover")
    ap.add_argument("--out", default="target/ab/w")
    ap.add_argument("--pkgs", default="config,detect,report", help="top directories to take functions from")
    ap.add_argument("--prefix", default="W", help="the id prefix, so a second family can live beside the first")
    a = ap.parse_args()
    repo, binary = ROOT / a.repo, ROOT / a.binary
    out = ROOT / a.out
    cov = coverage(ROOT / a.cover, a.module)

    cands = []
    for pkg in a.pkgs.split(","):
        for path in sorted((repo / pkg).rglob("*.go")):
            rel = str(path.relative_to(repo))
            if rel.endswith("_test.go") or "/testdata/" in rel:
                continue
            for name, s, e, parent in functions(binary, path):
                n = e - s + 1
                if not (14 <= n <= 70) or name in ("init", "main"):
                    continue
                frac, stm = covered_fraction(cov.get(rel, []), s, e)
                if frac >= 0.75 and stm >= 6:
                    cands.append(dict(file=rel, name=name, start=s, end=e, owner=parent if parent != "-" else "",
                                      covered=round(frac, 2), statements=stm))
    cands.sort(key=lambda c: hashlib.sha256((c["file"] + c["name"]).encode()).hexdigest())
    chosen, per_file = [], {}
    for c in cands:
        if per_file.get(c["file"], 0) >= 2:  # at most two per file, to spread the tasks
            continue
        per_file[c["file"]] = per_file.get(c["file"], 0) + 1
        chosen.append(c)
        if len(chosen) == a.n * 5:  # a pool, because the two checks below reject candidates
            break

    # A task has to compile with the body gone and fail the package's tests: the failing tests
    # are the whole of what the agent is told, so a tree that does not build would hand it a
    # compiler error instead, and a tree that passes would ask for nothing.
    # A package whose own tests already fail cannot judge anything, so each one is tried once
    # untouched first.
    sound = {}

    def package_is_sound(pkg, env):
        if pkg not in sound:
            probe = out / "probe"
            if probe.exists():
                shutil.rmtree(probe)
            shutil.copytree(repo, probe, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast"))
            rc = subprocess.run(["go", "test", f"./{pkg}/..."], cwd=probe, capture_output=True, env=env, timeout=1800).returncode
            sound[pkg] = rc == 0
            shutil.rmtree(probe)
            if not sound[pkg]:
                print(f"-   package {pkg}: its own tests do not pass untouched")
        return sound[pkg]

    tasks = []
    for c in chosen:
        if len(tasks) == a.n:
            break
        tid = f"{a.prefix}{len(tasks) + 1:02d}"
        pkg = str(Path(c["file"]).parent)  # the package the function is in, not its top directory
        env = dict(os.environ, GOFLAGS="-mod=mod")
        if not package_is_sound(pkg, env):
            continue
        base = out / tid / "base"
        if base.exists():
            shutil.rmtree(base)
        base.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(repo, base, symlinks=True, ignore=shutil.ignore_patterns(".git", ".codefast"))
        removed = mutate(base / c["file"], c["start"], c["end"])
        which = (c["owner"] + "." if c["owner"] else "") + c["name"]
        if subprocess.run(["go", "build", f"./{pkg}/..."], cwd=base, capture_output=True, env=env).returncode:
            print(f"-   {c['file']} {which}: the tree does not build without the body")
            shutil.rmtree(base)
            continue
        if subprocess.run(["go", "test", f"./{pkg}/..."], cwd=base, capture_output=True, env=env, timeout=1800).returncode == 0:
            print(f"-   {c['file']} {which}: the tests pass without the body")
            shutil.rmtree(base)
            continue
        tasks.append(dict(id=tid, family="W", repo_name=a.repo_name, base=str(base), pkg=pkg,
                          file=c["file"], name=c["name"], owner=c["owner"], removed_lines=removed,
                          covered=c["covered"], statements=c["statements"],
                          check_cmd=CHECK % pkg, tests_hash=test_hash(base)))
        print(tid, c["file"], which, f"L{c['start']}-{c['end']}", f"{removed} lines", "covered", c["covered"])
    (out).mkdir(parents=True, exist_ok=True)
    json.dump(tasks, open(out / "tasks.json", "w"), indent=1)
    print(len(tasks), "tasks written to", out / "tasks.json")


if __name__ == "__main__":
    main()
