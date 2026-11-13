#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Loc-Bench V1 (LocAgent, arXiv 2503.09089) -> normalised JSONL on stdout.

One line per task:
  {"id", "repo", "commit_or_version", "query_text", "gold_files": [...], "gold_functions": [...]}

  id                 Loc-Bench instance_id, e.g. "UXARRAY__uxarray-1117"
  repo               GitHub "owner/name"
  commit_or_version  base_commit (40-hex sha, the pre-fix commit)
  query_text         see --query
  gold_files         see --gold
  gold_functions     Loc-Bench edit_functions verbatim: "<file>:<func>" or "<file>:<Class>.<method>"
                     (LocAgent's function-level scorer additionally strips a trailing ".__init__";
                     functions the patch ADDS are not gold and are listed under extra.added_functions)

Subsets (--subset):
  all               all 560 instances, dataset order
  ripwire-heldout   held-out half of ripwire's repository-disjoint split as its code computes it
                    today (salt "ripwire-a7-v2"): 306 instances, 88 repos
  ripwire-train     the other half: 254 instances, 77 repos
  ripwire-heldout-243-legacy
                    the 243 instances / 78 repos behind the "held-out N=243, 60.9% strict file@10"
                    figure in ripwire's README. That list is NOT reproducible from the current salt
                    (it predates it); the ids are read from
                    bench/locbench/results/r1_anchorhop/heldout_baseline_release.json
  ripwire-r4-60     the exact 60 instances of ripwire head-to-head round 4 (the first 60 scored
                    held-out instances), in the order of results/r4_ripwire_for.json

