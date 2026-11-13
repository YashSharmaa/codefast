#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Check kernel-dataset gold against the full-history clone, without materialising any tree.

Reads normalised JSONL (from load_linuxflbench.py or load_live_kbench.py) on stdin and, per task:
  - resolves commit_or_version to a git object in linux-full (a version X becomes tag vX),
  - checks every gold file exists in that tree,
  - checks every gold function name occurs as a whole word in its gold file at that revision,
  - reports gold files whose path collides with another path when case is folded (those files are
    lost when the tree is written to a case-insensitive file system such as default macOS APFS).

It only reads git objects; it never fetches. Missing tags / commits are reported, not fetched.
Prints one JSON summary on stdout; --details FILE writes the per-task records as JSONL.

  uv run load_linuxflbench.py | uv run check_kernel_gold.py --details /tmp/lfl.jsonl
  uv run load_live_kbench.py  | uv run check_kernel_gold.py
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

BENCH = Path(__file__).resolve().parents[2]


def git(repo: Path, *args: str, check: bool = False) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, check=check)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--linux", type=Path, default=BENCH / "repos" / "linux-full")
    ap.add_argument("--details", type=Path, default=None)
    a = ap.parse_args()

    tasks = [json.loads(l) for l in sys.stdin if l.strip()]
    summary: Counter = Counter()
    collide_cache: dict[str, set[str]] = {}
    details = []
    for t in tasks:
        cv = t["commit_or_version"]
        ref = cv if re.fullmatch(r"[0-9a-f]{40}", cv) else f"refs/tags/v{cv}"
        rec = {"id": t["id"], "ref": ref, "resolved": False, "missing_files": [], "missing_functions": [],
               "case_colliding_gold_files": []}
        summary["tasks"] += 1
        tree = git(a.linux, "rev-parse", "--verify", "-q", f"{ref}^{{tree}}").stdout.decode().strip()
        if not tree:
            summary["ref_not_in_clone"] += 1
            details.append(rec)
            continue
        rec["resolved"] = True
        summary["ref_resolved"] += 1
        if tree not in collide_cache:
            names = git(a.linux, "ls-tree", "-r", "--name-only", "-z", tree).stdout.decode("utf-8", "surrogateescape").split("\0")
            folded = Counter(n.lower() for n in names if n)
            collide_cache[tree] = {n for n in names if n and folded[n.lower()] > 1}
        blobs: dict[str, str | None] = {}
        for f in t["gold_files"]:
            summary["gold_files"] += 1
            p = git(a.linux, "cat-file", "blob", f"{tree}:{f}")
            blobs[f] = p.stdout.decode("utf-8", "replace") if p.returncode == 0 else None
            if blobs[f] is None:
                rec["missing_files"].append(f)
                summary["gold_files_missing"] += 1
            if f in collide_cache[tree]:
                rec["case_colliding_gold_files"].append(f)
                summary["gold_files_case_colliding"] += 1
        for gf in t["gold_functions"]:
            f, _, name = gf.partition(":")
            summary["gold_functions"] += 1
            text = blobs.get(f)
            if text is None or not re.search(rf"(?<![A-Za-z0-9_]){re.escape(name)}(?![A-Za-z0-9_])", text):
                rec["missing_functions"].append(gf)
                summary["gold_functions_not_found_in_file"] += 1
        if rec["missing_files"]:
            summary["tasks_with_missing_gold_file"] += 1
        if rec["missing_functions"]:
            summary["tasks_with_function_not_found"] += 1
        if rec["case_colliding_gold_files"]:
            summary["tasks_with_case_colliding_gold_file"] += 1
        details.append(rec)

    if a.details:
        a.details.write_text("".join(json.dumps(d) + "\n" for d in details), encoding="utf-8")
    out = dict(sorted(summary.items()))
    out["unresolved_refs"] = sorted({d["ref"] for d in details if not d["resolved"]})
    print(json.dumps(out, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
