# LinuxFLBench

Fetched 2026-11-12. Re-fetch with `bench/scripts/datasets/fetch_datasets.sh linuxflbench`
(pinned revision, anonymous, then checks `../SHA256SUMS`).

## What is here

| Path | Source | Revision | Licence |
|---|---|---|---|
| `upstream/` | https://github.com/FudanSELab/LinuxFLBench (dataset, LinuxFL+ code, baseline results) | commit `de65c3958f264137835eb23cacf3591fb89fc09b` (2025-06-20) | MIT (`upstream/LICENSE`, "Copyright (c) 2025 queserasera22") |
| `functions_absent_at_version.json` | derived here with `check_kernel_gold.py` (see below) | | |

Paper: arXiv 2505.19489 (ACL 2026).

Dataset file: `upstream/dataset/LINUXFLBENCH_dataset.jsonl`, 250 lines,
sha256 `e27551b78d317e36cb8f1ef79981e1aac5332b46c470a16718cdebe4292c40a2`.

Not downloaded (Google Drive links in the upstream README, sizes unknown):

- kernel source snapshots for all versions: `https://drive.google.com/uc?export=download&id=18FaxpKbbs8f3Ys79fadRkdElUkmeoWkm`
- LKML mail data used by LinuxFL+: `https://drive.google.com/uc?export=download&id=1Hdhu4WTVMxWk3ixJ4w2Wsm9ATHdQrxuN`

Kernel trees come from git tags and kernel.org tarballs instead (next section). Whether those equal
the Drive snapshots file for file was **not checked**.

## The dataset

Each line is one kernel Bugzilla report with its fix. Fields used: `id`, `title`, `description`,
`Kernel Version`, `patch` (list of strings), `paths` (gold files), `methods` (gold function names).
The rest is Bugzilla metadata (`Status`, `Product`, `Component`, `Hardware`, `Reported`, ...).

- 250 tasks, 250 distinct ids, 120 distinct kernel versions from 2.4.26 to 5.6.7.
- Exactly one gold file per task: 248 `.c`, 2 `.h`. Top directory: drivers 182, arch 20, fs 16,
  net 10, sound 10, kernel 5, mm 3, block 2, include 1, init 1.
- 472 gold function names: 173 tasks have one, 38 have two, 39 have three or more.
- Reports are from 2003 to 2020 (211 are from 2011 or earlier).
- Upstream metric (`upstream/code/eval/evaluation_metrics.py`): Recall@k is `|gold ∩ top-k| / |gold|`
  averaged over tasks (with one gold file this is hit@k). MRR uses the first gold hit.

## Where each kernel version comes from

| Source | Versions | Tasks |
|---|---|---|
| mainline release tag in torvalds/linux (`v2.6.11` ... `v5.1`) | 39 | 153 |
| mainline release-candidate tag (`v2.6.18-rc3` ... `v5.6-rc2`) | 48 | 55 |
| stable tag, not in torvalds/linux, fetched from the stable tree | 20 | 21 |
| older than git (2.4.26, 2.4.32, 2.5.67, 2.5.68, 2.5.75, 2.6.0, 2.6.1, 2.6.2, 2.6.5, 2.6.6, 2.6.7, 2.6.8, 2.6.9), kernel.org tarball | 13 | 21 |

- `v2.6.11` is in git only as a tag on a tree object (no commit). `prepare_tree.sh` handles it.
- Stable tags: `https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git` (mirror
  `https://github.com/gregkh/linux.git`). All 20 were fetched into `bench/repos/linux-full` on
  2026-11-12 (one fetch, 28 s, about 190 MB).
- Tarballs: `https://cdn.kernel.org/pub/linux/kernel/v2.4/`, `v2.5/`, `v2.6/` as `linux-<version>.tar.xz`
  (about 24 to 30 MB each), with `sha256sums.asc` in the same directory. `prepare_tree.sh` downloads
  one only when a task on that version is requested. Only `linux-2.4.26.tar.xz` has been downloaded
  (sha256 `7ee24cc37f9efbd739cf49e288c1b910321ddbb252118f8bcb3c0877ccc4ddf7`, equal to the listed value.
  The PGP signature of `sha256sums.asc` was not checked).

