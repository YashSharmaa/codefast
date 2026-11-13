# k3: kernel code-navigation questions with verified answers

72 questions about the Linux kernel source, 8 in each of 9 families, each with a gold answer
and the evidence that proves it. The set is meant for a per-question token-efficiency
benchmark: give the same question to an agent that has codefast, to one that has another
code-intelligence tool, and to one that only has grep and file reads, then compare
correctness against `gold` and the tokens each one spent.

| | |
|---|---|
| Tree | `bench/repos/linux` |
| Commit | `9d66f26fefacebfeed15de31c1123451a17ff119` (Makefile: 7.4.0-rc2, "Merge tag 'vfs-7.4-rc3.fixes.2'") |
| Questions | `questions.jsonl`, 72 lines, sha256 `f0282a8e39eb771af833efe4e85abea5c42e4bbfd8e9ee83c8e840ed2dcc1b87` |
| Written | 2026-11-12 |

All file paths are relative to the kernel tree root and all line numbers are for that commit.

## Files

| File | What it is |
|---|---|
| `questions.jsonl` | The benchmark. One JSON object per line. |
| `tools/build_questions.py` | The gold answers as data, plus the checks that run while the file is written. Regenerates `questions.jsonl`. |
| `tools/verify.py` | Re-checks `questions.jsonl` against the tree. `--run` also re-runs every evidence command. |
| `tools/k3lib.py` | Shared helpers: run a command, read a line, compare. |
| `tools/enc.py` | Finds the function that encloses a line (brace at column 0), lists `name(` tokens in a function body. |
| `tools/fieldw.py`, `tools/recv.py` | Authoring helpers for F9: list write-like uses of a member name, show a receiver's declaration. |

## Record format

```
{"id": "F3-01", "family": "F3", "subsystem": "kernel/sched",
 "question": "Which functions call wake_up_new_task()?",
 "symbol": "wake_up_new_task",
 "gold": { ...family-specific... },
 "evidence": [{"cmd": "<rg or sed command, run from the tree root>",
               "lines": ["path:line: source text", ...], ...}],
 "difficulty": "easy" | "medium" | "hard",
 "notes": "traps, accepted alternatives, scope"}
```

`subsystem` is an extra key (one of `fs`, `mm`, `net`, `block`, `drivers`, `kernel/sched`,
`arch/x86`, `security`, `io_uring`). Source text in `lines` has runs of whitespace collapsed
to one space. An evidence item whose `cmd` starts with `sed -n '<line>p' <file>` is a list of
single lines to read (one `sed -n 'Np' file` each), not one literal command.

## Families and gold format

