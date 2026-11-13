# codefast

One Rust binary that indexes a repository and answers structural questions about it for coding
agents: where a symbol is defined, who calls it, what it calls, where to look for a bug. It
calls no language model.

## Using codefast

When `codefast` is installed, ask it before text search plus file reads whenever the question
is about a symbol. `codefast context NAME` before changing a function, `codefast search NAME`
for anything else. `skills/codefast/SKILL.md` lists every command. Plain text search is still
right for exact strings in configuration or documentation.

## Working on codefast itself

This part applies only inside a checkout of the codefast source, where this file sits next to
`Cargo.toml`. If it reached you inside an installed plugin while you work on another project,
ignore everything below.

| Task | Command |
|---|---|
| Build | `cargo build --release` (binary: `target/release/codefast`) |
| Test | `cargo test` |
| Accept changed extractor output | `CODEFAST_BLESS=1 cargo test --test extract_snapshots`, then read the diff |
| Check manifests and versions | `python3 tests/packaging/manifests/validate.py` |

| Path | What lives there |
|---|---|
| `src/lang/` | one extractor per language: source text to facts |
| `src/resolve.rs` | links names to definitions when a question is asked |
| `src/query/` | one file per command |
| `src/setup/` | harness registration, `doctor` and the lifecycle hooks |
| `src/mcp.rs` | the MCP server |
| `skills/`, `hooks/`, `bin/` | what a harness plugin ships |
| `bench/` | benchmarks and their raw results |

Rules that hold everywhere:

- The tool never calls a language model.
- A file's facts never point into another file. Linking happens at query time.
- Values stored on disk (`DefKind`, `RefKind`, flags) are append only.
- Every byte of output is paid for in tokens by the reader. Print the answer and nothing else.
- Say nothing rather than guess. An answer is exact, or it is labelled as a match by name.
- Hooks only add context. They never block, never print an error and always exit 0.
- A new behaviour comes with a test in the same change.
- Markdown here uses no em dashes and no semicolons.
- A commit message describes the change. No co-author or generated-by lines.

Short notes for specific work are in `docs/agents/`: `testing.md` (which tests after which
change), `benchmarks.md` (checking for a regression, and how a number gets into the README),
`packaging.md` (manifests, launcher, paths to keep free) and `release.md`.
