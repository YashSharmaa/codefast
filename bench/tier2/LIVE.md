# Live sessions: protocol

The earlier pilot (`PROTOCOL.md`) ran sub-agents with the guide pasted into the prompt. This
experiment runs the real thing: headless Claude Code sessions (`claude -p`), with and without
the codefast plugin, in the state a user would have it. `live.py` runs and scores it.

## What a run is

One session per task and arm. The prompt is the same in every arm: nothing in it says that a
tool is installed. An arm differs only in what the harness loads.

| Arm | What is loaded |
|---|---|
| `control` | nothing |
| `plugin` | the plugin as `claude plugin install` gives it: hooks, the skill, the MCP server, the command on `PATH` |
| `lean` | the plugin without its MCP server |
| `nohooks` | the plugin with `CODEFAST_HOOKS=off` |
| `uses-count`, `uses-list` | the plugin, with one form of the line about a symbol's uses forced (see "What a card should say") |
| `oracle` | nothing of codefast, and a hook of the task's own that states the file and function of the real fix (see "What a bug fix can save at best") |
| `oracle-context` | the same, and what `codefast context` knows about that function |

Every session runs with the project's settings only (no user plugins, no cloud connectors), in
bypass-permissions mode inside a private copy of the repository, with the web tools and
anything that outlives the session switched off. Tasks that change nothing (questions) share
one checkout and one index. The plugin arms start with an index that exists, which is the state
of a project from its second session on.

## Tasks

| Family | Task | Checked by |
|---|---|---|
| Z | The same question in TypeScript: functions and methods of zod whose name something else in the project shares | the call graph the TypeScript type checker built (`bench/oracles/zod`), scored the same way |
| N | Which functions call this method? Methods of a Go repository whose name several types share, so that a text search finds more than the answer | the call graph the Go type checker built (`bench/oracles`): F1 of the callers named with their file, against all real callers and against the callers of the other types' methods of that name |
| L | Four N questions in one session | the same, per question |
| E | A run ended with this error message: which function creates it, and which functions call that function? The message stands in one place of the source, written out in full | the function around that line and its callers in the type checker's call graph |
| K | Questions about the Linux kernel from `bench/k3` | the verified answers of `bench/k3` |
| D | Make failing tests pass: issues of Django and sqlglot from Loc-Bench, at the commit before the fix, with the tests of the fix added | the tests of the fix pass and are unchanged |
| I | The same issues without their tests: the issue text alone | the tests of the real fix, copied into the tree after the session, pass |
| R | Rename one Go method everywhere, where other types have a method of the same name | `go build` and `go vet` pass, and no other definition was renamed |
| C | Which functions of this repository does this function call? The mirror of N, on go-git | the call graph the Go type checker built: F1 of the callees named with their file on one line, against all real callees and against functions that have the name of a callee and are another |
| F | Which files does this kernel bug report point to? A report of LinuxFLBench in a checkout of the kernel version it was filed against. The session ends with up to ten files, the most likely first. The same is asked about issues of Python projects from Loc-Bench | the one file the real fix changed: whether it is first in the list, and whether it is in it (the two figures of arXiv 2611.07286) |
| W | Write code: one well-tested function of gitleaks has its body replaced by a panic, and the session has to write it again from what the rest of the project expects of it | the tests of that function's package pass, and no test file and no file of test data changed |

`make_n_tasks.py`, `make_c_tasks.py`, `make_e_tasks.py`, `make_d_tasks.py`, `make_i_tasks.py`,
`make_r_tasks.py`, `make_w_tasks.py` and `make_f_tasks.py` build them. None of the builders uses codefast to choose a task or to
judge an answer. A W task has to compile with the body gone and has to fail its package's
tests, and the package's tests have to pass with the body there, so the failing tests are the
whole of what the session is told.

## Development tasks and held-out tasks

Hooks are shaped by reading sessions, and a figure measured on the sessions that were read says
little. So the tasks are split.

* **Development tasks**: N, L and R on syft, 18 kernel questions and 12 bug-fix tasks. Every
  session of these may be read, and what the hooks say may be changed because of what was
  seen. The table "The tasks the hooks were developed on" in `BENCHMARKS.md` is from these. A few
  more (`DEV_IDS` in `live.py`) were used before any table existed and are never reported.
* **Held-out tasks** (`held_out` in `tasks.json`): tasks no session had been run on. N, L, R
  and half of E are on go-git, a repository nothing in codefast had been run on. The others are
  18 more kernel questions, the 15 bug-fix tasks not used before (as D and as I), E on syft,
  and the 8 code-writing tasks (W), which no build was looked at on at all. Later sets were
  added the same way: 12 more code-writing tasks on go-git (W2), the 18 kernel questions that
  were left (K2) and 12 callee questions on go-git (C). They are run when a build is final,
  with `live.py run --held K,N,E,R,D,I,L,W` and the names of the later sets. Once a held-out
  session has been read, its task is a development task for every later build. Every set here
  has now been run, so the next change to what the hooks say needs new tasks to be judged on.
  Family F came after that: 40 kernel bug reports to develop on, from the half of LinuxFLBench
  the file ranking was developed on, and 48 held-out ones from the other half. Two more of
  that half were read while the harness was made and count as development. 81 reports of it
  have not been run in a session. The 40 issues of Python projects (`--held F2`) were run once,
  with the released plugin, and nothing was developed on issues of that kind.

Two things were looked at on held-out tasks before the batch of version 0.2.2 ran, and neither
changed the build: whether the hook printed anything at all for a prompt (a count per family),
and what `codefast rename` does on the ten rename tasks without a model.

## What is measured

All of it comes from the session's own event stream (`--output-format stream-json`):

* requests, tool calls by tool, characters of tool output, hook events that added context
* tokens by kind, and the cost at list prices that the harness reports
* **warm cost**: the session priced from the size of each request alone, as if the cache held
  everything an earlier request had sent. Cache reads count 0.1, new tokens 1.25 and output 5
  times an input token. This is the figure the tables report.

Why not the billed cost: it depends on whether the harness's own prompt happened to be in the
cache when a run started. In one batch the same arm cost 30% more or less for that reason
alone. The warm cost is what a session costs for anyone who started another one within the
hour, and it is the same formula for every arm.

## The runs behind the tables in the README

**New projects and development conversations, 14 and 15 November 2026**
(`bench/results/live-new-project.jsonl` and `bench/results/live-conversation.jsonl`, Haiku 5.5,
two runs per task and arm, builds of the main branch after 0.3.1). The `tag` field says which
batch a run is from, and an arm that ran a later build or another setting has a name of its
own, so that its runs are never averaged with the runs they are compared to:

* New projects: `g1` (the four to develop on, control and plugin), `g2` (the six held out,
  both arms), `g3` (the six held out with the build of 15 November, arm `plugin-b`, no control
  of its own) and `ge-low` (the four to develop on at `--effort low`, arm `control-low`).
* Conversations to develop on: `s1` and `s1n` (control and plugin), `s1f` (a session for each
  message), `sac` and `sac125` (windows of 100,000 and 125,000), `skeep` (the window of 125,000
  with transcripts kept, arm `plugin-ac125-keep`) and `se-low` (that window at `--effort low`,
  arm `control-ac125-low`).
* Held-out conversations: `s2` (control and plugin, the build of the morning of 15 November),
  `s2ac` (the window of 125,000, arms `control-ac125` and `plugin-ac125-keep`) and `s2n` (the
  build after the hooks stopped commenting on code that is being written, arms `control-b`
  and `plugin-b`, run in pairs).
* A prototype that was not kept: `sdel` (conversations to develop on, a hook that hands a job
  to a sub-agent near the price step, arm `plugin-handover`, three runs).

The sections "Building a project from nothing" and "A development conversation" below read
them. Three hidden tests asked for more than their specification says (a nil slice against an
empty one twice, leading spaces once). They were relaxed on 15 November and every run graded
again: `bench/tier2/new/README.md` and `bench/tier2/staged/README.md` say which.

**TypeScript, version 0.2.6** (`bench/results/live-heldout-haiku.jsonl`, tag `nz1`): the same
kind of question on zod, a repository no session had used, two runs per task and arm. A session
costs 0.53 of the control and is cheaper in 8 of 8 pairs, with the answers as good. Four of the
eight questions are about the `create` of a v3 class, a name 34 other classes carry: both arms
score about 0.4 on those and 1.00 on the rest, which is where the 0.62 in the table comes from.

**Kernel questions, version 0.3.0** (`bench/results/live-heldout-haiku.jsonl`, tag `k2new`): the
18 of the 72 verified questions that no session had been run on, four runs per task and arm.
The README's kernel row is this batch. The row it replaces (0.86, tag `h1`, version 0.2.2) was
the 18 questions held out before these, and its sessions have been read since. The same 18
questions were run with the plugin of 0.2.10 right after, tag `k2old`, and all three arms are in
`bench/results/live-answer-cards.jsonl`. The section "A card that answers the question" reads
them.

**Callee questions, version 0.3.0** (`bench/results/live-heldout-haiku.jsonl`, tag `cnew`):
twelve functions of go-git, four runs per task and arm, and the plugin of 0.2.10 on the same
twelve after it (tag `cold`, in `live-answer-cards.jsonl`). The twelve prompts were used
without a model while the card for them was made: how "the method X of Y" is read was fixed on
them, and the card was scored against the compiler's lists. No session was run on them until
the build was final.

**Writing code, versions 0.2.9 and 0.2.10** (`bench/results/live-heldout-haiku.jsonl`, tags
`w1` and `w2h`): eight functions of gitleaks on 0.2.9, and twelve of go-git on 0.2.10, whose
hooks are those of 0.2.9. Four runs per task and arm. No session of this family had been run
before, on any build, so nothing in the plugin was shaped on it. Two of the gitleaks tasks
were run once beforehand to see that the harness worked, and only their pass or fail was
looked at. The go-git tasks were built to check the gitleaks result and run once, after the
build was final. The section "Writing code" below reads both.

**Held-out tasks, version 0.2.2** (`bench/results/live-heldout-haiku.jsonl`, tag `h1`): one
batch of both arms on Haiku 5.5, two runs per task and arm, interleaved so that a change in the
account's speed during the batch falls on both arms alike. A figure of the table is the mean
over the tasks of a family, each task being the mean of its runs. The sessions of four
questions (L) were run right after it with the same build, tag `h1l`.

The bug-fix rows of the README's table are four runs of each arm on the released build (tags
`h1`, `d4` and `prec1` for D, `h1`, `i4`, `i5` and `inc` for I), the rest two runs on 0.2.2,
whose hooks answer those shorter prompts in exactly the same way.

Of the 328 runs of that batch, 144 ended in an error from the API with no tokens used, in both
arms alike. They were deleted and run again the same day, when the API answered normally. The
run of a task that had succeeded was kept. Nothing else was re-run.

One change to the build came after the batch: `rename` now tells a Go type from a method of
the same name. It was checked to leave all ten rename tasks exactly as they were, by the
compiler and in what the card says, so the table stands for the released version.

While that batch ran, some of its sessions were read, how often the prompt hook printed
anything was counted per family, and the rename command was tried on the ten rename tasks
without a model. The build was not changed.

**Development tasks, version 0.2.0** (`bench/results/live-haiku.jsonl`, one run per task and
arm, the `tag` field says which batch a run is from):

* Bug fixes (D) and kernel questions (K): one batch of both arms.
* Caller questions (N), sessions of four (L) and the control arm of the renames (R): a second
  batch of both arms, run after the receivers of Go calls were typed further. The first batch
  had these at 0.49, 0.30 and 1.11 of the control.
* The plugin arm of the renames: a later run with `codefast rename`, after the card for a
  rename request was reworded once. With the first wording the agent ran the command in 4 of 8
  sessions (0.56 of the control), with the second in 8 of 8.

The held-out tasks that gain (N, L, E and R) were run again with Sonnet 5.5, both arms, one run
per task and arm: `bench/results/live-heldout-sonnet.jsonl`. N, L and R of the development
tasks were run with Sonnet too, at version 0.2.0: `bench/results/live-sonnet.jsonl`.

`bench/results/rename-offline.jsonl` and `rename-offline-go-git.jsonl` are the checks of the
rename command without a model (`rename_offline.py`), `rename-sample*.jsonl` the random samples
judged by the compiler, and `by-name.jsonl` the count of matches by name against the type
checker.

## Writing code

Every family above asks the session something. This one asks it to write: a function has its
body replaced by `panic("not implemented")`, and the session writes the body again. What the
function has to do is in the repository and nowhere else: its signature, its doc comment, the
table it is registered in, the places that call it and the tests that reach it.

It was measured on gitleaks first, eight functions. The result was a saving, and a family of
eight tasks on one repository is not much to stand on, so twelve functions of go-git were
built with the same builder and run once. They said the opposite.

| | gitleaks (8) | go-git (12) | Both (20) |
|---|---:|---:|---:|
| Cost with codefast | 0.95 | 1.08 | **1.03** |
| The same at 95%, resampling tasks and runs | 0.83 to 1.08 | 0.97 to 1.19 | 0.94 to 1.12 |
| Median task | 0.90 | 1.12 | 1.02 |
| Tasks cheaper with codefast | 6 of 8 | 3 of 12 | 9 of 20 |
| Requests, without and with | 7.5 and 5.9 | 7.2 and 7.4 | 7.3 and 6.8 |
| Tokens written by the model, without and with | 3,512 and 3,554 | 3,779 and 4,294 | 3,672 and 3,998 |
| Sessions whose function passes its tests | 32 and 32 of 32 | 48 and 46 of 48 | 80 and 78 of 80 |

So writing a function costs the same with the plugin as without it. Neither repository's
figure can be told from 1.00 at this size, nor the two from each other, and the first eight
tasks alone would have gone into the README as a saving. They did, for some hours.

The two sessions that failed are both of the plugin arm, on tasks the control solved four
times of four. In each the model wrote a body, saw a test fail, and stopped with an account of
why the project's own tests contradict each other. Nothing codefast had said in them was
wrong. Two of 48 against none of 48 is not a difference a batch of this size establishes, and
it is in the table because it happened.

### Where the cost goes: a call that does not replace the reading

The 80 plugin sessions split in two, each compared with the control runs of its own task:

| Plugin sessions | Sessions | Cost against the control | Requests against it | Tokens written |
|---|---:|---:|---:|---:|
| the card for the prompt, and no call to codefast | 55 | **0.96** | 0.9 fewer | 1.02 |
| the card, and a call to codefast as well | 25 | **1.12** | 0.2 more | 1.19 |

