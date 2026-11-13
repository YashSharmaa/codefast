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
| The same on tasks no build has been looked at on | `uv run bench/tier2/live.py run --tag H --held K,N,E,R,D,I,L --arms control,plugin --reps 2 --jobs 2` | the same, plus the go-git checkout and its oracle |

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
- A change to what the hooks say is judged on real sessions, not on how the text reads.
- Report what was measured, including the cases with no gain.