| Family | Question | Gold | Suggested scoring |
|---|---|---|---|
| F1 definition | where is X defined | `{"file", "line"}` (+ `kind`, `ambiguous_name`, `definitions_in_tree` where relevant) | file equal and line equal (allow the answer to point anywhere in the definition's header). A prototype or `DECLARE_*` line is wrong. `notes` names the wrong candidates. |
| F2 signature / members | signature of X, fields of struct X | `{"file", "line", "must_contain": [...]}` | every `must_contain` string present in the answer after collapsing whitespace |
| F3 direct callers | which functions call X | `{"callers": [{"file", "function", "lines"}], "complete": true, "call_sites", "definition"}` | precision and recall over the set of (file, function) |
| F4 callees | what does X call directly | `{"callees": [...], "macros": [...], "indirect": [{"expr", "slot", "line"}], "complete": true, "file", "line", "end_line", "function_lines"}` | recall over `callees`. Names in `macros` and `indirect` are neither required nor penalised. Anything else is a false positive |
| F5 ops-table dispatch | which function implements `.op` of table T, or which table/member holds F | `{"function", "table", "member", "file", "line", "function_def", "direction"}`. `file`/`line` is the initializer line inside the table | `direction: table->function`: function name (see `also_accept_function`), `function->table`: table and member |
| F6 indirect callers | F is only called through a pointer. Where is it dispatched | `{"slot": "struct.member", "dispatch_sites": [{"file", "function", "line"}], "complete", "registration", "other_sites_same_slot"?}` | slot named and recall over `dispatch_sites`. `other_sites_same_slot` are real calls through the same member that cannot reach F, credited but not required |
| F7 call path | how does execution get from A to B | `{"path": [...], "alternatives_ok": true, "hops": [{"from", "to", "kind", "site", "via"?, "registration"?}], "crosses_ops_table"}` | a chain from `path[0]` to `path[-1]` in which every hop is real. The gold path is one such chain, `notes` lists known alternatives |
| F8 log string | where does message "..." come from | `{"file", "line", "function", "format"}` (+ `also_accept_lines`, `also_accept_function`) | file and function, line as a bonus |
| F9 field writers | which functions write field F of struct T | `{"struct", "field", "writers": [{"file", "function", "lines"}], "complete": true, "asm_writers"?}` | precision and recall over (file, function). A writer of the same-named field of another struct is a false positive |

Hop `kind` in F7 is `call` (direct call at `site`), `ops_dispatch` (call through a table member
at `site`, with the table initializer in `registration`) or `callback_arg` (call through a
function-pointer parameter, with the line that passes the function in `registration`).

### What the set contains

- F1: three macro-defined symbols (`sys_io_uring_enter` via `SYSCALL_DEFINE6`, `runqueues` via
  `DEFINE_PER_CPU_SHARED_ALIGNED`, `exc_page_fault` via `DEFINE_IDTENTRY_RAW_ERRORCODE`), one
  function-like macro, two names that need disambiguation (ext4's `init_once` out of 31, e1000e's
  `e1000_xmit_frame` versus the legacy e1000 one), two plain functions.
- F2: five function signatures (one arch-disambiguated: x86 `copy_thread`), three structs.
- F3: 3 to 12 call sites each (3 to 10 distinct callers), including callers that are static inline
  functions in headers.
- F4: functions of 26 to 78 lines with 3 to 25 direct function callees. Three of them also make
  one call through a function pointer.
- F5: five table-to-function and three function-to-table questions, including a table declared
  through a macro (`DEFINE_SCHED_CLASS(fair)`), an LSM hook list, an opcode-indexed array, and a
  token-pasted initializer (`vt_op(vcpu_run)`).
- F6: slots dispatched through `->member(`, through `.ops.member(`, through static calls
  (LSM hooks, `x86_pmu_handle_irq`) and through a forwarding wrapper (`shm_fault`).
- F7: 3 to 6 hops. Six of the eight paths cross an ops table, an LSM hook or a callback argument.
- F8: eight messages. Seven have format specifiers, one (`I/O error, dev ...`) starts with text
  that is not in the format string at all.
- F9: member names shared by several structs (`on_rq`, `nr_requests`, `mnt_ns`, `sid`,
  `locked_vm`, `queue_count`, `sp`, `cq_entries`), 2 to 14 writers each.

Count by subsystem: arch/x86 9, io_uring 9, kernel/sched 9, mm 9, block 8, fs 8, drivers 7,
security 7, net 6. Count by difficulty: easy 22, medium 30, hard 20.

## How the gold answers were derived

No indexing tool was used: not codefast, not ctags/cscope/clangd/LSP, not a compiler. The
answers come from `rg` searches and from reading the source.

1. Candidates were picked by reading the tree and searching with `rg`.
2. Each answer was read in the source: definitions and signatures directly, callers by listing
   every textual occurrence of `name(` in the tree and looking at each hit, callees by reading
   the whole function body, table registrations and dispatch sites by reading the initializer
   and the call line, each hop of a path by reading the call inside the calling function, field
   writers by listing every write-like use of the member name and checking the declared type of
   each receiver.
3. The answers were typed into `tools/build_questions.py`. When that script writes
   `questions.jsonl` it re-checks all of it against the tree and aborts on any mismatch:
   - every evidence command is executed and must print every line recorded for it.
   - every cited line must contain the text recorded next to it.
   - F3: every hit of `\bNAME\s*\(` outside `Documentation/` and `tools/` must be a listed call
     site (and lie inside the listed function), the definition, a declaration, or a comment. A
     second search confirms the name is never used without `(` (no address-taken uses).
   - F4: the set of `identifier(` tokens in the body (comments and string literals removed) must
     equal `callees + macros + indirect`.
   - F6, F7, F8: each site line must lie inside the function the gold names.
   - F9: every hit of the write pattern must be a listed writer line or match a labelled
     other-struct or read pattern. A per-question guard regex makes the build fail if a hit that
     looks like a write to the target struct is not listed as a writer.

"Inside the function" is decided by a brace-at-column-0 heuristic (`tools/enc.py`), which relies
on kernel coding style. The F4 function-versus-macro split was made by searching for
`#define NAME(` in generic and x86 headers and for a function definition of each remaining name.

The F9 write pattern is:

```
rg -n -e '(->|\.)\s*F\s*(=($|[^=])|\+=|-=|\|=|&=|\^=|<<=|>>=|\+\+|--)' \
      -e '(\+\+|--)\s*[\w>.\-\[\]()]*(->|\.)F\b' \
      -e '&\s*[\w>.\-\[\]()]*(->|\.)F\b' \
      -e '(WRITE_ONCE|rcu_assign_pointer)\(\s*[\w>.\-\[\]()]*(->|\.)F\s*,' \
      -g '*.[chS]' -g '!tools' -g '!Documentation' [dir]
```

## Reproducing

```
cd bench/k3
python3 tools/verify.py            # schema, cited lines, gold-versus-source checks (about 1 s)
python3 tools/verify.py --run      # also re-runs all evidence commands (about 30 s)
python3 tools/build_questions.py   # regenerates questions.jsonl (about 40 s)
shasum -a 256 questions.jsonl
```

Both scripts read the tree from `$LINUX` (default as above) and never write to it. They need
ripgrep: `$RG_BIN` if set, else `rg` on `PATH`.

Versions used: ripgrep 14.1.1 (rev 424a542a9c), Python 3.15.0, GNU bash 3.2.57, BSD sed,
macOS arm64. The scripts run ripgrep with `-j4`. Nothing was downloaded or installed.

Result on 2026-11-12: `verify.py --run` reports `72 questions, 0 problems`.

To check one question by hand, run the commands in its `evidence` from the tree root and
compare with `lines`.

## Known limitations

- **Text-level truth, not compiler truth.** Nothing was compiled or preprocessed. A call that
  exists only after macro expansion with token pasting would be missed by the F3 searches (none
  is known for the eight chosen functions). F4 lists what the body calls textually. What a callee
  macro expands to is not followed.
- **Configuration.** The tree is read as source, with every `#ifdef` branch visible. F4's
  function-versus-macro split is for x86 plus generic headers. Several callees have a real
  version and an empty stub depending on config (`scx_tick`, `sched_core_tick`,
  `perf_event_task_tick`, ...). F5-05 depends on `CONFIG_KVM_INTEL_TDX`. Both outcomes are in the
  gold.
- **Scope of "complete".** F3 and F9 ignore `Documentation/` and `tools/` (selftests, userspace
  copies of headers). F9-04 searches only `security/selinux` and F9-07 only `arch/x86`, with the
  reason given in the notes. F6 `complete` means every textual call through that member name was
  listed. It does not prove which sites can reach the function at run time.
- **F9 counts explicit writes only.** Not counted: zeroing by `kzalloc`/`memset`, whole-struct
  copies (for example `dup_mm()` copies the parent `mm_struct`, including `locked_vm`, before
  `mm_init()` resets it), static initializers (listed in the notes where they exist), and writes
  made later through a pointer whose address was taken (the address-of site itself is listed).
  F9-07 has an assembly writer (`__switch_to_asm`), reported separately in `asm_writers`.
- **Enclosing-function heuristic.** Function attribution in the checks assumes the opening brace
  of a function is at column 0. Every site was also read by hand, but the automated check and
  the manual one share that assumption.
- **F7 paths are one valid chain.** Other chains exist for most of them. `notes` lists the ones
  seen while reading. A scorer must validate hop by hop, not compare with the gold string.
- **F8 line numbers** point at the source line that holds the (start of the) format string, which
  for multi-line calls is not the line of the `printk`/`dev_warn` call itself.
- **Difficulty labels are judgement calls**, not measurements.
- **No second reviewer.** The checks are mechanical and were written by the same author as the
  answers.
- **Pinned tree only.** Line numbers are valid for commit 9d66f26. On this macOS checkout about a
  dozen netfilter files show as modified because their names differ only by case
  (`xt_CONNMARK.h` / `xt_connmark.h`, ...). No question cites them.
- **Popular functions.** A handful of well-known names appear (`handle_mm_fault`,
  `try_to_wake_up`, `submit_bio`, `vfs_get_tree`, `exc_page_fault`), well under a quarter of the
  set. A model may answer some of those from memory, which matters for a token benchmark.

## Independent verification (added 2026-11-12 by a second engineer)

Use `questions.verified.jsonl` for scoring, not `questions.jsonl`. It holds the same 72
questions with a `verification` object on each: 67 were confirmed and 5 had their gold
corrected (F1-03, F2-03, F2-04, F2-05, F4-02), none was dropped. `VERIFICATION.md` has the
counts per family, the corrections, the method and what was not verified. The verifier's
scripts are in `tools/verifier/`:

```
python3 -I -B tools/verifier/make_verified.py   # rewrites questions.verified.jsonl
python3 -I -B tools/verifier/recheck.py         # independent re-check, about 50 s
```

`tools/build_questions.py` and `tools/verify.py` still describe and check the original
`questions.jsonl`. The "No second reviewer" limitation above no longer applies.
