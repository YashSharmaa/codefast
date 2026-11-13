# Compiler-grade call-graph oracles

Answer keys for call-graph precision and recall, produced by each language's
own compiler front end so that no tool under test supplies its own ground
truth. Format and scoring rules: [`FORMAT.md`](FORMAT.md). Generating scripts:
`bench/scripts/oracles/`.

Built 2026-11-12 on macOS 27.2 (arm64), Xcode 27.2.

| repo | language | commit (pin) | analysed files | function definitions | edges (site-level rows) | distinct function-level edges | of which dynamic candidates |
|---|---|---|---:|---:|---:|---:|---|
| gitleaks | Go | `8ad8470035d31a209322c580153b45c18e21b980` | 184 | 431 | 2,141 | 1,615 | 12 interface + 21 funcptr rows |
| syft | Go | `b15c5dbfe2bb21c9d73002c1056a829c8c411c75` | 662 | 3,292 | 10,339 | 8,258 | 2,221 interface + 1,774 funcptr rows, 4 generic-instantiation rows |
| zod | TypeScript | `bbc68f990c7e6a5e3f506c56fb04bd0279b9c9b5` | 116 | 1,157 (+49 classes, +352 variables) | 3,120 | 2,512 | 648 interface rows (class-hierarchy fold) |
| redis | C | `498ecd0d6d007db11ddb3aea9428552598a78622` (8.10.2) | 335 | 6,947 | 35,365 (30,853 static + 4,512 macro) | 24,223 | none in `edges.jsonl` beyond 68 link-variant rows, 1,559 function-pointer sites listed as unresolved |

A fifth oracle, **go-git** (Go, `12b788e49978b5807b748e425de6fba31af979c9`, 349 analysed
files, 2,704 function definitions, 5,412 static edge rows), was added on 2026-11-13 with the
same program. It is the answer key of the held-out tasks of `bench/tier2` and a repository that
codefast had not been run on before. go-git is a library without a `main` to start a reachability
analysis from, so the file holds static edges only and lists all 1,637 dynamic call sites as
unresolved. It was not checked by hand like the four above.

Scored tiers (`score.py` defaults, function-level pairs):

| repo | must | candidate | dynamic sites | judged functions |
|---|---:|---:|---:|---:|
| gitleaks | 1,585 | 15 | 7 | 430 of 431 |
| gitleaks, with tests | 1,688 | 26 | 15 | 488 of 488 |
| syft | 4,920 | 2,060 | 551 | 3,212 of 3,292 |
| zod | 1,329 (1,972 with `--include-variable-endpoints`) | 540 | 18 | 792 of 1,157 (716 of 1,029 top-level) |
| redis | 24,167 (19,687 for `src/` + `modules/`) | 56 | 34 | 6,319 of 6,947 |

The oracles were built on 2026-11-12 and then checked and corrected by an
independent verifier the same day. See "Independent verification" for what was
measured, what was wrong and what changed.

The pins are the ones repowise-bench records for its published call-graph
check: `graph/corpus/corpus.lock` (gitleaks, syft, zod) and
`graph/experiments/g9-language-oracles/README.md` (redis 8.10.2). The harness
was read at repowise-bench commit `0fb3c03aab35fb67dc72540fd6a70e2c2c599878`
(Apache-2.0). The code here is an independent implementation of the same
method, nothing was copied.

## Layout

```
bench/oracles/
  FORMAT.md  README.md  SHA256SUMS
  <repo>/edges.jsonl  functions.jsonl  unresolved_sites.jsonl  meta.json
  gitleaks/*.with_tests.*                  with _test.go and test mains
  redis/edges.funcptr_candidates.jsonl     heuristic, opt-in, not compiler truth
  .work/                                   caches and scratch builds, git-ignored, safe to delete
bench/scripts/oracles/
  build_all.sh        rebuild everything           fetch_repos.sh   clone at the pins
  env.sh              sandboxed environment        score.py         scorer (FORMAT.md "Scoring")
  go/main.go          Go oracle                    selftest_score.py
  ts/oracle.mjs       TypeScript oracle            sanity_sample.py seeded hand-check sample
  c/build_compdb.sh, cc_wrapper.py, make_compdb.py, extract.py      C oracle
  go/compare_with_reference.py    cross-check against repowise-bench's own Go oracle
  verify/run_all.sh               the independent verification, one command
  verify/invariants.py            mechanical invariants over every row (also run by build_all.sh)
  verify/sample.py                the verifier's seeded hand-check sample (seed 4051977)
  go/crosscheck/ + verify/go_crosscheck.py   static edges re-derived from go/ast + go/types, no SSA
  ts/crosscheck.mjs                          static edges re-checked against the symbol table
  verify/c_crosscheck.py                     direct calls re-derived from clang's own CallGraph
```

