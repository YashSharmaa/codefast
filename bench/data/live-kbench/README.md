# Live-kBench-2512 (metadata only)

Fetched 2026-11-12. Re-fetch with `bench/scripts/datasets/fetch_datasets.sh live-kbench`
(pinned revisions, anonymous, then checks `../SHA256SUMS`).

No kernel images, builds, reproducer runs or syzbot pages were fetched. Everything below comes from
the published dataset files.

## What is here

| Path | Source | Revision | Licence |
|---|---|---|---|
| `hf-live-kbench/` | https://huggingface.co/datasets/live-kbench/live-kbench (`bugs/train-0000{0,1}-of-00002.parquet`, `datasets/train-00000-of-00001.parquet`, `README.md`) | dataset commit `e42003c93fe2c7da335d18252cb11823931445f6`, last modified 2026-05-07 | MIT (dataset card) |
| `upstream-code/` | https://github.com/ARiSE-Lab/live-kbench (kArena, kGym, agent configs, reference only) | commit `c9d1e6cd6cf9b87d2a37eb1189ad72cfe0753514` (2026-09-14) | **no licence file and none stated**. Read it, do not copy from it |
| `lkbench-2512.metadata.jsonl` | derived here by `bench/scripts/datasets/extract_live_kbench.py` | 534 lines, 9,085,058 bytes, sha256 `c44d96070e2cfc594bfc562f912db20a731991ae39821380bc00b96dc879b298` | MIT data |
| `lkbench-2512.functions_absent_at_parent.json` | derived here with `check_kernel_gold.py` (see below) | 46 bugs, 58 functions | |

Paper: arXiv 2602.02690 (ICML 2026). Leaderboard: https://karena.io

The two `bugs` parquet files are 116,656,994 and 113,835,344 bytes. Their sha256 values equal the LFS
object ids Hugging Face reports. They can be deleted after the extract step (222 MB). Everything the
loader needs is in the 9 MB JSONL.

## The dataset

- `datasets` config: one row, `datasetName = "lkbench-2512"`, description "Live-kBench 2025-12",
  with a JSON list of 534 bug ids.
- `bugs` config: 1,451 syzbot bugs (899 fixed, 552 open). All 534 bugs of lkbench-2512 are present
  and fixed.
- Per bug: `bugId`, `extid`, `title`, `status`, `subsystem`, `syzbotCrashReport`, `syzbotReproducer`,
  syzkaller commit, and `syzbotData`, a JSON document with the fix commit(s), `parentOfFixCommit`,
  the developer `patch`, `patchModifiedFiles`, `patchModifiedFunctions` (one list per file), the crash
  (kernel commit and tree, `.config`, syz and C reproducers) and the cause commit.

`lkbench-2512.metadata.jsonl` keeps, per bug: ids and title, subsystems, the crash report text, the
crash kernel commit and tree, fix commits, parent of fix commit, commit dates, the patch and its
message, modified files and functions, discussion links. It drops reproducers, kernel configs and
cause-commit function lists.

Facts measured on the 534 bugs:

- Fix commit dates run from 2024-06-25 to 2025-12-17. `reportedTime` runs from 2024-06-25 to
  2025-12-24 and carries the same time of day for every bug, so it is accurate to the day at best.
- Every fix commit is in `torvalds/linux` master. 526 bugs list one fix commit and 8 list two. Gold is
  taken from the first only. For all 534, `parentOfFixCommit` is the first parent of the first fix
  commit (checked in `bench/repos/linux-full`).
- 493 distinct parent commits. All are in `linux-full`, so no extra fetch is needed.
- Gold: 817 files (429 bugs touch one file, 105 more than one, mean 1.53) and 1,745 functions.
  8 bugs have no gold function.
- Crash commit trees: torvalds 384, linux-next 62, net-next 39, net 21, bpf 10, usb 10, bpf-next 8.
  359 of the 411 distinct crash commits are in `linux-full` (469 of 534 bugs). The rest live only in
  those other trees.
- `syzbotCrashReport` equals `syzbotData.rawCrashReport` for all 534. Median length 4,100 characters.
- Architecture is amd64 for all.

How the paper uses it: agents are started in the tree at `parentOfFixCommit` (falling back to the
crash commit) and given the crash report (`upstream-code/kArena/crawler.py`). Localisation is scored
as file IoU and function IoU between the files and functions the agent's patch modifies and
`patchModifiedFiles` / `patchModifiedFunctions`. Neither needs a kernel build.

## Loader