On gitleaks the two rows are 0.86 and 1.11, on go-git 1.04 and 1.13. A session that struggles
may be what reaches for a tool, so the second row is not the price of a call. But the sessions
show what a call bought. In one, the agent had to write `Branch.marshal` and asked for
`Remote.marshal` to copy from. There is no such method (the type is `RemoteConfig`), the
answer was "no matches", and the session went on to read the same lines with `grep` and `sed`
that the control read, one request later. In a navigation task the answer of a call is the
answer of the task. Here it is one more thing read before the file that has to be read anyway.

That dead end is closed in 0.2.10: a member asked for on a type that does not have it is
answered with the member of the type whose name is like it (`RemoteConfig.marshal`, with its
body), or with every type that has one. It was not measured in sessions. A third of these
sessions call the tool at all, so it can move the family by a few percent at most, which is
inside the band of the table above and would take ten times the runs to see.

What the card does for this family is small and real where the function hangs in a table: for
`decodePercent` of gitleaks it says the function is registered as `encoding.decode` in
`encodings` and reached through that table from `findEncodedSegments`, which no text search
for its name finds. The functions of go-git here are mostly methods with common names
(`Write`, `get`, `Match`, `marshal`), and for those the card has a count and a command.

| gitleaks | Lines taken out | Cost with codefast | Requests, without and with | Sessions that called codefast |
|---|---:|---:|---|---:|
| `Detector.DetectReader` | 24 | 0.76 | 6.2 and 3.5 | 0 of 4 |
| `decodePercent` | 29 | 0.85 | 6.0 and 4.0 | 0 of 4 |
| `ViperConfig.parseAllowlist` | 44 | 0.87 | 12.8 and 10.5 | 2 of 4 |
| `Detector.StreamDetectReader` | 31 | 0.88 | 5.2 and 3.2 | 0 of 4 |
| `Allowlist.CommitAllowed` | 15 | 0.92 | 7.5 and 5.2 | 1 of 4 |
| `checkCommitOrPathAllowed` | 43 | 0.93 | 6.8 and 5.5 | 0 of 4 |
| `Config.extendPath` | 18 | 1.04 | 8.8 and 7.5 | 4 of 4 |
| `toOriginal` | 34 | 1.16 | 6.8 and 7.8 | 2 of 4 |

| go-git | Lines taken out | Cost with codefast | Requests, without and with | Sessions that called codefast |
|---|---:|---:|---|---:|
| `Encoder.encodeOptions` | 14 | 0.84 | 7.5 and 5.8 | 0 of 4 |
| `RefSpec.matchGlob` | 13 | 0.85 | 4.5 and 4.0 | 0 of 4 |
| `ctxWriter.Write` | 54 | 0.93 | 8.0 and 6.5 | 0 of 4 |
| `hexDecode` | 13 | 1.01 | 8.2 and 7.8 | 0 of 4 |
| `bracketSet.addRange` | 14 | 1.02 | 3.5 and 4.0 | 0 of 4 |
| `matcher.Match` | 24 | 1.08 | 14.5 and 13.0 | 3 of 4 |
| `bracketCache.get` | 22 | 1.16 | 3.2 and 5.2 | 2 of 4 |
| `Config.Marshal` | 26 | 1.16 | 8.8 and 9.8 | 0 of 4 |
| `Config.unmarshalUser` | 12 | 1.17 | 7.0 and 7.8 | 3 of 4 |
| `Branch.marshal` | 32 | 1.19 | 6.2 and 7.0 | 3 of 4 |
| `newIter` | 15 | 1.19 | 7.8 and 8.2 | 2 of 4 |
| `decodeLine` | 55 | 1.24 | 7.5 and 9.5 | 3 of 4 |

What the model writes is 0.36 to 0.40 of the cost of such a session and is as long either
way, so there was never much to take out. Both sets of tasks are development tasks for every
later build, like any held-out family that has been read.

### The same split in every family

The table of the README, with each plugin session put in one of two rows by whether the agent
called codefast in it, and compared with the control runs of its own task
(`bench/results/live-heldout-haiku.jsonl`):

| Family | Sessions with hooks only | Their cost | Sessions that called codefast | Their cost |
|---|---:|---:|---:|---:|
| Who calls this method? (N) | 0 | | 24 | 0.41 |
| The same in TypeScript (Z) | 0 | | 16 | 0.64 |
| Four such questions (L) | 0 | | 6 | 0.16 |
| Where does this error come from? (E) | 1 | 0.64 | 23 | 0.67 |
| Rename a method (R) | 1 | 0.91 | 19 | 0.53 |
| A kernel question (K) | 9 | 0.82 | 27 | 0.97 |
| Write a function (W) | 55 | 0.96 | 25 | 1.12 |
| Fix a bug, tests given (D) | 57 | 0.93 | 3 | 1.39 |
| Fix a bug, issue alone (I) | 57 | 0.99 | 3 | 1.00 |

These are means of single sessions, so they are not the pooled figures of the README, and a
session that struggles is more likely to reach for a tool, so a row is not what a call costs.
What the table does show is where the saving is made. Where the task is a question about
structure, every session calls the tool and the call is the answer. Where the task is the
model's own work, or code it knows by heart, most sessions never call it, what the hooks say
is the whole of the gain, and a call comes on top of the reading instead of in place of it.

### A larger card for a function that is not written yet, tried and not kept

Reading those sessions showed what a session goes and gets before it writes: the lines that
call the function, the test that reaches it, and the types its signature names. So a card was
built for a prompt that names a function whose whole body is a placeholder
(`panic("not implemented")`, `todo!()`, `raise NotImplementedError`): its comment and
signature, the line of each call, the tests that reach it with the command that runs them, a
short type of its signature whole and a long one by its place. All of it exact, about one
symbol the index resolves. On the eight gitleaks tasks, four runs, against the same control,
before the go-git tasks were run:

| The card for the function to write | Cost | Median task | Requests | Shell commands | Files read | Bytes from hooks |
|---|---:|---:|---:|---:|---:|---:|
| as released: its lines, and where it is called from | 0.95 | 0.90 | 5.9 | 3.8 | 1.3 | 622 |
| and the calling lines, the tests, the types | 0.94 | 1.00 | 5.6 | 2.8 | 2.4 | 1,307 |

A shell command went and a file read came: the card named the test file and the files of the
types, and the session opened them. Two tasks that need those types fell a long way (0.87 to
0.68 and 1.16 to 0.91), four that do not rose, and the total stood still at twice the bytes.
Every function was written correctly in every arm, so nothing the larger card said was needed
to get it right. It was not kept, by the rule this file keeps arriving at: what is true and
not needed costs what it weighs.

What it found does stay, in the command an agent asks for by name. `codefast context` now
prints the types a function's signature names, and it finds the tests that reach a function
through two calls where it found them through one: for a helper deep in a package, that is
the test of the function the package offers, and it was missing.

## A card that answers the question

The split above says where a session's saving is made, and it says something about a call. On
the kernel questions the sessions that got the hooks and did not call codefast cost 0.82 of
the control. The ones that also called it cost 0.97. A kernel question is three or four
requests long, a call is one of them, and what it brings back is read on top of what the model
already knew. So what the card for the prompt can say, it should say, and a pointer to a
command is a request the card could have saved.

For three kinds of question the index has the whole answer and the card of 0.2.10 did not
give it: what a function calls, how one function reaches another, and where a member is
written. The card named the function, its place and its callers, and the session then called
`callees`, `path` or `refs` for the answer, or read the code. Version 0.3.0 reads the question
from the shape of the sentence and puts the answer in the card: what the command for it
prints, about the one definition the prompt can mean.

Three more things came out of the same reading, each of them about which definition is meant:

* A name with a definition per architecture or per driver was left alone, because nothing
  said which one. A word of the same sentence that is a directory of exactly one of them does
  say it (`copy_thread on x86`, `ext4's init_once`), and the card says how it chose: "of 22
  definitions of copy_thread, the one whose path has `x86`, a word of the prompt". Words that
  ask rather than name (`function`, `driver`, `file`) pick nothing: `function` is a directory
  of the kernel, and the first version of this picked a USB gadget for "which function".
* A slot of an operations table has a hundred bindings. The same words pick the ones that are
  asked about: "of its 104 bindings, under `ext4`: ext4_file_read_iter".
* A type said with its kind word (`struct kiocb`) is a type, though the word alone is prose.

### Measured without a model

`card_k.py` hands the prompt of each of the 72 verified kernel questions to the prompt hook
and scores what it prints with the scorer of the live sessions. A card that scores 1.00 has
answered the question before the session starts.

| Kind of question (8 of each) | Answered by the card, 0.2.10 | 0.3.0 |
|---|---:|---:|
| Where is it defined? | 5 | 6 |
| What is its signature, what are its members? | 4 | 7 |
| Who calls it? | 8 | 8 |
| What does it call? | 0 | 8 |
| Which function is behind this slot, which table holds this function? | 3 | 5 |
| Where is it reached from, when nothing calls it by name? | 8 | 8 |
| How does A get to B? | 0 | 4 |
| Where does this log line come from? | 5 | 5 |
| Who writes this member? | 0 | 8 |
| **All 72** | **33** | **59** |

The 54 questions that sessions had been run on before were there to develop on: 26 answered
became 48. The 18 that no session had seen were not looked at while the card was shaped: 7
became 11. A card is 555 bytes on average where it was 412.

`card_diff.py` runs both builds over the prompts of every other family: 155 prompts of the
caller, error, rename, bug-fix and code-writing tasks. The two builds print the same thing for
every one of them. On the kernel tree the hook answers a prompt in 0.08 seconds at the median
and 0.17 at the ninetieth percentile. A search for a call path gives up after half a second
of work, and a path that is not found is not mentioned.

### Measured in sessions, on questions no session had seen

The 18 kernel questions that had never been run, four runs per task and arm, the control and
the plugin of 0.3.0 in one batch and the plugin of 0.2.10 on the same questions after it. In
both plugin arms every session got a prompt card exactly where the check without a model said
it would.

| Arm | Cost | Median task | Requests | Right | Sessions that called codefast |
|---|---:|---:|---:|---:|---:|
| control | 1.00 | | 4.2 | 0.97 | |
| the plugin of 0.2.10 | 0.79 | 0.75 | 2.9 | 1.00 | 46 of 72 |
| the plugin of 0.3.0 | **0.71** | 0.66 | 2.7 | 1.00 | 33 of 72 |

By what the card holds, which was known before a session ran:

| The card | Questions | Cost with 0.2.10 | Cost with 0.3.0 |
|---|---:|---:|---:|
| answers the question in 0.3.0 and did not in 0.2.10 | 4 | 0.95 | **0.64** |
| answers the question in both | 7 | 0.60 | 0.61 |
| says something, and not the whole answer | 3 | 0.79 | 0.81 |
| says nothing | 4 | 0.86 | 0.99 |

The four questions the new card answers went from 0.95 to 0.64, each of the four on its own
(0.76 to 0.60, 1.07 to 0.76, 0.82 to 0.56, 1.49 to 0.81). Their sessions made 2.8 requests
where they had made 4.0 and the control made 5.1, and called codefast 0.2 times a session
where they had called it 1.3 times. The questions whose card did not change did not move. So the gain is
where the change is, and it is the size the split predicted: a session whose card is the
answer costs about 0.6 of the control, whichever build wrote the card.

The totals are 18 questions each, of which four changed. Resampling questions and runs, 0.3.0
is 0.59 to 0.88 of the control and 0.2.10 is 0.66 to 0.93, and the difference between them is
below zero in 83% of the resamples: likely, and not settled by this batch alone. The last row
is the price of saying nothing. A question of 6,000 tokens that gets no card and one call
costs twice the control.

### The same on a second repository, against a compiler

Four changed questions of eighteen do not settle much, and the kernel is one repository. So
the kind of question the new card answers was built as a family of its own on go-git: which
functions of the repository does this function call? The answer is the Go type checker's
call graph, twelve functions with five to seven callees each, and an answer has to name each
callee with its file on one line. Without a model, the card of 0.2.10 names none of the
callees that way (it is the definition and who calls it) and the card of 0.3.0 scores 0.98
against the compiler, with ten of the twelve lists complete, at 1,072 bytes where the old
card was 891 (`bench/results/card-c.jsonl`).

| Arm | Cost | Median task | Requests | Right | Sessions that called codefast | Calls in a session |
|---|---:|---:|---:|---:|---:|---:|
| control | 1.00 | | 5.0 | 0.98 | | |
| the plugin of 0.2.10 | 1.14 | 1.01 | 4.2 | 0.97 | 48 of 48 | 2.9 |
| the plugin of 0.3.0 | **0.56** | 0.54 | 2.4 | 0.98 | 12 of 48 | 0.3 |

Every one of the twelve is cheaper with 0.3.0, from 0.41 to 0.84 of the control, and the
answers are as right as the control's. Resampling tasks and runs, 0.3.0 is 0.49 to 0.64 and
0.2.10 is 0.97 to 1.33, and the difference is below zero every time.

The middle row is the finding that was not looked for. Before this change the plugin made a
callee question cost more than no plugin at all. Its card named the function and its callers,
the session called codefast three times on top of its own reading, and it read 9.5 thousand
characters of tool output where the control read 4.4. It is the split of the previous section
in one family: a call that does not replace the reading is paid for twice. The family was
never measured before, because the questions that were measured are the ones the tool was
built around.

## Where a reported bug is, next to the one published comparison

Every figure above sets a session with the plugin against the same session without it. One
published study measures that for other tools. arXiv 2611.07286 gives Claude Code a bug report
from LinuxFLBench and a checkout of the kernel version it was filed against, asks for the files
the fix has to change, and compares a session without a repository map with sessions that have
one of six. Its settings: Haiku 4.5 at low effort, the six built-in tools, 30 turns, 152
reports. It reports how often the file the real fix changed is first in the answer, how often
it is among the first ten, and every token the session processed.

| As published | File first | File in the ten | Tokens |
|---|---:|---:|---:|
| no map | 0.572 | 0.717 | 1,008,163 |
| the best map (Hybrid), with a sentence in the system prompt that its tools must be used | 0.526 | 0.829 | 610,910 |

Family F is that task. `make_f_tasks.py` takes reports from the half of LinuxFLBench that the
ranking of `codefast locate` was never tuned on, in the order of a hash of their id, and with
`--dev` from the other half. A session is asked for up to ten files, the most likely first,
and the file of the real fix is looked for in that list. `score_f.py` prints the three
figures of the paper, with tokens counted as the paper counts them.

