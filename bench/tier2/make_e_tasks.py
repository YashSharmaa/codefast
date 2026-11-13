# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, family E: from an error message to the code that raises it.

Debugging often starts with a message and nothing else. Each task quotes an error text of a Go
project and asks which function creates the error and which functions call that function. The
message is one that stands in exactly one place of the non-test source, written out in full (no
format verbs). The verified answer is the function around that line together with its callers
in the call graph the Go type checker built (`bench/oracles/REPO`).

usage: python3 bench/tier2/make_e_tasks.py [--repos go-git,syft] [--per-repo 8]
"""
import argparse, collections, gzip, hashlib, json, re, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target/ab/e"
NAMES = {"go-git": "go-git/go-git", "syft": "anchore/syft"}
LITERAL = re.compile(r'(?:errors\.New|fmt\.Errorf)\(\s*"((?:[^"\\]|\\.)*)"')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repos", default="go-git,syft")
    ap.add_argument("--per-repo", type=int, default=8)
    a = ap.parse_args()
    tasks = []
    for repo in a.repos.split(","):
        src, oracle = ROOT / "bench/repos" / repo, ROOT / "bench/oracles" / repo
        F = [json.loads(l) for l in gzip.open(oracle / "functions.jsonl.gz", "rt")]
        E = [json.loads(l) for l in gzip.open(oracle / "edges.jsonl.gz", "rt")]
        callers = collections.defaultdict(set)
        for e in E:
            if e.get("kind") == "static":
                callers[(e["callee"]["file"], e["callee"]["line"])].add((e["caller"]["name"], e["caller"]["file"]))
        by_file = collections.defaultdict(list)
        for f in F:
            by_file[f["file"]].append(f)
        files = subprocess.run(["git", "ls-files", "*.go"], cwd=src, capture_output=True, text=True).stdout.split()
        text = {f: (src / f).read_text(errors="replace") for f in files}
        found = []
        for f in files:
            if f.endswith("_test.go") or "/testdata/" in f or f not in by_file:
                continue
            for n, line in enumerate(text[f].split("\n"), 1):
                m = LITERAL.search(line)
                if not m:
                    continue
                msg = m.group(1)
                if "%" in msg or "\\" in msg or len(msg.split()) < 4 or len(msg) < 24:
                    continue
                # Written once in the whole repository, tests and documentation included.
                if sum(t.count(msg) for t in text.values()) != 1:
                    continue
                fn = next((x for x in by_file[f] if x["line"] <= n <= x["end_line"]), None)
                if not fn or not fn.get("judged", True):
                    continue
                who = callers[(fn["file"], fn["line"])]
                who = {(c, cf) for c, cf in who if (c, cf) != (fn["name"], fn["file"])}
                if not 2 <= len(who) <= 12:
                    continue
                found.append(dict(repo=repo, file=f, line=n, message=msg, function=fn["name"], receiver=fn.get("receiver_or_class") or "", callers=sorted(who)))
        # One task per function, in an order that does not depend on the file system.
        seen, picked = set(), []
        for c in sorted(found, key=lambda c: hashlib.sha256((c["repo"] + c["message"]).encode()).hexdigest()):
            if (c["file"], c["function"]) in seen:
                continue
            seen.add((c["file"], c["function"]))
            picked.append(c)
        for c in picked[: a.per_repo]:
            slug = re.sub(r"[^A-Za-z0-9]+", "-", c["function"])[:24].strip("-")
            tid = f"E-{repo.replace('-', '')}-{slug}-{c['line']}"
            gold = [dict(name=c["function"], file=c["file"])] + [dict(name=n, file=f) for n, f in c["callers"]]
            tasks.append(dict(id=tid, family="E", repo_name=NAMES[repo], repo_dir=str(src), message=c["message"], file=c["file"], line=c["line"], function=c["function"], gold=gold, wrong=[]))
            print(f"{tid:44} {c['file']}:{c['line']} callers={len(c['callers'])}  \"{c['message']}\"")
        print(f"{repo}: {len(picked)} candidates, {min(len(picked), a.per_repo)} taken")
    OUT.mkdir(parents=True, exist_ok=True)
    json.dump(tasks, open(OUT / "tasks.json", "w"), indent=1)
    print(len(tasks), "tasks written to", OUT / "tasks.json")


if __name__ == "__main__":
    main()
