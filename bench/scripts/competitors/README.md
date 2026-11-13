# Competitor benchmark scripts

Install notes, versions, checksums, smoke-test results and caveats: `bench/tools/README.md`.

```
index_<tool>.sh REPO_DIR OUT_JSON                      # build a fresh index, write one JSON measurement
query_<tool>.sh REPO_DIR def|callers|callees|locate ARG   # print the tool's raw output
gortex_daemon.sh start|stop|status REPO_DIR            # keep gortex warm across many queries
sbx.sh CMD ARGS...                                     # run anything in the sandbox environment
```

`<tool>`: `ripwire`, `codebase-memory-mcp`, `codegraph`, `semcode`, `gortex`, `ctags`, `cscope`, `global`,
`tilth`, `ast-grep`.

- All scripts are bash and take the repository as an argument. Nothing is hard-coded to one repo.
- Every tool runs with an empty environment and `HOME=bench/tools/home` (see `_common.sh`).
- Benchmark repositories are never written to. CodeGraph and gortex work on a throwaway APFS clone under
  `bench/tools/_work/<tool>/copy-<key>`. The others read the repo and write their index elsewhere.
- `BENCH_JOBS` (default 6) caps threads where the tool allows it. ripwire and tilth cannot be capped.
- Query exit codes: tool status, `2` bad arguments, `3` query kind not supported by the tool, `4` no index.
- The codebase-memory-mcp index and the gortex daemon socket are under `/private/tmp/codefast-bench-<uid>`
  (override with `BENCH_SHORT_ROOT`). That directory does not survive a reboot.

Helper files: `_common.sh` (sandbox, measurement, file list), `_write_json.py`, `_gortex.sh`,
`_gortex_ids.py`, `_gortex_json.py`, `_astgrep_rules.py`.

Example:

```bash
bench/scripts/competitors/index_ripwire.sh bench/repos/django /abs/path/out/ripwire.django.json
bench/scripts/competitors/query_ripwire.sh bench/repos/django callers get_object_or_404
```
