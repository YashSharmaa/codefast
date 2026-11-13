# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Tier 2 pilot: build code-writing tasks from a real Go project with a fast test suite.

A task removes the body of one well-tested function (it is replaced by a panic) and asks the agent
to write it again so that the package's tests pass. The outcome is checked by running those tests.

For every task and arm a private copy of the project is made (no git history), so runs cannot see
each other or the original body.

usage: uv run bench/tier2/make_w_tasks.py [--n 8] [--arms control,rule]
"""
import argparse, hashlib, json, os, re, shutil, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AB = ROOT / "target/ab"
REPO = ROOT / "bench/repos/gitleaks"
BIN = AB / "codefast-bin-v2"
PKGS = ["config", "detect", "report"]


def functions(path):
    out = subprocess.run([str(BIN), "debug-facts", str(path)], capture_output=True, text=True).stdout
    for line in out.splitlines():
        m = re.match(r"\s*\d+\s+(fn|method)\s+(\S+)\s+L(\d+)-(\d+)\s+parent=(\S+)", line)
        if m:
            yield m.group(2), int(m.group(3)), int(m.group(4)), m.group(5).lstrip("~")


def coverage(profile):
    cov = {}
    for line in open(profile):
        m = re.match(r"(.+?):(\d+)\.\d+,(\d+)\.\d+ (\d+) (\d+)", line)
        if m:
            f = m.group(1).split("gitleaks/v8/", 1)[-1]
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
    lines = Path(path).read_text().split("\n")
    open_i = next(i for i in range(start - 1, end) if lines[i].rstrip().endswith("{"))
    close_i = end - 1
    assert lines[close_i].strip() == "}", (path, lines[close_i])
    body = lines[open_i + 1: close_i]
    lines[open_i + 1: close_i] = ['\tpanic("not implemented")']
    Path(path).write_text("\n".join(lines))
    return len(body)


def test_hash(repo):
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
    ap.add_argument("--arms", default="control,rule")
    a = ap.parse_args()
    arms = a.arms.split(",")
    cov = coverage(AB / "w/gitleaks.cover")
    cands = []
    for pkg in PKGS:
        for path in sorted((REPO / pkg).rglob("*.go")):
            rel = str(path.relative_to(REPO))
            if rel.endswith("_test.go") or "/testdata/" in rel:
                continue
            for name, s, e, parent in functions(path):
                n = e - s + 1
                if not (14 <= n <= 70) or name in ("init", "main"):
                    continue
                frac, stm = covered_fraction(cov.get(rel, []), s, e)
                if frac >= 0.75 and stm >= 6:
                    cands.append(dict(file=rel, name=name, start=s, end=e, parent=parent if parent != "-" else "", covered=round(frac, 2), statements=stm))
    cands.sort(key=lambda c: hashlib.sha256((c["file"] + c["name"]).encode()).hexdigest())
    # at most two per file, to spread the tasks
    chosen, per_file = [], {}
    for c in cands:
        if per_file.get(c["file"], 0) >= 2:
            continue
        per_file[c["file"]] = per_file.get(c["file"], 0) + 1
        chosen.append(c)
        if len(chosen) == a.n:
            break
    tasks = []
    for i, c in enumerate(chosen):
        tid = f"W{i + 1:02d}"
        pkg = c["file"].split("/")[0]
        for arm in arms:
            dst = AB / "w" / tid / arm / "repo"
            if dst.exists():
                shutil.rmtree(dst)
            shutil.copytree(REPO, dst, ignore=shutil.ignore_patterns(".git", ".codefast"))
            removed = mutate(dst / c["file"], c["start"], c["end"])
            tool = ""
            if arm != "control":
                tdir = AB / "tools2" / f"{tid}-{arm}"
                tdir.mkdir(parents=True, exist_ok=True)
                tool = str(tdir / "codefast")
                Path(tool).write_text(f'#!/bin/sh\nCODEFAST_INDEX_DIR={AB}/w/{tid}/{arm}/index exec {BIN} --root {dst} "$@"\n')
                os.chmod(tool, 0o755)
            tasks.append(dict(id=tid, kind="W", arm=arm, repo=str(dst), tool=tool, pkg=pkg, file=c["file"], name=c["name"], owner=c["parent"],
                              removed_lines=removed, covered=c["covered"], tests=test_hash(dst)))
        print(tid, c["file"], (c["parent"] + "." if c["parent"] else "") + c["name"], f"L{c['start']}-{c['end']}", "covered", c["covered"])
    json.dump(tasks, open(AB / "w/tasks.json", "w"), indent=1)
    print(len(tasks), "task copies written to", AB / "w")


if __name__ == "__main__":
    main()