### On reports nothing was developed on

48 reports over 38 kernel versions, two runs per arm, Haiku 5.5
(`bench/results/live-kernel-bugs-heldout.jsonl`).

| Arm | File first | File in the ten | Tokens | Requests | Cost |
|---|---:|---:|---:|---:|---:|
| no plugin | 0.542 | 0.771 | 135,316 | 5.2 | 1.00 |
| the plugin of 0.3.0 | 0.521 | 0.729 | 105,115 | 4.2 | 0.77 |
| the plugin with a ranking card, which was not released (below) | 0.542 | 0.740 | 100,163 | 4.0 | 0.73 |

The plugin takes 22% of the tokens off (between 8 and 36% when the reports are resampled),
and 33 of the 48 reports take fewer tokens with it. It does not find the file more often. The file
is in the ten for 0.729 of the reports against 0.771, a difference that lies between -0.104
and +0.010: two reports came into the ten and six went out of it, most of them in one run of
two. That cannot be told from no difference, and if it leans anywhere it leans the wrong way.
A session with the plugin makes one request fewer, and a session that stops sooner has looked
at fewer places.

### The same question on Python projects

A family measured on one project says little about another, and the kernel is one project.
`make_f_tasks.py --locbench` asks the same question about 40 issues of 27 Python projects from
Loc-Bench: the half its file ranking was never tuned on, no issue that a bug-fix family has
used, no project more than three times. Two runs per arm, Haiku 5.5
(`bench/results/live-bug-files-python-heldout.jsonl`).

| Arm | A file of the fix first | In the ten | Tokens | Requests | Cost |
|---|---:|---:|---:|---:|---:|
| no plugin | 0.875 | 0.925 | 97,337 | 4.3 | 1.00 |
| the plugin of 0.3.0 | 0.900 | 0.925 | 85,710 | 3.6 | 0.84 |

The same picture with less in it. The plugin takes 12% of the tokens off (between none and 22%)
and 16% of the cost, and it finds the file for exactly the reports the control finds it for.
These issues name what they are about, both arms are right for 37 of 40, and there is little
for a tool to add.

### A ranking card, tried and not kept

A prompt that holds a report and asks which files are behind it asks for what `codefast
locate` works out. So a card for such a prompt was built that is that ranking: ten files, the
most likely first, each with the function of the file the report's words point at, and a line
saying that it is a ranking and not an answer.

On forty development reports, one run per arm (`bench/results/live-kernel-bugs-dev.jsonl`):

| Arm | File first | File in the ten | Tokens | Requests | Cost |
|---|---:|---:|---:|---:|---:|
| no plugin | 0.575 | 0.800 | 143,681 | 5.7 | 1.00 |
| the plugin of 0.3.0 | 0.550 | 0.825 | 110,669 | 4.2 | 0.86 |
| the plugin with the ranking card | 0.625 | 0.825 | 89,239 | 3.6 | 0.65 |

That is 19% fewer tokens than the released plugin (between 6 and 31%), with nothing lost. On
the held-out reports it is 5% fewer (between 13% fewer and 4% more) and the same recall. The
card does on reports it has not seen what the plugin did without it, so it was not released.

What the held-out sessions show about such a card:

| The card | Reports | Found without the plugin | Found with the card | Tokens without | Tokens with |
|---|---:|---:|---:|---:|---:|
| has the file in its first three | 22 | 21.0 | 21.0 | 119,335 | 96,307 |
| has it fourth to tenth | 5 | 3.5 | 3.5 | 126,985 | 102,959 |
| does not have it | 21 | 12.5 | 11.0 | 154,040 | 103,538 |

Where the ranking has the file, the session is a fifth cheaper and finds what it found before.
Where it does not, the session stops a third sooner and finds less. For one report the ranking
was noise (the report is prose with no name in it). One of its two sessions said so in its
answer, and both stopped after three requests where the sessions without a card went on to
the file.
Without a model, the card has the file for 41 of 69 development reports (0.594) and for 52 of
90 held-out ones (0.578). Two more things were tried on the development reports and changed
nothing:

- **Twenty files instead of ten.** The card then has the file for 48 of the 69 reports
  (0.696). The sessions found nothing more: 0.800 in the ten against 0.825, for the same
  tokens. A session answers with three or four files, and the file a ranking has in
  fifteenth place is not one of them.
- **Saying nothing when the ranking is flat.** The ratio of the first score to the tenth
  separates good rankings from bad ones only weakly (the flattest quarter has the file for 8
  of 21 reports, the rest for 42 of 64), and on the forty reports with sessions it would have
  kept 31 cards of which 18 have the file.

- **The files next to the ranked ones in the call graph.** This is what the paper's best map
  does, and it was tried outside the engine, on the call graph `codefast export-edges` writes
  for each kernel tree. Of the 35 development reports whose file the ranking does not have in
  its ten, 14 have it one call away from one of the ten. Letting each of the ten pass its
  score on to the files it calls and is called by, with files that everything calls damped,
  brings four of those into the ten and pushes none out (50 to 54 of 85), and loses three
  places between eleven and twenty. That is a gain for the ranking, and too small to change
  what a session finds: it is not built into `codefast locate`, and would have to be checked
  on the other two benchmarks of the ranking first.

The build is the branch `tried/ranking-card`, with `bench/tier2/card_f.py`, the check without
a model.

### With the settings of the published comparison

Twenty of the development reports with Haiku 4.5 at low effort, the six built-in tools and 30
turns, run with the build that has the ranking card
(`bench/results/live-kernel-bugs-dev-published-settings.jsonl`).

| Arm | File first | File in the ten | Tokens | Requests | Sessions without an answer |
|---|---:|---:|---:|---:|---:|
| no plugin | 0.350 | 0.550 | 740,085 | 22.7 | 6 of 20 |
| the plugin with the ranking card | 0.300 | 0.600 | 667,008 | 20.1 | 3 of 20 |

Under these settings the plugin changes little: 10% fewer tokens, which resampling puts
anywhere between 31% fewer and 15% more. Five reports came into the ten and four went out of
it, which is what two runs of one arm do here. This model at this effort goes through the code
one function at a time whatever it is handed. It takes the names on the card as the place to
start, asks for the body of each, then for what each calls, and has made twenty requests
before it answers. A session that reaches its thirtieth turn has answered nothing, and that
is where much of the recall of both arms goes.

The paper steered its maps with a sentence in the system prompt saying that their tools must
be used for searching and grep must not. The `steer` arm of `live.py` adds that sentence, with
codefast's tools named. On six reports it cost 15% more tokens than no plugin at all and found
two files where the control found three and the plugin without the sentence four. Forced to
search through the tools, a session made more requests (24.3) and not fewer.

### The paper's own protocol, and its own best arm on the same reports

The comparison above used a prompt of its own and codefast as it was. Neither is what the paper
ran, and its repository shows the difference
(github.com/MelikaSepidband/How-Should-Agents-See-Code, at `1512fa9`).

- **Its prompt asks for candidates.** "Identify up to 10 candidate files that most likely
  contain the fault, ranked most-likely first." The prompt used above asks for "the source
  files that would have to be changed to fix it", and a session then lists two or three files
  and digs for the cause. With the paper's prompt the same model lists five to eight.
- **Its arms have one tool that returns code.** Each map has an `explore` tool whose answer is
  the best files with the verbatim source of their symbols, up to 20,000 characters, and a
  system prompt that says to call it first ("Usually the ONLY call you need"). The notes in
  its runner say why: "front-loading the code lets the agent conclude without the
  drill-down/read-reason-read loop (turns * context = cache_read = ~95% of tokens)".
- **codefast answered an open question with a list.** Free text given to `search` printed ten
  files with the names of their best functions. A session then fetched the code one function
  at a time: the drill-down loop.

`bench/tier2/paper_f.py` runs the paper's protocol: its prompt template and its system prompts
read from a clone of its repository, Haiku 4.5 at low effort, the six built-in tools, 30 turns,
a dollar a session, its grading rule (the last JSON block of the answer) and its token count
(everything processed, cached or not). The codefast arm has codefast's MCP server and no hooks,
a system prompt of the shape the paper gives its own tools with codefast's tools named in it,
and the paper's sentence that the tools must be used.

The paper's 152 sampled reports are not published, and neither are its transcripts. So its
best arm was run here as well, on the same reports as the other two: its Hybrid map, built by
its own builder (`python -m perrep.build --rep hybrid`) for each kernel version, served by its
own server, with the playbook its own code renders and the same sentence. That arm is the one
its paper reports as best (0.829 in the ten at 610,910 tokens, against 0.717 at 1,008,163 for
no map).

**What was built, in three steps, on 45 reports set aside to develop on**
(`bench/results/paper-protocol-dev.jsonl`, measured on 14 November 2026). Twenty of them are
development reports of the family above. The other 25 are the reports the first version was
run on untouched, and they were development from then on.

| Arm | Reports | File first | File in the ten | Tokens | Tool calls | Dollars | Sessions without an answer |
|---|---:|---:|---:|---:|---:|---:|---:|
| no map (the paper's baseline) | 45 | 0.400 | 0.556 | 767,291 | 27.1 | 0.196 | 16 of 45 |
| codefast, first version: source of ten files in the answer | 45 | 0.511 | 0.778 | 786,148 | 17.1 | 0.203 | 3 of 45 |
| codefast, short answers (tried on 8 reports) | 8 | 0.375 | 0.500 | 869,259 | 24.2 | 0.197 | 3 of 8 |
| codefast as released in 0.3.1 | 45 | 0.533 | 0.889 | 597,171 | 12.6 | 0.180 | 2 of 45 |
| the paper's Hybrid map, run here (on 25 of the 45) | 25 | 0.600 | 0.960 | 919,957 | 15.6 | 0.270 | 0 of 25 |

Against the baseline on the same reports: the first version found 13 files it did not and
missed 3 it found, with 2% more tokens (11% fewer to 17% more at 95%). The released one found
16 and missed 1, with 22% fewer tokens (9 to 35%) and 9% fewer dollars. The paper's map, on its
25 reports, found 10 and missed none, with 24% more tokens than the baseline (5% fewer to 57%
more) and 43% more dollars.

The first version put the source of the candidates into the answer, for ten files, and found
the file far more often with as many tokens as before. Its sessions showed two things.

- **A call asks several things and one was answered.** These sessions put two or three
  queries into nearly every call. The answer had one room for all of them, the first query
  took it, and the others got a few lines: 52% of the queries of such calls were answered in
  under 700 bytes, and 43% of those were asked about again later in the session. That is
  about four questions a session asked twice.
- **A session reads until it has seen the code it needs.** A variant with short answers (a
  search answer averaged 1,300 tokens) was tried on eight of the reports: its sessions made
  24 calls, where the first version (2,700 tokens an answer) made 17, and three of the eight
  reached their thirtieth turn with no answer. What an answer does not give, the session
  comes back for, and every time it comes back the whole conversation is sent again.

So `search` now answers every query of a call, each in a room of its own. The open questions
of a call (free text, not a name or a place) are one question asked several ways: their words
are ranked as one text and answered once. Measured without a model on the first calls of those
sessions, the file of the fix is first in that ranking for 49% of them, against 37% for the
first wording alone, and in the first ten for 70% against 60%. The answer goes deep into the
leading files (the source of up to eight functions of the first, five of the second) and
names the others with the function to look at. Where the caller names no budget it has 8,000
tokens for the first open question of a session, 6,000 for the second and 4,000 after that. A
budget the caller names is the room of the whole call, as it was, and an open question then
gets the ranking alone in that room, as it did before. The end of this section says why, and
where else the source is left out.

**On 56 reports no session of any build had been run on**
(`bench/results/paper-protocol-final.jsonl`, measured on 14 November 2026):

| Arm | File first | File in the ten | Tokens | Tool calls | Dollars | Sessions without an answer |
|---|---:|---:|---:|---:|---:|---:|
| no map (the paper's baseline) | 0.464 | 0.571 | 760,282 | 27.4 | 0.192 | 17 of 56 |
| codefast 0.3.1 | 0.554 | 0.821 | 713,470 | 14.1 | 0.201 | 4 of 56 |
| the paper's Hybrid map, run here | 0.589 | 0.804 | 809,160 | 14.6 | 0.254 | 6 of 56 |

The 56 reports are of 44 kernel versions. The three arms ran on the same day with the same
harness (Claude Code 2.1.323), two sessions at a time.

**codefast against no map.** The file is in the ten for 14 reports more and for none fewer
(+0.250, between 0.143 and 0.357 at 95%). The tokens are 6% fewer, which resampling puts
anywhere between 21% fewer and 10% more: no difference that can be claimed. Dollars are 5%
more (6% fewer to 17% more).

**codefast against the paper's map, on the same reports.** In the ten they are level: 0.821
and 0.804, four files found by codefast alone and three by the map alone. The map has the file
first for two reports more (0.589 and 0.554). codefast's sessions take 12% fewer tokens (28%
fewer to 10% more) and 21% fewer dollars (12 to 29%), and that last difference is the one
that is clear.

**The paper's map against no map.** It finds the file as the paper says, and more (+0.232
here, +0.11 published). Its saving of 39% of the tokens is not there: here its sessions take
6% more tokens than no map (14% fewer to 30% more) and 33% more dollars (20 to 46%). In the
paper a session with the map made 10 tool calls and one without made 25. Here it makes 14.6:
six `explore` calls of 21 thousand bytes each, one `node` call, and eight reads and searches
with the built-in tools. What differs from the paper's runs is the reports (its sample is not
published), the version of Claude Code and five months.

**What the answers held**, apart from what a session did with them. The file of the fix was
in the first answer of codefast's `search` in 38 of the 56 sessions and in the first answer of
the map's `explore` in 23. It was in some answer of the session for 51 and for 46. Of the 51,
six never reached the final list: three in sessions that ran out of turns and three that saw
it and left it out. The map's sessions listed 45, three of them files that no answer had
shown, named from what the model knows of the kernel.

**Where the tokens go.** A session that reaches its thirtieth turn has answered nothing and
has paid the most. Without a map that is 17 of 56 sessions and 40% of the arm's tokens (1.01
million each). With codefast it is four sessions and 17% (1.73 million each: the context is
larger when the tail of small lookups begins). A session that does answer costs about the
same in both arms, 635 thousand tokens with codefast and 651 thousand without. So codefast
does not make an answering session cheaper under this protocol. It makes more sessions answer.

