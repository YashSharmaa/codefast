# Competitor tools: sandboxed installs and benchmark scripts

Installed and smoke-tested on 2026-11-12, macOS 27.2 arm64 (Apple M5 Max, 18 cores, 36 GB).
The machine was under heavy unrelated load during the measurements in this file (load average
between 10 and 70), so the timings below are indicative only. The kernel index times quoted
in the top level README come from a later run of the same scripts with the thread cap lifted
(`BENCH_JOBS=18`), recorded in `bench/results/k0/`.

- Tools live in `bench/tools/<name>/`.
- Scripts live in `bench/scripts/competitors/`.
- Captured smoke-test outputs live in `bench/tools/_smoke/`.

## 1. Sandbox rules that the scripts enforce

Every tool command runs through `sbx` (`bench/scripts/competitors/_common.sh`, or the wrapper
`bench/scripts/competitors/sbx.sh CMD ARGS...`):

- `env -i`: the command starts with an empty environment. This matters because a shell may export
  agent configuration variables such as `CLAUDE_CONFIG_DIR`, and the installers or `install` subcommands of ripwire,
  codebase-memory-mcp, CodeGraph, gortex and tilth write agent config (skills, hooks, MCP entries) into
  whatever agent directories they can find.
- `HOME=bench/tools/home`, with `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME` and
  `TMPDIR` inside it.
- `PATH` = the sandbox tool directories, then `/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin`.
- Telemetry off: `DO_NOT_TRACK=1`, `CODEGRAPH_TELEMETRY=0`, `CODEGRAPH_NO_UPDATE_CHECK=1`,
  `GORTEX_TELEMETRY=0`. `codegraph telemetry off` was also persisted in the sandbox HOME.
  `codegraph telemetry` reports "disabled", `gortex telemetry status` reports "disabled (decided by: env)"
  and "Ingest endpoint: not configured". codebase-memory-mcp states it collects none. ripwire, semcode,
  tilth, ast-grep, ctags, cscope and GNU Global document no telemetry.
