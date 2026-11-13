# Call-graph oracle format and scoring rules

An oracle is a directory `bench/oracles/<repo>/` holding a call graph that was
produced by the language's own compiler front end, never by a tool under test.
Any tool that can answer "who calls X" / "what does X call" can be scored
against it with `bench/scripts/oracles/score.py`.

| repo | language | answer key |
|---|---|---|
| gitleaks, syft | Go | `golang.org/x/tools`: `go/ssa` static callees + `go/callgraph/rta` |
| zod | TypeScript | `tsc` type checker, `getResolvedSignature` per call site |
| redis | C | clang AST (libclang) per translation unit of a real build |

How each one was built, with versions and commands, is in `README.md`.

## Files

| file | content |
|---|---|
| `edges.jsonl` | one JSON object per call edge (per call site and target) |
| `functions.jsonl` | one JSON object per function definition (and the other entities an edge can end on) |
| `unresolved_sites.jsonl` | call sites the oracle saw but could not take to a target |
| `meta.json` | commit, tool versions, the analysed file set, exclusions, counts |
| `edges.funcptr_candidates.jsonl` | redis only: heuristic candidates for function-pointer calls, **not** compiler truth |
| `*.with_tests.*` | gitleaks only: the same four files with `_test.go` included |
| `../verification/handcheck-2026-11-12.jsonl` | the independent verifier's seeded sample (160 rows, 60 functions) with a verdict per item |

All paths are repository-relative with `/` separators. Lines and columns are
1-based. Columns count bytes. Files are sorted and byte-reproducible
(`SHA256SUMS`).

In git the `.jsonl` tables are stored gzip-compressed, as `edges.jsonl.gz` and so on, to keep
a checkout small. `score.py` reads either form. The checksums in `SHA256SUMS` are those of
the uncompressed files, and `gunzip -k bench/oracles/*/*.jsonl.gz` restores them for the
build and verification scripts.

## `edges.jsonl`

```json
{"caller": {"file": "cmd/detect.go", "name": "runDetect", "line": 50},
 "callee": {"file": "cmd/root.go", "name": "initConfig", "line": 132},
 "site_line": 56, "site_col": 2, "kind": "static", "rta_reachable": true}
```

Required fields

| field | meaning |
|---|---|
| `caller.file/name/line` | the function the call is written in, keyed at its **declaration line** (see "Declaration line"). When a call sits in a closure or nested function, `caller` is the **outermost enclosing declared function**. |
| `callee.file/name/line` | the function that runs, keyed at its declaration line. Always a *definition* in the analysed file set. |
| `caller.receiver`, `callee.receiver` | Go receiver type (`*Detector`) or TypeScript class. Omitted for plain functions. |
| `site_line`, `site_col` | position of the **callee name token** at the call site (`f` in `f(x)`, `(f)(x)`, `M` in `x.M()`, `X` in `new X()`). For a call produced by a C macro this is where the macro is used. Rows are unique per (caller, site position, callee, kind): two calls to one function that come out of the same macro expansion share a position and are one row (29 such sites in redis). |
| `kind` | how the target was determined, below. |

`kind`

| kind | meaning | one site has |
|---|---|---|
| `static` | the compiler resolved the call to exactly this function | 1 target. The two exceptions carry a `candidates` field and a `candidate_reason`: C link variants and Go generic instantiations (below). |
| `macro` | C only: a direct call that comes out of a macro body. The callee's name is not written at the site (`serverLog(...)` → `_serverLog`). `via_macro` names the macro. Otherwise identical to `static`. | 1 target |
| `interface` | dynamic dispatch through an interface / abstract member | a **candidate set** |
| `funcptr` | call through a function value | a **candidate set** |

Optional fields

