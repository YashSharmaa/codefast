# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Agent experiment, family F: which files does this kernel bug report point to?

The tasks are LinuxFLBench (arXiv 2505.19489): a Bugzilla report against a kernel version, and
the one file the real fix changed. A session is given the report in a checkout of that version
and has to end with up to ten files, the most likely first. This is the setting of the one
published comparison of repository maps for a coding agent on this benchmark (arXiv 2611.07286:
Claude Code, 152 sampled tasks, file recall at 1 and at 10, total tokens).

Tasks are taken from the half of the benchmark that the ranking of `codefast locate` was never
tuned on (`bench/tier1/run_locate.py` splits by a hash of the id), in the order of a hash of
the id, so that the sample is fixed and is not chosen by anything a tool does.

usage: python3 -I bench/tier2/make_f_tasks.py [--n 50]
       (the kernel trees come from bench/scripts/datasets/prepare_tree.sh)
"""
import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target/ab/f"
TEXT_CAP = 6000  # as in the published harness


def is_dev(task_id):
    return int(hashlib.sha256(task_id.encode()).hexdigest(), 16) % 2 == 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=50)
    ap.add_argument("--dev", action="store_true", help="take the tasks from the other half, the one that may be developed on, into target/ab/fdev")
    a = ap.parse_args()
    global OUT
    if a.dev:
        OUT = ROOT / "target/ab/fdev"
    out = subprocess.run([sys.executable, "-I", str(ROOT / "bench/scripts/datasets/load_linuxflbench.py")], capture_output=True, text=True, check=True).stdout
    tasks = [json.loads(l) for l in out.splitlines() if l.strip()]
    held = [t for t in tasks if is_dev(t["id"]) == a.dev]
    held.sort(key=lambda t: hashlib.sha256(("f:" + t["id"]).encode()).hexdigest())
    # What `codefast locate` alone said for each, for reading the result afterwards.
    alone = {}
    p = ROOT / "bench/results/tier1-linuxflbench.jsonl"
    if p.exists():
        for l in open(p):
            r = json.loads(l)
            alone[r["id"]] = (r.get("ranks") or [None])[0]
    picked = []
    for t in held:
        if len(picked) == a.n:
            break
        tree = subprocess.run([str(ROOT / "bench/scripts/datasets/prepare_tree.sh"), "linuxflbench", t["id"]], capture_output=True, text=True)
        path = tree.stdout.strip().splitlines()[-1] if tree.returncode == 0 and tree.stdout.strip() else ""
        if not path or not Path(path).is_dir():
            print(f"-   {t['id']}: no tree for {t['commit_or_version']}", file=sys.stderr)
            continue
        if not all((Path(path) / g).is_file() for g in t["gold_files"]):
            # A case-insensitive file system can lose one of two files that differ in case.
            print(f"-   {t['id']}: a gold file is not in the tree", file=sys.stderr)
            continue
        picked.append(dict(id=("FD-" if a.dev else "F-") + t["id"], family="F", repo_name="torvalds/linux", version=t["commit_or_version"], repo_dir=path,
                           report=t["query_text"][:TEXT_CAP], gold_files=t["gold_files"], gold_functions=t["gold_functions"], locate_rank=alone.get(t["id"])))
    OUT.mkdir(parents=True, exist_ok=True)
    json.dump(picked, open(OUT / "tasks.json", "w"), indent=1)
    trees = {t["repo_dir"] for t in picked}
    r1 = sum(1 for t in picked if t["locate_rank"] == 1)
    r10 = sum(1 for t in picked if t["locate_rank"] and t["locate_rank"] <= 10)
    print(f"{len(picked)} tasks of {len(held)} in the half that {'may be developed on' if a.dev else 'was not tuned on'}, {len(trees)} kernel versions")
    print(f"`codefast locate` alone on these: right file first for {r1}, in the first ten for {r10}")


if __name__ == "__main__":
    main()