## Rebuild from scratch

```bash
brew install go                       # go 1.27.3; the only system-level install
bash bench/scripts/oracles/build_all.sh            # all four, about 45 s with warm caches on an idle machine
bash bench/scripts/oracles/build_all.sh zod redis  # or a subset
bash bench/scripts/oracles/verify/run_all.sh       # invariants, second derivations, hand-check sample
```

The first run also downloads syft's module dependencies (2.5 GB, about 3
minutes). `build_all.sh` ends by running `verify/invariants.py` and the scorer self-test
on every oracle and writing `SHA256SUMS`. Two consecutive full runs reproduce
every output file byte for byte, and so does a run from fresh clones in a
different directory with empty caches (see "Independent verification").

`env.sh` points `HOME`, `GOPATH`, `GOMODCACHE`, `GOCACHE`, the npm cache, the
pip cache and `TMPDIR` at `bench/oracles/.work/`, so nothing is written under
the real home directory. `GOTOOLCHAIN=local`, `GOMAXPROCS=6`. For the Go
oracles `build_all.sh` also sets `GOMEMLIMIT=4500MiB GOGC=40`, which keeps the
syft run at 4.9 GB of RAM instead of 7.2 GB and does not change its output. The Go module
cache is read-only on disk. Remove it with
`. bench/scripts/oracles/env.sh && go clean -modcache`.

### What each step runs

**Clones** (`fetch_repos.sh`): `git init; git fetch --depth 1 origin <sha>; git checkout FETCH_HEAD` from
`github.com/gitleaks/gitleaks`,
`anchore/syft`, `colinhacks/zod`, `redis/redis`. Nothing in the clones is
modified or built in place (`git status` is clean after a build).

**Go** (gitleaks, syft):

```bash
. bench/scripts/oracles/env.sh
(cd bench/scripts/oracles/go && go build -o "$ORACLE_WORK/bin/go-oracle" .)
export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
cd bench/repos/gitleaks && go mod download
"$ORACLE_WORK/bin/go-oracle" -repo . -name gitleaks -commit "$(git rev-parse HEAD)" -out ../../oracles/gitleaks
"$ORACLE_WORK/bin/go-oracle" -repo . -tests -suffix .with_tests -name gitleaks -commit "$(git rev-parse HEAD)" -out ../../oracles/gitleaks
```

`packages.Load("./...")` with full type information and dependencies,
`ssautil.AllPackages(pkgs, ssa.InstantiateGenerics)`, then (a) every call
instruction with a `StaticCallee` in every function of the module, and (b)
`rta.Analyze(roots, true)` rooted at each `main.main` and `main.init` for
interface and function-value calls. Target platform is pinned to linux/amd64
without cgo so that the result does not depend on the host.

**TypeScript** (zod):

```bash
cd bench/scripts/oracles/ts && npm ci --ignore-scripts
node oracle.mjs -repo ../../../repos/zod -subdir packages/zod/src \
  -tsconfig ../../../repos/zod/packages/zod/tsconfig.json -name zod -commit <sha> -out ../../../oracles/zod
```

Scope is `packages/zod/src` without test files (116 files, the same set
repowise analysed). zod's own dependencies are **not** installed. The library
source has no runtime dependencies, so only ambient types are missing.
Compiler options come from zod's `packages/zod/tsconfig.json` with `types: []`.

**C** (redis):

```bash
bash bench/scripts/oracles/c/build_compdb.sh
# = git archive HEAD | tar -x  into .work/redis-build, then
#   make -C src -j6 V=1 CC=<cc_wrapper.py> MALLOC=libc redis-server redis-cli redis-benchmark
.work/venv/bin/python -I bench/scripts/oracles/c/extract.py --build-root .work/redis-build \
  --out bench/oracles/redis --name redis --libclang <Xcode toolchain>/usr/lib/libclang.dylib \
  --jobs 6 --exclude-prefix tests/
```

