# k3: independent verification of the question set

Verified on 2026-11-12 against `bench/repos/linux` at commit
`9d66f26fefacebfeed15de31c1123451a17ff119`, by a second engineer who did not write the
questions. Every gold answer in `questions.jsonl` was re-derived from the source with
ripgrep and reading. No codefast, ctags, cscope, clangd or other indexer was used, and
nothing was compiled.

| | |
|---|---|
| Input | `questions.jsonl`, 72 questions, sha256 `f0282a8e39eb771af833efe4e85abea5c42e4bbfd8e9ee83c8e840ed2dcc1b87` |
| Output | `questions.verified.jsonl`, 72 questions, sha256 `e1179ba7539e002823604bfccf496e1d503e4de4673f2a7406b4bfb0227e9c4d` |
| Checked | 72 |
| Confirmed (gold unchanged) | 67 |
| Corrected (gold changed) | 5 |
| Dropped | 0 |
| Replacement questions written | 0 (no family fell below 6) |

## How many gold answers were wrong

- **Primary answer wrong: 0 of 72.** No definition location, signature, caller set, callee
  set, table registration, dispatch site, path hop, log-string location or writer set
  pointed at the wrong place or missed a member.
- **Factual error in a secondary gold field: 2 of 72** (F1-03, F4-02). Neither changed
  which answer is right for the main question, but F4-02 changes scoring: a callee that was
  optional is now required.
- **Gold too strict for a correct answer: 3 of 72** (F2-03, F2-04, F2-05). The gold was true
  for the definition, but a correct answer quoting the function's prototype would have been
  scored wrong because the prototype uses different or no parameter names.
- **Set completeness** (the part most likely to be wrong): F3, 53 call sites in 46 caller
  functions. F6, 12 dispatch sites plus 3 same-slot sites. F9, 65 write lines in 60 writer
  functions. F4, 80 function callees, 25 macros, 3 pointer calls. My independent searches
  found **no missing and no spurious member** in any of them. The one F4 change is a
  reclassification of a name that was already listed.

## Counts per family

| Family | Checked | Confirmed | Corrected | Dropped | Usable |
|---|---|---|---|---|---|
| F1 definition | 8 | 7 | 1 | 0 | 8 |
| F2 signature / members | 8 | 5 | 3 | 0 | 8 |
| F3 direct callers | 8 | 8 | 0 | 0 | 8 |
| F4 callees | 8 | 7 | 1 | 0 | 8 |
| F5 ops-table dispatch | 8 | 8 | 0 | 0 | 8 |
| F6 indirect callers | 8 | 8 | 0 | 0 | 8 |
| F7 call path | 8 | 8 | 0 | 0 | 8 |
| F8 log string | 8 | 8 | 0 | 0 | 8 |
| F9 field writers | 8 | 8 | 0 | 0 | 8 |
| Total | 72 | 67 | 5 | 0 | 72 |

"Corrected" means the `gold` object changed. Two more records (F1-02, F6-05) have an
amended `notes` text with unchanged gold. They count as confirmed and the edit is listed in
their `verification.changes`.

## Corrections

### F4-02 `__do_fault`: `pte_alloc_one` is a function on x86, not a macro

The gold listed `pte_alloc_one` under `macros` (optional for scoring) with the note that it
is a macro wrapping `pte_alloc_one_noprof`. That is true only for architectures that use
the generic helper. x86 defines `__HAVE_ARCH_PTE_ALLOC_ONE`
(`arch/x86/include/asm/pgalloc.h:11`), which switches the generic macro off
(`include/asm-generic/pgalloc.h:88-101`), and provides a real function
`pgtable_t pte_alloc_one(struct mm_struct *mm)` at `arch/x86/mm/pgtable.c:16`. The set
states that the function-versus-macro split is made against x86 plus generic headers, so
the name belongs in `callees`.

Change: `pte_alloc_one` moved from `macros` to `callees` (9 to 10 required callees).
`gold.callee_aliases = {"pte_alloc_one": ["pte_alloc_one_noprof"]}` added so that a tool
that resolves the generic macro is not penalised. Note corrected. One evidence item added.

The author's check could not catch this: it searched for `#define NAME(` in generic and x86
headers and found the generic one.

### F1-03 ext4 `init_once`: 32 same-named definitions, not 31

Gold file and line (`fs/ext4/super.c:1523`) are right. `definitions_in_tree` was 31, taken
from `rg -l '^static void init_once\('`. `net/sunrpc/rpc_pipe.c:1219-1220` writes
`static void` and `init_once(void *foo)` on separate lines and is missed by that pattern.