```
uv run bench/scripts/datasets/load_live_kbench.py --count        # 534
uv run bench/scripts/datasets/load_live_kbench.py --limit 3 --extra
uv run bench/scripts/datasets/load_live_kbench.py --findable-only
```

`commit_or_version` is `parentOfFixCommit` (`--commit crash` gives the crash commit). `query_text`
is the bug title, a blank line, and the crash report (`--no-title` for the report alone).
`gold_functions` are `"<file>:<function>"`. Verified 2026-11-12: 534 tasks, 817 gold files,
1,745 gold functions (1,687 with `--findable-only`), no empty query.

## Gold checked against git (all 534 tasks)

```
uv run bench/scripts/datasets/load_live_kbench.py | uv run bench/scripts/datasets/check_kernel_gold.py --details details.jsonl
jq -c 'select(.missing_functions|length>0) | {(.id): .missing_functions}' details.jsonl | jq -s add > lkbench-2512.functions_absent_at_parent.json
```

- All 534 parent commits resolve, and all 817 gold files exist at the parent commit.
- 58 gold functions in 46 bugs do not occur in their file at the parent commit. Every one of them
  appears on an added line of the fix patch: they are functions the fix creates. No tool can find them
  in the parent tree. `--findable-only` drops them (9 bugs are then left with no gold function).
- One gold file is lost on a case-insensitive file system: `net/netfilter/xt_RATEEST.c` in bug
  `8684da9ee723bbdf23bf7c8dd27a3ad59435bbe9` (its tree holds `xt_rateest.c` instead, a different file).

## Problems found

1. Bug `a9807a63ad00...` has an empty `patchModifiedFiles`. Its first "fix commit" (`9aa8fe29f624`) is
   a merge commit. The stored patch is the merge diff and touches `kernel/bpf/verifier.c` and two
   selftest programs. The loader falls back to the paths in the patch's `diff --git` lines and marks
   the row `gold_source: patch_diff_headers` under `--extra`.
2. Gold functions include functions the fix adds (58, see above), as upstream computes them with
   pydriller's `changed_methods`.
3. `patchModifiedFiles` is every file the fix touches: 682 `.c`, 103 `.h`, 26 files with no extension
   (Kconfig, Makefile, MAINTAINERS) and 3 `.rst` (6 files are under `tools/` or `Documentation/`).
   They are all kept as gold.
4. Gold mean is 1.53 files per bug here. The paper says 1.48. The difference was not explained.
5. The paper says bugs were reported from April 2024. The earliest `reportedTime` here is 2024-06-25.
6. 24 parent commits are shared by more than one bug (65 bugs between them), so tasks are not all
   independent at the tree level.
7. The crash happened on the crash commit, not on the parent of the fix. Line numbers in the crash
   report refer to the crash commit's tree, which is a different mainline or `-next` snapshot.
8. A full kernel tree is about 1.6 GB and 87,000 to 90,000 files. All 493 trees would need about 790 GB.
   Materialise them a few at a time and delete them after use.
9. The code repository has no licence.

## Example trees (materialised and verified 2026-11-12)

| Bug | Parent commit | Tree | Files on disk / in git | Gold |
|---|---|---|---|---|
| `7daff104e6f71e7dd9aa016f37e7efe0ffb63a6b` "kernel BUG in clear_inode (4)" | `869833f54e8306326b85ca3ed08979b7ad412a4a` (v6.17-rc1+34) | `bench/repos/_trees/linux@869833f54e8306326b85ca3ed08979b7ad412a4a` | 90,485 / 90,498 | 1 of 1 files, 1 of 1 functions |
| `027131a7eada3eb9ffc54819b87516b79c08fd44` "KASAN: slab-use-after-free Read in l2cap_connect (2)" | `f53e1c9c726d83092167f2226f32bd3b73f26c21` (v6.11+11504) | `bench/repos/_trees/linux@f53e1c9c726d83092167f2226f32bd3b73f26c21` | 86,645 / 86,658 | 3 of 3, 3 of 3 |
| `8684da9ee723bbdf23bf7c8dd27a3ad59435bbe9` "WARNING in xt_cluster_mt (2)" | `983e35ce2e1ee4037f6f5d5398dfc107b22ad569` (v6.12-rc1+178) | `bench/repos/_trees/linux@983e35ce2e1ee4037f6f5d5398dfc107b22ad569` | 86,658 / 86,671 | 15 of 16 files really present (`xt_RATEEST.c` is the case-collision victim), 20 of 22 functions |

Each tree is 1.6 GB and took about 25 s to write and verify. The 13 files missing from each are the
case-colliding ones. Every other file was re-hashed and equals its git blob.