**What was looked at in the paper's tool and not taken.** Its ranking starts from a text
search over symbol names, walks one call or include away and ranks by a random walk. Letting
the first files of codefast's ranking vote for the files they call was tried without a model
on 85 development reports: two to four more files in the first ten and three fewer in the
first twenty, depending on the weights, which is not a gain. The idea that was taken is the
paper's main one, the source in the answer.

**In sessions with the plugin as it is installed.** The paper's protocol gives a session the
tools and nothing else. The plugin is used another way: Haiku 5.5, the hooks on, one question,
a card before the first request. So the build was run there as well before it was released,
on the held-out question families next to 0.3.0: the plugin arm of both, one session of each
build at a time, each build with index directories of its own
(`bench/results/live-check-0.3.1.jsonl`, measured on 14 November 2026).

The first build, the one in the table above, did not pass. Error questions cost 0.72 of what
they cost with 0.3.0 (0.56 to 0.91), because a quoted message is now answered with the
function that writes it. The others cost more: kernel questions 1.14, callee and caller
questions 1.09 each, on one run per task and none of them a clear difference alone. Read one
by one, the dearer sessions had asked one thing in a sentence (`blk_rq_map_user_iov callers`,
"where io_uring invokes io_fsync through its opcode table") or typed a message that several
places print, and had been given thousands of tokens of ranked source where 0.3.0 gave a
list. These sessions also name a budget in half of their `search` calls, which the sessions
under the paper's protocol did in none of 703, and the source then filled it.

Four rules came out of that:

- A call that names a budget gets, for an open question, the ranking in that size, as in
  0.3.0.
- Where a hook of the plugin has run in the repository in the last ten minutes, an open
  question gets the ranking too. That session has had its card. The source is the default
  for a session that has the tools and no hooks, which is where it was measured to help.
  `codefast locate` prints it either way.
- A text of three words or more that stands in the code exactly as it was typed is answered
  with the places it stands in.
- A sentence that names one to three things defined in the repository, or a name with the
  word for what is wanted of it, is answered about those things.

With them, on two runs per task:

| Family | Tasks | Right, 0.3.0 and 0.3.1 | Requests | Cost with 0.3.1, against 0.3.0 |
|---|---:|---|---|---:|
| Where does this error come from? | 12 | 1.00 and 1.00 | 3.7 and 2.3 | **0.62** (0.52 to 0.74) |
| A question about the Linux kernel | 18 | 1.00 and 1.00 | 2.9 and 2.7 | **0.94** (0.85 to 1.04) |
| Who calls this method? | 12 | 0.99 and 0.99 | 4.2 and 4.3 | **0.99** (0.75 to 1.28) |
| What does this function call? | 12 | 0.99 and 0.99 | 2.3 and 2.8 | **1.21** (1.05 to 1.39) |
| All four | 54 | 0.99 and 1.00 | 3.2 and 3.0 | **0.94** (0.83 to 1.06) |

The callee row is not the build. A callee session gets the same card from both builds, byte
for byte, and then checks it with one or two reads of its own: a codefast command is run in
22 of the 120 callee sessions of that evening. The family read 1.09, 0.83 and 1.21 in the
three checks, and 1.02 (0.93 to 1.12) over all 60 sessions a side. What varies is how the
model chooses to check a card it has been handed.

The machine was busy with other work during these checks (a load average between 11 and 75
on 18 cores), and that reaches the result in one place. The prompt hook gives up after 2.5
seconds, and on the kernel it answered in 26 of the 36 sessions of each build, where on a
quiet machine it answers in 28. Both builds lost the same cards.

**The build that was released, run again.** The four rules change little of what a session
under the paper's protocol is given: of the 972 queries that the 56 codefast sessions of the
table made, the released build answers 15 differently, nearly all of them sentences about a
name. To see that the table still describes it, that build was run once more on the same 56
reports (before the last narrowing of the third rule, which touches two of those queries):

| Arm | File first | File in the ten | Tokens | Tool calls | Dollars | Sessions without an answer |
|---|---:|---:|---:|---:|---:|---:|
| codefast, the build of the table | 0.554 | 0.821 | 713,470 | 14.1 | 0.201 | 4 of 56 |
| codefast, the build that was released | 0.518 | 0.821 | 672,421 | 13.4 | 0.203 | 2 of 56 |

The file is in the ten for 46 reports in both runs, and they are not the same 46: 41 in both,
five in the one alone and five in the other. That is how far two runs of one tool on the same
reports lie apart here, and it is the measure for the two or three reports that separate
codefast from the paper's map in either direction. Against no map the second run has 12%
fewer tokens (24% fewer to 3% more), and against the paper's map 17% fewer tokens (33% fewer
to 4% more) and 20% fewer dollars (12 to 28%).

**A second set of reports.** One set of 56 reports leaves the two tools two or three reports
apart, which is less than two runs of one tool differ by. So the same three arms were run on
the 48 reports that were left: the held-out reports of family F above. Sessions had been run on
them once, with that family's prompt and the plugin as installed, and nothing was developed on
them. `paper_f.py run --set second` runs them, with the released build of codefast
(`bench/results/paper-protocol-second.jsonl`, measured on 14 November 2026).

| Arm | File first | File in the ten | Tokens | Tool calls | Dollars | Sessions without an answer |
|---|---:|---:|---:|---:|---:|---:|
| no map (the paper's baseline) | 0.375 | 0.583 | 757,034 | 26.9 | 0.189 | 15 of 48 |
| codefast 0.3.1 | 0.542 | 0.812 | 622,338 | 12.6 | 0.183 | 1 of 48 |
| the paper's Hybrid map, run here | 0.479 | 0.667 | 759,238 | 13.9 | 0.250 | 2 of 48 |

Against no map codefast has the file in the ten for 13 reports more and 2 fewer (+0.229,
between 0.083 and 0.375), on 18% fewer tokens (3 to 30%). Against the paper's map it has it for
8 more and 1 fewer (+0.146, between 0.042 and 0.271), on 18% fewer tokens (1 to 31%) and 27%
fewer dollars (21 to 32%). The paper's map against no map: +0.083 (0.062 fewer to 0.229 more),
on the same tokens and 32% more dollars.

Both sets together are 104 reports, with the released build of codefast on both (for the first
set, its second run above):

| Arm | File first | File in the ten | Tokens | Tool calls | Dollars | Sessions without an answer |
|---|---:|---:|---:|---:|---:|---:|
| no map (the paper's baseline) | 0.423 | 0.577 | 758,783 | 27.2 | 0.190 | 32 of 104 |
| codefast 0.3.1 | 0.529 | 0.817 | 649,306 | 13.1 | 0.194 | 3 of 104 |
| the paper's Hybrid map, run here | 0.538 | 0.740 | 786,119 | 14.3 | 0.252 | 8 of 104 |

- **codefast against no map:** the file in the ten for 29 reports more and 4 fewer (+0.240,
  between 0.144 and 0.337), on 14% fewer tokens (4 to 24%). Dollars are level (2% more, 5%
  fewer to 9% more).
- **codefast against the paper's map:** 13 files found by codefast alone and 5 by the map alone
  (+0.077, between 0.000 and 0.154), on 17% fewer tokens (4 to 28%) and 23% fewer dollars (18
  to 28%). The lead in finding the file is the second set's. On the first the two were level,
  and the map has the file first as often.
- **The paper's map against no map:** +0.163 (0.067 to 0.269), on 4% more tokens (11% fewer to
  19% more) and 32% more dollars (24 to 42%).

With 104 reports the saving of tokens against no map is clear, and it is made the way the
first set showed. A session that answers takes 620 thousand tokens with codefast and 632
thousand with no map. A session that reaches its thirtieth turn has answered nothing and has
taken a million or more: 32 of 104 sessions and 42% of the arm's tokens with no map, 3
sessions and 7% with codefast, 8 sessions and 21% with the paper's map.

One mistake on the way: the first attempt ran the two builds at once on the same index
directories. An index belongs to the version that wrote it, so each build kept rebuilding the
kernel's index for the other, inside a session. Those sessions were thrown away, and `live.py`
takes `BENCH_INDEX_SUFFIX` so that two builds each get directories of their own.

### What the comparison comes to

The paper reports for its best map, against no map: the file in the ten for 0.11 more of the
reports, on 39% fewer tokens.

Under its own protocol, on 104 reports nothing was developed on and next to its own map on the
same reports, codefast 0.3.1 has the file in the ten for 0.24 more of the reports than no map
(the map: 0.16), on 14% fewer tokens than no map (the map: 4% more), and for 23% less money
than the map. So on the question the paper asks, which tool lets this model find the file,
codefast finds it at least as often as the paper's best map (ahead on the second set of
reports, level on the first) and costs less to run. The paper's saving of 39% of the tokens
does not repeat here for its own map. What codefast saves, it saves by making sessions answer:
a session that answers costs the same with a tool as without, and a session that runs out of
turns costs the most and answers nothing.

With a current model and the plugin as it is installed (the sections above: Haiku 5.5, the
hooks on, a prompt that asks for the files to change) the picture is another one. There the
session without a plugin already has the file in its ten for 0.771 of the reports at 135,316
tokens, and the plugin takes cost off and does not find more. Most of what a map bought on
this task in the paper's setting, a current model does alone.

### A mistake on the way: which repository a directory belongs to

The kernel trees of this family are exported directories inside the codefast repository, one
per kernel version. To codefast, as to git, the project of a directory is the work tree around
it. So in the first batches the plugin's server and its command line took codefast itself for
the project, rebuilt the index kept for a kernel tree as an index of codefast, and answered
about that. Every plugin session of those batches was thrown away, and none of their figures
is in this document. Each tree is now made a git work tree of its own before a run (`live.py`
does it, and names the tree to the session as well), as a real kernel checkout is. A batch is
not read before one of its transcripts has been checked for the size of the index that the
session start reports, which for a kernel is tens of thousands of files.

## What a bug fix costs

The held-out bug-fix families came out at 0.99 (with the failing tests given) and 1.11 (from
the issue text alone). To see whether the 1.11 is a cost the plugin imposes or the spread of
that kind of task, the other 14 issue-only tasks, the ones kept for development, were run in
three arms (`bench/results/live-bugfix-arms.jsonl`):

| Arm | Cost | Solved |
|---|---:|---:|
| `control` | 1.00 | 7 of 14 |
| `plugin` | 0.98 | 8 of 14 |
| `lean` (the plugin without its MCP server) | 1.03 | 8 of 14 |

So fixing a bug from an issue text is about as expensive either way, and which side of 1.00 a
set of 15 tasks lands on is the spread of the family. Taking the MCP tool out does not make
the plugin cheaper there: what it saves on the tool's definition it loses in steps.

A second attempt went the other way. The shell commands of these sessions were counted: 124
greps printing 273,000 characters, 42 partial file reads printing 124,000, against 97,000 for
the test runs. The after-search note answered 34 of the 124 greps, because it asks for one
known name and these greps are alternations (`grep -n "A\|B\|C"`). So two notes were added:
one for an alternation, which answers about the first branch the index pins down, and the
outline of a file the agent reads a range of. Both fired (the note after a search went from 6
to 18 over the 14 tasks, the new one before a read 7 times), and the same tasks went from 0.98
of the control to 1.23, with the control arm within 3% of its earlier self. The agent did more
work, not less: 4.1k characters of shell output became 5.6k. The change was reverted.

That is the push-precision finding of the README met from the other side: a note that is right
but not needed is not free, and the rule stays that a hook speaks about one symbol the index
pins down and says what the agent's own output did not show.

A third attempt is the one that stayed. A report of a problem names no symbol for the index to
answer about, so the card for it is the ranking of the files whose text its words match, with
the best symbol of each. That ranking is the one `locate` makes, and on these very reports it
puts the right file first for 0.41 of them and in the first three for 0.90, which is above the
break-even precision the push study reports (0.45 to 0.68 for the top candidates). It fires
only for a prompt of 900 bytes over 12 lines or more: every kind of question an agent is asked
about code is shorter than that, and a report is not.

Over two batches and 26 tasks, pooled against two or three control runs each:

| | Cost | Requests | Solved |
|---|---:|---|---|
| Fix a bug, failing tests given (12) | 0.96 | 5.0 and 4.6 | 12 and 12 of 12 |
| Fix a bug from the issue text alone (14) | 1.02 | 6.9 and 6.8 | 8.0 and 9.0 of 14 |

So the card takes a step or two out of a bug fix and does not make it cheaper. Single tasks run
from 0.58 to 1.43 of the control, which is the spread of this family and the reason both
figures are pooled over four runs per task.

## What a bug fix can save at best

Three attempts had moved the figure by a few percent in either direction, so the next question
was how much there is to win at all. The `oracle` arm answers it: the session is told, before
it starts, the file and the function the real fix changed, by a hook of its own, for nothing.
No tool that finds where a change belongs can do better than that.

| Arm | Issue text alone (14) | Failing tests given (12) |
|---|---:|---:|
| control | 1.00 | 1.00 |
| the plugin as released | 0.98 | 0.98 |
| told the file and function for free | **0.95** | **0.97** |

Averaged over two or three control runs per task, because a single run of one arm differs from
its own average by 4 to 6%. Perfect localisation, handed over at no cost, is worth less than
that noise, and the plugin already reaches it.

A second oracle says where the 5% is not the whole story. Told the gold function **and what
`codefast context` knows about it** (its body, what it calls, who calls it, the tests that
reach it), a session costs **0.86** of the control, against 0.95 for the location alone. So
there is 14% in a bug fix for a push that lands on the right function with its code.

Trying to collect it showed why it stays there. The card was given the body of the
best-ranked function, which the ranker picks right (a file of the real fix) 0.45 of the time:

| What the card carried | Cost, issue text alone | Requests |
|---|---:|---|
| the ranking only (version 0.2.3) | 1.01 | 7.0 and 6.6 |
| the ranking and the body of the best-ranked function of the first file | 1.00 | 7.0 and 6.2 |
| the ranking and the body of the best-ranked function of the repository (right 0.55) | 1.00 | 7.0 and 6.5 |
| the gold function and its body | 0.86 | 6.9 and 5.8 |

Four runs per arm against four control runs. Two runs had said 0.92 for the second row, which
is what this family's spread does to two runs. The body takes requests out and leaves the cost
alone, so it was not kept.

All of those arms ran on a build whose ranking was worse than it should have been: the path
experiment below was swept into a commit by a careless `git add -A` and went out in version
0.2.3, where it cost the card's own precision (a file of the real fix among the first three for
0.57 of the reports instead of 0.79). Version 0.2.4 takes it out. The arms above are therefore
a floor for what a card can do, not a ceiling.