| field | where | meaning |
|---|---|---|
| `candidates` | dynamic kinds, C link variants, Go generic instantiations | number of distinct in-repo targets this call site has. All edges sharing `(caller, site_file, site_line, site_col)` form the candidate set. **A row has this field if and only if it belongs to the candidate tier** (see Scoring). |
| `candidates_external` | Go | further targets outside the repository (not listed) |
| `candidate_reason: "link-variant"` | C | a `static`/`macro` call whose callee name is defined in more than one program of the build (`_serverAssert` in `debug.c` for redis-server and in `redisassert.c` for redis-cli): one edge per definition the caller can link against |
| `candidate_reason: "generic-instantiation"` | Go | a `static` call on a value whose type is a type parameter (`x.M()` with `x T`). The compiler resolves it separately in each instantiation of the enclosing generic function: one edge per instantiation target that the analysed code creates |
| `resolution` | TS, C | set when the target comes from something other than the plain compiler answer: `class-hierarchy` (TS interface candidates, our own fold), `const-initializer` (TS, call of a typed `const f = () => …`), `signature-match` (C candidate file only) |
| `via` | Go, TS | `go`, `defer`, `new`, `tagged_template` |
| `via_macro` | C | macro written at the site, for `kind: "macro"` |
| `caller_closure` `{name,line}` | Go | the func literal the call is actually written in |
| `caller_inner` `{name,line}`, `in_closure` | TS | innermost named function containing the site when it is not `caller`. Site is inside an anonymous callback |
| `callee_closure: true`, `callee_outer` | Go, TS | the target is an anonymous function. `callee` is then the literal's own position and `callee_outer` the declared function it is written in. |
| `callee_alt` `[endpoint]` | TS | equally acceptable callee: for `new X()` the edge names class `X`, and the constructor that runs is listed here |
| `caller_kind: "variable"`, `callee_kind: "variable"` | TS | the endpoint is a module-level variable, not a function: calls made while initialising `export const X = factory(...)`, and `X(...)` / `new X(...)` where `X` holds a factory-made value |
| `rta_reachable` | Go | the calling SSA function is reachable from a `main` under RTA |
| `site_file` | C | present only when the site is in another file than the caller's declaration |
| `implicit` | Go | SSA call with no call expression in the source |

## `functions.jsonl`

```json
{"file": "detect/detect.go", "name": "DetectSource", "line": 208, "end_line": 262,
 "receiver_or_class": "*Detector", "kind": "method", "judged": true, ...}
```

| field | meaning |
|---|---|
| `file`, `name`, `line`, `end_line` | definition span. `line` is the declaration line |
| `receiver_or_class` | Go receiver type, TypeScript class (or object-literal owner), `""` otherwise |
| `kind` | `function`, `method`. Go also `var_func` (`var f = func(){}` at package level). TS also `var_func`, `field_func`, `property_func`, `constructor`, `getter`, `setter`, plus the non-function entities `class` and `variable` (see above) |
| `judged` | the oracle claims to know **every** in-repo call edge out of this function. `false` when it contains a call site listed in `unresolved_sites.jsonl`, or when `unjudged_reasons` is present. |
| `unjudged_reasons` `[string]` | present only when the function is not judged for a reason that is not a call site: C `"conditional-compilation"` (the body contains `#if` / `#ifdef` / `#ifndef` / `#elif` / `#else`, so part of its text was never compiled in this configuration and the key cannot say what that text calls). Go `"implicit call without a source position (range-over-func)"` |
| `nested_in` `{name,line}` | TS: the outermost named function this one is written inside (its calls are attributed there) |
| `start_line`, `name_line` | TS: first line including decorators, when different from `line`. C: line of the name token, when the return type is on an earlier line. |
| `rta_reachable`, `dynamic_sites`, `unresolved_dynamic_sites`, `closures`, `generic`, `exported`, `qualname` | Go extras |
| `linkage`, `indirect_call_sites`, `from_macro` | C extras |

`(file, line, name)` is unique and is the identity used everywhere.

### Declaration line

| language | `line` is the line of |
|---|---|
| Go | the `func` keyword (doc comment excluded) |
| TypeScript | the first token of the declaring statement after decorators and JSDoc: `export const f = () => {}` keys at `export`. A method at its first modifier |
| C | the first token of the definition (storage class or return type). `name_line` is added when the name is on a later line |

## `unresolved_sites.jsonl`

```json
{"caller": {"file": "detect/files.go", "name": "DetectFiles", "line": 17, "receiver": "*Detector"},
 "site_line": 49, "site_col": 12, "kind": "interface", "reason": "caller not reached by RTA"}
```

One row per call site that the oracle saw and could not resolve: Go dynamic
calls in functions RTA never reached, and calls on a type parameter in a generic
body that the analysed code never instantiates. TypeScript calls that `tsc`
resolves to a bodiless signature (`interface`, `funcptr`), calls through a
parameter or a reassignable `let` / `var` binding (`funcptr`: `tsc` names only
the value the binding was declared with), calls it cannot resolve at all
(`unresolved`: `any`, missing dependency), and property accesses that run an
in-repo getter or setter (`accessor`: an implicit call that is not written as
an edge). C calls through a function pointer (with `callee_expr`, `struct`,
`fn_type`). A function with at least one such site has `judged: false`.

