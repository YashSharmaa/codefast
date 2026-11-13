# codefast

codefast is one Rust binary that indexes a repository and answers structural questions about
it for coding agents: where a symbol is defined, who calls it, what it calls, how execution
gets from one function to another, which function prints a message, where to look for a bug.
It calls no language model.

## Using codefast

When `codefast` is installed, ask it before text search plus reading files whenever the
question is about a symbol. `codefast search NAME` is the entry point. `skills/codefast/SKILL.md`
lists every command. Plain text search is still right for exact strings in configuration or
documentation.

## Working on codefast itself

This section applies only inside a checkout of the codefast source, where this file sits next to
`Cargo.toml` and `src/`. If this file reached you as part of an installed plugin while you work
on another project, ignore everything below.

- Build with `cargo build --release` (the binary is `target/release/codefast`) and run the tests
  with `cargo test`. `docs/EXTRACTOR_GUIDE.md` explains how a language extractor is written.
- The repository root is also the plugin root, the marketplace root and the extension root
  for every coding harness. `docs/HARNESSES.md` says which harness reads which file, and
  `docs/INSTALL.md` how each one installs it.
- After touching a manifest, `hooks/`, `bin/codefast`, `install.sh` or `npm/`, run
  `python3 tests/packaging/manifests/validate.py`. It also checks that the version is the same
  everywhere: `Cargo.toml` is the source, and the manifests, `bin/codefast` and `npm/` repeat it.
  After touching `bin/codefast` or `install.sh` also run
  `sh tests/packaging/manifests/launcher_test.sh target/release/codefast`, once more with
  `CODEFAST_TEST_SH=dash`. After touching `npm/`, also run `npm_shim_test.sh` in the same directory.
  `cargo test --test mcp_manifests --test hooks_manifests` starts the MCP server and the hooks
  the way each shipped manifest does, and compares the hooks files with what `codefast setup`
  writes: run it after a change to a manifest, the launcher, `src/mcp.rs` or `src/setup/`.
- Do not add `hooks/hooks.json`, a root `.mcp.json` or a root `CLAUDE.md`: several harnesses
  read those paths with different meanings (`docs/HARNESSES.md`, "Paths to keep free").
- `bin/codefast` and `install.sh` must stay POSIX `sh` (they are tested under dash and BusyBox),
  and `bin/codefast` must never write to stdout: stdout belongs to the MCP protocol.