Change: `gold.definitions_in_tree` 31 to 32. Note rewritten. One evidence item added.

### F2-03, F2-04, F2-05: accept the prototype's parameter names

| Question | Definition (gold, unchanged) | Prototype | Difference |
|---|---|---|---|
| F2-03 `copy_thread` (x86) | `arch/x86/kernel/process.c:170` `(struct task_struct *p, const struct kernel_clone_args *args)` | `include/linux/sched/task.h:83` | no parameter names |
| F2-04 `try_to_wake_up` | `kernel/sched/core.c:4330` `(struct task_struct *p, ...)` | `kernel/sched/sched.h:3951` | `tsk` instead of `p` |
| F2-05 `nvme_submit_sync_cmd` | `drivers/nvme/host/core.c:1224` `(..., void *buffer, unsigned bufflen)` | `drivers/nvme/host/nvme.h:1006` | `buf` instead of `buffer` |

Change: `gold.also_accept_prototype = {"file", "line", "must_contain"}` added to each.
`gold.must_contain` unchanged. A scorer should accept an answer that satisfies either list.
One note sentence and one evidence item added to each.

## Notes-only amendments (gold unchanged)

- **F1-02 `runqueues`.** The note named one `DECLARE_PER_CPU_SHARED_ALIGNED` line as the
  wrong candidate. There are two (`kernel/sched/sched.h:1448` and
  `include/linux/sched.h:2427`).
- **F6-05 `loopback_xmit`.** The note said the two extra `->ndo_start_xmit(` call sites
  "cannot reach" `loopback_xmit`. That holds for `ncm_tx_timeout` (its own gadget netdev).
  `rmnet_map_send_ack` calls the op of `skb->dev`, the lower device rmnet is attached to,
  and nothing in that function excludes `lo`. Scoring is unaffected because both sites are
  credited but not required.

## Method

All function attribution in this verification comes from a brace-depth scanner written for
it (`tools/verifier/vk.py`): comments, strings and preprocessor lines are stripped, then
brace depth is tracked and the function name is taken from the text before the opening
brace. It shares no code with the author's column-0 heuristic (`tools/enc.py`). The two
agree on every site in the set.

| Family | What was done for every question |
|---|---|
| F1 | Searched the name as a word in the whole tree (all file types), read each hit, confirmed one definition fits the question. |
| F2 | Read the definition. Checked each `must_contain` string against the source with whitespace collapsed. Read every prototype of the same name. |
| F3 | Listed every occurrence of the name as a word in the whole tree, `tools/` and `Documentation/` included. Each code occurrence followed by `(` must be a gold call site in the gold function, the definition, or a prototype. Occurrences without `(` are only `EXPORT_SYMBOL*` lines and comments, so none of the eight is used through a pointer. Probed for token-pasted spellings of the eight names: none. |
| F4 | Read the body. Re-extracted the `identifier(` tokens with the independent scanner: equal to `callees + macros + indirect` for all eight. For each of the 105 names, searched for `#define NAME(` and for a function definition in generic and x86 code. |
| F5 | Read the initializer and the function. Searched all uses of the function name and all `.member =` initializers in the subsystem. |
| F6 | Searched the member name as a word (not only `->member(`), so NULL checks, comparisons, copies and static-call updates were seen and told apart from calls. The set of call lines through each slot equals the gold sites. Confirmed each function has no direct caller. |
| F7 | All 39 hops: site line inside the `from` function, line calls `to` by name or through the stated member or parameter, registration line names `to` inside the stated table. Read the branch conditions where the question states one (loopback has no qdisc, PMU version, `IORING_OP_READ` entry). |
| F8 | Searched a shorter fragment of each message than the author used, in all file types. One producing site each. Checked that the quoted message can be produced (prefix from `pr_fmt` or `dev_warn`, `%s` arguments). |
| F9 | Listed every occurrence of the member name in scope. For five fields every occurrence was read (51 to 144 lines each). For `queue_count` all 65 occurrences under `drivers/nvme` were read and the other 43 files using the name were checked to be unrelated drivers. For `sid` (620 occurrences under `security/selinux`) and `sp` (929 under `arch/x86`) the lines matching a wider write pattern than the author's were read (108 and 64). Read the declaration that fixes each gold receiver's type. Checked each field declaration lies in the named struct. |

The mechanical part of this is `tools/verifier/recheck.py`. On `questions.verified.jsonl`
it reports `72 questions checked, 0 problems`. I corrupted 11 gold entries in a scratch
copy (one each in F1, F2, F5, F6, F7, F8, F9 and two each in F3 and F4). It flagged all 11.

