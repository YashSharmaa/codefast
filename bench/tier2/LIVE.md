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