`cc_wrapper.py` logs every compiler invocation and then execs the real clang
(no `bear`, no Homebrew llvm). 187 compile commands and 6 link commands were
recorded. All 187 translation units parse with zero clang errors. Each unit is
re-parsed by the Xcode toolchain's own `libclang.dylib` with its exact flags.
Callees are resolved to definitions inside the unit by clang and across units
by external-linkage name, using the recorded link commands when a name has
more than one definition.

## Versions and checksums

| component | version | source / checksum |
|---|---|---|
| Go | 1.27.3 | Homebrew bottle `go--1.27.3.arm64_golden_gate`, sha256 `c7bf84a6f727873581b3e1b5f79c1254cd5856f1241534e2f6943a8acc193e6e` |
| golang.org/x/tools | v0.52.0 | `go/go.sum`: `h1:ryxg4U63oaUYk7uOLTF37n43f4qgtGvqV4Ioit1BaEs=` |
| golang.org/x/mod, x/sync | v0.42.0, v0.24.0 | `go/go.sum` |
| Node.js / npm | v26.13.0 / 11.20.0 | already installed |
| typescript | 5.9.3 | `ts/package-lock.json`, integrity `sha512-jl1vZzPDinLr9eUt3J/t7V6FgNEw9QjvBPdysz9KfQDD41fQrC2Y4vKQdiaUpFT4bXlb1RHhLpp8wtm6M5TgSw==` |
| clang | Apple clang 21.0.0 (clang-2100.4.1.1), Xcode 27.2 | already installed. Same toolchain compiles redis and provides `libclang.dylib` |
| Python | 3.15.0 | already installed |
| `clang` Python bindings | 21.1.7 | PyPI wheel `clang-21.1.7-py3-none-any.whl`, sha256 `23ee8f7b62af648009aee5139516b2a2a9320680dbce6e42a53e48bd5e8983ea` (pure Python, pinned with hash in `c/requirements.txt`) |
| GNU Make | 3.81 | Xcode |

Output checksums are in `SHA256SUMS`.

## Validation

### Against repowise-bench's own Go oracle

Their oracle (`g4-oracle-anchored/oracle/main.go`, x/tools v0.38.0, built
unmodified in a scratch directory) was run on the same checkouts and compared
with `go/compare_with_reference.py`:

| cell | published | reference re-run here | this oracle, same population | reference edges missing here | extra here |
|---|---:|---:|---:|---:|---|
| gitleaks, no tests | 1,584 edges / 415 functions | 1,584 / 415 | 1,584 / 415 | 0 | 0 |
| gitleaks, with tests | 1,747 / 480 | 1,747 / 480 | 1,747 | 0 | 0 |
| syft, no tests | 7,898 / 2,979 | 7,899 / 2,979 | 7,969 / 2,979 | 0 | 70, all `funcptr` |

"Same population" = function-level edges whose caller is RTA-reachable, which
is all their oracle emits. The 70 extra syft edges come from 6 call sites of
`func() error` values (`s.closer()`, `cleanupFn()`, …) whose RTA targets
include a bound interface method value (`x.Close` taken as a value). This
oracle follows the compiler's bound-method wrapper to the in-repo `Close`
implementations. Theirs drops the wrapper for lack of a position. Like all
`funcptr` candidates they are coarse. This oracle then
adds what theirs cannot have: 31 (gitleaks) and 287 (syft) static edges out of
functions no `main` reaches.

The reference binary ran with the host platform (darwin/arm64), this oracle
with linux/amd64. The reference panics inside x/tools v0.38.0 when
cross-loading for linux under Go 1.27, so the two were not run on an identical
platform. The edge sets agree regardless, as the table shows.

zod: the reference TypeScript oracle (`g4-oracle-anchored/oracle-ts/oracle.mjs`,
sha256 `990ab721…62f76`, run unmodified with the same TypeScript 5.9.3) was
re-run by the verifier and reproduces the published figure exactly: 1,269
edges, 753 judged functions, 4,889 call sites. On the same population (static
edges out of named functions, `tsc` only, `--repowise-compat`) this oracle has
1,271, and the difference is fully accounted for:

* 5 edges are here and not in the reference: the five functions of
  `export const coerce = { string: ((arg) => ZodString.create(…)) as …, … }`
  (`v3/types.ts:5085-5093`). The reference does not look through the
  parentheses and the `as` cast, so those arrows have no name there and their
  calls count as top-level code.