## What counts as an edge

* A call expression in a function body whose target is a function **defined in
  the analysed file set**. One row per (call site, target).
* Go: `f()`, `x.M()`, `go f()`, `defer f()`, calls of method values and method
  expressions (looked through the compiler's wrapper), generic functions (all
  instantiations folded onto the generic declaration). Every function
  declaration with a body is walked, including methods of generic types that
  nothing instantiates.
* TypeScript: `CallExpression`, `NewExpression`, `TaggedTemplateExpression`.
  An overload resolves to its implementation. `new X()` targets class `X`.
* C: every `CallExpr` whose callee is a function name, written plainly or in
  parentheses (`f(x)`, `(f)(x)`). `(*fp)(x)`, `s->cb(x)` and
  `(c ? f : g)(x)` are function-pointer calls. The
  callee is resolved to its definition inside the translation unit by clang
  (so two `static` functions with one name in two files are kept apart), and
  across translation units by external-linkage name.

## What is excluded

| excluded | why / where it is recorded |
|---|---|
| **Tests**: Go `_test.go`. TS files under `test/`, `tests/`, `__tests__/`, `spec/` or named `*.test.ts` / `*.spec.ts`. redis `tests/` | default variant is "no tests" (`meta.tests_included: false`). gitleaks also ships `*.with_tests.*`. TS test files are listed in `meta.excluded_files`, the redis prefix in `meta.excluded_prefixes`. Go test files are simply not loaded. |
| **Generated code**: Go files with the standard `// Code generated … DO NOT EDIT.` header. C headers created by the build (`src/release.h`) | `meta.excluded_files`, `meta.build_generated_files_seen` |
| **External packages**: Go standard library and module dependencies, `lib.*.d.ts` and anything outside the TS source subtree, libc / system headers | dropped on both ends, counted in `meta.counts` |
| **Files the build configuration does not compile**: other-OS Go files, redis sources not built on macOS (`ae_epoll.c`, jemalloc, example modules) | `meta.build_constraint_ignored_files`, `meta.source_files_not_compiled_in_this_configuration` |
| **Calls outside any function**: Go package-level `var x = f()`, TS module top level (other than inside a variable initialiser, which is kept with `caller_kind: "variable"`) | counted in `meta.counts` |
| **Compiler builtins and implicit calls**: Go builtins, range-over-func iterator invocation (no source position), property getters/setters invoked by plain property access, C `__builtin_*` | not call edges to repository functions. Where such an implicit call does reach a repository function (a TS accessor, a Go iterator that is a declared function) the caller is `judged: false`, so a tool that reports the call is not charged for it. |
| **C code the preprocessor removed**: the inactive branch of `#if` / `#ifdef` inside a function that is otherwise compiled | not in the key. The function has `unjudged_reasons: ["conditional-compilation"]` (206 functions in redis). A function defined only in an inactive branch is not in `functions.jsonl` at all. |
| **Closures as callers**: a call written in an anonymous function at package / module level that is not the initialiser of a `var f = func(){}` (Go) or of a module-level variable (TS) | no enclosing declared function, counted in `meta.counts` |

`meta.analysed_files` is the exact file set. **A tool is never charged for
anything outside it**: both sides are restricted to it before counting.

Vendored code that the build compiles (redis `deps/`) is *inside* the analysed
set. Use `--scope-prefix src/ --scope-prefix modules/` to score first-party code
only.

## Dynamic dispatch: candidate sets

A compiler cannot name the one function a dynamic call reaches, so a dynamic
call site is represented as the set of functions it may reach, one edge per
candidate, all sharing `(caller.file, site_line, site_col)` and carrying
`candidates: N`.

| language | kind | where the set comes from | trust |
|---|---|---|---|
| Go | `interface` | RTA: methods of every type that the reachable program converts to an interface the receiver satisfies | sound over-approximation (unsound only under reflection / `go:linkname`) |
| Go | `funcptr` | RTA: every address-taken function or closure of the same signature | very coarse: a `func()` value gets every `func()` in the program |
| Go | `static` + `candidate_reason: "generic-instantiation"` | the compiler's own resolution in each instantiation | exact for the instantiations the analysed (non-test) code creates |
| TypeScript | `interface` | classes that extend/implement the declaring type and define the member (`resolution: "class-hierarchy"`) | our own fold on top of `tsc`. Misses structural (object-literal) implementers |
| C | `static` / `macro` + `candidate_reason: "link-variant"` | one definition per program of the build that the calling object file is linked into | exact |
| C | (none in `edges.jsonl`) | function-pointer calls are listed in `unresolved_sites.jsonl` | |
| C | `funcptr` in `edges.funcptr_candidates.jsonl` | address-taken functions with the identical canonical type | heuristic, opt-in |

