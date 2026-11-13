# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, family N: who calls this method? Answers come from a compiler.

Each task asks for every function that calls one method of the syft project. The method's name
is shared by methods of other types, so a text search finds more than the answer and each hit
has to be told apart by the type of its receiver. The verified answer is the set of callers in
the call graph that the Go type checker built (`bench/oracles/syft`), tests included.

A long task (family L) is five of these questions in one session: what a session costs once
its context has grown.

usage: python3 bench/tier2/make_n_tasks.py [--n 20]
       python3 bench/tier2/make_n_tasks.py --repo go-git --repo-name go-git/go-git --out n2
The second form builds the same kind of question from another repository that has an oracle.
"""
import argparse, collections, gzip, hashlib, json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ORACLE = ROOT / "bench/oracles/syft"
OUT = ROOT / "target/ab/n"
SKIP = {"String", "Name", "ID", "Error", "Len", "Less", "Swap", "MarshalJSON", "UnmarshalJSON"}


def main():
    global ORACLE, OUT
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--repo", default="syft", help="directory under bench/repos and bench/oracles")
    ap.add_argument("--repo-name", default="anchore/syft")
    ap.add_argument("--out", default="n", help="directory under target/ab")
    a = ap.parse_args()
    ORACLE, OUT = ROOT / "bench/oracles" / a.repo, ROOT / "target/ab" / a.out
    tag = "" if a.repo == "syft" else a.repo.replace("-", "") + "-"
    E = [json.loads(l) for l in gzip.open(ORACLE / "edges.jsonl.gz", "rt")]
    F = [json.loads(l) for l in gzip.open(ORACLE / "functions.jsonl.gz", "rt")]
    byloc = {(f["file"], f["line"]): f for f in F}
    owners = collections.defaultdict(set)
    for f in F:
        if f.get("receiver_or_class"):
            owners[f["name"]].add(f["receiver_or_class"].lstrip("*"))
    callers = collections.defaultdict(set)  # (receiver, name, file, line) -> {(caller name, caller file)}
    for e in E:
        c = e["callee"]
        fr = byloc.get((c["file"], c["line"]))
        if fr and fr.get("receiver_or_class") and e.get("kind") == "static":
            callers[(fr["receiver_or_class"].lstrip("*"), c["name"], c["file"], c["line"])].add((e["caller"]["name"], e["caller"]["file"]))
    cands = []
    for (recv, name, file, line), who in callers.items():
        rivals = owners[name] - {recv}
        files = {f for _, f in who}
        if 5 <= len(who) <= 25 and len(files) >= 3 and rivals and name not in SKIP:
            # Callers of the same-named methods of other types: naming one of these is a wrong answer.
            others = set()
            for (r2, n2, _, _), w2 in callers.items():
                if n2 == name and r2 != recv:
                    others |= w2
            gold = sorted(who)
            wrong = sorted(o for o in others if o[0] not in {n for n, _ in who})
            cands.append(dict(id=f"N-{tag}{recv}-{name}", family="N", repo_name=a.repo_name, repo_dir=str(ROOT / "bench/repos" / a.repo), receiver=recv, name=name, file=file, line=line,
                              gold=[dict(name=n, file=f) for n, f in gold], wrong=[dict(name=n, file=f) for n, f in wrong], rivals=len(rivals)))
    cands.sort(key=lambda c: hashlib.sha256(c["id"].encode()).hexdigest())
    tasks = cands[: a.n]
    OUT.mkdir(parents=True, exist_ok=True)
    json.dump(tasks, open(OUT / "tasks.json", "w"), indent=1)
    for t in tasks:
        print(f"{t['id']:34} callers={len(t['gold']):2d} other types with the name={t['rivals']} their callers={len(t['wrong'])}")
    print(len(tasks), "of", len(cands), "candidates written to", OUT / "tasks.json")


if __name__ == "__main__":
    main()