* 3 reference edges are not here, on purpose: `flattenError`, `formatError` and
  `treeifyError` (`v4/core/errors.ts:265, 293, 347`) call their own parameter
  `mapper`, whose default value is an arrow written on the declaration line.
  The reference keys that arrow at the enclosing function's line, so the three
  show up as self-recursive edges. Here a call through a parameter is a
  `funcptr` site, not an edge.

Every other reference edge (1,266) is in this oracle. The judged-function sets
were not reconciled one by one: 716 top-level functions are judged here against
753 there. This oracle additionally withholds callers with accessor reads and
parameter calls and additionally resolves calls of typed `const` functions.

redis: repowise's key is scip-clang on Linux with jemalloc (34,349 edges, 512
files). This one is a macOS build without jemalloc, so the numbers are not
comparable and no cross-check was made.

### Hand check (seed 20261112, `sanity_sample.py`)

For each oracle, 20 random edges were read against the source (caller
declaration, call-site line and column, callee declaration), and 10 random
top-level functions of 8 to 45 lines were read in full to confirm that every
call to a repository function in the body is in the oracle.

| repo | 20 edges | 10 functions complete | notes |
|---|---|---|---|
| gitleaks | 20 / 20 correct | 10 / 10 | all 20 drawn edges are `static` |
| syft | 20 / 20 are what the method defines, 3 of them are not real calls | 10 / 10 | 10 static and 6 interface edges correct. Of 4 `funcptr` edges, 1 is exact and 3 are RTA signature matches that cannot happen at run time (`defer cancel()` paired with an unrelated `func()` closure). All 3 have a closure callee, which default scoring excludes. |
| zod | 20 / 20 correct | 10 / 10 | drawn twice (before and after the variable-endpoint change). The first draw was read in full, the second by annotated lines. Interface edges are class-hierarchy candidates, each a real override of the called abstract method. |
| redis | 20 / 20 correct | 10 / 10 | plus 6 extra `macro` edges (`s_free`→`zfree`, `lua_pop`→`lua_settop`, `REGISTER_API`→`moduleRegisterApi`, …) checked against the macro definitions |

"Complete" means: no call to a function defined in the analysed files is
missing. Calls to the standard library, to dependencies and to builtins are
absent by design. In zod, several of the functions read contain a call the
compiler cannot take to a body (`new Class(...)` on a constructor-typed
parameter, `$ZodType.init(...)`). Those sites are in `unresolved_sites.jsonl`
and the functions are `judged: false`, as intended.

### Independent verification (2026-11-12, second engineer)

Everything in this section is reproduced by
`bash bench/scripts/oracles/verify/run_all.sh` plus the files named below. The
generators were changed as a result. The "before" columns are the oracle as
first built (sha256 of `edges.jsonl`: gitleaks `348d42e2…`, syft `514a848b…`,
zod `116fee78…`, redis `d4957eab…`), the "after" columns are the files in this
directory.

**1. Hand check of 40 random rows per oracle** (`verify/sample.py`, seed
4051977, drawn uniformly from the original `edges.jsonl`. Verdicts in
`verification/handcheck-2026-11-12.jsonl`). Each row was read against the
source at the pinned commit: caller declaration, call site, callee declaration,
imports, and the macro definition for `macro` rows.

| repo | compiler-resolved rows (`static`, `macro`) | dynamic candidate rows | wrong rows |
|---|---|---|---|
| gitleaks | 39 of 39 are real calls | 1 `funcptr`: an RTA signature match that cannot happen at run time | none |
| syft | 27 of 27 | 6 `interface`: 5 feasible, 1 not (`emptySource` offered as an `io.Closer` behind an `io.Reader`). 7 `funcptr`: 3 exact, 4 unrelated closures of type `func()` | none |
| zod | 26 of 27 | 13 `interface` (class-hierarchy fold): all real overrides of the called abstract member | 1: `treeifyError` → its own default argument (a call through the parameter `mapper`, labelled `static`) |
| redis | 40 of 40 (37 static, 3 macro) | none drawn | none |

Compiler-resolved rows: 132 of 133 correct (99.2%, 95% Wilson interval 95.9% to
99.9%). The one wrong row had a closure callee and so was already outside
default scoring. The generator defect behind it is fixed (below). Dynamic
candidates are over-approximations by construction: 6 of 27 sampled could not
happen at run time (5 of 8 Go `funcptr`, 1 of 6 Go `interface`, 0 of 13
TypeScript). All 5 infeasible `funcptr` rows have closure callees, which
default scoring ignores.