Sites RTA could not resolve (caller unreachable from any `main`) and sites
`tsc` stops at (a function-typed value, an `any`) have no edges. They appear in
`unresolved_sites.jsonl` and make their caller `judged: false`.

## Scoring

Implemented in `bench/scripts/oracles/score.py`. The rules below are the
specification: they are written so that a second implementation gives the same
numbers, and `score.py` is deterministic (two runs on the same input print the
same bytes).

### Tool input

JSON Lines, one edge per line. Each endpoint has `file` (required,
repository-relative, `/` separators) and at least one of `line` (1-based) and
`name`. `receiver` is optional and only used to narrow a name.

```json
{"caller": {"file": "cmd/root.go", "line": 132, "name": "initConfig"},
 "callee": {"file": "config/config.go", "line": 88, "name": "Translate"}}
```

What the tool's adapter must do before scoring, because the scorer will not:

* **Report the callee at its definition.** A C prototype in a header, a Go
  interface method declaration and a TypeScript overload signature or
  `declare` are not definitions and map to nothing.
* **Give lines.** A name without a line is accepted but is ambiguous wherever a
  file has two functions of that name (492 of syft's rows, 1,230 of zod's), and
  an ambiguous row earns credit for one pair only.
* Lines are 1-based. Duplicated rows, and call-site multiplicity, do not
  matter: everything is reduced to distinct function-level pairs. A recursive
  call (caller = callee) is an ordinary pair.

### 1. Scope

Drop a tool edge unless both files are in `meta.analysed_files` (and, with
`--scope-prefix`, both are under one of the prefixes. The same filter is applied
to oracle rows). Dropped tool edges are counted as `out_of_scope` and charged to
nobody.

### 2. Map each endpoint to an oracle record

Terms, for a record `f` of `functions.jsonl`: its **span** is
`[start_line or line, end_line]`. Its **header** is
`[start_line or line, max(line, name_line)]`. A tool `name` is reduced to its
last component before any comparison (`(*T).M`, `T.M`, `T::M`, `pkg.F` →
`M`, `F`). "Innermost" among several records whose spans contain a line means:
greatest start line, then smallest end line. Records that still tie are all
kept (the endpoint is ambiguous).

Position comes first. **When a line is given it decides. A name only narrows.
A line that maps nowhere is never rescued by searching the file for the
name.**

**Caller**

1. With a line: take the records, other than `kind: "class"`, whose span
   contains the line. If a name is given and some of them have it, the
   innermost of those, otherwise the innermost of all of them. None → the
   caller is unmapped. (So any line inside the function works: the declaration
   line, the call-site line, a line inside a closure.)
2. Without a line: every record in the file with that name, other than
   classes. If `receiver` is given and matches some of them (leading `*`
   ignored), only those.
3. Replace each record that has `nested_in` by the record it names: callers are
   compared at the outermost function.

**Callee**

1. With a line: take the records whose **header** contains the line.
   * Exactly one → that record, whatever name the tool gave (tools name
     constructors, default exports and methods differently).
   * Several (two declarations on one line) → those with the given name, or all
     of them if no name is given or none has it.
   * None → if a name is given, the innermost record with that name whose span
     contains the line, otherwise unmapped. A line in the middle of a body that
     names no enclosing function is a closure or a local: not a callee the
     oracle has.
2. Without a line: every record in the file with that name. `receiver` narrows
   as for callers.

Callees are not replaced by `nested_in`. A `callee_alt` endpoint of an oracle
row is accepted in place of its `callee` (`new X()`: class `X` or the
constructor that runs).

**Variable records** (`kind: "variable"`, TypeScript). Unless
`--include-variable-endpoints` is given: if every record an endpoint maps to is
a variable, the row is counted as `variable_endpoint` and dropped. Otherwise
variable records are removed from the endpoint's set.

**Unmapped** rows are counted as `unmapped_caller`, or else `unmapped_callee`,
and are in no denominator of `precision`.

### 3. Oracle tiers (function-level pairs)

