#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Live-kBench-2512 (arXiv 2602.02690) metadata -> normalised JSONL on stdout.

Reads bench/data/live-kbench/lkbench-2512.metadata.jsonl (written by extract_live_kbench.py).

One line per task:
  {"id", "repo", "commit_or_version", "query_text", "gold_files": [...], "gold_functions": [...]}

  id                 syzbot bug id (40-hex), https://syzkaller.appspot.com/bug?id=<id>
  repo               always "linux"
  commit_or_version  parentOfFixCommit: the parent of the developer's fix commit in torvalds/linux.
                     This is the tree the paper's agents are started in (kArena/crawler.py).
                     (--commit crash gives the commit syzbot crashed on instead; 150 of those are in
                     net/bpf/usb/linux-next trees rather than mainline.)
  query_text         syzbot crash report text ("<title>\\n\\n<report>" unless --no-title)
  gold_files         patchModifiedFiles of the developer patch
  gold_functions     "<file>:<function>" from patchModifiedFunctions (upstream computed these with
                     pydriller/lizard "changed_methods"; a file with no entry changed no function body)

Run:
  uv run bench/scripts/datasets/load_live_kbench.py --count
  uv run bench/scripts/datasets/load_live_kbench.py --limit 3 --extra
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

BENCH = Path(__file__).resolve().parents[2]
DEFAULT_DATA = BENCH / "data" / "live-kbench" / "lkbench-2512.metadata.jsonl"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data", type=Path, default=DEFAULT_DATA)
    ap.add_argument("--commit", default="parent", choices=["parent", "crash"])
    ap.add_argument("--no-title", action="store_true", help="query_text is the crash report only")
    ap.add_argument("--ids", default="", help="comma-separated bug ids (full, or unique prefixes of >= 8 hex chars)")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--extra", action="store_true")
    ap.add_argument("--count", action="store_true")
    ap.add_argument("--findable-only", action="store_true",
                    help="drop gold functions that do not exist at the parent commit (the 58 functions the fixes add; "
                         "listed in lkbench-2512.functions_absent_at_parent.json)")
    a = ap.parse_args()

    rows = [json.loads(l) for l in a.data.read_text(encoding="utf-8").splitlines() if l.strip()]
    ids = [x for x in a.ids.split(",") if x]
    if ids:
        sel = []
        for i in ids:
            m = [r for r in rows if r["bugId"] == i] or ([r for r in rows if r["bugId"].startswith(i)] if len(i) >= 8 else [])
            if len(m) != 1:
                raise SystemExit(f"bug id {i!r} matches {len(m)} rows")
            sel.append(m[0])
        rows = sel
    if a.limit:
        rows = rows[: a.limit]
    if a.count:
        print(len(rows))
        return 0

    absent_path = a.data.parent / "lkbench-2512.functions_absent_at_parent.json"
    absent = json.loads(absent_path.read_text(encoding="utf-8")) if absent_path.exists() else None
    if a.findable_only and absent is None:
        raise SystemExit(f"--findable-only needs {absent_path}")
    for r in rows:
        files = [f for f in r["patchModifiedFiles"] if f]
        gold_source = "patchModifiedFiles"
        if not files:
            # one bug (a9807a63...) ships an empty list although its patch touches files
            files = list(dict.fromkeys(re.findall(r"^diff --git a/(\S+) b/", r.get("patch") or "", re.M)))
            gold_source = "patch_diff_headers"
        funcs = []
        for f, names in zip(r["patchModifiedFiles"], r["patchModifiedFunctions"]):
            for n in names:
                key = f"{f}:{n}"
                if key not in funcs:
                    funcs.append(key)
        added = (absent or {}).get(r["bugId"], [])
        if a.findable_only:
            funcs = [f for f in funcs if f not in added]
        commit = r["parentOfFixCommit"] if a.commit == "parent" else r["crashKernelCommit"]
        report = r["crashReport"] or ""
        out = {
            "id": r["bugId"],
            "repo": "linux",
            "commit_or_version": commit,
            "query_text": report if a.no_title else f"{r['title']}\n\n{report}",
            "gold_files": files,
            "gold_functions": funcs,
        }
        if a.extra:
            out["extra"] = {
                "title": r["title"],
                "subsystems": r["subsystems"],
                "syzbot_url": r["syzbotUrl"],
                "reported": r["reportedTime"],
                "fix_commits": [c["hashValue"] for c in r["fixCommits"]],
                "fix_commit_date": r["patchCommitDate"],
                "parent_of_fix_commit": r["parentOfFixCommit"],
                "crash_commit": r["crashKernelCommit"],
                "crash_tree": r["crashKernelSourceGit"],
                "gold_source": gold_source,
                "functions_absent_at_parent": added,
                "patch": r["patch"],
            }
        sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