Version 0.2.4, with the ranking as it should be, was run on the same 14 reports twice: the
median task costs 1.02 of the control and the batch 1.19, the second figure carried by five
sessions where the agent wandered (9 requests became 16). The median is the honest reading of
this family, and it says the same as everything else here.

Three ways of raising the precision itself were tried and measured on the development reports
and on the development halves of the localisation sets: scoring path components (the one that
reached 0.2.3 by accident), stronger length normalisation (`B` 0.4 to 0.75, which lifts the
reports from 0.29 to 0.36 at the first file and drops ContextBench from 0.684 to 0.474) and
damping a file that no code-shaped word of the query matches (no effect: every candidate
already matches one). None was kept.

## What a card for a report is worth

The arms above all pushed one guess: the ranking, or the body of the best-ranked function.
They were chasing the wrong number. The ranker puts a file of the real fix first for 0.36 of
the development reports, but among the first three for **0.79**, and the oracle says the body
is what pays. So version 0.2.5 carries the body of the best function of each of the three
files it names, as `context` tells it, 300 tokens each.

| Arm | Issue text alone (14 reports) | Failing tests given (12) |
|---|---:|---:|
| control | 1.00 | 1.00 |
| the ranking alone (0.2.3, 0.2.4) | 1.01 to 1.19 | 0.97 to 1.19 |
| the body of the best-ranked function | 1.00 | - |
| **the bodies of all three** | **0.95** | 1.09 |

Four card runs against six control runs for the first column, two against four for the second.
Requests fall from 7.2 to 6.2, the fixes are the same (the card solved 6 to 9 of the 14 over
its runs, the control 7 to 9 over its own), and the saving sits in the long sessions: the
median task is unchanged and 7 of 14 are cheaper.

The second column is why the card asks a question first. Where a failing test is given, the
agent already knows where to start, and three function bodies are tokens for nothing: 1.09.

Then the held-out tasks were run on the shipped build, and the saving did not reproduce:

| | Development tasks | Held out |
|---|---:|---:|
| issue text alone | 0.95 (14 reports, 4 runs) | **1.03** (15 reports, 2 runs) |
| failing tests given | 1.09 with bodies | **1.04** with the ranking card, 0.99 with no card |

So the card is neutral on a bug fix, and the 0.95 belongs to the fourteen reports it was built
on. One refinement did carry over, because it takes text away rather than adding it: a file
whose symbol the prompt cards already explained gets no body, nothing is said twice, and the
held-out reports went from 1.03 to **0.96** with 6.9 requests down to 6.4. The fixes in that
run were 6.5 of 15 against the control's 7.5, which is one task in a run and inside the band
this family resolves: three card arms have come in at 7.0, 6.5 and 6.5 against a control
measured once at 7.5, and nothing here separates them. What the held-out runs did settle is
one rule: where a report names a test file of this repository the card says nothing at all,
which restores every fix (15 of 15 against 14.5) and takes the median task to 0.95. That rule
is why the card never fires on the family with the failing tests given, whose every prompt
names one, and it is the reason the two families had to be read apart from here on.

## The tests that reach what an edit changed

The expensive bug-fix sessions are not expensive because the agent cannot find the code. They
are expensive because of the loop after the edit: run the tests, read the output, run them
again. In one held-out session a single test run printed 78 KB. codefast knows which tests
reach a function, so a hook was added to say so after an edit changed one:

```
the tests that reach create_permissions, which this edit changed: tests/auth_tests/
test_management.py: test_default_permissions, test_unavailable_models,
test_create_permissions_checks_contenttypes_created, test_set_permissions_fk_to_using_parameter
```

On that session's own task the line names the test that was failing. On the development
reports it did what it was meant to do, and on the held-out tasks it did not:

| | Cost | Median task | Shell output | Fixes |
|---|---:|---:|---:|---|
| 14 development reports, 4 runs | **0.93** | 0.96 | 6.5k to 4.3k | 7.8 against 8.0 of 14 |
| 15 held-out reports, 2 runs | 1.03 | 1.09 | 8.9k to 8.3k | 6.5 against 7.5 of 15 |
| 15 held-out tasks with their tests, 2 runs | 1.02 | **0.88** | 11.6k to 9.6k | 15 of 15 |

It was reverted. The one reading worth keeping is the last row: where the tests of the change
are in the tree, the median session falls to 0.88 and every fix is kept, and the total is
carried by a few long sessions that the line does not shorten. Where the benchmark removes
those tests (the issue-only family), the line names tests that cannot check the fix, and it
reads as the push-precision rule again: a fact that is true and not the one needed costs.

What that leaves is a number to beat rather than a wall: the push is worth 14% at a selection
precision of 1.00 and nothing at 0.45, which is the break-even band the push study reports
(0.45 to 0.68). Raising what `locate` does with the text of a report above that band is the
work that would make a bug fix cheaper.

The reason the rest of the session does not move is in what the cost is made of. Output tokens weigh five times an input token, and
they are 28 to 60% of a session in every family. Where codefast saves, it saves there: a caller
question goes from 8,477 output tokens to 2,731, four of them in one session from 42,942 to
7,860, a rename from 5,100 to 2,823. A bug fix writes the fix and runs the tests, and that
output does not shrink: 4,536 to 4,335 with the failing tests given, 4,510 to 4,805 without
them. An index shortens the finding, and finding is not what a bug fix spends.

## Taking the card out again

Five attempts had now been measured on the held-out bug-fix tasks, and not one of them paid.
The question left was the plain one: what does the family do with no card for a report at all?
The `no-report` arm answers it, and version 0.2.9 is that arm shipped.

Every row below is two runs of 15 tasks of one build, scored against the same four control runs
of the same tasks, so the rows are comparable to each other and to the README's table.

Fix a bug from the issue text alone (15 reports, the control at 7.1 requests and 7.5 fixes):

| Build | What the prompt hook added for a report | Cost | Median task | Requests | Fixed |
|---|---|---:|---:|---:|---:|
| 0.2.5 (`h25`) | the ranking and the bodies of its three files | 1.01 | 0.96 | 7.1 | 7.0 |
| 0.2.6 (`h27`) | the same, and the tests that reach the edit | 1.02 | 1.06 | 6.7 | 6.5 |
| 0.2.6 (`h28`) | the same, that note reverted | 0.95 | 0.96 | 6.4 | 6.5 |
| 0.2.8 (`i5`) | **nothing** | 0.96 | 1.02 | 6.8 | 7.5 |
| 0.2.8 (`inc`) | **nothing** | 0.89 | 0.93 | 6.2 | 7.0 |

Fix a bug, failing tests given (15 issues, the control at 6.3 requests and 15 fixes). Every
prompt of this family names a test file of the repository, so from 0.2.6 on the card says
nothing here by the rule above, and the only build that ever carded these prompts is 0.2.5:

| Build | Cost | Median task | Requests | Fixed |
|---|---:|---:|---:|---:|
| 0.2.5 (`h25`), the ranking card | 1.01 | 1.03 | 6.0 | 14.5 |
| 0.2.5 (`h26`), no card on these prompts | 1.01 | 0.98 | 6.1 | 15 |
| 0.2.6 (`h27`), and the tests that reach the edit | 1.00 | 0.93 | 5.8 | 15 |
| 0.2.7 (`prec1`) | 1.00 | 0.98 | 6.3 | 15 |
| 0.2.8 (`d4`) | 0.87 | 0.94 | 5.3 | 15 |

Pooled over four runs each, the issue-only family costs **0.98** with a card (`h25` and `h28`,
6.8 fixes) and **0.92** without one (`i5` and `inc`, 7.2 fixes). Two runs of one build differ
from two other runs of the same build by as much as the two columns differ from each other
(0.96 and 0.89 are both 0.2.8), so the reading is not that 0.92 beats 0.98. It is that over
five builds and ten batches the card never once paid for its tokens, while the arm with no card
is the only one that also keeps every fix. So it was removed, with the constants, the test and
the `CODEFAST_REPORT_CARD` switch, and what the prompt hook says about a report is now nothing.

The README's two bug-fix rows are the four runs of the released behaviour: `prec1` and `d4` for
the tests-given family at 0.93, `i5` and `inc` for the issue-only family at 0.92, each against
the four control runs. Their median tasks are 0.99 and 1.03, so what the totals save sits in a
few long sessions and not in the typical one.

### Why the line stops here, and it is the input and not the ranking

Six arms had moved nothing, so the last question was asked of the text rather than of the
ranker: what does a bug report name that an index can state as a fact? Measured on the 14
development reports, with the repository indexed (`exact.py` and `sel.py` of this directory):

| What the report names | Reports |
|---|---:|
| a file path that exists in the tree | **1** of 14 |
| that path is a file the real fix changed | 1 of 14 |
| an identifier the index resolves to exactly one definition | 14 of 14 |
| one of those definitions is in a file the real fix changed | 11 of 14 |
| one of them **is** a function the real fix changed | 4 of 14 |

So a report does name symbols, and the index does resolve them, but which of the ten or twelve
it names is the one that matters is not in the text. Eight ways of choosing one were scored:

| Which named symbol the card would carry | Fires | Its file is a gold file | It is a gold function |
|---|---:|---:|---:|
| the first in the title | 12 | **0.58** | 0.17 |
| the first in the report | 14 | 0.50 | 0.14 |
| the most mentioned | 14 | 0.50 | 0.14 |
| the first that is called, as `f(` | 11 | 0.18 | 0.00 |
| the first after a dot, as `x.f` | 10 | 0.10 | 0.00 |
| the first in backticks or quotes | 3 | 0.00 | 0.00 |

The best of them, 0.58 at the file, sits inside the break-even band of the push study (0.45 to
0.68), and an arm already measured at 0.55 bought nothing. At the function, which is where the
oracle's 14% lives, the best selector is right 0.17 of the time. One of its picks is the word
`recursively`, which happens to be a symbol name in Django, so even the exactness is thin.

That is the finding: for an issue text of this kind the information a push would need is not in
the prompt, and no ranking of the index can put it there. It is not a tuning failure. The
families where codefast saves 0.15 to 0.63 are the ones whose prompt names what it is about,
and this one does not.

## One meaning or nothing

The note after a search picked the first of several things a name stands for when none of them
stood out. In a held-out session an agent grepping Django's test client for `context` was told
about `DecimalField.context`, and that session ran to 19 requests where the control took 10.
The note now says nothing unless the name means one thing in this repository.

| | Cost | Median task | Right |
|---|---:|---:|---:|
| 15 held-out tasks with their tests, before | 1.04 | 0.95 | all fixed |
| the same, after | 1.02 | 0.97 | all fixed |
| 12 held-out caller questions, before | 0.28 | 0.28 | 0.98 |
| the same, after | 0.30 | 0.34 | 0.99 |

Two runs per arm, so the cost columns say the same thing twice: nothing moved outside the
noise. What moved is what the tool says, and the answers got slightly better rather than
worse, which is the trade the project's own rule asks for.

## What a card should say

The prompt card of version 0.2.2 was settled by three small experiments on development tasks
(`bench/results/live-card-wording.jsonl`, one run per task and arm, so read them as direction).

1. **A list that can be checked with grep gets checked with grep.** The 13 caller questions on
   syft, with the card giving a method's call sites in three ways. The number of sites and the
   command that lists them: 0.31 of the control. The list itself and the command: 0.34. The
   list alone, on four of the questions: 0.64, where the same four had cost 0.37 with the
   number and the command. Handed a list, the agent checked it against a text search, which
   finds the method of that name on every type, and then read each hit to tell the types
   apart. Handed a number and a command, it ran the command and took the answer as it was.
   So the card lists call sites only for a name nothing else in the repository has, where a
   search of the agent's own finds the same places.
2. **What grep cannot find is taken as it is.** The card also gained the table a function is
   bound into and the calls that reach it through that table. Six kernel questions went from
   0.80 of the control to 0.70, and their requests from 2.7 to 2.2. Of the three that ask how
   a function is reached through a pointer, two fell to 0.43 and 0.64 and the third rose to
   1.12, which is one session each and the kind of swing a single pair shows.
3. **The code in the card does not save the read before an edit.** With the definition
   printed in the card, the agent still read the same few lines before changing them, as the
   harness's edit tool expects. Six bug-fix tasks: 4.2 requests before and 4.3 after, 0.97 and
   then 0.95 of the control. The body is worth its tokens when the prompt is a question about
   the function, and not when the prompt is a bug to fix.

<!-- new-code sections: generated, see the kit -->
## Building a project from nothing

Every family above works in a repository that exists. This one starts with an empty directory
and a specification (`bench/tier2/new/`, family G of `live.py`). Ten projects of 350 to 700
lines in their reference implementations: three in Python, three in Go, two in Rust and two in
TypeScript, standard library only, four to develop on and six held out. A session builds the
project and writes tests of its own. Afterwards 28 to 39 hidden tests, written against the
interface the specification gives, grade what it built. Each test set passes on a reference
implementation and fails on every one of 8 to 14 small faults put into that reference.

### What such a session is, with three models

Two of the projects to develop on, no plugin, one run each (measured on 14 November 2026). A
weighted token is one uncached input token of the model that ran: a cache read weighs 0.1, a
cache write 1.25, an output token 5.

| Model | Requests | Hidden tests passed | Weighted tokens | Reading the conversation again | Writing to the cache | What the model writes | Of that, thinking |
|---|---|---|---|---|---|---|---|
| Haiku 4.5 at low effort | 50 and 76 | 38 of 39 and 32 of 38 | 437,507 and 684,515 | 52% and 61% | 17% and 14% | 31% and 26% | 5% and 4% |
| Haiku 5.5 | 11 and 8 | 39 of 39 and 37 of 38 | 262,672 and 416,605 | 15% and 10% | 26% and 24% | 59% and 66% | 23% and 42% |
| Sonnet 5.5 | 3 and 3 | 39 of 39 and 36 of 38 | 64,847 and 97,680 | 7% and 5% | 35% and 31% | 57% and 64% | 6% and 15% |

- **The model of a year ago** makes one tool call per request: a request for every file it
  writes, then rounds of running the tests and editing, and then it tries the finished program
  by hand, one case per request, for twenty requests more. Half to three fifths of its tokens
  are the conversation read again. This is the session that the published work on what follows
  an edit shortens (FlowPlan, arXiv 2611.13241, with this model at this effort: 0.32 to 0.47 of
  the tokens).