- No installer script was executed. Each tool was installed from a release tarball (checksum compared
  with the project's published checksum file), from source with cargo, or with Homebrew.
- No `install` / `init` / `wrap` subcommand of any tool was run (those are the ones that edit agent config).

Two things could not live under `bench/tools` and go to `/private/tmp/codefast-bench-<uid>/` (mode 700)
instead. Override with `BENCH_SHORT_ROOT`. That directory is wiped on reboot.

1. codebase-memory-mcp refuses any cache directory that has a world-writable ancestor, and
   the benchmark directory had one here. With `CBM_CACHE_DIR` under `bench/tools` it fails with
   `could not load saved runtime settings`. Its index is therefore in
   `/private/tmp/codefast-bench-<uid>/cbm/<repo-key>/` and must be rebuilt after a reboot.
2. The gortex daemon socket: unix socket paths are limited to 103 bytes on macOS. Only the socket, pid
   file and logs are there. The gortex store itself stays under `bench/tools/_work/gortex/`.

Other environment variables the scripts honour: `BENCH_TOOLS_DIR`, `BENCH_WORK_DIR` (default
`bench/tools/_work`), `BENCH_JOBS` (default 6).

## 2. What was installed

| Tool | Version | How | Binary |
|---|---|---|---|
| ripwire | 0.6.10 (`built_from=ac869640e`) | release tarball | `ripwire/bin/ripwire` |
| codebase-memory-mcp | 0.12.1 | release tarball | `codebase-memory-mcp/bin/codebase-memory-mcp` |
| CodeGraph | 1.6.2 (bundled Node) | release tarball | `codegraph/bin/codegraph` (symlink into `codegraph/pkg/`) |
| semcode | 0.1.1 at git `e63a1e179c7a` (2026-11-11) | `cargo build --release --locked` | `semcode/bin/{semcode,semcode-index,semcode-mcp,semcode-lsp}` |
| gortex | v0.64.11+c7abb9b | release tarball | `gortex/bin/gortex` |
| tilth | 0.14.0 | `cargo install --locked --root` | `tilth/root/bin/tilth` |
| universal-ctags | 6.2.1 | Homebrew | `/opt/homebrew/bin/ctags`, `readtags` |
| cscope | 15.9 | Homebrew | `/opt/homebrew/bin/cscope` |
| GNU Global | 6.7 | Homebrew | `/opt/homebrew/bin/gtags`, `global` |
| ast-grep | 0.51.0 | already present in Homebrew | `/opt/homebrew/bin/ast-grep` |
| protobuf | 36.2 | Homebrew (build dependency of semcode) | `/opt/homebrew/bin/protoc` |

Nothing was skipped. All ten tools installed and ran.

### Exact commands

`S` is `bench/scripts/competitors/sbx.sh`, `T` is `bench/tools`. The GitHub API returned 403 (rate
limit) for anonymous calls, so release tags and asset lists were read from
`https://github.com/<repo>/releases/latest` (redirect) and `.../releases/expanded_assets/<tag>`.

```bash
# Homebrew (run with the sandbox HOME; nothing already installed was upgraded)
HOME=$T/home HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ANALYTICS=1 HOMEBREW_NO_INSTALL_CLEANUP=1 \
HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1 brew install universal-ctags cscope global protobuf
# new formulae: universal-ctags 6.2.1, cscope 15.9, global 6.7, protobuf 36.2, jansson 2.15.1, ncurses 6.6

# ripwire
cd $T/ripwire/dl
B=https://github.com/redhat-et/ripwire/releases/download/v0.6.10
$S curl -fsSL -O $B/ripwire-0.6.10-macos-arm64.tar.gz -O $B/ripwire-0.6.10-macos-arm64.tar.gz.sha256
shasum -a 256 ripwire-0.6.10-macos-arm64.tar.gz      # compare with the .sha256 file
tar -xzf ripwire-0.6.10-macos-arm64.tar.gz -C ../pkg && cp ../pkg/ripwire-0.6.10-macos-arm64/ripwire ../bin/

# codebase-memory-mcp
cd $T/codebase-memory-mcp/dl
B=https://github.com/DeusData/codebase-memory-mcp/releases/download/v0.12.1
$S curl -fsSL -O $B/codebase-memory-mcp-darwin-arm64.tar.gz -O $B/checksums.txt
tar -xzf codebase-memory-mcp-darwin-arm64.tar.gz -C ../pkg && cp ../pkg/codebase-memory-mcp ../bin/

# CodeGraph
cd $T/codegraph/dl
B=https://github.com/colbymchenry/codegraph/releases/download/v1.6.2
$S curl -fsSL -O $B/codegraph-darwin-arm64.tar.gz -O $B/SHA256SUMS
tar -xzf codegraph-darwin-arm64.tar.gz -C ../pkg
ln -sf ../pkg/codegraph-darwin-arm64/bin/codegraph ../bin/codegraph
$S codegraph telemetry off

# gortex
cd $T/gortex/dl
B=https://github.com/zzet/gortex/releases/download/v0.64.11
$S curl -fsSL -O $B/gortex_darwin_arm64.tar.gz -O $B/checksums.txt
tar -xzf gortex_darwin_arm64.tar.gz -C ../pkg && cp ../pkg/gortex ../bin/

# semcode (8 min 31 s with 6 jobs; peak rustc RSS 2.9 GB)
cd $T/semcode && $S git clone https://github.com/facebookexperimental/semcode.git src   # e63a1e179c7a
cd src && SBX_EXTRA_ENV="CARGO_HOME=$T/cargo-home RUSTUP_HOME=$HOME/.rustup \
  RUSTUP_TOOLCHAIN=stable-aarch64-apple-darwin CARGO_TARGET_DIR=$T/semcode/target CARGO_BUILD_JOBS=6 \
  LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib \
  PATH=$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  $S cargo build --release --locked
cp $T/semcode/target/release/{semcode,semcode-index,semcode-mcp,semcode-lsp} $T/semcode/bin/

# tilth (59 s)
SBX_EXTRA_ENV="CARGO_HOME=$T/cargo-home RUSTUP_HOME=$HOME/.rustup \
  RUSTUP_TOOLCHAIN=stable-aarch64-apple-darwin CARGO_TARGET_DIR=$T/tilth/target CARGO_BUILD_JOBS=6 \
  PATH=$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  $S cargo install tilth --version 0.14.0 --locked --root $T/tilth/root
```

`$HOME/.rustup` in the two cargo commands is the real rustup directory, read only (existing toolchain
rustc 1.99.1). Crates were downloaded to `bench/tools/cargo-home`, not to `~/.cargo`. The two cargo
target directories were deleted after the binaries were copied out.

### SHA-256

Release archives. Each matched the checksum file published in the same release. That proves the
download is intact, not who built it. The gortex release also carries cosign signatures and the
codebase-memory-mcp release carries sigstore bundles: neither was verified (cosign is not installed).

```
0a75a214477cd6e294fa67c93d9cde7aa05031bf94720c25fb425ead6391c8df  ripwire-0.6.10-macos-arm64.tar.gz
9bfcbe38414b47969a3e1d1c1999f5357409387c86fa3d24221b5458267660d2  codebase-memory-mcp-darwin-arm64.tar.gz
d74d1bfb4060db63ec3c2b72c4e17f76c31978af3bad6ace4370d1501ac0662e  codegraph-darwin-arm64.tar.gz
c53042ce268622299bdb2c36c354bb90caff0b0f1982774d13ed8e08f5e3b1c1  gortex_darwin_arm64.tar.gz
```

Installed binaries:

```
98c20173df464b7606333a1ba1de8d8ef97f67716b2ebd422106ea03351542a5  ripwire/bin/ripwire
a36cfa1f5d10d8c4762501d63feef082711aae4704b9e8f9066db52779a8601e  codebase-memory-mcp/bin/codebase-memory-mcp
1ee75375e33b94fc34b3b19aede049e11dae90efb63b374dc96d6bdace70c4b8  codegraph/pkg/codegraph-darwin-arm64/node
d40ab8b9d84fd839ecef70a9cfe93aa34073e5d46673f0a9d077d3a509540eda  gortex/bin/gortex
4af7746f46beeed182be590accb84c78053c6d9ca388e43cf78012a0caf1a9c4  tilth/root/bin/tilth
6eeb4585a2bafb65dbdc63c3c2e8e50b52a1bab2efe72d68cd8000a6f5a9a2c0  semcode/bin/semcode
86653b8a12e9402b41ae464db2192a6f66088548a3af870f71f2cac6ac497e5a  semcode/bin/semcode-index
eaf5297ee9263cccc90b0cfe1eb9f42cd978f43c553dd78dbbc3479343c57782  semcode/bin/semcode-lsp
f1a882598045e9fb27463e824ea169a8095b20944ac5ede4a6118a0d8b560984  semcode/bin/semcode-mcp
```

The semcode and tilth hashes are of locally built binaries and will differ on a rebuild.

## 3. Scripts

```
bench/scripts/competitors/index_<tool>.sh REPO_DIR OUT_JSON
bench/scripts/competitors/query_<tool>.sh REPO_DIR def|callers|callees|locate ARG
```

`<tool>` is one of `ripwire`, `codebase-memory-mcp`, `codegraph`, `semcode`, `gortex`, `ctags`, `cscope`,
`global`, `tilth`, `ast-grep`.

`index_<tool>.sh` wipes any previous index for that repo, builds a new one and writes one JSON object.
The tool's own output goes to `OUT_JSON.log`.

| JSON field | Meaning |
|---|---|
| `wall_seconds`, `user_seconds`, `sys_seconds` | from `/usr/bin/time -l` around the index command only (not the clone, not daemon start) |
| `peak_rss_bytes` | the larger of the next two. `peak_rss_source` says which |
| `peak_rss_bytes_time_l` | `maximum resident set size` from `/usr/bin/time -l`: the largest single process the command waited for |
| `peak_rss_bytes_tree_sampled` | for daemon-based tools: maximum over 0.2 s samples of the summed RSS of the tool's processes and their descendants. Can miss short spikes |
| `index_bytes`, `index_disk_bytes` | apparent and allocated size of `index_paths` |
| `exit_status` | exit status of the index command |
| `threads_requested` | the cap that was passed (`BENCH_JOBS`), `1` for single-threaded tools, or `"uncapped"` when the tool has no knob |
| `command`, `extra_env`, `version`, `repo`, `repo_git_files`, `host`, `timestamp` | provenance |

`query_<tool>.sh` prints the tool's raw output on stdout. Exit codes: the tool's own status, `2` for bad
arguments, `3` when the tool has no such query (`UNSUPPORTED: ...` on stderr), `4` when there is no index.

A repo is identified by `<basename>-<8 hex of sha256(absolute path)>`, for example `django-52f09f72`.

### Where each index goes, and whether the repo is touched

| Tool | Index location | Runs on | Writes into the repo? |
|---|---|---|---|
| ripwire | `_work/ripwire/<key>/idx.{lean,rich}.ripwirecache` | the repo, read only | No. Its default cache is in `TMPDIR` |
| codebase-memory-mcp | `/private/tmp/codefast-bench-<uid>/cbm/<key>/` | the repo, read only | No, with `persistence=false` (the default) |
| CodeGraph | `_work/codegraph/copy-<key>/.codegraph/` | a throwaway APFS clone | Yes: `.codegraph/` always, plus `post-commit`, `post-merge`, `post-checkout` git hooks when the watcher is off. The script removes the hooks from the clone |
| semcode | `_work/semcode/<key>/.semcode.db/` | the repo, read only | Not with `-d`. The default is `REPO/.semcode.db` |
| gortex | `_work/gortex/<key>/store/store.sqlite` | a throwaway APFS clone | None seen, but the daemon has a file watcher and edit tools, so a clone is used |
| ctags | `_work/ctags/<key>/tags` | the repo, read only | No |
| cscope | `_work/cscope/<key>/cscope.out{,.in,.po}` | the repo, read only | No |
| GNU Global | `_work/global/<key>/db/{GTAGS,GRTAGS,GPATH}` | the repo, read only | No |
| tilth | none | the repo, read only | No |
| ast-grep | none | the repo, read only | No |

`git status --porcelain --ignored` on `bench/repos/django` was empty after every run.

### Query mapping

| Tool | def | callers | callees | locate |
|---|---|---|---|---|
| ripwire | `ripwire REPO --cache=IDX.lean.ripwirecache --defs=ARG` | `... --callers=ARG` | `... --callees=ARG` | `ripwire REPO --cache=IDX.rich.ripwirecache --for=ARG` |
| codebase-memory-mcp | `cli --quiet search_graph --project KEY --name-pattern '^ARG$'` | `cli --quiet trace_path --project KEY --function-name ARG --direction inbound --depth 1 --include-tests true` | same with `--direction outbound` | `cli --quiet search_graph --project KEY --query ARG` |
| CodeGraph | `codegraph query ARG -p COPY --limit 10` | `codegraph callers ARG -p COPY --limit 100` | `codegraph callees ARG -p COPY --limit 100` | `codegraph explore -p COPY ARG` |
| semcode | `semcode -d IDX --git-repo REPO -q "func ARG"` | `-q "callers ARG"` | `-q "calls ARG"` | `-q "grep ARG"` (regex over function bodies) |
| gortex | `gortex query symbol ARG --index COPY` | `gortex query callers ARG --index COPY --depth 1` | `gortex query calls ID --index COPY --depth 1` for each id that `query symbol ARG --format json` resolves to | `gortex explore ARG --index COPY` |
| ctags | `readtags -t TAGS -e -n ARG` | unsupported | unsupported | unsupported |
| cscope | `cscope -d -q -k -f DB -L -1 ARG` | `... -L -3 ARG` | `... -L -2 ARG` | `... -L -4 ARG` (literal text, `CSCOPE_LOCATE=egrep` uses `-L -6`) |
| GNU Global | `global -x ARG` | `global -rx ARG` (reference lines, not caller names) | unsupported | `global -gx -e ARG` (grep pattern) |
| tilth | `tilth ARG --scope REPO` | `tilth ARG --callers --full --scope REPO` | `tilth grok ARG --scope REPO` | `tilth ARG --scope REPO` (literal text) |
| ast-grep | `ast-grep scan --inline-rules RULES -j N --json=stream .` with the rule set from `_astgrep_rules.py def ARG` | same with `callers` rules (call sites) | same with `callees` rules | unsupported |

Per-tool options (see the header comment of each script): `RIPWIRE_FLAGS`, `CBM_MODE`, `CBM_FORMAT`,
`CBM_LIMIT`, `CODEGRAPH_LIMIT`, `CODEGRAPH_JSON`, `CODEGRAPH_KIND`, `SEMCODE_EXTENSIONS`, `SEMCODE_DEF`,
`SEMCODE_LOCATE`, `GORTEX_LSP`, `GORTEX_LIMIT`, `GORTEX_FORMAT`, `CTAGS_EXTRA_ARGS`, `GTAGS_LABEL`,
`GLOBAL_ICASE`, `TILTH_FLAGS`, `ASTGREP_OUTPUT`, `BENCH_SRC_EXT_REGEX`.

Only four tools accept a natural-language question for `locate`: ripwire (`--for`), codebase-memory-mcp
(BM25 `search_graph --query`), CodeGraph (`explore`) and gortex (`explore`). semcode, cscope, GNU Global
and tilth treat `ARG` as a regex or literal string, so a sentence returns nothing: pass them a keyword.
semcode has a vector search (`vgrep`), but it needs a separate `semcode-index --vectors` pass and a model,
which was not set up.

## 4. Smoke test on `bench/repos/django` (commit 9600da0b98, 7,100 tracked files, 2,943 `.py`)

Queries: `def get_object_or_404`, `callers get_object_or_404`, `callees get_object_or_404`,
`locate "where is the CSRF token validated"`. Every output is saved as
`bench/tools/_smoke/<tool>.<kind>.txt`. The index JSON is `bench/tools/_smoke/<tool>.index.json`.

### Index cost

Three cold runs each except gortex (one per mode). Wall time is the median.

| Tool | Wall s (runs) | Peak RSS | Index size | Threads | Exit |
|---|---|---|---|---|---|
| ripwire | 1.26 (1.28, 1.25, 1.26) | 701 MB | 140.2 MB (lean 42.7 + rich 97.4) | uncapped | 0 |
| codebase-memory-mcp, mode full | 9.10 (9.10, 10.92, 8.60) | 579 MB (tree) | 235.7 MB | 6 | 0 |
| CodeGraph | 38.74 (29.42, 41.19, 38.74) | 3,513 MB | 183.2 MB | 6 | 0 |
| semcode | 3.11 (3.32, 3.11, 2.50) | 354 MB | 34.5 MB | 6 | 0 |
| gortex, `GORTEX_LSP=auto` (pyright started) | 171.28 in the final run, 62.98 in an earlier run | 2,757 MB and 4,732 MB (tree) | 910.8 MB | 6 | 0 |
| gortex, `GORTEX_LSP=off` | 65.12 in the final run, 38.12 and 41.36 earlier | 3,758 MB, 4,109 MB, 4,178 MB (tree) | 948.5 MB | 6 | 0 |
| universal-ctags | 0.60 (0.89, 0.58, 0.60) | 36 MB | 23.6 MB | 1 | 0 |
| cscope | 1.33 (1.33, 1.33, 1.15) | 96 MB | 51.1 MB | 1 | 0 |
| GNU Global (native-pygments) | 16.97 (16.97, 15.14, 20.68) | 35 MB | 17.5 MB | 1 | 0 |
| tilth | no index. One whole-tree search for an absent name: 0.20 | 7 MB | 0 | uncapped | 0 |
| ast-grep | no index. One full scan pass: 0.37 | 40 MB | 0 | 6 | 0 |

Notes on these numbers:

- CodeGraph's own banner says "62,824 nodes, 193,156 edges in 2.1s". The process took 29 to 41 s of wall
  time and 3.4 GB on django.
- gortex's banner says "index settled (1.0s)". `gortex track --wait` took 38 to 171 s. Its timings varied
  by more than 2x between runs on this loaded machine, so treat them as unmeasured until re-run.
- gortex's store is measured after `gortex daemon stop`. While the daemon runs the WAL makes it larger
  (1.2 to 1.4 GB observed).
- The tilth figure is a text prefilter that found nothing, so it parsed nothing. Real tilth queries on
  django took 0.14 to 0.24 s.

### Query output size and latency

Bytes are stable. Seconds are wall time for the whole script (sandbox start included) under load.

| Tool | def | callers | callees | locate |
|---|---|---|---|---|
| ripwire | 804 B, 0.52 s | 2,620 B, 0.52 s | 2,301 B, 0.52 s | 41,586 B, 1.13 s |
| codebase-memory-mcp | 274 B, 0.42 s | 510 B, 1.96 s | 296 B, 1.70 s | 7,198 B, 1.70 s |
| CodeGraph | 982 B, 0.28 s | 657 B, 0.21 s | 282 B, 0.22 s | 25,084 B, 0.49 s |
| semcode | 2,143 B, 0.40 s | 253 B, 0.25 s | 207 B, 0.25 s | 219 B, 0.30 s (no match for a sentence) |
| gortex (daemon started and stopped per call) | 903 B, 4.37 s | 836 B, 6.90 s | 436 B, 5.82 s | 4,924 B, 6.98 s |
| universal-ctags | 162 B, 0.05 s | unsupported | unsupported | unsupported |
| cscope | 289 B, 0.03 s | 1,149 B, 0.03 s | 0 B | 0 B (no literal match) |
| GNU Global | 90 B, 0.04 s | 2,144 B, 0.03 s | unsupported | 0 B (no literal match) |
| tilth | 1,416 B, 0.23 s | 39,414 B, 0.24 s (`--full`, 8,916 B without, capped at 10 of 15 sites) | 2,515 B, 0.24 s | 127 B (no literal match) |
| ast-grep (`--json=stream`) | 4,226 B, 0.77 s | 15,796 B, 0.54 s | 25,296 B, 0.66 s | unsupported |

codebase-memory-mcp's one-shot CLI starts and stops a temporary daemon on every call, and
`query_gortex.sh` starts and stops the gortex daemon when it is not already running. For warm latency
keep a daemon up: `gortex_daemon.sh start REPO` ... `gortex_daemon.sh stop REPO`.

### What the tools answered

For `callers get_object_or_404`, ripwire, codebase-memory-mcp, semcode and gortex all returned the same
five functions (`flatpage`, `get_person`, `test_get_object_or_404`, `test_bad_class`,
`test_get_object_or_404_queryset_attribute_error`). CodeGraph returned those five plus three files that
import the name. tilth reported 15 call sites. GNU Global returned 18 reference lines (15 calls and 3
imports).

For `def`, every tool reported `django/shortcuts.py:81`. Example outputs:

```
# ctags
get_object_or_404	django/shortcuts.py	/^def get_object_or_404(klass, *args, **kwargs):$/;"	kind:function	line:81	access:public	signature:(klass, *args, **kwargs)
# GNU Global
get_object_or_404   81 django/shortcuts.py def get_object_or_404(klass, *args, **kwargs):
# codebase-memory-mcp
  django-52f09f72.django.shortcuts.get_object_or_404 Function django/shortcuts.py 81-109 5 4
# gortex
function     django-52f09f72/django/shortcuts.py::get_object_or_404 django-52f09f72/django/shortcuts.py:81
# ripwire (after its legend comment)
<d n="get_object_or_404" t="fn" p="django/shortcuts.py:81" sel="django/shortcuts.py:get_object_or_404" lang="py" class="source" in="5"/>
```

For `callees`: ripwire listed `Http404`, `_get_queryset` and one declined call (`get`, 54 candidates).
codebase-memory-mcp, CodeGraph and gortex listed `_get_queryset`, `gettext`, `Http404`. semcode listed six
names including the builtins `hasattr`, `isinstance` and the alias `_`.

For `locate`, `django/middleware/csrf.py` (where the check lives) appears in the output of all four tools
that accept a sentence, but not equally. ripwire's first row is `_does_token_match` in that file.
codebase-memory-mcp ranks test methods first and `_does_token_match` sixth. gortex lists the file under
`files_to_edit`. CodeGraph printed source for five other files (`defaulttags.py`,
`views/decorators/csrf.py`, `forms/fields.py`, `contrib/postgres/forms/array.py`,
`context_processors.py`) and named `django/middleware/csrf.py` only in its trailing "Not shown above"
list. I did not grade these beyond reading them.

cscope is a C tool. On django it returned the right definition line plus two false "definitions", named
the wrong enclosing functions for callers (`assertRaises`, `set`), and returned no callees. Its results
are only meaningful on C repositories.

## 5. Things a benchmark author needs to know

1. **gortex uses language servers when it finds them.** With default settings the daemon auto-registers
   every language server on `PATH` (here `clangd`, `pyright`, `sourcekit-lsp`) and started
   `pyright-langserver` for django. Results therefore depend on the machine. `GORTEX_LSP=off` sets
   `GORTEX_LSP_RESOLVER=0` and writes `semantic: {enabled: false}` to `.gortex.yaml` in the clone. With it
   no language server was started. On a C tree the default would start `clangd --background-index`
   without a compile database, which was not tried.
2. **gortex downloaded a model.** On first daemon start it fetched `potion-code-16M-v2` (32 MB, static
   embeddings) into `home/.local/share/gortex/models`, although the README says "no model download to get
   started". It is shared by all runs now. No other tool was seen to fetch anything at run time, but
   network traffic was not monitored.
3. **codebase-memory-mcp `full` mode includes its built-in embedding model** ("full: all+semantic").
   `CBM_MODE=fast` or `moderate` selects the lighter modes.
4. **Memory on large trees.** On django alone CodeGraph used 3.5 GB and gortex 2.8 to 4.7 GB. For semcode
   the earlier research notes quote the systemd CI patch: above 11 GB on the kernel with four analysis
   threads, below 9 GB with two. Expect these three to exceed a 6 GB budget on the Linux kernel or LLVM
   (not measured here).
5. **Thread caps.** `BENCH_JOBS` is applied as `CBM_WORKERS`, `CODEGRAPH_PARSE_WORKERS` and
   `CODEGRAPH_RESOLVE_WORKERS`, `semcode-index -j`, `GOMAXPROCS` (gortex daemon only, not its language
   servers) and `ast-grep -j`. ripwire has no thread option and used about 7.5 cores on django. tilth has
   no option either. `RAYON_NUM_THREADS` is set but was not verified to have an effect.
6. **The classical tools get an explicit file list.** `source_file_list` in `_common.sh` passes tracked
   files with a source extension to ctags, cscope and gtags. Without it gtags' Pygments plug-in aborts the
   whole run on a binary file (`gtags: unexpected type string` on a gettext `.mo` file) and leaves empty
   tag files. For the kernel, set `CTAGS_EXTRA_ARGS` (for example `--c-kinds=+px --langmap=c:+.h`). The
   73 `regex_c` rules of the kernel's `scripts/tags.sh` are not applied by these scripts.
7. **semcode extensions.** The script indexes every extension semcode supports
   (`c,h,cc,cpp,cxx,hh,hpp,hxx,rs,py,zig`). The tool's own default is `c,h,rs,zig`. semcode needs a git
   checkout. It built on macOS arm64 without changes (the README only documents Ubuntu and Fedora).
8. **ast-grep and gortex callees need adapter logic.** ast-grep has no symbol verbs, so
   `_astgrep_rules.py` generates per-language structural rules (checked on small samples of Python, C,
   C++, Go, Rust, Java and TypeScript. JavaScript and TSX rules were not exercised). `gortex query calls`
   only accepts a graph id, so the script resolves ids first.
9. **Control-arm contamination.** These binaries are not on the normal `PATH`, only on the sandbox one.
   Keep it that way for agent runs that are supposed to work without them.

## 6. Not verified

- During this install pass nothing was run on the Linux kernel or LLVM. The only non-django runs were
  ast-grep rule checks on a few small subdirectories of the other repos. The kernel was indexed and
  queried later, with ripwire, codebase-memory-mcp, cscope, GNU Global and ctags: see `bench/README.md`
  and `bench/results/`.
- MCP server modes of the tools were not started.
- In this pass answer correctness was read by eye for one symbol and one question. The scored
  comparison is the kernel question set in `bench/k3`.
- cosign and sigstore signatures of the gortex and codebase-memory-mcp releases were not checked.
- Timings were taken on a heavily loaded machine.
- Incremental re-index, warm-daemon latency and embeddings-based search were not measured.

## 7. Cleaning up

```bash
bench/scripts/competitors/gortex_daemon.sh stop REPO      # only if you started one
rm -rf bench/tools/_work /private/tmp/codefast-bench-$(id -u)     # indexes and clones
rm -rf bench/tools                                                # everything except Homebrew formulae
brew uninstall global universal-ctags cscope protobuf jansson ncurses   # optional
```

No daemon was left running. `pgrep -f 'bench/tools/(gortex|codebase-memory-mcp|codegraph)/'` returned
nothing after the last run.