Oracle rows are first filtered: out-of-scope rows, rows with
`callee_closure: true` (no declared symbol a tool could point at), and, unless
`--include-variable-endpoints`, rows with `caller_kind` or `callee_kind`
`"variable"` are set aside. `edges.funcptr_candidates.jsonl` is read only with
`--with-funcptr-candidates`. Each remaining row is the pair
`(caller id, callee id)`.

| tier | oracle rows |
|---|---|
| **must** | rows without a `candidates` field (`kind` `static` or `macro`) |
| **candidate** | rows with a `candidates` field: `kind` `interface` or `funcptr`, C link variants, Go generic instantiations. A pair that is also *must* through another row is *must* only. |

A **dynamic site** is a distinct `(caller, site_file, site_line, site_col)`
that has at least one candidate-tier row left after the filter above (rows
whose pair was promoted to *must* still count for their site). Sites whose
targets are all closures or all outside the repository are not dynamic sites.

### 4. One pair per tool row, then buckets

A mapped tool row gives a set of pairs (callers × callees, more than one only
when an endpoint is ambiguous). It stands for **exactly one** of them: the
smallest pair that is in *must*, else the smallest that is in *candidate*, else
the smallest of all. "Smallest" is by `(caller file, caller line, caller name,
callee file, callee line, callee name)`. Rows that stand for the same pair are
one pair.

| bucket | condition |
|---|---|
| `matched_must` | the pair is in *must* |
| `matched_candidate` | the pair is in *candidate* |
| `contradicted` | the pair is in neither, and **every** caller record the row's caller endpoint could map to is `judged` (for every row standing for that pair) |
| `unjudged` | the pair is in neither, otherwise |

### 5. Metrics

```
precision              = (matched_must + matched_candidate)
                         / (matched_must + matched_candidate + contradicted)
precision_strict       = the same, with unmapped_callee_distinct_with_judged_caller
                         added to the denominator
recall_static          = matched_must / |must|                       <- headline recall
recall_dynamic_sites   = dynamic sites with >= 1 of their pairs reported / dynamic sites
recall_candidate_edges = matched_candidate / |candidate|   (also per kind / candidate_reason)
recall_all_edges       = (matched_must + matched_candidate) / (|must| + |candidate|)
unjudged_share         = unjudged / distinct tool pairs
```

`unmapped_callee_distinct_with_judged_caller` counts distinct
`(smallest caller record, callee file, callee line, callee name)` among
`unmapped_callee` rows all of whose possible callers are judged.

Every rate is printed with its counts and a 95% Wilson interval.

Reading them:

* `precision` is *precision against the oracle*. `contradicted` is strong
  evidence of a wrong edge, not proof (reflection, `any`, casts of function
  pointers). `unjudged` is reported at full size and charged to nobody. A large
  `unjudged_share` means the oracle cannot speak about much of the tool's
  output.
* The oracle describes **one build configuration without tests**. A tool that
  reads the source text will also report calls from code this configuration
  does not contain. Those are `out_of_scope`, `unmapped` or `unjudged` by the
  rules above, never `contradicted`, as long as the function they sit in is
  not `judged`.
* `recall_static` is the number to compare tools on: every *must* edge is one
  the compiler resolved to exactly one target. On redis 4,512 of the must rows
  are `macro` rows, which a tool without a preprocessor cannot see. Report
  recall with and without them when that matters (`--dump` writes
  `missed_static.jsonl` with the `kind` of every missed row).
* Candidate tiers over-approximate, most of all Go `funcptr` (in a sample of
  40 syft rows, 4 of 7 `funcptr` rows and 1 of 6 `interface` rows could not
  happen at run time). A tool is not expected to reach 1.0 on
  `recall_candidate_edges`, and emitting every candidate is not what a reader
  wants. `recall_dynamic_sites` asks the fairer question: did the tool find
  *some* accepted target for the dynamic call.
* Never fold precision and recall into one F1, and never report one without
  the other.
* Do not compare recall across repositories. Compare tools within one.

### Comparing with repowise's published table

`--repowise-compat` reproduces the population of repowise-bench G4: Go edges
restricted to RTA-reachable callers, closure callees kept in the denominator,
dynamic candidates pooled with static edges into one recall, TypeScript limited
to what `tsc` alone resolves. With it the oracle has 1,584 function-level edges
on gitleaks (published: 1,584), 1,747 with tests (published: 1,747), 7,969 on
syft (published: 7,898, see README) and 1,271 on zod (published: 1,269. The
reference oracle was re-run and the difference is 5 edges it lacks and 3
self-edges it has through default parameter values, see README).
