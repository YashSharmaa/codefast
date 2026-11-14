"""Whether the card for a kernel bug report names the file of the real fix. No model.

For each LinuxFLBench report, the prompt a live session of family F would get is handed to the
prompt hook in a checkout of the report's kernel version, and the files the card lists are
compared with the file the fix changed. In the sessions of family F a card that names the file
was followed every time, and a card that does not left the session to find it alone, so this
is the part of a session's result that can be measured without one.

By default the reports are those of the half that may be developed on (`run_locate.py` splits
by a hash of the id). `--held` takes the other half: look at it once a build is final.

usage: python3 -I bench/tier2/card_f.py [--held] [--indexed] [--show] [--out FILE]
       CODEFAST_BIN names the binary. A kernel version gets its index on first use.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIN = os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast"))
TEXT_CAP = 6000  # as in make_f_tasks.py


def is_dev(task_id):
    return int(hashlib.sha256(task_id.encode()).hexdigest(), 16) % 2 == 0


def load_live():
    spec = importlib.util.spec_from_file_location("live", ROOT / "bench/tier2/live.py")
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    argv, sys.argv = sys.argv, [sys.argv[0]]
    try:
        spec.loader.exec_module(mod)
    finally:
        sys.argv = argv
    return mod


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--held", action="store_true", help="the half that nothing was developed on")
    ap.add_argument("--indexed", action="store_true", help="only kernel versions that already have an index")
    ap.add_argument("--show", action="store_true", help="print each card")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    live = load_live()
    out = subprocess.run([sys.executable, "-I", str(ROOT / "bench/scripts/datasets/load_linuxflbench.py")], capture_output=True, text=True, check=True).stdout
    tasks = [t for t in (json.loads(l) for l in out.splitlines() if l.strip()) if is_dev(t["id"]) != a.held]
    rows = []
    for t in sorted(tasks, key=lambda t: t["id"]):
        tree = subprocess.run([str(ROOT / "bench/scripts/datasets/prepare_tree.sh"), "linuxflbench", t["id"]], capture_output=True, text=True)
        path = tree.stdout.strip().splitlines()[-1] if tree.returncode == 0 and tree.stdout.strip() else ""
        if not path or not all((Path(path) / g).is_file() for g in t["gold_files"]):
            continue
        index = ROOT / "target/bench-index" / ("lfl-" + Path(path).name)
        if a.indexed and not index.is_dir():
            continue
        env = dict(os.environ, CODEFAST_INDEX_DIR=str(index), CODEFAST_ROOT=path)
        subprocess.run([BIN, "--root", path, "status"], capture_output=True, text=True, env=env)
        task = dict(id="F-" + t["id"], family="F", repo_name="torvalds/linux", version=t["commit_or_version"], repo_dir=path, report=t["query_text"][:TEXT_CAP], gold_files=t["gold_files"])
        payload = json.dumps({"hook_event_name": "UserPromptSubmit", "prompt": live.prompt_for(task, Path(path)), "cwd": path, "session_id": "card-f-" + uuid.uuid4().hex})
        p = subprocess.run([BIN, "--root", path, "hook", "user-prompt-submit"], input=payload, capture_output=True, text=True, env=dict(env, CODEFAST_NO_REFRESH="1"))
        card = ""
        if p.stdout.strip():
            try:
                card = json.loads(p.stdout)["hookSpecificOutput"]["additionalContext"]
            except (ValueError, KeyError):
                card = ""
        # The ranking the card gives (its numbered lines), and every file it names anywhere,
        # which adds the places where it found text of the report word for word.
        ranked = re.findall(r"^\d+\. (\S+)", card, re.M)
        named = []
        for f in re.findall(r"[\w./+-]+\.[chS]\b", card):
            if f not in named and (Path(path) / f).is_file():
                named.append(f)
        gold = t["gold_files"][0]
        rows.append(dict(id=t["id"], version=t["commit_or_version"], card=bool(card), bytes=len(card), listed=len(ranked), where=ranked.index(gold) + 1 if gold in ranked else 0, named=gold in named, gold=gold))
        if a.show:
            print(f"--- {t['id']} {t['commit_or_version']} fix in {gold}: {'ranked ' + str(rows[-1]['where']) if rows[-1]['where'] else 'named, not ranked' if rows[-1]['named'] else 'not named'}\n{card or '(no card)'}\n")
    n = len(rows)
    if not n:
        sys.exit("no report with a tree")
    hit = lambda k: sum(1 for r in rows if r["where"] and r["where"] <= k)
    mean = lambda xs: sum(xs) / len(xs) if xs else 0.0
    print(f"{n} reports of the half that {'nothing was developed on' if a.held else 'may be developed on'}, {len({r['version'] for r in rows})} kernel versions")
    anywhere = sum(1 for r in rows if r["named"])
    print(f"with a card: {sum(r['card'] for r in rows)}, files a card ranks: {mean([r['listed'] for r in rows if r['card']]):.1f}, bytes of a card: {mean([r['bytes'] for r in rows if r['card']]):.0f}")
    print(f"the file of the fix is ranked first: {hit(1)} ({hit(1) / n:.3f}), in the first five: {hit(5)} ({hit(5) / n:.3f}), ranked at all: {hit(10**6)} ({hit(10**6) / n:.3f}), named anywhere in the card: {anywhere} ({anywhere / n:.3f})")
    if a.out:
        with open(a.out, "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")


if __name__ == "__main__":
    main()
