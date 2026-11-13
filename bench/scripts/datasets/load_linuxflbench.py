#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""LinuxFLBench (arXiv 2505.19489) -> normalised JSONL on stdout.

One line per task:
  {"id", "repo", "commit_or_version", "query_text", "gold_files": [...], "gold_functions": [...]}

  id                 Bugzilla bug id (string), e.g. "207835"
  repo               always "linux"
  commit_or_version  the dataset's "Kernel Version" verbatim, e.g. "5.6.7", "2.6.27-rc2", "2.4.26"
                     (the git tag is "v" + this; see prepare_tree.sh for versions that have no tag)
  query_text         title + blank line + description (what the paper's agents are given)
  gold_files         dataset "paths" (always exactly one file)
  gold_functions     "<file>:<name>" for each dataset "methods" entry (names are bare C identifiers;
                     a few are tables / structs rather than functions, exactly as upstream labels them)

Run:
  uv run bench/scripts/datasets/load_linuxflbench.py                 # all 250
  uv run bench/scripts/datasets/load_linuxflbench.py --ids 207835,2567 --extra
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

BENCH = Path(__file__).resolve().parents[2]
DEFAULT_DATA = BENCH / "data" / "linuxflbench" / "upstream" / "dataset" / "LINUXFLBENCH_dataset.jsonl"
ABSENT = BENCH / "data" / "linuxflbench" / "functions_absent_at_version.json"
META_KEYS = ("Status", "Product", "Component", "Hardware", "Importance", "Reported", "Modified",
             "Regression", "Bisected commit-id", "Subsystem")


def load(path: Path):
    rows = []
    with path.open(encoding="utf-8") as fh:
        for n, line in enumerate(fh, 1):
            if not line.strip():
                continue
            r = json.loads(line)
            for k in ("id", "title", "description", "Kernel Version", "paths", "methods"):
                if k not in r:
                    raise SystemExit(f"{path}:{n}: missing field {k!r}")
            rows.append(r)
    return rows


def normalise(r: dict, extra: bool, absent: dict | None = None) -> dict:
    files = list(dict.fromkeys(p.strip() for p in r["paths"]))
    # one upstream name carries a stray pointer star ("*ocfs2_get_write_source"); strip it
    methods = list(dict.fromkeys(m.strip().lstrip("*") for m in r["methods"]))
    if len(files) == 1:
        funcs = [f"{files[0]}:{m}" for m in methods]
    else:  # never happens in the 250 released tasks; keep the names honest if it ever does
        funcs = [f":{m}" for m in methods]
    if absent is not None:
        gone = set(absent.get(str(r["id"]), []))
        funcs = [f for f in funcs if f not in gone]
    title = r["title"].strip()
    out = {
        "id": str(r["id"]),
        "repo": "linux",
        "commit_or_version": r["Kernel Version"].strip(),
        "query_text": f"{title}\n\n{r['description']}",
        "gold_files": files,
        "gold_functions": funcs,
    }
    if extra:
        out["extra"] = {
            "title": title,
            "git_tag": "v" + r["Kernel Version"].strip(),
            "patch": r.get("patch"),
            "bugzilla_url": f"https://bugzilla.kernel.org/show_bug.cgi?id={r['id']}",
            **{k: r.get(k) for k in META_KEYS},
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--data", type=Path, default=DEFAULT_DATA, help="LINUXFLBENCH_dataset.jsonl")
    ap.add_argument("--ids", default="", help="comma-separated task ids to keep")
    ap.add_argument("--limit", type=int, default=0, help="keep only the first N (dataset order)")
    ap.add_argument("--extra", action="store_true", help='add an "extra" object (patch, tag, Bugzilla metadata)')
    ap.add_argument("--count", action="store_true", help="print only the number of tasks")
    ap.add_argument("--findable-only", action="store_true",
                    help="drop gold functions whose name does not occur in the gold file at the task's kernel "
                         "version (listed in functions_absent_at_version.json; mostly functions the fix adds)")
    a = ap.parse_args()

    rows = load(a.data)
    ids = [x for x in a.ids.split(",") if x]
    if ids:
        by_id = {str(r["id"]): r for r in rows}
        missing = [i for i in ids if i not in by_id]
        if missing:
            raise SystemExit(f"unknown task id(s): {missing}")
        rows = [by_id[i] for i in ids]
    if a.limit:
        rows = rows[: a.limit]
    if a.count:
        print(len(rows))
        return 0
    absent = json.loads(ABSENT.read_text(encoding="utf-8")) if a.findable_only else None
    for r in rows:
        sys.stdout.write(json.dumps(normalise(r, a.extra, absent), ensure_ascii=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
