# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, family C: what does this function call? Answers come from a compiler.

Each task asks for every function of the repository that one function or method calls directly.
It is the mirror of family N (who calls this method). The verified answer is the set of callees
in the call graph that the Go type checker built (`bench/oracles/<repo>`): calls the checker
resolved statically, to functions the repository defines. Calls into the standard library and
into other modules are not in the graph and are not asked for.

A function is taken when it calls between 5 and 16 functions of the repository that are spread
over at least 2 files, when none of its calls goes through an interface or a function value
(those the checker cannot name, so the list would not be complete), and when it is not a test.
Naming a function that has the name of a callee and is another one (a method of another type,
in another file) is a wrong answer.

usage: python3 -I bench/tier2/make_c_tasks.py --repo go-git --repo-name go-git/go-git [--n 12]
"""
import argparse
import collections
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SKIP = {"String", "Error", "Len", "Less", "Swap", "init", "main"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=12)
    ap.add_argument("--repo", default="go-git", help="directory under bench/repos and bench/oracles")
    ap.add_argument("--repo-name", default="go-git/go-git")
    ap.add_argument("--out", default="c", help="directory under target/ab")
    a = ap.parse_args()
    oracle, out = ROOT / "bench/oracles" / a.repo, ROOT / "target/ab" / a.out
    tag = a.repo.replace("-", "")
    E = [json.loads(l) for l in gzip.open(oracle / "edges.jsonl.gz", "rt")]
    F = [json.loads(l) for l in gzip.open(oracle / "functions.jsonl.gz", "rt")]
    byloc = {(f["file"], f["line"]): f for f in F}
    named = collections.defaultdict(list)
    for f in F:
        named[f["name"]].append(f)
    callees = collections.defaultdict(set)  # caller (file, line) -> {(callee name, callee file)}
    for e in E:
        if e.get("kind") != "static":
            continue
        c = e["callee"]
        if (c["file"], c["line"]) in byloc:
            callees[(e["caller"]["file"], e["caller"]["line"])].add((c["name"], c["file"]))
    cands = []
    for (file, line), who in callees.items():
        f = byloc.get((file, line))
        if not f or f["name"] in SKIP or file.endswith("_test.go") or "/testdata/" in file or "_examples/" in file:
            continue
        if f.get("dynamic_sites", 0) or f.get("unresolved_dynamic_sites", 0) or f.get("closures", 0):
            continue  # the list the checker has would not be the whole list
        lines = f["end_line"] - f["line"] + 1
        files = {cf for _, cf in who}
        who = {(n, cf) for n, cf in who if not cf.endswith("_test.go")}
        if not (5 <= len(who) <= 16) or len(files) < 2 or not (12 <= lines <= 90):
            continue
        recv = (f.get("receiver_or_class") or "").lstrip("*")
        # Functions that have the name of a callee and are not it.
        wrong = sorted({(n, o["file"]) for n, _ in who for o in named[n] if (n, o["file"]) not in who and not o["file"].endswith("_test.go")})
        cands.append(dict(id=f"C-{tag}-{recv or 'fn'}-{f['name']}", family="C", repo_name=a.repo_name, repo_dir=str(ROOT / "bench/repos" / a.repo), receiver=recv, name=f["name"],
                          file=file, line=line, lines=lines, gold=[dict(name=n, file=cf) for n, cf in sorted(who)], wrong=[dict(name=n, file=cf) for n, cf in wrong]))
    # One per file, so that the tasks are spread over the repository.
    cands.sort(key=lambda c: hashlib.sha256(c["id"].encode()).hexdigest())
    tasks, seen = [], set()
    for c in cands:
        if c["file"] in seen or any(t["id"] == c["id"] for t in tasks):
            continue
        seen.add(c["file"])
        tasks.append(c)
        if len(tasks) == a.n:
            break
    out.mkdir(parents=True, exist_ok=True)
    json.dump(tasks, open(out / "tasks.json", "w"), indent=1)
    for t in tasks:
        print(f"{t['id']:44} {t['file']}:{t['line']} lines={t['lines']:2d} callees={len(t['gold']):2d} namesakes that are not called={len(t['wrong'])}")
    print(len(tasks), "of", len(cands), "candidates written to", out / "tasks.json")


if __name__ == "__main__":
    main()
