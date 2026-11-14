"""What two builds of the prompt hook say for the prompts of the live families. No model.

A change to what a card says is meant for one kind of prompt. This prints every prompt of every
family for which the two builds differ, so that a change that reaches a prompt it was not meant
for is seen before a session is spent on it.

usage: python3 -I bench/tier2/card_diff.py --old BIN --new BIN [--families N,E,R,D,I,W,Z] [--show]
"""
import argparse
import collections
import importlib.util
import json
import os
import subprocess
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_live():
    spec = importlib.util.spec_from_file_location("live", ROOT / "bench/tier2/live.py")
    mod = importlib.util.module_from_spec(spec)
    argv, sys.argv = sys.argv, [sys.argv[0]]
    try:
        spec.loader.exec_module(mod)
    finally:
        sys.argv = argv
    return mod


VERSIONS = {}


def card(binary, repo, index, prompt):
    # An index is tied to the version that wrote it, and a hook that finds another version's
    # index says nothing: silence would then compare as a difference. Each build has its own.
    if binary not in VERSIONS:
        VERSIONS[binary] = subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.split()[-1]
    index = Path(f"{index}-{VERSIONS[binary]}")
    if not index.is_dir():
        subprocess.run([binary, "--root", str(repo), "index"], capture_output=True, env=dict(os.environ, CODEFAST_INDEX_DIR=str(index), CODEFAST_THREADS="2"))
    env = dict(os.environ, CODEFAST_INDEX_DIR=str(index), CODEFAST_NO_REFRESH="1")
    payload = json.dumps({"hook_event_name": "UserPromptSubmit", "prompt": prompt, "cwd": str(repo), "session_id": "diff-" + uuid.uuid4().hex})
    p = subprocess.run([binary, "--root", str(repo), "hook", "user-prompt-submit"], input=payload, capture_output=True, text=True, env=env)
    if not p.stdout.strip():
        return ""
    try:
        return json.loads(p.stdout)["hookSpecificOutput"]["additionalContext"].replace(binary, "codefast")
    except Exception:
        return ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--old", required=True)
    ap.add_argument("--new", required=True)
    ap.add_argument("--families", default="N,L,E,R,Z,D,I,W")
    ap.add_argument("--show", action="store_true")
    a = ap.parse_args()
    live = load_live()
    want = set(a.families.split(","))
    tasks = []
    n, long = live.n_tasks()
    tasks += n + live.load("n2") + live.load("nz") + long
    tasks += live.load("e") + live.load("r") + live.load("r2") + live.load("d") + live.load("i") + live.load("w") + live.load("w2")
    stats = collections.Counter()
    for t in tasks:
        fam = t["family"]
        if fam not in want:
            continue
        if fam in ("N", "L", "E", "Z", "R"):
            # The shared checkouts are indexed afresh for this comparison: an index older
            # than its files makes every hook silent, and silence compares equal.
            repo = live.repo_of(t["parts"][0] if fam == "L" else t)[0]
            index = ROOT / "target/bench-index" / f"diff-{Path(repo).name}"
        else:
            repo = Path(t["base"])
            index = ROOT / "target/bench-index" / f"diff-{t['id']}"
        if not Path(repo).is_dir():
            stats[(fam, "no repository")] += 1
            continue
        prompt = live.prompt_for(t, repo)
        old, new = card(a.old, repo, index, prompt), card(a.new, repo, index, prompt)
        stats[(fam, "prompts")] += 1
        stats[(fam, "with a card, old")] += bool(old)
        stats[(fam, "with a card, new")] += bool(new)
        if old != new:
            stats[(fam, "differ")] += 1
            stats[(fam, "bytes added")] += len(new) - len(old)
            if a.show:
                print(f"=== {t['id']} ({fam})\n--- old ({len(old)} bytes)\n{old or '(no card)'}\n--- new ({len(new)} bytes)\n{new or '(no card)'}\n")
    fams = sorted({k[0] for k in stats})
    print(f"{'family':7s} {'prompts':>7s} {'card, old':>9s} {'card, new':>9s} {'differ':>6s} {'bytes added':>11s}")
    for f in fams:
        print(f"{f:7s} {stats[(f, 'prompts')]:7d} {stats[(f, 'with a card, old')]:9d} {stats[(f, 'with a card, new')]:9d} {stats[(f, 'differ')]:6d} {stats[(f, 'bytes added')]:11d}")


if __name__ == "__main__":
    main()
