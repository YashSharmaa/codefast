"""How much of a kernel question's verified answer the prompt card already holds. No model.

For each of the 72 verified questions of bench/k3 the prompt a live session would get is handed
to the prompt hook, and what the hook prints is scored against the verified answer with the
scorer the live sessions are scored with. A card that scores 1.00 has answered the question
before the session starts, and a question with no card is one the session has to go and ask.

usage: python3 -I bench/tier2/card_k.py [--only F2,F5] [--show]
       CODEFAST_BIN names the binary. It gets a kernel index of its own.
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
LINUX = ROOT / "bench/repos/linux"
BIN = os.environ.get("CODEFAST_BIN", str(ROOT / "target/release/codefast"))


def index_dir():
    """An index is tied to the version that wrote it, and a hook that finds another version's
    index says nothing. So every version has a kernel index of its own here, built on first
    use, and no other program's index is touched."""
    if os.environ.get("CODEFAST_INDEX_DIR"):
        return os.environ["CODEFAST_INDEX_DIR"]
    version = subprocess.run([BIN, "--version"], capture_output=True, text=True).stdout.split()[-1]
    d = ROOT / "target/bench-index" / f"card-linux-{version}"
    env = dict(os.environ, CODEFAST_INDEX_DIR=str(d))
    subprocess.run([BIN, "--root", str(LINUX), "status"], capture_output=True, text=True, env=env)
    return str(d)


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"bench/tier2/{name}.py")
    mod = importlib.util.module_from_spec(spec)
    argv, sys.argv = sys.argv, [sys.argv[0]]
    try:
        spec.loader.exec_module(mod)
    finally:
        sys.argv = argv
    return mod


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="", help="families, comma separated")
    ap.add_argument("--show", action="store_true", help="print each card")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    live, score_ab = load("live"), load("score_ab")
    k3 = score_ab.load_k3()
    tasks = json.load(open(ROOT / "bench/tier2/tasks.json"))
    used = {q: name for name in ("kernel_questions_round1", "kernel_questions_round2", "kernel_questions_held_out") for q in tasks.get(name, [])}
    only = set(a.only.split(",")) - {""}
    env = dict(os.environ, CODEFAST_INDEX_DIR=index_dir(), CODEFAST_NO_REFRESH="1")
    rows = []
    for line in open(ROOT / "bench/k3/questions.verified.jsonl"):
        q = json.loads(line)
        if only and q["family"] not in only:
            continue
        t = dict(id="K-" + q["id"], family="K", repo_name="torvalds/linux", question=q["question"], qid=q["id"])
        prompt = live.prompt_for(t, LINUX)
        payload = json.dumps({"hook_event_name": "UserPromptSubmit", "prompt": prompt, "cwd": str(LINUX), "session_id": "card-k-" + uuid.uuid4().hex})
        p = subprocess.run([BIN, "--root", str(LINUX), "hook", "user-prompt-submit"], input=payload, capture_output=True, text=True, env=env)
        card = ""
        if p.stdout.strip():
            try:
                card = json.loads(p.stdout)["hookSpecificOutput"]["additionalContext"]
            except Exception:
                card = ""
        s = float(score_ab.score_k(k3, q, card)) if card else 0.0
        rows.append(dict(id=q["id"], family=q["family"], card=bool(card), bytes=len(card), score=s, used=used.get(q["id"], "never run live"), question=q["question"]))
        if a.show:
            print(f"--- {q['id']} [{rows[-1]['used']}] {q['question']}\n{card or '(no card)'}\n    card score {s:.2f}\n")
    fam = collections.defaultdict(list)
    for r in rows:
        fam[r["family"]].append(r)
    mean = lambda xs: sum(xs) / len(xs) if xs else 0.0
    print(f"{'family':7s} {'questions':>9s} {'with a card':>11s} {'card score':>10s} {'answered by the card':>21s} {'bytes':>6s}")
    for f in sorted(fam):
        v = fam[f]
        print(f"{f:7s} {len(v):9d} {sum(r['card'] for r in v):11d} {mean([r['score'] for r in v]):10.2f} {sum(1 for r in v if r['score'] >= 0.999):21d} {mean([r['bytes'] for r in v if r['card']]):6.0f}")
    print(f"{'all':7s} {len(rows):9d} {sum(r['card'] for r in rows):11d} {mean([r['score'] for r in rows]):10.2f} {sum(1 for r in rows if r['score'] >= 0.999):21d} {mean([r['bytes'] for r in rows if r['card']]):6.0f}")
    # The questions a live session has been run on were there to be developed on. The rest
    # were not looked at while the card was shaped.
    for label, keep in (("run live before", lambda r: r["used"] != "never run live"), ("never run live", lambda r: r["used"] == "never run live")):
        v = [r for r in rows if keep(r)]
        if v:
            print(f"{label:16s} {len(v):3d} questions, {sum(r['card'] for r in v):3d} with a card, card score {mean([r['score'] for r in v]):.2f}, {sum(1 for r in v if r['score'] >= 0.999):3d} answered by the card")
    if a.out:
        with open(a.out, "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")


if __name__ == "__main__":
    main()
