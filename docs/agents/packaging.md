# Packaging

The repository root is also the plugin root, the marketplace root and the extension root for
every coding harness. `docs/HARNESSES.md` says which harness reads which file.

| You changed | Run |
|---|---|
| a manifest, `hooks/`, `bin/codefast`, `install.sh` or `npm/` | `python3 tests/packaging/manifests/validate.py` |
| `bin/codefast` or `install.sh` | `sh tests/packaging/manifests/launcher_test.sh target/release/codefast`, and once more with `CODEFAST_TEST_SH=dash` |
| `npm/` | `sh tests/packaging/manifests/npm_shim_test.sh` |
| a manifest, the launcher, `src/mcp.rs` or `src/setup/` | `cargo test --test mcp_manifests --test hooks_manifests` |

Things that must stay true:

- Do not add `hooks/hooks.json`, a root `.mcp.json` or a root `CLAUDE.md`. Several harnesses
  read those paths with different meanings (`docs/HARNESSES.md`, "Paths to keep free").
- Do not add a root `agents/` directory. Harnesses read it as sub-agent definitions.
- `bin/codefast` and `install.sh` stay POSIX `sh`. They are tested under dash and BusyBox.
- `bin/codefast` never writes to stdout. Stdout belongs to the MCP protocol.
- The version is written in `Cargo.toml`. The manifests, `bin/codefast` and `npm/` repeat it,
  and `validate.py` fails when one of them differs.