## Loader

```
uv run bench/scripts/datasets/load_linuxflbench.py --count          # 250
uv run bench/scripts/datasets/load_linuxflbench.py --ids 207835 --extra
```

`commit_or_version` is the `Kernel Version` string. `query_text` is the title, a blank line, then the
description. `gold_functions` are `"<gold file>:<method>"`. Verified 2026-11-12: 250 tasks,
250 gold files, 472 gold functions, no empty query, median query length 1,252 characters.

## Gold checked against git (229 of 250 tasks)

```
uv run bench/scripts/datasets/load_linuxflbench.py | uv run bench/scripts/datasets/check_kernel_gold.py --details details.jsonl
jq -c 'select(.missing_functions|length>0) | {(.id): .missing_functions}' details.jsonl | jq -s add > functions_absent_at_version.json
```

- All 229 tasks whose version is in git resolve to a tag, and all 229 gold files exist at that tag.
- 9 of their 433 gold function names (8 tasks) do not occur in the gold file at that version, mostly
  functions or tables that the fix introduces. They are listed in `functions_absent_at_version.json`.
  `load_linuxflbench.py --findable-only` drops them (5 tasks are then left with no gold function).
- No gold file is one of the kernel's case-colliding paths.
- The 21 tasks on pre-git versions were not checked this way, except task 2567 (tree materialised below).

## Problems found

1. The dataset is old-kernel heavy: 209 of 250 tasks are on 2.x kernels. It is a test of localisation
   in a 12,000 to 67,000 file tree, not in today's 97,000 file tree.
2. Some "methods" are not functions (`video_detect_dmi_table`, `ata_device_blacklist` and
   `iwl_dev_info_table` are data tables). They are kept as upstream labels them.
3. One method name carries a stray `*` (`*ocfs2_get_write_source`, task 8838). The loader strips it.
4. 53 rows have a field whose key is the empty string, and 19 have `Duplicates (n)` keys. Ignored.
5. The report text (title plus description) contains the gold file's basename in only 5 tasks and a
   gold function name in 32, so plain keyword matching on the report rarely names the answer.
6. The `Kernel Version` is what the reporter typed. Nothing guarantees the bug is present, or the
   fix absent, at exactly that tag.
7. Kernel trees lose files on a case-insensitive file system (22 in v2.6.27-rc2, 13 in v5.6.7, 12 in
   2.4.26). See `bench/scripts/datasets/README.md`.

## Example trees (materialised and verified 2026-11-12)

| Task | Version | Source | Tree | Files on disk / in source | Gold |
|---|---|---|---|---|---|
| 2567 | 2.4.26 | kernel.org tarball | `bench/repos/_trees/linux@v2.4.26` (182 MB) | 12,556 / 12,568 | `drivers/acpi/pci_link.c` present, `acpi_pci_link_allocate` found |
| 11568 | 2.6.27-rc2 | tag in torvalds/linux, commit `0967d61ea0d8e8a7826bd8949cd93dd1e829ac55` | `bench/repos/_trees/linux@v2.6.27-rc2` (314 MB) | 24,230 / 24,252 | `arch/x86/kernel/acpi/sleep.c` present, `acpi_save_state_mem` found |
| 207835 | 5.6.7 | stable tag, commit `55b2af1c23eb12663015998079992f79fdfa56c8` | `bench/repos/_trees/linux@v5.6.7` (1.0 GB) | 67,328 / 67,341 | `drivers/acpi/video_detect.c` present, `video_detect_dmi_table` found |

Every missing file is one of a pair of names that differ only by case (listed in the sidecars under
`bench/repos/_trees/.meta/`). For the two git-sourced trees every other file was re-hashed and equals
its git blob.