**2. Hand check of 15 functions per oracle** (same script, seed 4051978, five
each of 3-10, 11-40 and 41-150 lines). Every call in the body was listed by
hand and looked up. 60 of 60 functions complete, 250 in-repo calls found by
hand (gitleaks 61, syft 27, zod 40, redis 122), 0 missing from the oracle.
Calls that are absent by design were checked to be absent for the stated
reason (standard library, dependencies, builtins, C macros that expand to no
call, sites listed as unresolved). The sample is small against the defect
rates found next, which is why step 3 exists.

**3. Second derivation over every row**, by a route that shares no code with
the generators:

| oracle | second derivation | compared | missing from the oracle, before → after | in the oracle but not confirmed |
|---|---|---|---|---|
| gitleaks | `go/crosscheck`: `go/ast` + `go/types` `typeutil.StaticCallee`, no SSA | 2,098 static call sites | 0 → 0 | 0 (10 further rows are local closures, which have no `types.Func`) |
| syft | same | 6,267 static call sites | 2 → 0 | 0 (73 closure rows, 4 rows are calls on a type parameter, now marked) |
| zod | `ts/crosscheck.mjs`: the symbol the callee expression names, against the resolved signature | 2,357 static rows | not measurable this way (see below) | 0 disagreements, 7 rows called through a parameter or `let` → 0 |
| redis | `verify/c_crosscheck.py`: clang's own `CallGraph` (`clang --analyze`, `debug.DumpCallGraph`) on all 187 units | 34,823 direct call sites, 24,159 caller/callee name pairs | 1 pair → 0 | 0 of 34,794 |

For redis the remaining site-level difference (29 sites) is two calls from one
macro expansion sharing a position. No function-level edge is affected.
For zod the symbol check cannot find lost edges where `tsc` gives no
signature. It does confirm that the 55 call sites with a resolvable in-repo
function and no row are all module-level code outside any function or variable
initialiser (51 of them in `v3/benchmarks/`).
`verify/invariants.py` checks nine mechanical invariants on every row (both
endpoints exist, the site lies inside the caller, the callee's name is the
token at the site, candidate counts, `judged` consistency): no violations
before or after.

**4. Defects found and fixed**

| # | defect | effect before | after |
|---|---|---|---|
| 1 | Go: methods of generic types that nothing instantiates were never walked (`ssautil.AllFunctions` skips them), yet were listed as `judged` | syft: 19 functions unwalked, 2 must edges missing (`Set[T].Equals` → `Contains`, `ptrMap[T].Remove` → `ptrTo`), 4 dynamic sites unlisted | all declared functions walked (`declared_functions_missed_by_AllFunctions: 19` in `meta.json`), +2 rows, unresolved sites 133 → 137, judged 3,218 → 3,212 |
| 2 | Go: a call on a type parameter was emitted as plain `static` once per instantiation, and the same site was also listed as unresolved | syft: 4 rows in the must tier at 2 sites with 2 targets each | `candidate_reason: "generic-instantiation"`, candidate tier, must 4,920 → 4,920 (+2 −2), candidate 2,058 → 2,060 |
| 3 | C: `(f)(x)` was filed as a function-pointer call (libclang does not look through the parentheses) | redis: 1 must edge missing (`f_parser` → `luaY_parser`) | +1 row (35,364 → 35,365), unresolved sites 1,560 → 1,559 |
| 4 | C: a function with an `#if` in its body was `judged`, so a tool reading the inactive branch would be marked wrong | redis: 194 such functions judged | not judged, judged functions 6,512 → 6,319 |
| 5 | TypeScript: a call through a parameter with a default value (or a `let`) was a `static` edge to the default | zod: 7 rows mislabelled, all with closure callees (outside default scoring). Their callers were already not judged for other reasons | unresolved `funcptr` sites, rows 3,127 → 3,120 |
| 6 | TypeScript: reading a property backed by an in-repo getter is an implicit call the oracle does not emit, but the reading function stayed `judged` | zod: 24 sites | listed as unresolved (`accessor`), judged top-level functions 726 → 716 |
| 7 | `score.py`: with ambiguous endpoints the pair a row stood for depended on Python's set order | same input, four runs: precision 0.8039, 0.8079, 0.8085, 0.8097 (zod, names-only rows plus 400 wrong edges) | sorted, specified in FORMAT.md step 4, identical output on every run |
| 8 | `score.py` / FORMAT.md disagreed on a line that maps nowhere: the document said unmapped, the code fell back to a file-wide name search (so the inactive `#else` twin of a C function, or a mid-body callee line, could be matched by name) | not exercised by the oracle's own rows | position first, no fallback, class bodies are not callers, self-test probes added |
| 9 | redis `meta.json` was not reproducible (link order under `make -j`, a build-path hash) | one file differed between two builds | sorted and normalised, identical |

