# Which tests to run

`cargo test` runs everything. After a narrow change these are the ones that matter.

| You changed | Run |
|---|---|
| a language extractor (`src/lang/`) | `cargo test --test extract_snapshots`, and the language's own file where it has one (`tests/lang_php.rs`, `tests/lang_components.rs` for Vue, Svelte and Astro) |
| name linking (`src/resolve.rs`) or a command (`src/query/`) | `cargo test --test resolve_cli`, then the call graph check in `benchmarks.md` |
| the MCP server (`src/mcp.rs`) | `cargo test --test mcp_modern --test mcp_legacy --test mcp_malformed --test mcp_roots` |
| hooks (`src/setup/hook.rs`, `hooks/`) | `cargo test --test hooks_claude --test hooks_codex --test hooks_hostile --test hooks_manifests --test hooks_push` |
| the check after an edit (`src/query/changed.rs`) | `cargo test --test hooks_push`, then `codefast check --false-alarms` in two or three large repositories: it must stay silent on working code |
| the context audit (`src/setup/audit.rs`) | `cargo test --test doctor_context`, then compare `codefast doctor context` with `/context` in a real session |
| setup or doctor (`src/setup/`) | `cargo test --test setup_cli --test setup_doctor --test setup_harnesses --test setup_native` |
| a manifest, `bin/codefast`, `install.sh` or `npm/` | see `packaging.md` |

Extractor snapshots live in `tests/fixtures/`. When one changes, run the test again with
`CODEFAST_BLESS=1`, read the diff of the snapshot, and keep it only if every changed line is
what you meant.

A fixture is the smallest source file that shows the behaviour. A test of a command runs the
real binary on a small project it writes itself (`tests/resolve_cli.rs` is the pattern).
