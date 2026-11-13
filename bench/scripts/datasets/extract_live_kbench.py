#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["pyarrow>=17"]
# ///
"""Extract slim, metadata-only JSONL for Live-kBench-2512 from the Hugging Face parquet files.

Input  : bench/data/live-kbench/hf-live-kbench/{bugs,datasets}/*.parquet
Output : bench/data/live-kbench/lkbench-2512.metadata.jsonl  (one line per bug, sorted by bugId)

Dropped on purpose (they are not needed for localisation and make up >95% of the bytes):
syz/C reproducers, the kernel .config, cause-commit function lists, the userspace image name.

Run:  uv run bench/scripts/datasets/extract_live_kbench.py [--src DIR] [--out FILE] [--dataset lkbench-2512]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import pyarrow.parquet as pq

BENCH = Path(__file__).resolve().parents[2]
DEFAULT_SRC = BENCH / "data" / "live-kbench" / "hf-live-kbench"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", type=Path, default=DEFAULT_SRC)
    ap.add_argument("--dataset", default="lkbench-2512", help="datasetName row in the 'datasets' config")
    ap.add_argument("--out", type=Path, default=None)
    a = ap.parse_args()
    out = a.out or (BENCH / "data" / "live-kbench" / f"{a.dataset}.metadata.jsonl")

    ds_rows = []
    for p in sorted((a.src / "datasets").glob("*.parquet")):
        ds_rows += pq.read_table(p).to_pylist()
    match = [r for r in ds_rows if r["datasetName"] == a.dataset]
    if len(match) != 1:
        raise SystemExit(f"dataset {a.dataset!r} not found; have {[r['datasetName'] for r in ds_rows]}")
    wanted = json.loads(match[0]["bugIds"])
    if len(wanted) != len(set(wanted)):
        raise SystemExit("duplicate bug ids in the dataset row")
    wanted_set = set(wanted)

    recs = {}
    for p in sorted((a.src / "bugs").glob("*.parquet")):
        pf = pq.ParquetFile(p)
        for batch in pf.iter_batches(batch_size=32):
            for r in batch.to_pylist():
                if r["bugId"] not in wanted_set:
                    continue
                d = json.loads(r["syzbotData"])
                crash = (d.get("crashes") or [{}])[0]
                files = d.get("patchModifiedFiles") or []
                funcs = d.get("patchModifiedFunctions") or []
                recs[r["bugId"]] = {
                    "bugId": r["bugId"],
                    "extid": r["extid"],
                    "title": r["title"],
                    "status": r["status"],
                    "reportedTime": r["reportedTime"],
                    "subsystems": d.get("subsystems") or json.loads(r["subsystem"] or "[]"),
                    "syzbotUrl": f"https://syzkaller.appspot.com/bug?id={r['bugId']}",
                    # the text the kArena agent invoker hands to the agent (kArena/crawler.py: rawCrashReport)
                    "crashReport": r["syzbotCrashReport"],
                    "rawCrashReport": d.get("rawCrashReport"),
                    "crashTitle": crash.get("title"),
                    "crashKernelCommit": crash.get("kernelSourceCommit"),
                    "crashKernelSourceGit": crash.get("kernelSourceGit"),
                    "architecture": crash.get("architecture"),
                    "fixCommits": d.get("fixCommits") or [],
                    "parentOfFixCommit": d.get("parentOfFixCommit"),
                    "patchCommitDate": d.get("patchCommitDate"),
                    "causeCommit": d.get("causeCommit"),
                    "causeCommitDate": d.get("causeCommitDate"),
                    "patch": d.get("patch"),
                    "patchMessage": d.get("patchMessage"),
                    "patchModifiedFiles": files,
                    "patchModifiedFunctions": funcs,
                    "discussions": d.get("discussions") or [],
                }
    missing = sorted(wanted_set - set(recs))
    if missing:
        raise SystemExit(f"{len(missing)} bug ids of {a.dataset} are not in the bugs parquet, e.g. {missing[:3]}")

    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8") as fh:
        for bid in sorted(recs):
            fh.write(json.dumps(recs[bid], ensure_ascii=False, sort_keys=True) + "\n")
    digest = hashlib.sha256(out.read_bytes()).hexdigest()
    print(f"wrote {out}  rows={len(recs)}  bytes={out.stat().st_size}  sha256={digest}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
