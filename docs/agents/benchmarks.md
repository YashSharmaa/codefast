# Benchmarks

`bench/README.md` has the full instructions. This is the short version for checking a change.

```
cargo build --release
export CODEFAST_BIN=$PWD/target/release/codefast
```

| Check | Command | Needs |
|---|---|---|
| Call graph against compiler oracles | `python3 bench/scripts/oracles/score.py --oracle bench/oracles/gitleaks --tool edges.jsonl --out score.json`, where `edges.jsonl` comes from `codefast export-edges` | the repositories from `bench/scripts/oracles/fetch_repos.sh` |
| 72 verified questions about the Linux kernel | `uv run bench/k3/run_k3.py --questions bench/k3/questions.verified.jsonl` | a kernel checkout in `bench/repos/linux` |
| Localisation without a model | `uv run bench/tier1/run_locate.py --dataset locbench --loader-args "--subset ripwire-r4-60"` | `bench/scripts/datasets/fetch_datasets.sh` |
| Real sessions, with and without the plugin | `uv run bench/tier2/live.py run --tag T --n 13 --arms control,plugin --jobs 2`, then `score --tag T` | Claude Code and `CLAUDE_CONFIG_DIR` set to the login to use. This spends model usage |
| The same on tasks no build has been looked at on | `uv run --with tiktoken python -I bench/tier2/live.py run --tag H --held K2,C,N,E,R,D,I,L,W,W2 --arms control,plugin --reps 2 --jobs 2` | the same, plus the go-git checkout and its oracle |
| How much of a kernel question's verified answer the prompt card already holds, with no model | `uv run --with tiktoken python -I bench/tier2/card_k.py` | the kernel checkout. It builds an index of its own for the binary |
| Which prompts of which family a change to the hooks reaches, with no model | `python3 -I bench/tier2/card_diff.py --old OLD_BINARY --new NEW_BINARY --show` | the task files the `make_*_tasks.py` builders write |
| The hooks, the MCP server and the command line inside Codex, with no model | `python3 -I bench/codex/wiring.py` | `codex` on `PATH`. It runs in a sandbox home against a local stand-in for the model |

What a bug fix can save at best: the `oracle` arm of `live.py` tells a session the file and
function of the real fix before it starts, for nothing (0.95 of the control from the issue text
alone, 0.97 with the failing tests given), and `oracle-context` adds that function's body, its
callers and its tests (0.86). The plugin is at 1.00. So the push is worth 14% when it lands on
the right function and nothing at the 0.45 precision the ranker has on report text. A change
that claims to help this family has to raise that precision, and has to be measured over four
runs per arm: two runs of the same build said 0.92 where four said 1.00.

What has been tried on the file ranking and did not work, so that it is not tried again (each
measured on the development halves and on the 14 development bug reports):

- **Scoring a query word that is also a part of a file's path** (this one reached a release:
  it went out in 0.2.3 by a careless `git add -A` and was taken out again in 0.2.4, which is
  why a change to the ranking is measured on the dev halves before it is committed, and why
  `git add -A` is not how a source change is staged) (`iwlwifi`, `a4tech`,
  `nouveau`). 53 of the 80 kernel reports whose file the ranking misses name a component of
  that file's path, so the headroom is real, but adding the signal lowered every set: on the
  development halves LinuxFLBench went from 0.370 to 0.303 (right file first), ContextBench
  from 0.684 to 0.474, Loc-Bench 60 from 0.519 to 0.519 with a worse top 5. Restricting it to
  components that name at most 64 files did not recover it (ContextBench 0.474, Loc-Bench 60
  0.444). A path word promotes files that have no other evidence, and there are many of them.
- **Stronger length normalisation** (`B` from 0.4 to 0.75). The bug reports improve, from 0.29
  to 0.36 for the right file first, and ContextBench falls from 0.684 to 0.474.
- **Damping a file that no code-shaped word of the query matches.** No effect: every candidate
  file already matches at least one.

And on the bug-fix sessions, three things that worked on the development tasks and not on the
held-out ones, which is why both are always run before a claim: the bodies of the three
best-ranked functions (0.95 against 1.03), the tests that reach what an edit changed (0.93
against 1.03) and, earlier, the ranking alone. A development-set gain in this family is not
evidence until the held-out tasks repeat it.

Rules for numbers:

- A change to name linking must not lower precision or recall on any oracle. Run all four
  (redis, gitleaks, syft, zod) before and after.
- A change to how receivers are typed is also counted on the matches by name
  (`codefast export-edges --loose --tests`, then `bench/scripts/oracles/by_name.py`): fewer
  candidates, and none of the confirmed ones lost.
- A number in the README comes from a file in `bench/results/`. Change the file first, then
  the README.
- Other tools are timed in the same session, with the same thread limit, on the same tree.
- Agent experiments keep two sets apart. Development tasks may be read and tuned on. Held-out
  tasks are run once a build is final and are what the README reports. Once a held-out session
  has been read, its task counts as a development task for every later build
  (`bench/tier2/LIVE.md`).
- A change to what the hooks say is judged on real sessions, not on how the text reads. Before
  a session is spent on it, `card_diff.py` says which prompts it reaches: a change meant for
  one kind of question should leave the prompts of every other family exactly as they were.
- A new family is measured on a second repository before its figure goes into the README.
  Eight code-writing tasks of one repository said 0.95 of the control. Twelve of a second,
  built the same way, said 1.08, and the README had carried the first figure for some hours.
- An index is tied to the version of the program that wrote it, and a hook that finds another
  version's index says nothing until the index has been rebuilt. Two builds never share an
  index directory while sessions run: the comparison would be with a build that has no hooks.
  `live.py` rebuilds the shared indexes for its binary when a batch starts, and the offline
  checks keep one index per version.
- Report what was measured, including the cases with no gain.