gitleaks is unchanged byte for byte by all of this.

**5. FORMAT.md** was tightened where two implementers could have scored the
same output differently: the candidate tier is now "rows with a `candidates`
field". Endpoint mapping is spelled out for every combination of line and name,
including ties and nested or same-line declarations. One tool row stands for
exactly one pair. `contradicted` needs every possible caller to be judged. The
dynamic-site denominator, `precision_strict` and the treatment of variable
records on the tool side are defined. The adapter's obligations (definition,
not prototype, and lines, not just names) are stated.

**6. Rebuild from scratch.** The generator scripts were copied to an empty
directory and run there (fresh clones of gitleaks, zod and redis at the pinned
commits, empty Go module, Go build, npm and pip caches, a fresh redis build):
all 16 data files identical to the originals. Only redis `meta.json` differed
(defect 9). After the fixes, two consecutive full `build_all.sh` runs for all
four repositories were identical in every file including `meta.json`. syft was
not rebuilt from empty caches (2.5 GB download). It was rebuilt three times
from the existing module cache with identical output.

**Not verified.** The redis function-pointer candidate file (heuristic, opt-in)
was not hand-checked. The Go `interface` and `funcptr` candidate sets have no
second derivation. RTA is taken as given, and the sample above shows how loose
it is. zod's class-hierarchy candidates were only sampled (13 rows). No tool
has been scored yet, so the scoring rules have been exercised only by the
self-test. Different Go, clang or TypeScript versions were not tried.

### Scorer self-test (`selftest_score.py`, run by `build_all.sh`)

Feeding each oracle back as a tool gives precision 1.0 and recall 1.0. With
lines only and the caller reported at its last line, still 1.0 / 1.0. With
half the edges dropped and 200 wrong edges from judged callers added, recall is
0.50 to 0.52 and at least 199 of the 200 are counted as contradicted. With names
only and no lines the result is the same under two hash seeds (syft 492 and zod
1,230 ambiguous rows. Recall falls to 0.998 and 0.855 because an ambiguous row
earns one pair). 50 callers reported at a line outside every function and 50
callees at a line inside a body without a name are all unmapped.

## Scoring a tool

```bash
python3 bench/scripts/oracles/score.py --oracle bench/oracles/gitleaks --tool tool_edges.jsonl --out score.json
#   --variant .with_tests               gitleaks with tests
#   --scope-prefix src/                 first-party redis only (repeatable)
#   --include-variable-endpoints        zod: count module-variable callers / callees
#   --with-funcptr-candidates           redis: load the heuristic function-pointer candidates
#   --repowise-compat                   the population repowise published against
#   --dump DIR                          write missed / contradicted / unjudged / unmapped rows
```

## Caveats, per oracle

**gitleaks, syft (Go)**