--gold:
  locagent   files named by edit_functions (what LocAgent's eval_metric.py scores)      [default]
  ripwire    ripwire's "primary_files": (edit_functions files UNION files in the fix patch) that
             ripwire could index. Only recorded for the r4 slice, so only valid with
             --subset ripwire-r4-60, where it is the default. This is the gold behind ripwire's
             published "strict file@10" (all primary files in the top 10) and "any@10".
  patch      edit_functions files UNION files touched by the fix patch (ripwire's "gold_files",
             recomputed here from the dataset rows; available for every subset)

--query:
  raw           problem_statement verbatim                                            [default]
  ripwire1200   " ".join(problem_statement.split())[:1200], the query every arm of ripwire's
                round 4 received (default for --subset ripwire-r4-60)

Run:
  uv run bench/scripts/datasets/load_locbench.py --count
  uv run bench/scripts/datasets/load_locbench.py --subset ripwire-r4-60 --extra
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

BENCH = Path(__file__).resolve().parents[2]
LB = BENCH / "data" / "locbench"
DEFAULT_ROWS = LB / "ripwire-r4-slice" / "rows_czlll__Loc-Bench_V1_test_560.json"
ROWS_SHA256 = "5bbcea4bff11396f38f8aca3e64d697a8ea1da2bc54d705da7f6e34886804c97"  # ripwire bench/locbench/dataset.lock
LEGACY_243 = LB / "ripwire" / "bench" / "locbench" / "results" / "r1_anchorhop" / "heldout_baseline_release.json"
DEFAULT_R4 = LB / "ripwire" / "bench" / "headtohead" / "r4-2026-08-06" / "results" / "r4_ripwire_for.json"
RIPWIRE_QUERY_CHARS = 1200


def ripwire_partition(repo: str) -> str:
    # bench/locbench/run_locbench.py frozen_partition(): salted, repository-disjoint 50/50 split.
    digest = hashlib.sha256(("ripwire-a7-v2\0" + repo.lower()).encode("utf-8")).digest()
    return "train" if digest[0] < 128 else "heldout"


def patch_files(patch: str) -> set[str]:
    # bench/locbench/run_locbench.py patch_files(), same two regexes.
    return set(re.findall(r"^\+\+\+ b/(.+)$", patch, re.M)) | set(re.findall(r"^--- a/(.+)$", patch, re.M))


def edit_files(row: dict) -> list[str]:
    return list(dict.fromkeys(e.split(":")[0] for e in row.get("edit_functions") or []))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rows", type=Path, default=DEFAULT_ROWS, help="frozen 560-row JSON snapshot")
    ap.add_argument("--r4", type=Path, default=DEFAULT_R4, help="ripwire r4_ripwire_for.json (defines the 60-slice)")
    ap.add_argument("--subset", default="all", choices=["all", "ripwire-heldout", "ripwire-train", "ripwire-heldout-243-legacy", "ripwire-r4-60"])
    ap.add_argument("--gold", default="auto", choices=["auto", "locagent", "ripwire", "patch"])
    ap.add_argument("--query", default="auto", choices=["auto", "raw", "ripwire1200"])
    ap.add_argument("--ids", default="", help="comma-separated instance ids to keep")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--extra", action="store_true", help='add an "extra" object')
    ap.add_argument("--count", action="store_true")
    ap.add_argument("--no-verify", action="store_true", help="skip the sha256 check of the rows snapshot")
    a = ap.parse_args()

    raw = a.rows.read_bytes()
    if not a.no_verify and a.rows == DEFAULT_ROWS:
        got = hashlib.sha256(raw).hexdigest()
        if got != ROWS_SHA256:
            raise SystemExit(f"rows snapshot sha256 mismatch: expected {ROWS_SHA256}, got {got}")
    rows = json.loads(raw)
    by_id = {r["instance_id"]: r for r in rows}
    if len(by_id) != len(rows):
        raise SystemExit("duplicate instance_id in rows snapshot")

    r4 = None
    if a.subset == "ripwire-r4-60":
        r4 = {i["instance_id"]: i for i in json.loads(a.r4.read_text(encoding="utf-8"))["instances"]}
        missing = [i for i in r4 if i not in by_id]
        if missing:
            raise SystemExit(f"r4 slice ids missing from the dataset: {missing[:5]}")
        sel = [by_id[i] for i in r4]
    elif a.subset == "ripwire-heldout-243-legacy":
        legacy = {i["instance_id"] for i in json.loads(LEGACY_243.read_text(encoding="utf-8"))["instances"]}
        if not legacy <= set(by_id):
            raise SystemExit("legacy 243 list has ids missing from the dataset")
        sel = [r for r in rows if r["instance_id"] in legacy]
    elif a.subset == "ripwire-heldout":
        sel = [r for r in rows if ripwire_partition(r["repo"]) == "heldout"]
    elif a.subset == "ripwire-train":
        sel = [r for r in rows if ripwire_partition(r["repo"]) == "train"]
    else:
        sel = rows

    gold_mode = a.gold if a.gold != "auto" else ("ripwire" if r4 else "locagent")
    query_mode = a.query if a.query != "auto" else ("ripwire1200" if r4 else "raw")
    if gold_mode == "ripwire" and not r4:
        raise SystemExit("--gold ripwire needs --subset ripwire-r4-60 (primary_files are only recorded for that slice); "
                         "use --gold patch for ripwire's un-filtered gold_files on other subsets")

    ids = [x for x in a.ids.split(",") if x]
    if ids:
        have = {r["instance_id"]: r for r in sel}
        missing = [i for i in ids if i not in have]
        if missing:
            raise SystemExit(f"id(s) not in subset {a.subset}: {missing}")
        sel = [have[i] for i in ids]
    if a.limit:
        sel = sel[: a.limit]
    if a.count:
        print(len(sel))
        return 0

    for r in sel:
        iid = r["instance_id"]
        la_files = edit_files(r)
        p_files = sorted(set(la_files) | patch_files(r.get("patch") or ""))
        if gold_mode == "locagent":
            gold = la_files
        elif gold_mode == "patch":
            gold = p_files
        else:
            gold = list(r4[iid]["primary_files"])
        ps = r.get("problem_statement") or ""
        query = " ".join(ps.split())[:RIPWIRE_QUERY_CHARS] if query_mode == "ripwire1200" else ps
        out = {
            "id": iid,
            "repo": r["repo"],
            "commit_or_version": r["base_commit"],
            "query_text": query,
            "gold_files": gold,
            "gold_functions": list(dict.fromkeys(r.get("edit_functions") or [])),
        }
        if a.extra:
            ex = {
                "category": r.get("category"),
                "created_at_ms": r.get("created_at"),
                "added_functions": r.get("added_functions") or [],
                "locagent_gold_files": la_files,
                "patch_gold_files": p_files,
                "ripwire_partition": ripwire_partition(r["repo"]),
                "gold_mode": gold_mode,
                "query_mode": query_mode,
                "patch": r.get("patch"),
            }
            if r4:
                ex["ripwire_primary_files"] = r4[iid]["primary_files"]
                ex["ripwire_gold_files"] = r4[iid]["gold_files"]
                ex["ripwire_for_arm"] = {k: r4[iid]["arms"]["for"].get(k) for k in ("file_first", "file_worst", "all_file_worst")}
            out["extra"] = ex
        sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
