#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["pyarrow>=17"]
# ///
"""ContextBench (arXiv 2602.05892) -> normalised JSONL on stdout.

One line per task:
  {"id", "repo", "commit_or_version", "query_text", "gold_files": [...], "gold_functions": [...]}

  id                 ContextBench instance_id, e.g. "SWE-Bench-Verified__python__maintenance__bugfix__8adb3e00"
  repo               "owner/name" taken from repo_url, NOT from the parquet's own "repo" column: that
                     column is truncated at the first hyphen for 88 rows of full / 34 of verified
                     (e.g. "grpc/grpc" for grpc/grpc-go, "fasterxml/jackson" for both jackson-core
                     and jackson-databind). The raw column is kept as extra.repo_column.
  commit_or_version  base_commit (40-hex sha)
  query_text         problem_statement verbatim ("" for the one row of full.parquet where it is null)
  gold_files         distinct files of the human-annotated gold_context spans, sorted
  gold_functions     ALWAYS EMPTY. ContextBench gold is line spans, not named functions; its evaluator
                     maps spans to definitions with tree-sitter at scoring time, inside a checkout.
                     The spans are in extra.gold_spans ({file, start_line, end_line}, 1-based inclusive).

Subsets (--subset):
  full               data/full.parquet, 1,136 instances
  verified           data/contextbench_verified.parquet, 500 instances                  [default]
  repowise-sealed42  the 42 sealed (held-out "test") instances of repowise's bake-off
  repowise-dev70     the 70 development instances of the same split
  repowise-112       both halves (all django/django and cli/cli instances of "verified")

Run:
  uv run bench/scripts/datasets/load_contextbench.py --count
  uv run bench/scripts/datasets/load_contextbench.py --subset repowise-sealed42 --extra
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

import pyarrow.parquet as pq

BENCH = Path(__file__).resolve().parents[2]
CB = BENCH / "data" / "contextbench"
UP = CB / "upstream" / "data"
SPLIT = CB / "repowise-bench" / "results" / "bakeoff_2026_08" / "dev_test_split.json"
COLS = ["instance_id", "original_inst_id", "repo", "repo_url", "language", "base_commit",
        "gold_context", "patch", "problem_statement", "source"]


def repo_from_url(url: str) -> str:
    m = re.fullmatch(r"https://github\.com/([^/]+)/([^/]+?)(?:\.git)?/?", url)
    if not m:
        raise SystemExit(f"unexpected repo_url {url!r}")
    return f"{m.group(1)}/{m.group(2)}"


def rel_path(p: str) -> str:
    """Gold spans of some sources carry the path inside the annotation container
    (/workspace/<repo>__<version>/pkg/x.go). The official evaluator drops leading components
    until a repository file matches. The container layout is fixed, so drop it here."""
    m = re.match(r"^/(?:workspace|testbed)/[^/]+/(.+)$", p)
    return m.group(1) if m else p.lstrip("/") if p.startswith("/home/") else p


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--subset", default="verified",
                    choices=["full", "verified", "repowise-sealed42", "repowise-dev70", "repowise-112"])
    ap.add_argument("--parquet", type=Path, default=None, help="override the parquet file")
    ap.add_argument("--split-file", type=Path, default=SPLIT, help="repowise dev_test_split.json")
    ap.add_argument("--language", default="", help="keep only this language (python, go, java, c, cpp, rust, javascript, typescript)")
    ap.add_argument("--ids", default="", help="comma-separated instance ids (or original_inst_id values) to keep")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--drop-unusable", action="store_true",
                    help="drop rows with an empty problem statement or no gold span (4 rows of full, 0 of verified)")
    ap.add_argument("--extra", action="store_true", help='add an "extra" object (spans, language, source, patch files)')
    ap.add_argument("--count", action="store_true")
    a = ap.parse_args()

    parquet = a.parquet or (UP / ("full.parquet" if a.subset == "full" else "contextbench_verified.parquet"))
    keep = None
    if a.subset.startswith("repowise-"):
        sp = json.loads(a.split_file.read_text(encoding="utf-8"))
        got = hashlib.sha256(parquet.read_bytes()).hexdigest()
        if a.parquet is None and got != sp["dataset_sha256"]:
            raise SystemExit(f"verified parquet sha256 {got} != repowise split's dataset_sha256 {sp['dataset_sha256']}")
        keep = {"repowise-sealed42": sp["test"], "repowise-dev70": sp["dev"],
                "repowise-112": sorted(sp["dev"] + sp["test"])}[a.subset]

    rows = pq.read_table(parquet, columns=COLS).to_pylist()
    by_id = {r["instance_id"]: r for r in rows}
    if len(by_id) != len(rows):
        raise SystemExit("duplicate instance_id in parquet")
    if keep is not None:
        missing = [i for i in keep if i not in by_id]
        if missing:
            raise SystemExit(f"split ids missing from parquet: {missing[:5]}")
        rows = [by_id[i] for i in keep]
    if a.language:
        rows = [r for r in rows if r["language"] == a.language]
    ids = [x for x in a.ids.split(",") if x]
    if ids:
        have = {r["instance_id"]: r for r in rows} | {r["original_inst_id"]: r for r in rows}
        missing = [i for i in ids if i not in have]
        if missing:
            raise SystemExit(f"id(s) not in subset {a.subset}: {missing}")
        rows = [have[i] for i in ids]
    if a.drop_unusable:
        rows = [r for r in rows if (r["problem_statement"] or "").strip() and json.loads(r["gold_context"])]
    if a.limit:
        rows = rows[: a.limit]
    if a.count:
        print(len(rows))
        return 0

    for r in rows:
        spans = json.loads(r["gold_context"])
        out = {
            "id": r["instance_id"],
            "repo": repo_from_url(r["repo_url"]),
            "commit_or_version": r["base_commit"],
            "query_text": r["problem_statement"] or "",
            "gold_files": sorted({rel_path(s["file"]) for s in spans}),
            "gold_functions": [],
        }
        if a.extra:
            out["extra"] = {
                "original_inst_id": r["original_inst_id"],
                "repo_url": r["repo_url"],
                "repo_column": r["repo"],
                "language": r["language"],
                "source": r["source"],
                "gold_spans": [{"file": s["file"], "start_line": s["start_line"], "end_line": s["end_line"]} for s in spans],
                "patch_files": sorted(set(re.findall(r"^diff --git a/(\S+) b/", r["patch"] or "", re.M))),
            }
        sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