The author's own checker also passes on both files: `python3 tools/verify.py --run` on
`questions.jsonl` printed `72 questions, 0 problems` (52 s on this machine under load), and
the same script run on a scratch copy of `questions.verified.jsonl` printed the same.

## Reproducing

```
cd bench/k3
python3 -I -B tools/verifier/make_verified.py     # rewrites questions.verified.jsonl (under 1 s)
python3 -I -B tools/verifier/recheck.py           # independent re-check, about 50 s
python3 -I -B tools/verifier/recheck.py questions.verified.jsonl --ids F3-01,F9-04
python3 -I -B tools/verifier/vk.py show wake_up_new_task     # every occurrence with its enclosing function
python3 -I -B tools/verifier/vk.py at kernel/sched/core.c 5861
shasum -a 256 questions.verified.jsonl
```

Versions: ripgrep 14.1.1 (rev 424a542a9c), Python 3.15.0, macOS arm64. The scripts use
`$RG_BIN`, then `rg` on PATH. ripgrep ran with `-j4`. Nothing was downloaded or installed. The Linux tree was only read. The only git command run in it
was `git rev-parse HEAD`.

Files added by the verification:

| File | What it is |
|---|---|
| `questions.verified.jsonl` | The 72 questions with `verification: {status, changes, verifier_notes, verified_on, commit}`. Five have a changed `gold`. |
| `VERIFICATION.md` | This file. |
| `tools/verifier/vk.py` | Independent helpers: comment stripper, brace-depth function scanner, ripgrep wrapper. |
| `tools/verifier/recheck.py` | Independent mechanical re-check of all nine families. |
| `tools/verifier/make_verified.py` | The verifier's decisions as data. Writes `questions.verified.jsonl`. |

Schema additions in the verified file: `verification` on every record,
`gold.also_accept_prototype` (F2-03, F2-04, F2-05), `gold.callee_aliases` (F4-02),
`"added_by": "verifier"` on the six evidence items I added.

## What was not verified, and remaining concerns

- **Text-level truth only.** Like the author, I did not compile or preprocess anything. A
  call or a write that exists only after macro expansion would be missed by both of us.
  I looked for token-pasted spellings of the F3 names and found none, but that probe is a
  heuristic. Compiler-level confirmation (for example from a build with
  `-fdump-rtl-expand` or clangd) would be the next step if the set is to be called ground
  truth in a paper.
- **Configuration.** F4's function-versus-macro split is for x86 with generic headers. Other
  callees in F4 change shape with config: several have a real version and an empty stub
  (`scx_tick`, `sched_core_tick`, `perf_event_task_tick`, `unmap_mapping_folio`). The F4-02 error came from this kind of difference and I cannot rule
  out that another architecture-specific override exists for a name I checked only against
  x86.
- **F9 scope.** For `sid` and `sp` I did not read every occurrence, only the write-like
  lines found by my pattern. A write through an alias taken on an earlier line
  (`u32 *p = &isec->sid; ... *p = x;`) is found at the address-of line only. Zeroing by allocation,
  `memset`, whole-struct copies and static initializers are outside the question's
  definition of a writer, as the author states.
- **F6 `complete: true`** means every textual call through the member is listed. It does
  not prove run-time reachability, and F6-05's rmnet site shows that "cannot reach" claims
  about same-slot sites should be read as "not the intended path".
- **F7** gold is one valid chain per question. I verified that chain, not the notes' lists
  of alternative chains.
- **Notes.** I checked only those claims in `notes` that I came across while verifying
  gold, and fixed two. I did not check every sentence of every note.
- **Difficulty labels** and the suggested scoring rules were not evaluated. No scorer exists
  yet. The F2 corrections show that string-containment scoring needs accepted alternatives,
  and the same may apply to F8 `format` strings if a scorer compares them.
- **Evidence commands have no path argument.** Most `evidence[].cmd` strings are
  `rg ... -g ...` without a directory. ripgrep reads standard input instead of searching
  the tree when stdin is a pipe or a file, so these commands hang when pasted into a
  non-interactive shell. Run them from the tree root with ` . </dev/null` appended (the
  author's `tools/k3lib.py` already closes stdin). The commands I added name their files.
- **Popular functions.** Unchanged from the author's README: a handful of well-known names
  can be answered from memory by a model, which matters for a token benchmark.
- **One reviewer.** This is a second pair of eyes, not a third. The corrections above were
  not reviewed by anyone else.