- **Haiku 5.5** writes every source file in one request and every test file in the next, fixes
  what its tests show, and ends when they pass. Reading the conversation again is a tenth to a
  seventh of the cost. Three fifths is what the model writes: the code, the tests and its
  thinking, which is a quarter to two fifths of the whole session by itself.
- **Sonnet 5.5** reads the specification, writes the project with its tests in one shell
  command, and is done in three requests.

So with a current model there is nothing around the writing for a tool to take away: the
session is the writing. Writing is also dearer than its size suggests. These sessions paid one
output token for every 1.7 to 2.0 bytes of the files they wrote. A file leaves the model
inside a tool call, as a JSON string with every tab, newline and quote escaped.

### With the plugin

Every project twice in each arm with Haiku 5.5. The four projects to develop on were measured
on 14 and 15 November 2026, the six held-out ones on 15 November. Tests passed are the mean of a
project's two runs, summed over the projects.

| Projects | Runs an arm | Requests, control and plugin | Warm tokens, plugin against control | Dollars | Hidden tests passed, control and plugin | Runs that passed every test |
|---|---|---|---|---|---|---|
| 4 to develop on | 8 | 9.6 and 10.8 | 1.05 | 1.05 | 143 of 143 and 143 of 143 | 8 and 8 |
| 6 held out | 12 | 9.2 and 9.8 | 1.03 | 1.17 | 210.5 of 211 and 210.5 of 211 | 11 and 11 |
| all 10 | 20 | 9.4 and 10.2 | 1.03 | 1.13 | 353.5 of 354 and 353.5 of 354 | 19 and 19 |
| 6 held out, the build of 15 November, control of the night before | 12 | 9.2 and 9.4 | 0.98 | 0.95 | 210.5 of 211 and 211 of 211 | 11 and 12 |

The plugin neither saves nor costs here, and it could not save: a project that does not exist
has nothing to index. No hook says anything in a directory without code. Of the ten projects,
four were cheaper with the plugin and six dearer, from 0.79 to 1.18 a project, which is what
two runs of a task differ by without any plugin.

What the plugin adds to such a session is its start. Measured in pairs of sessions started
together in an empty directory (15 November 2026, the size of the first request): 726 to 734
tokens with the build of that morning, 213 to 224 with the build of the same day that says
nothing about itself where there is no code. Before, the server marked one tool to be loaded
from the start and sent a paragraph of instructions, and both went with every request while
the session wrote its first files. Now a tool is loaded when it is asked for.

The last row of the table needs one caveat. Its control ran the night before, and at 07:36 on
15 November a global instruction file of the test machine (`~/.claude/CLAUDE.md`, which the
harness loads for every project under the home directory) grew by about 365 tokens. Every
session after that carries them with each request, in whatever arm. So that row compares the
plugin's arm, with 365 tokens a request that are not the plugin's, against a control without
them. It was found because the first request of the new build looked 583 tokens larger than
the control's where pairs started together show 213 to 224.

The dollars of the first three rows are another matter, and they are about the model. The
TypeScript project of the held-out six (G-ts-2) is large enough for the conversation to pass
100,000 tokens while the model is still fixing what its own tests found, and from there Claude
Haiku 5.5 charges five times as much for every token of a request. The control ran 3 and 7
requests over that length and the plugin arm of the first batch 5 and 9. Those few requests
are 28% and 39% of what all twelve runs of an arm cost. Without that one project the other
nine come to 1.00 in tokens and 1.01 in dollars.

### What thinking costs and buys

Thinking is output, and a quarter to two fifths of what one of these builds costs. The harness
has a setting for it (`/effort`, `--effort`, `effortLevel`), and Haiku 5.5 runs at `medium`
when nothing is set. The four projects to develop on, without the plugin, at `low` (15
November 2026):

| Effort | Projects | Runs | Requests | Thinking tokens | Output tokens | Dollars a project | Against the model's own level | Hidden tests passed | Runs that passed every test |
|---|---|---|---|---|---|---|---|---|---|
| the model's own level (medium) | 4 | 8 | 9.6 | 19,193 | 36,995 | 0.033 | 1.00 | 143 of 143 | 8 of 8 |
| low | 4 | 8 | 7.8 | 16,833 | 30,087 | 0.027 | 0.82 | 143 of 143 | 8 of 8 |

Every hidden test passed at both levels, in all sixteen runs, and the lower level cost 0.82
of the dollars: fewer requests and a fifth less written. (These runs came after the
instruction file above had grown and their control from before, so the saving is understated
by those 365 tokens a request.)

A build from a precise specification is the kindest case for it. In the development
conversations, in the window of 125,000 on the two projects to develop on, `low` cost 0.73 of
the dollars of the same shape at `medium` (0.090 against 0.123 a conversation), and one of
the four conversations lost 9 of its 96 hidden tests, in the second and third message and
before its first compaction. At `medium` that shape passed every test in all four. Four
conversations are few, and the direction is the expected one: less thinking is cheaper and
gets a change with several rules wrong more often. So `codefast doctor sessions` shows what
thinking cost a session and names the setting, and leaves the level to the person.

## A development conversation

A project is not built in one message. The staged family (`bench/tier2/staged/`, family S) asks
for one of the projects above and then, message after message in the same conversation, for
five changes to it: a feature that needs several modules to change, a new module wired into the
old ones, a change of behaviour that was specified before, a change across the whole project,
and a bug report that pins down something the specification left open. Six projects, two to
develop on and four held out, 85 to 98 hidden tests of the final state each, and a test set for
every state on the way.

`live.py` sends the next message when the reply to the last has ended (`claude -p` with
`--input-format stream-json`), keeps the project as it stands after every reply, and grades
each state.

### With the plugin

| Projects | Runs, control and plugin | Requests, control and plugin | Warm tokens, plugin against control | Dollars | Hidden tests passed, control and plugin |
|---|---|---|---|---|---|
| 2 to develop on | 4 and 8 | 38.0 and 38.6 | 1.00 | 0.98 | 193 of 193 and 193 of 193 |
| 4 held out | 8 and 8 | 45.1 and 46.6 | 1.08 | 1.13 | 375.5 of 376 and 375.5 of 376 |
| all 6 | 12 and 16 | 42.8 and 44.0 | 1.06 | 1.10 | 568.5 of 569 and 568.5 of 569 |
| 4 held out, the build after the change | 8 and 8 | 47.6 and 51.9 | 0.96 | 0.95 | 375 of 376 and 376 of 376 |

There is nothing to save here either, for the reason of the family before: the session wrote
every line it is asked about. On the two projects to develop on the arms are level. On the
four held-out ones the plugin arm of the first batch cost 8% more in tokens and 13% more in
dollars. Nothing the plugin does should cost a conversation that much, so its transcripts were
read: that is the next section, and the row for the build after the change is what came of it.

### What the hooks said in these conversations, and what was changed

In a conversation that builds its project the hooks have little to add: the session wrote
every file it is asked about. They spoke all the same. In the eight held-out conversations of
the plugin arm (15 November 2026, the build of that morning) they put 28,126 bytes in front of
the model, about 900 tokens a conversation:

| What a hook said | Times | Bytes |
|---|---|---|
| For a prompt: where a symbol it names is defined, its body, who calls it | 34 notes, 61 cards and 2 for a quoted message | 23,825 |
| After an edit: a member that nothing in the repository defines | 13 notes, 17 members | 3,264 |
| A rename command | 2 | 411 |
| An outline hint before a whole-file read | 2 | 324 |
| A note after a search | 2 | 302 |

All 63 cards were about a file the conversation had written in full a few requests before,
with the write tool or from a here document in the shell.

The 17 members that "nothing defines" were read one by one in the transcripts. Thirteen were
written by a later edit of the same request: a session sends several edits in one request, a
call first and the method it calls next, and the hook after the first edit runs before the
second has landed. One was written by the next request. One was a leftover use that the same
request removed. Two were `message` on a class that extends the built-in `Error`. Not one of
the 17 was still missing when the request that caused the note had ended. The session remarked
on 12 of the 13 notes ("the hook's index is stale again"), and twice it spent a request on a
search to make sure.

The same holds for every plugin run recorded before the change, in all families: 2,782 runs,
76 such notes naming 101 members, and 100 of the 101 were members of a type defined in the
very file being edited. The one exception was a field of a type the conversation had written
itself. A member of a type the session had not touched, which is the mistake the check was
built for, did not occur once.

What was changed (commits `0f170fd`, `87eec56` and `c80fc48`, 15 November 2026):

- A member that does not exist, of a type whose file the conversation has changed, is held
  back. It is said when the conversation runs something (its tests, a build, the program) and
  the member is still missing, or with the next prompt when it stopped without running
  anything. A member of a type the conversation has not touched is said at once, as before.
- Nothing is said about a class whose header names a base the repository does not define, nor
  about members every object of the language has. With every definition of django, superset,
  vscode and elasticsearch treated as just written (2.47 million member uses), the check spoke
  1,488 times before and 29 times after.
- No card, search note or outline hint about a file the conversation wrote in full, with the
  write tool or through the shell from a here document (one whole-file write in five in these
  conversations). A compaction takes that back.
- In a directory without code the MCP server marks no tool to be loaded from the start and
  sends no instructions. A session that begins there is about to write the first files.
- A probe hook showed what a hook can know (15 November 2026, Claude Code 2.1.323): the hooks
  of one request's tool calls run one after another before the model reads any of them, and
  the transcript does not hold that request's calls yet. So a hook cannot tell whether another
  edit follows, and has to wait for a moment that shows the writing is over.

What the change does to these eight conversations can be said without running a model. Each
recorded conversation was replayed through the hooks of both builds
(`bench/tier2/replay_conversation.py`): its files rebuilt in a
scratch directory from the recorded writes and edits (526 of 545, the rest touched a file that
a shell command had changed), and every prompt, read, edit and shell command handed to
`codefast hook` as the harness had handed it. The build of that morning said 28,505 bytes in
55 notes, within 2% of what the live runs recorded. The build after the change says 500 bytes
in 2 notes, and both are the command for the rename one message asks for, which the old build
had offered for the wrong symbol. No held finding was still standing at any test run.

Then the eight conversations were run again with the new build, and the control with them
(15 November 2026, tag `s2n`):

| Held-out conversations, plugin against control | Warm tokens | Dollars | Bytes from hooks, a conversation | Tokens added to the first request | Hidden tests passed, plugin |
|---|---|---|---|---|---|
| The build of that morning, 8 runs against 8 of the same batch | 1.08 (1.00 to 1.17) | 1.13 | 3,516 | 734 | 375.5 of 376 |
| The same, against all 16 control runs | 1.04 (0.96 to 1.13) | 1.08 |  | |  |
| The build after the change, 8 runs against 8 of the same batch | 0.96 (0.88 to 1.06) | 0.95 | 63 | 224 | 376 of 376 |
| The same, against all 16 control runs | 1.00 (0.92 to 1.08) | 0.99 |  | |  |
| The control of the second batch against the control of the first | 1.08 (0.96 to 1.20) | 1.09 |  | |  |

The last row is the measure of what such a comparison can show: the same arm, run twice with
eight conversations each, differs by 8%. A conversation of six messages takes 30 to 65
requests, and one that happens to need ten more pays for them at the full length of the
conversation, on this model at five times the price once it is over 100,000 tokens. So neither
the 1.08 of the morning nor the 0.96 of the new build is a difference from 1.00 that eight
runs can carry. What the table does show without that noise is in its middle columns: the
hooks now say 63 bytes in a conversation where they said 3,516, and the plugin adds 224
tokens to the first request where it added 734 (in a directory without code the server no
longer announces itself). By those two figures the plugin costs such a conversation about a
tenth of a percent.

### The shape of the conversation

What does move the bill is how the conversation is cut, and with this model for one reason.
Claude Haiku 5.5 charges five times as much for every token of a request whose prompt is over
100,000 tokens. A conversation of six messages gets there: on the two projects to develop on,
18.5 of its 38 requests had a prompt over that length, and those were four fifths of what it
cost. Three other shapes of the same six messages, without the plugin:

| Shape | Runs | Requests | Compactions | Requests over 100,000 tokens | Dollars a conversation | Against one conversation | Billed tokens, against one conversation | Hidden tests passed |
|---|---|---|---|---|---|---|---|---|
| one conversation | 4 | 38.0 | 0.0 | 18.5 | 0.269 | 1.00 | 1.00 | 193 of 193 |
| window of 100,000 | 4 | 50.0 | 3.5 | 0.0 | 0.138 | 0.51 | 1.27 | 193 of 193 |
| window of 125,000 | 4 | 45.0 | 1.8 | 0.0 | 0.123 | 0.46 | 1.15 | 193 of 193 |
| a session for each message | 4 | 62.8 | 0.0 | 0.0 | 0.122 | 0.45 | 1.11 | 193 of 193 |