* One build configuration: linux/amd64, no cgo. Three files are outside it
  (`regexp/wasilibs_regex.go`, syft's `get_xid_win.go` and Windows tests).
* Generated file `internal/spdxlicense/license_list.go` (syft) is excluded.
* RTA is rooted at `main` packages: 4 roots in gitleaks, 30 in syft (syft's
  examples and generators have their own `main`). Dynamic calls in functions no
  root reaches, and calls on a type parameter in a generic body nothing
  instantiates, are unresolved: 2 sites in gitleaks, 137 in syft, making 1 and
  80 functions `judged: false`.
* `funcptr` candidates are signature matches and badly over-approximate (up to
  72 in-repo targets for one site, plus hundreds outside). Read
  `recall_dynamic_sites`, not candidate-edge recall, for them.
* 1,534 syft rows (27 in gitleaks) have an anonymous function as the callee and
  are outside default scoring.
* Not emitted: calls at package-initialiser level (10 in gitleaks, 275 in
  syft), SSA calls without a source position (15 in syft, all the implicit
  iterator call of a range-over-func loop, all reaching closures. A caller
  whose implicit call reached a declared function would be marked not judged,
  and there is none), calls into the standard library or dependencies.
* A call on a type parameter inside a generic function is one row per
  instantiation target, marked `candidate_reason: "generic-instantiation"` and
  scored in the candidate tier (4 rows in syft, in `internal/cmptest/set.go`).
* RTA is unsound under reflection and `go:linkname`.
* syft with tests was **not** built. The no-tests run needs 4.9 GB of RAM under
  the memory limit `build_all.sh` sets (7.2 GB without it). The with-tests
  variant has 268 roots and was not attempted on this machine.

**zod (TypeScript)**

* Library source tree only, no tests, dependencies not installed (repowise's
  setup). `tsc` cannot resolve 157 call sites at all and stops at a bodiless
  signature for 915 more. Together with 24 property accesses that run an
  in-repo getter and 7 calls through a parameter or `let` binding, that leaves
  313 of 1,029 top-level functions `judged: false`.
* zod v4 builds its schemas as `export const X = $constructor("X", (inst, def)
  => {…})`. 1,500 of 4,889 call sites sit in such initialisers. They are kept
  with `caller_kind: "variable"` (625 rows) and left out of default scoring,
  which follows repowise in scoring named functions only.
* `interface` rows are our class-hierarchy fold over `tsc` symbols, not a
  compiler answer. `const-initializer` rows (123) rest on a `const` binding to
  a function literal. Both are marked with `resolution` and are dropped by
  `--repowise-compat`.
* A method call on a class type resolves to the statically declared member.
  Overrides in subclasses are not enumerated.
* Property getters and setters invoked by property access are not call edges.

**redis (C)**

* One build configuration: macOS arm64, Apple clang, `MALLOC=libc`, no TLS, no
  systemd, targets `redis-server`, `redis-cli`, `redis-benchmark`. Code under
  `#ifdef __linux__`, `ae_epoll.c`, jemalloc, the example modules and
  `tests/modules` are not in the key (405 `.c`/`.h` files listed in
  `meta.source_files_not_compiled_in_this_configuration`). A Linux build would
  give a somewhat different graph.
* Vendored `deps/` (lua, hiredis, tre, xxhash, linenoise, hdr_histogram,
  fpconv) is compiled and therefore inside the key: 119 of 335 files, 1,455 of
  6,947 functions. Use `--scope-prefix` to leave it out (19,687 must edges
  remain for `src/` + `modules/`).
* Direct calls only. 1,559 call sites go through a function pointer (command
  table, `dictType` callbacks, the module API). 434 functions are
  `judged: false` because of them.
* 206 functions contain an `#if` / `#ifdef` / `#else` in their body. The text
  of the inactive branch is not in this key, so they are `judged: false`
  (`unjudged_reasons: ["conditional-compilation"]`). Their compiled edges are
  still in the must tier. In all, 628 of 6,947 functions are not judged.
* Two calls to the same function that come out of one macro expansion share a
  source position and are one row (29 sites, for example `BIT_OP` in
  `lua_bit.c`, or a macro that evaluates its argument twice). Function-level
  edges are unaffected. `edges.funcptr_candidates.jsonl` (10,301
  rows) matches each site to address-taken functions of the identical type. It
  is a heuristic, wrong wherever redis casts function pointers, and was not
  hand-checked.
* "From a macro body" is decided by reading the source text at the site,
  because libclang has no true spelling-location API
  (`clang_getSpellingLocation` returns the file location).
* 68 rows are link variants (`_serverAssert`, `_serverPanic` have one
  definition for the server and one for the CLI tools).
* The build ran in a scratch copy (`.work/redis-build`). `src/release.h` is
  generated there and excluded.

## Resource use

| step | wall time | peak RSS |
|---|---:|---:|
| gitleaks oracle | 14 s cold | 1.5 GB |
| syft oracle | 15 s (after a 3 min module download) | 7.2 GB without a memory limit, 4.9 GB and 24 to 37 s with `GOMEMLIMIT=4500MiB GOGC=40`, which `build_all.sh` sets |
| zod oracle | 3 s | 0.5 GB |
| redis build (`-j6`) + extraction (6 workers) | 30 s + 5 s | not measured |
| whole `build_all.sh`, all four, warm caches, machine under heavy load (load average above 50) | 334 s | 4.1 GB |
| gitleaks + zod + redis from fresh clones and empty caches, same load | 163 s | 1.6 GB |
| `verify/run_all.sh`, all four | about 1 min | 0.7 GB (Go cross-check of syft) |