- **A smaller window.** `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, which is `/autocompact` in a
  session, sets the length at which the harness compacts a conversation. The harness compacts
  33,000 tokens under the window it is given: at 65,000 to 67,000 tokens with a window of
  100,000, the smallest it takes, and at 86,000 to 91,000 with a window of 125,000. In neither
  did a request reach 100,000.
- **A session for each message.** Every message starts a new session in the same directory, as
  a person does who types `/clear` when a job is done. From the second message on it begins
  with one sentence that says the project exists and that the code counts.

Both smaller windows and the fresh sessions kept every request under 100,000 tokens and cost
0.45 to 0.51 of the dollars. They did it with more requests and more tokens:
a compaction is a request of its own that reads the whole conversation and writes a summary,
the request after it writes what is left to the cache again, and a fresh session reads the
project again. In billed tokens each of the three costs 1.11 to 1.27 of the one conversation.
So this is a saving of the price step and of nothing else. On a model that charges the same
for every token, none of these shapes pays for a conversation of this size. The window of
125,000 compacts half as often as the window of 100,000 (1.8 times a conversation against 3.5)
and is the cheaper of the two.

On the four held-out projects (15 November 2026, 8 conversations a shape) the window of 125,000
did more than on the two it was found on:

| Shape | Runs | Requests | Compactions | Requests over 100,000 tokens | Dollars a conversation | Against one conversation | Billed tokens, against one conversation | Hidden tests passed |
|---|---|---|---|---|---|---|---|---|
| one conversation | 8 | 45.1 | 0.0 | 27.9 | 0.566 | 1.00 | 1.00 | 375.5 of 376 |
| window of 125,000 | 8 | 58.0 | 3.4 | 0.0 | 0.222 | 0.39 | 1.19 | 373.5 of 376 |

Every lost test was looked up. In one conversation of the Python project the harness compacted
during the second message, before a line of that message's feature was written. The message
says of ORDER BY that "a qualified name always means a column of the sources, so it works only
in a query without aggregation". The summary, 12,232 bytes long, names "ORDER BY qualified
rules" among the things to test and does not say the rule. The feature was then written from
the summary, a grouped query with `ORDER BY p.city` was accepted, and two hidden tests failed
on it, one of them a test of the fifth message. The other three lost tests (one more in that
conversation and one each in the two conversations of the TypeScript project) were not traced
to a summary: that project loses a test in two of eight conversations without any compaction.

The plugin arm of the same batch ran the build of that morning with the hook that gives the
last request back after a compaction: 375.5 of 376 hidden tests, as in one conversation, and
the one test it lost was a mistake written before the compaction. That is 8 conversations
against 8 and not a proof, but it is the direction the mechanism predicts.

### The same conversations at the prices of other models

The shapes above were run with Haiku 5.5 only. The conversation itself was also run with
Sonnet 5.5 (the last section of this part), where it reaches 61,000 tokens on average and
101,000 at most, a length at which none of these shapes matters for that model. What can be
said without running anything is what the recorded requests would
cost at another price list. Every request of the runs above, priced again (a compaction, which
the stream does not record, added as a read of the conversation and a summary of 3,600
tokens):

| Projects | Prices | One conversation | Window of 100,000 | Window of 125,000 | A session for each message |
|---|---|---|---|---|---|
| 2 to develop on | Haiku 5.5, as it was run | 1.00 | 0.43 | 0.37 | 0.38 |
| 2 to develop on | priced as Sonnet 5.5 or Opus 5.5 | 1.00 | 1.53 | 1.25 | 1.36 |
| 2 to develop on | priced as Fable 5.2 | 1.00 | 1.72 | 1.36 | 1.53 |
| 4 held out | Haiku 5.5, as it was run | 1.00 | | 0.28 | |
| 4 held out | priced as Sonnet 5.5 or Opus 5.5 | 1.00 | | 1.27 | |
| 4 held out | priced as Fable 5.2 | 1.00 | | 1.44 | |

The first row of each pair is the check of the method: the harness's own dollars give 0.51,
0.46, 0.45 and 0.39 where this gives 0.43, 0.37, 0.38 and 0.28, so the repricing leaves out
about a fifth of what the cut shapes cost, and the other rows are lower bounds. At the prices
of a model without the step, every one of these shapes costs at least a quarter more than the
conversation left alone, and the cheaper a cache read is, the more. That is the other half of
the advice: `codefast doctor sessions` says "compact earlier" on Haiku 5.5 from 100,000 tokens,
and on Sonnet 5.5, Opus 5.5 and Fable 5.2 only from 200,000 and 430,000, where reading a long
conversation again has come to cost more than starting it over. These are the requests of
Haiku 5.5 with other prices, not sessions of those models.

### What a summary drops, and what the hook gives back

A summary is written by the model from the conversation, and it shortens the request the
conversation was working on. The hook after a compaction (`SessionStart` with the source
`compact`) reads the last request the person typed from the transcript and gives it back word
for word, after the list of the definitions the uncommitted work has changed. What that does
can be counted without a model, over every compaction whose summary the runs recorded (97, in
all the compacted arms of 15 November 2026). The exact strings of a request are what it has
in backticks: names, values, messages.

| Compactions | Count | Exact strings of the request in progress | Still in the summary | In front of the model after the compaction | Summaries that quote the request |
|---|---|---|---|---|---|
| Without the request given back | 68 | 2,247 | 1,659 (74%) | 1,661 (74%) | 0 |
| With it (the arms that keep their transcript) | 29 | 986 | 738 (75%) | 986 (100%) | 0 |

A summary keeps three quarters of the exact strings and never the request as it was written.
One conversation to develop on then built a feature wrong: the request said "every other
month", the summary did not, the session built "every 2 months" from the wrong month, and six
hidden tests failed. With the hook the request is in front of the model again after every one
of the 29 compactions, in a note of 1,447 to 5,975 bytes (the median 4,238, of which the
request is 3,328).

What it does to the tests, over every compacted conversation that was run:

| Compacted conversations | Conversations | Compactions | Conversations that lost a hidden test | Tests lost |
|---|---|---|---|---|
| Without the request given back (control and plugin, windows of 100,000 and 125,000) | 24 | 68 | 4 | 11 |
| With it (plugin, window of 125,000) | 12 | 31 | 1 | 1 |

Of the eleven tests lost without it, eight were traced to a rule that a summary left out (the
six above, and two of the held-out Python project that are described with the shapes). The
one test lost with it is a mistake the session made before its compaction. One in twelve
against four in twenty-four is not a difference these counts can prove, and the arms differ
in more than this hook. It is what the mechanism predicts, and nothing was found against it.

### The plugin within each shape

| Shape | Projects | Runs, control and plugin | Warm tokens, plugin against control | Dollars | Hidden tests passed, control and plugin |
|---|---|---|---|---|---|
| one conversation | 2 to develop on | 4 and 8 | 1.00 | 0.98 | 193 of 193 and 193 of 193 |
| one conversation | 4 held out | 8 and 8 | 1.08 | 1.13 | 375.5 of 376 and 375.5 of 376 |
| window of 100,000 | 2 to develop on | 4 and 4 | 1.03 | 1.03 | 193 of 193 and 193 of 193 |
| window of 125,000 | 2 to develop on | 4 and 4 | 0.98 | 0.98 | 193 of 193 and 190 of 193 |
| window of 125,000, request given back | 2 to develop on | 4 and 4 | 0.95 | 1.00 | 193 of 193 and 193 of 193 |
| window of 125,000, request given back | 4 held out | 8 and 8 | 0.98 | 0.92 | 373.5 of 376 and 375.5 of 376 |
| a session for each message | 2 to develop on | 4 and 4 | 1.06 | 1.06 | 193 of 193 and 193 of 193 |

Inside the compacted shapes and the fresh sessions the plugin is level with its control,
within what four or eight runs show: 0.95 to 1.06 in tokens. (The rows of one conversation are
those of the first table, with the build of that morning.) The row to read is the held-out
one with the request given back. It is the measurement of the whole setting that
`codefast doctor sessions` and the note at the price step lead to, a window of 125,000 with
the plugin's hook after each compaction: 0.36 of the dollars of one conversation without any
of it (0.204 against 0.566 a conversation), with 375.5 of 376 hidden tests passed in both.

### Handing a job to a sub-agent at the price step, tried and not kept

The saving of the smaller window needs a setting, typed once by the person. A plugin cannot
make it for them: no output of a hook starts a compaction or changes the window, and a plugin
cannot carry that setting (hooks and plugin references of the harness, read on 15 November
2026). What a hook can do is tell the model something. So one thing was tried that a plugin
could do by itself. At a prompt where the conversation, with the largest job it has had so
far, would pass nine tenths of 100,000 tokens, a hook told the session to hand the request to
a sub-agent with a fresh context, and a second hook gave that sub-agent the request word for
word when it started (`SubagentStart`). It was a script added to the plugin for one batch
(`BENCH_EXTRA_HOOKS` of `live.py`) and is not in the binary.

| Project | Requests with the hand-over, of them in sub-agents | Dollars with the hand-over | Dollars of the conversation left alone | Dollars compacted at 125,000 | Hidden tests passed with the hand-over |
|---|---|---|---|---|---|
| S-go-1 | 85 (62) and 117 (81) | 0.172 and 0.226 | 0.180 and 0.230 | 0.102 and 0.137 | 96 of 96 and 96 of 96 |
| S-py-1 | 127 (98) | 0.238 | 0.305 and 0.362 | 0.131 and 0.120 | 97 of 97 |

The session did as it was told: all 13 messages that got the note were handed over with one
call each, the request reached the sub-agent every time, the main conversation then grew by
2,000 to 3,000 tokens a message, and every hidden test passed. (Three conversations: the
batch stopped at the usage limit of the account.) It did not pay. Over the two projects the
hand-over cost 0.81 of the dollars of the conversation left alone, and on one of the two
almost nothing less, where the smaller window costs 0.46.

There are two reasons, and neither is an accident of the prototype. A sub-agent reads the
project again and takes 18 to 39 tool calls a job where the main conversation takes 5 to 10
requests: the sub-agents were two fifths to a half of the cost. And the harness runs a
sub-agent in the background. The reply to the person ends when the job is handed over, the
result arrives later as a notification that the main conversation answers in a turn of its
own, and by the last message the main conversation had passed the step after all and paid
five times for those requests, a third to two fifths of the cost. (The benchmark also sent the next message
as soon as a reply had ended, so jobs overlapped, which a person would not do.) With those
last requests taken out, the hand-over would still cost about 0.55 to 0.6 of the conversation
left alone.

So the note at the price step stays what it was: one line to the person with the command
that sets the window, which the harness remembers for the model.

### The same conversations with Sonnet 5.5

The six projects of the staged family were run with Sonnet 5.5 on 15 November 2026, through
OpenRouter with an API key (so with the five-minute prompt cache), with the released build
0.4.0 as the plugin. Two runs per project and arm.

| Projects | Runs, control and plugin | Requests, control and plugin | Seconds a conversation, control and plugin | Time, plugin against control | Dollars | Hidden tests passed, control and plugin | Conversations that passed every test |
|---|---|---|---|---|---|---|---|
| 2 to develop on | 4 and 4 | 18.0 and 18.0 | 187 and 154 | 0.82 | 0.99 | 386 and 386 of 386 | 4 and 4 of 4 |
| 4 held out | 8 and 8 | 24.8 and 21.0 | 280 and 256 | 0.92 | 0.94 | 748 and 749 of 752 | 5 and 5 of 8 |

The plugin arm is level with the control in dollars (0.94 on the held-out projects, 0.90 to
1.01 by project, and 0.99 on the two to develop on) and somewhat faster. The control run twice
differs by more than that, so this is no saving. It is also no cost.

What these sessions showed is how Sonnet 5.5 writes: every file through the shell, with
`cd /the/project && cat > src/a.ts <<'EOF'`, and every later change with a few lines of Python
that replace text in a file. 112 shell calls and 8 reads in eight conversations, and not one
call of the write or the edit tool. The build 0.4.0 took a here document after a `cd` for a
file it could not place, so these conversations were handed cards about their own code: 28
notes with 43 cards, 1,440 bytes a conversation, where a conversation of Haiku 5.5 gets 63.
That is fixed (commit 0126034): replayed without a model, the twelve conversations get 2,949
bytes from the hooks where 0.4.0 says 19,419, and what is left are rename commands.

Effort, with no plugin:

| What was built | Effort | Runs | Requests | Dollars a run | Against the model's own level | Hidden tests passed | Runs that passed every test |
|---|---|---|---|---|---|---|---|
| a project from nothing, 4 to develop on | the model's own level (medium) | 8 | 3.9 | 0.157 | 1.00 | 285 of 286 | 7 of 8 |
| a project from nothing, 4 to develop on | low | 8 | 3.5 | 0.139 | 0.89 | 283 of 286 | 6 of 8 |
| a development conversation, 2 to develop on | the model's own level (medium) | 4 | 18.0 | 0.402 | 1.00 | 386 of 386 | 4 of 4 |
| a development conversation, 2 to develop on | low | 4 | 15.8 | 0.350 | 0.87 | 386 of 386 | 4 of 4 |

At `low` Sonnet 5.5 builds a project for 0.89 of the dollars and loses two hidden tests in
286, and it carries a conversation of six messages for 0.87 with every hidden test passed.
Four runs against four in the conversations and eight against eight in the projects.
<!-- end of the new-code sections -->

<!-- by-model sections: generated, see the kit -->
## Every model on the same held-out tasks

The four kinds of held-out task that gain from an index (who calls this method, four such
questions in one session, where an error comes from, a rename everywhere) were run with Opus
5.5 on 15 November 2026, one run per task and arm, as they had been with Sonnet 5.5 on 13
November (`bench/results/live-heldout-opus.jsonl`, `live-heldout-sonnet.jsonl`, and the rows of
Haiku 5.5 in `live-heldout-haiku.jsonl`, two runs per task and arm). Time is the wall clock of
a session on a machine that other work shares, with the two arms of a batch taking turns. Cost
is priced per request by the weights of this file (a cached token 0.1, a new one 1.25, an
output token 5). Dollars are what the harness itself reports at list prices.

| Model | Tasks | Time, plugin against control | Requests | Cost | Dollars as billed | Measured |
|---|---|---|---|---|---|---|
| Haiku 5.5 | 37 | 0.36 | 0.50 | 0.31 | 0.30 | 2026-11-13, 2026-11-14 |
| Sonnet 5.5 | 37 | 0.52 | 0.53 | 0.42 | 0.59 | 2026-11-13 |
| Opus 5.5 | 37 | 0.56 | 0.62 | 0.50 | 0.62 | 2026-11-15 |

| Model | Task | Tasks, runs an arm | Requests, control and plugin | Seconds a session, control and plugin | Time, plugin against control | Cost, plugin against control | Dollars as billed | Answers right, control and plugin |
|---|---|---|---|---|---|---|---|---|
| Haiku 5.5 | Who calls this method? | 12, 2 | 7.2 and 3.7 | 40 and 14 | 0.35 | 0.30 | 0.30 | 1.00 and 0.99 |
| Haiku 5.5 | Four such questions in one session | 3, 2 | 19.5 and 5.3 | 182 and 32 | 0.18 | 0.15 | 0.10 | 0.99 and 0.98 |
| Haiku 5.5 | Where does this error come from? | 12, 2 | 5.8 and 3.4 | 11 and 7 | 0.62 | 0.63 | 0.77 | 0.99 and 1.00 |
| Haiku 5.5 | Rename a method everywhere | 10, 2 | 9.3 and 5.4 | 38 and 20 | 0.52 | 0.51 | 0.59 | 1.00 and 1.00 |
| Sonnet 5.5 | Who calls this method? | 12, 1 | 7.0 and 2.7 | 27 and 9 | 0.34 | 0.29 | 0.49 | 0.98 and 0.99 |
| Sonnet 5.5 | Four such questions in one session | 3, 1 | 13.0 and 3.7 | 62 and 20 | 0.32 | 0.23 | 0.30 | 0.95 and 0.99 |
| Sonnet 5.5 | Where does this error come from? | 12, 1 | 6.0 and 4.0 | 13 and 10 | 0.72 | 0.70 | 0.87 | 0.98 and 1.00 |
| Sonnet 5.5 | Rename a method everywhere | 10, 1 | 5.7 and 4.2 | 24 and 18 | 0.77 | 0.74 | 0.86 | 1.00 and 1.00 |
| Opus 5.5 | Who calls this method? | 12, 1 | 6.8 and 4.7 | 42 and 27 | 0.64 | 0.51 | 0.60 | 0.98 and 1.00 |
| Opus 5.5 | Four such questions in one session | 3, 1 | 16.7 and 6.0 | 127 and 45 | 0.36 | 0.34 | 0.42 | 1.00 and 1.00 |
| Opus 5.5 | Where does this error come from? | 12, 1 | 5.6 and 4.2 | 24 and 18 | 0.76 | 0.77 | 0.89 | 1.00 and 1.00 |
| Opus 5.5 | Rename a method everywhere | 10, 1 | 6.9 and 4.2 | 44 and 23 | 0.52 | 0.47 | 0.63 | 1.00 and 1.00 |

With each of the three models a session with the plugin takes about half the time (0.36 to
0.56) and half the cost (0.31 to 0.50) of the same session without it, and its answers are as
often right or more often. The saving is largest in the sessions of four questions. Each
answer that comes in one call is a request the conversation does not make, and every request
reads the whole conversation again.

Fable 5.2 was started on the same tasks and stopped after nine runs, when the account's weekly
limit for that model stood at its warning level. It is not in the table.

The sessions of Opus 5.5 were read, and four things were changed because of what they showed.
All four came after these runs, so the table is the build before them.

- **A card that named a command for the wrong symbol.** go-git has five types called `Encoder`,
  each with an `Encode`. The card for the one in the packfile package ended with
  `codefast callers Encoder.Encode`, and the command answered for another of the five. The
  session said "that output is for the wrong Encoder", asked again by place and then checked
  the answer against a search of its own. The command now answers for each of the things a
  name stands for, and a card names its symbol by place where the name is shared (commit
  1b47427).
- **The card for an error message.** Of twelve error texts, five got no card although each
  stands in exactly one string of the source ("temp dir has been cleaned up"), and one got the
  card of a neighbouring message that shares most of its words ("too many entries in resource
  directory" for "no entries in resource directory"). In the four of those sessions that were
  read, the plugin made no difference, 5 requests in each arm. A quoted message is now looked
  for as it stands before its phrases are ranked (commit 02e8c2a), and all twelve get the card
  of the line that writes them, where six did.
- **zsh and a pattern that matches nothing.** `grep -rn x --include=*.go .` ends with "no
  matches found" in zsh before grep runs. It happened in these sessions in both arms, and over
  thirty days of sessions on this machine in 2% of all shell calls. One line in the shell setup
  of a session turns it off (commit d0036f6, and section 4 of `docs/INSTALL.md`).
- **A name remembered from another version**, and a method named the way Swift calls it
  (`validate(statusCode:)`): both were answered with "no matches" (commits 00b5997 and
  139111f).

One thing was seen and not changed. In the who-calls tasks Opus 5.5 asks codefast for the
callers and then, in most sessions, checks the list against one or two text searches of its
own: 4.7 requests a session, where Haiku 5.5 makes 3.7 and Sonnet 5.5 2.7.

## The seven-repository protocol another tool publishes

The most quoted figures for a code graph in a coding harness are those of CodeGraph: 62% fewer
tokens, 44% cheaper, 88% fewer tool calls and 53% faster, measured on 5 August 2026 with Opus
4.8 (https://github.com/colbymchenry/codegraph, README). The protocol is published: seven
repositories cloned at depth 1 (VS Code, Excalidraw, Django, Tokio, OkHttp, Gin, Alamofire),
one question about the architecture of each ("How does the extension host communicate with
the main process?"), the session run headless with and without the tool, four runs an arm,
the median of each. Tokens are everything the requests of a session processed, sub-agents
included, cost is the harness's own dollar figure, and time is wall clock.

`bench/public/seven_repos.py` runs it with the codefast plugin as the arm (the plugin as
people install it: hooks, skill, command and MCP server). The repositories were cloned on 15
November 2026 (Django and VS Code on 11 November), the questions are theirs word for word, and
the model is the one named, since theirs is no longer current. The answers are not graded in
their protocol. Here the files an answer names are checked against the repository, and the
two arms name as many that exist. Rows in `bench/results/public-seven-repos.jsonl`.

| Model | Runs an arm | Without any tool: seconds, tool calls, tokens, dollars a question | With codefast | Time | Tool calls | Tokens | Cost |
|---|---|---|---|---|---|---|---|
| Opus 5.5 | 28 | 36 s, 3.0, 81k, 0.141 | 36 s, 3.2, 101k, 0.159 | 0.98 | 0.98 | 1.14 | 1.16 |
| Sonnet 5.5 | 28 | 15 s, 1.9, 54k, 0.043 | 12 s, 1.5, 49k, 0.042 | 0.84 | 0.91 | 0.96 | 0.98 |
| Haiku 5.5 | 28 | 20 s, 5.2, 113k, 0.005 | 16 s, 3.6, 93k, 0.004 | 0.79 | 0.76 | 0.90 | 0.80 |
| published for the other tool, Opus 4.8 | 28 | 46 to 163 s, 6 to 43, 180k to 991k, 0.31 to 2.43 | 28 to 63 s, 1 to 4, 87k to 209k, 0.31 to 0.66 | 0.47 | 0.12 | 0.38 | 0.57 |

With Opus 5.5, by repository, next to what was published:

| Repository | Without any tool here: seconds, tool calls, dollars | With codefast | Published without the other tool | Published with it | Files an answer names that exist, without and with |
|---|---|---|---|---|---|
| VS Code | 31 s, 2, 0.09 | 34 s, 3, 0.16 | 130 s, 28, 1.80 | 58 s, 2, 0.53 | 10 and 7 |
| Excalidraw | 42 s, 4, 0.15 | 45 s, 5, 0.18 | 162 s, 43, 2.43 | 45 s, 2, 0.54 | 10 and 10 |
| Django | 39 s, 2, 0.12 | 35 s, 2, 0.18 | 83 s, 14, 0.63 | 54 s, 3, 0.55 | 3 and 4 |
| Tokio | 39 s, 4, 0.16 | 44 s, 4, 0.17 | 163 s, 29, 1.83 | 63 s, 3, 0.66 | 7 and 6 |
| OkHttp | 27 s, 2, 0.08 | 23 s, 1, 0.07 | 58 s, 6, 0.50 | 33 s, 1, 0.39 | 4 and 3 |
| Gin | 26 s, 2, 0.08 | 22 s, 1, 0.07 | 46 s, 7, 0.31 | 28 s, 1, 0.31 | 4 and 5 |
| Alamofire | 45 s, 4, 0.24 | 45 s, 5, 0.19 | 142 s, 33, 1.27 | 54 s, 4, 0.54 | 6 and 6 |

What this says:

1. **The session without any tool is not the one the figures were measured against.** With
   Opus 4.8 in August it made 6 to 43 tool calls and took 46 to 163 seconds. With Opus 5.5 in
   November it makes 2 to 4 and takes 26 to 45 seconds, at 8 to 24 cents. That is less than the
   published session with the tool (28 to 63 seconds, 31 to 66 cents). The models now put
   several searches and several ranges of several files into one shell command: the two to
   four calls of a session of Opus 5.5 hold 7 to 19 commands. A saving of 62% of the tokens and
   53% of the time against the older session says nothing about this one.
2. **Against this session there is little left to save.** codefast is level with Opus 5.5 in
   time and tool calls and costs 16% more. With Sonnet 5.5 it is 16% faster at the same cost.
   With Haiku 5.5 it is 21% faster and 20% cheaper.
3. **Where it gains and where it costs, with Opus 5.5.** On the two small repositories (Gin,
   OkHttp) one call of codefast holds the bodies the question needs, and the session ends
   after it: one call where the control makes two, and 15% less time. On the four where the
   answer runs through many functions, the first answer of codefast is one more call: the
   session then reads as many ranges through the shell as the control does.
4. **How Opus 5.5 asks.** It names the symbols it remembers, several in one call, with a room
   of 4,000 to 7,000 tokens: `QuerySet._fetch_all`, `ModelIterable.__iter__`,
   `SQLCompiler.execute_sql`. Of the 107 answers codefast gave in the sessions of Opus 5.5 and
   Sonnet 5.5 (this protocol and the tasks of the section above), 8 were "nothing found" for a
   name that the checkout writes a little differently. Those are the last two changes of the
   section above.
5. **These are questions about code the models have read.** A model says how a well-known
   repository works from memory and checks a few places. The tasks of the section above ask
   for something that cannot be recalled and has to be complete (every caller, in a
   repository the model knows less well), and there the same plugin halves the session.

Seven questions are few, one run of an arm differs from the next by a quarter, and the
runs of one model share a batch. Fable 5.2 was not run (see above).
<!-- end of the by-model sections -->

## The same questions in Codex

Everything above is Claude Code. `bench/codex/live.py` runs the held-out question tasks in
Codex: the same prompts and the same checks, by `codex exec` in its read-only sandbox
(codex-cli 0.164.0 with its default model, gpt-6.1-sol, reached through OpenRouter). One arm
has a home with nothing of codefast in it. The other has what
`codefast setup codex --hooks --apply` writes there: the MCP server, the four hooks, the skill,
and the command on `PATH`. Two runs per task and arm, 216 sessions, measured on 14 November
2026 with the first build of 0.3.1 (`bench/results/codex-live.jsonl`). The release differs
from that build only in how `search` answers some queries, and no Codex session called
`search`.

| Task | Tasks | Right, without and with | Tool calls | Tokens of a session | Dollars of a session | With the plugin it costs |
|---|---:|---|---|---|---|---:|
| Who calls this method? | 12 | 1.00 and 1.00 | 3.8 and 3.5 | 86,681 and 60,106 | 0.0694 and 0.0482 | **0.69** (0.62 to 0.77), cheaper in 12 of 12 |
| What does this function call? | 12 | 1.00 and 1.00 | 3.1 and 1.0 | 56,006 and 27,991 | 0.0386 and 0.0318 | **0.82** (0.78 to 0.87), cheaper in 12 of 12 |
| Where does this error come from? | 12 | 1.00 and 1.00 | 3.7 and 2.8 | 59,854 and 46,645 | 0.0393 and 0.0340 | **0.86** (0.79 to 0.93), cheaper in 11 of 12 |
| A question about the Linux kernel | 18 | 1.00 and 1.00 | 2.4 and 1.8 | 49,076 and 40,232 | 0.0436 and 0.0384 | **0.88** (0.83 to 0.94), cheaper in 14 of 18 |
| All four | 54 | 1.00 and 1.00 | 3.2 and 2.2 | 61,368 and 43,353 | 0.0473 and 0.0382 | **0.81** (0.76 to 0.85), cheaper in 49 of 54 |

Codex reports the totals of a session and not the size of every request, so the cost here is
dollars at the provider's list prices of that day for those totals: $2 for a million tokens of
input, $0.10 where the provider served them from its cache, $10 for output. Tokens are all the
input of all requests plus the output.

What the sessions show:

- **This model answers every one of these questions right, with the plugin or without.** What
  changes is the work: 3.2 tool calls become 2.2, and 61 thousand tokens become 43 thousand.
- **It never called the MCP server.** Codex gives this model one tool that runs a script, and
  the server's tools are functions inside it (`bench/codex/wiring.py` shows the request). In 108
  sessions with the plugin they were not used once. What reached the model was the card of the
  prompt hook and the command line. The caller questions ran `codefast callers` in 23 of 24
  sessions. In the other three families the card was the answer, and the session checked it
  with one or two `rg` or `sed` commands of its own.
- **The saving is smaller than in Claude Code,** where the same four families cost 0.30, 0.56,
  0.63 and 0.71 of the control. The session without the plugin is already short here: three
  tool calls, where Haiku in Claude Code makes four to seven requests. And the first request
  of a Codex session is 57 thousand bytes of Codex's own before any of the task, which no
  plugin shortens.

Only the four families that change nothing were run. The renames, the code-writing tasks and
the bug fixes need Codex to build and run tests inside its sandbox, and that was not set up.

## Limits

* Two harnesses. All but one section is Claude Code 2.1.323 with, unless a table says
  otherwise, one model (Haiku 5.5). Codex was run with one model, on the four families of
  questions.
* Single pairs vary by a factor of two in either direction, also with two runs each. Read the
  totals.
* The kernel is code the model has seen in training, and its names are unique. A control agent
  answers such questions in three or four requests, and there is not much left to save.
* The tasks of N were chosen for names that several types share. That is where structure
  matters most, and it is not every question an agent asks.
* The messages of E are written out in the source in full. A message that is put together at
  run time (a format string with values in it) is found by its longest fixed part, which the
  kernel questions about log lines exercise and E does not.
* A bug fix is mostly the model's own work: reproducing, editing, running tests again. The
  bug-fix families show what the plugin costs or saves there, and the answer is that it costs
  nothing and saves little. Their medians are level with the control and the totals, 0.93 and
  0.92, come from a few long sessions. Six attempts to make that figure move are above.
* The card answers three shapes of question, and it reads them from an English sentence. The
  callee family that measures it is twelve questions of one repository in one language, and
  their prompts were used without a model while the card was made. The kernel questions are
  the only place where the path and the member questions were measured in sessions, two and
  two of them.
* The code-writing family is two repositories, one language and twenty functions of 12 to 55
  lines whose body was taken out of a project that already has their callers and their tests.
  Writing a new function into a project that has neither is a different task, and nothing here
  measures it.
* The bug reports (F), of the kernel and of the Python projects, were run with the plugin as
  it is installed on Haiku 5.5 only, and that was version 0.3.0.
* Under the protocol of the published comparison each of the 56 final reports was run once in
  each arm, so a difference of two or three reports between two arms is noise. What ran there
  is codefast's MCP server with no hooks, which is how the paper runs a tool and not how the
  plugin is installed. The paper's arm is rebuilt from its repository as its README and its
  runner describe it. Its own runs, its sampled reports and its transcripts are not published,
  so nothing here says what its sessions did in the version of Claude Code it used.
* There is no rename family for a language without a compiler. Two attempts to build one for
  Python (sqlglot) failed for want of a reference to check against: a refactoring library
  renamed the wrong places, and a rename traced from the running tests left 12 of 16
  candidates with new failing tests. The four that passed changed three or four lines each.
