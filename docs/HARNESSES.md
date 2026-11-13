# Harness integrations

This repository is at once a Rust crate, a plugin, a plugin marketplace and an extension: its
root is what every coding harness installs. This page says what each harness gets, which file
it reads, and which paths must stay free. `docs/INSTALL.md` has the commands.

## What an integration can contain

| Part | What it is | Cost in the agent's context |
|---|---|---|
| Skill | `skills/codefast/SKILL.md`: when to use codefast and which command answers what. The agent runs `codefast ...` in its shell. | The name and description, about 140 tokens. The body (under 1,000 tokens) is read only when the skill is used. Both figures are Claude Code's own estimate (`claude plugin details codefast` in 2.1.323 reported 140 always on and 940 on invoke for the skill as it stood on the morning of 2026-11-13). |
| MCP server | `codefast mcp`: five read-only tools (`search`, `show`, `refs`, `path`, `grep`) with the same plain-text answers as the command line. | The server instructions and five tool definitions. Most harnesses defer them until needed. |
| Hooks | `codefast hook EVENT`, with the events `session-start`, `pre-tool-use` and `post-tool-use`: at session start they make sure the index exists, after a search for one known symbol they add where it is defined and who calls it, and before a whole-file read of a long source file they add one line naming the outline command. | Nothing unless they have something to add. They never block or rewrite a tool call. |
| Rules block | A marked block of fifteen lines in a project's `AGENTS.md`, written by `codefast setup agents-md`: which command answers which question, and when plain grep is still right. For harnesses that read nothing else (Aider), and the one part that is in context without the agent choosing it. | Always: 1.0 KB, about 250 tokens. |
| Launcher | `bin/codefast`: a POSIX shell script that finds or fetches the binary. It is what plugins put on `PATH` and what their MCP entries start. | None. |

The design is command line first: the skill works in every harness that has a shell, including
those without MCP. The MCP server and the hooks are additions.

## Compatibility matrix

"Mark" is the strongest evidence there is for that row: **S** tested in a sandbox on 2026-11-13,
**V** the files that harness reads are validated against the published schema of their format,
**D** documented only.

| Harness | Installs this repository as | Skill | MCP server | Hooks | Mark |
|---|---|---|---|---|---|
| Claude Code | plugin from the marketplace in `.claude-plugin/` | yes, plus `codefast` on the Bash tool's `PATH` | yes, declared in `.claude-plugin/plugin.json` | yes: `SessionStart`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure` | S (2.1.322, 2.1.323) |
| OpenAI Codex CLI | plugin (`plugin.json` + `mcp.json`) from the marketplace in `.agents/plugins/` | yes, as `codefast:codefast` | yes, started in the plugin's directory (every call needs `root`). Better is the entry `codefast setup codex` adds, which Codex uses in its place | not from the plugin. Use `codefast setup codex --hooks`, or `hooks/codex.json`, in a config layer | S (0.164.0) |
| Gemini CLI | extension (`gemini-extension.json`) | yes | yes, with `--root ${workspacePath}` | none | D |
| Cursor | Agent Plugin (`plugin.json` + `mcp.json`) | yes | yes | none | V |
| GitHub Copilot CLI, VS Code | Agent Plugin | yes | yes | none | V |
| Qwen Code | Agent Plugin (loaded natively) | yes | yes | none | V |
| Kiro | Power (Agent Plugins format) | yes | yes | none | V |
| Antigravity | Agent Plugin | yes | yes | none | V |
| Factory Droid | plugin, translated from the Claude Code layout | yes | yes | not known | D |
| Devin Desktop | plugin, read from the Claude Code layout | yes | yes | none | D |
| Goose | MCP extension in its config, plus the skill from `goose plugin install` | yes | yes, by config | none | D |
| OpenCode | MCP entry, plus the skill in `.agents/skills` | yes | yes, by config | none | D |
| Amp | MCP entry, plus `amp skill add` | yes | yes, by config | none | D |
| Cline | MCP entry, plus the skill | yes | yes, by config | none | D |
| Kilo Code | MCP entry, plus the skill | yes | yes, by config | none | D |
| Crush | `crushrc` line, plus the skill | yes | yes, by config | none | D |
| Junie | `.junie/mcp/mcp.json`, plus the skill copied to `.junie/skills` | yes | yes, by config | none | D |
| Deep Agents Code | `.mcp.json`, plus the skill | yes | yes, by config | none | D |
| DeepSeek Harness (dsh) | MCP entry in its settings, plus the skill in `.agents/skills` | yes | yes, by config | none | D |
| Zed | `context_servers` entry | through the external agents it hosts | yes, by config | none | D |
| Pi | package from git (it finds `skills/`) | yes | no MCP client | none | D |
| Aider | instruction block in `AGENTS.md` | no | no | no | D |

In every harness except Claude Code the skill needs `codefast` on `PATH` (`install.sh`): only
Claude Code puts a plugin's `bin/` on the agent's `PATH`. A plugin's MCP server starts through the
launcher everywhere, which finds the binary on `PATH` or in its cache, or downloads it once a
release exists.

What "tested in a sandbox" covered, exactly:

- **Claude Code 2.1.322 and 2.1.323**: `claude plugin validate --strict` on the plugin manifest and on the
  marketplace (validation passes with no errors and no warnings in either manifest, and 2.1.323
  adds one remark from its plugin-directory checks, that there is no icon).
  `claude plugin install codefast --marketplace URL` from a git URL served on 127.0.0.1.
  `claude plugin details` listing one skill, four hook events and one MCP server.
  `claude mcp list` connecting to the plugin's server through the launcher, once with the binary
  named by `CODEFAST_BIN` and once with the launcher fetching it from a local directory. The
  three hook command lines run by hand from the plugin cache.
  `claude mcp add ... -- codefast mcp` followed by `claude mcp list`. No session was started, so
  no hook was fired by Claude Code itself.
- **Codex CLI 0.164.0**: `codex plugin marketplace add URL`, `codex plugin add codefast@codefast`,
  `codex plugin list`, `codex mcp list`. Then the app server's `skills/list` (the skill is there),
  `mcpServerStatus/list` (the server started through the launcher, with `codefast` on `PATH`, and
  listed five tools), `plugin/read` and `hooks/list`. `codex mcp add codefast -- codefast mcp`.
  No session was started.
- Both harnesses were also given the repository as a plain directory (`claude plugin marketplace
  add DIR`, `codex plugin marketplace add DIR`) with the same result.
- **The whole route once more by hand** from a clone-like copy of the repository (Claude Code
  2.1.323, codex-cli 0.164.0): `install.sh`, the plugin installs, the MCP server driven through
  the launcher exactly as each manifest starts it (both protocol eras, five tools, real answers),
  every hook command line with a payload in the shape that harness documents, and
  `codefast setup` and `codefast doctor` for both harnesses with and without the plugin. Tool
  calls were also made through Codex itself (the app server's `mcpServer/tool/call` on a thread
  without a turn, so without a model): the plugin's server answers only with `root`, the
  configured entry answers without it, and with both present Codex starts only the configured
  one.
- **Launcher and `install.sh`**: 134 checks under sh (bash 3.2 in POSIX mode), dash and
  `bash --posix` on macOS, 133 under BusyBox 1.36.1 on Linux arm64 (with a stub in place of the
  binary), against a local directory in place of the release server.
- **npm wrapper**: 12 checks in a hand-made `node_modules` layout (Node.js 26). Nothing was
  installed from or published to a registry.
- **`npx skills add`** (skills 1.8.1) with a local copy of the repository as the source, for the
  agent ids named in `docs/INSTALL.md`. Only the files it wrote were checked.

## Which file is for whom

| File | Read by | Purpose |
|---|---|---|
| `plugin.json` | Codex, Cursor, Copilot CLI, VS Code, Qwen Code, Kiro, Antigravity and other Agent Plugins clients | Agent Plugins 1.0.0 manifest. `extensions.com.openai.interface` is Codex's listing text. |
| `mcp.json` | the same clients | The MCP server: `./bin/codefast mcp`. No placeholders, because Cursor does not expand `${PLUGIN_ROOT}`. |
| `.claude-plugin/plugin.json` | Claude Code. Also Devin Desktop and Factory Droid (they read the Claude layout) | Claude Code manifest: the hooks file and the MCP server, inline. |
| `.claude-plugin/marketplace.json` | Claude Code. Also Copilot CLI, Factory Droid and Codex as a fallback | Marketplace with one plugin whose source is `./`. |
| `.agents/plugins/marketplace.json` | Codex | Marketplace with one plugin whose source is the marketplace root. |
| `gemini-extension.json`, `GEMINI.md` | Gemini CLI. Qwen Code, Goose and Antigravity can import them | Extension manifest and its always-loaded context (at most 40 lines). |
| `skills/codefast/SKILL.md` | every harness with Agent Skills, and `npx skills add` | The skill. Frontmatter keeps to the six portable keys. |
| `skills/codefast/agents/openai.yaml` | Codex | Display name, short description and starter prompt of the skill. Codex 0.164.0 reports all three back in `skills/list`. |
| `hooks/claude-code.json` | Claude Code, through `.claude-plugin/plugin.json` | Plugin hooks. Commands start at `${CLAUDE_PLUGIN_ROOT}`. |
| `hooks/codex.json` | nobody automatically | The hooks for a Codex configuration layer, with `codefast` taken from `PATH`: exactly what `codefast setup codex --hooks --scope project` writes into `.codex/hooks.json`, kept here for those who merge it by hand. |
| `bin/codefast` | every plugin install | The launcher. |
| `install.sh` | people | Installs the binary into `~/.local/bin`. |
| `server.json` | the MCP Registry | Registry entry that points at the npm package. |
| `npm/codefast`, `npm/platforms/*` | npm | A wrapper package whose optional dependencies carry one binary per platform. Not published. |
| `AGENTS.md` | agents working in this repository. Devin loads a plugin's root `AGENTS.md` as an always-on rule (its plugin documentation, read on 2026-11-13) | Starts with how to use codefast, so that it is also right as a rule. The part about working on codefast says of itself that it applies only inside a codefast checkout, so that it is not taken for a rule about the user's project. |

## What `codefast setup` registers

`codefast setup ID` writes the MCP entry and the skill (and, for Claude Code and Codex, the hooks
with `--hooks`). Where the harness has a command of its own and that program is on `PATH`, setup
runs the command and then checks the harness's file. Otherwise it edits the file. `BIN` is this
binary's absolute path at user scope and plain `codefast` at project scope. The skill goes into
a `codefast/` directory under the place named in the last column. `docs/INSTALL.md`, section 2,
has the options.

| Target | MCP entry, user scope | MCP entry, project scope | Skill directory (user / project) |
|---|---|---|---|
| `claude-code` | `claude mcp add --transport stdio --scope user codefast -- BIN mcp`, else `mcpServers.codefast` in `~/.claude.json` (inside `$CLAUDE_CONFIG_DIR` when that is set) | the same command with `--scope project`, else `.mcp.json` | `~/.claude/skills` / `.claude/skills` |
| `codex` | `codex mcp add codefast -- BIN mcp`, else `[mcp_servers.codefast]` in `~/.codex/config.toml` (`$CODEX_HOME`) | `.codex/config.toml` | `~/.agents/skills` / `.agents/skills` |
| `gemini-cli` | `gemini mcp add --scope user codefast BIN mcp`, else `mcpServers.codefast` in `~/.gemini/settings.json` (`$GEMINI_CLI_HOME/.gemini`) | the same command with `--scope project`, else `.gemini/settings.json` | `~/.agents/skills` / `.agents/skills` |
| `qwen-code` | `qwen mcp add --scope user codefast BIN mcp`, else `~/.qwen/settings.json` | the same command with `--scope project`, else `.qwen/settings.json` | `~/.qwen/skills` / `.qwen/skills` |
| `cursor` | `mcpServers.codefast` in `~/.cursor/mcp.json` | `.cursor/mcp.json` | `~/.agents/skills` / `.agents/skills` |
| `copilot` (VS Code) | `code --add-mcp '{...}'` only: the profile's `mcp.json` is VS Code's to write, so without `code` on `PATH` the command is printed for you | `servers.codefast` in `.vscode/mcp.json` | `~/.agents/skills` / `.agents/skills` |
| `copilot-cli` | `copilot mcp add codefast -- BIN mcp`, else `~/.copilot/mcp-config.json` (`$COPILOT_HOME`) | `.mcp.json` | `~/.agents/skills` / `.agents/skills` |
| `opencode` | `mcp.codefast` in `~/.config/opencode/opencode.json` or `.jsonc` (`mcp.servers.codefast` when the file already uses that layout) | `opencode.json`, `opencode.jsonc` or the same under `.opencode/`, whichever exists | `~/.agents/skills` / `.agents/skills` |
| `goose` | `extensions.codefast` in `~/.config/goose/config.yaml` | none: Goose has no project-level extensions | `~/.agents/skills` / `.agents/skills` |
| `amp` | `amp mcp add codefast -- BIN mcp`, else `"amp.mcpServers"` in `~/.config/amp/settings.json` | `.amp/settings.json` (Amp asks for approval once: `amp mcp approve codefast`) | `~/.agents/skills` / `.agents/skills` |
| `cline` | `cline mcp add --yes codefast -- BIN mcp`, else `~/.cline/data/settings/cline_mcp_settings.json` (`$CLINE_MCP_SETTINGS_PATH`) | none: Cline keeps servers per user | `~/.agents/skills` / `.agents/skills` |
| `kilo-code` | `mcp.codefast` in `~/.config/kilo/kilo.jsonc` (or `kilo.json` when that exists) | `kilo.jsonc`, `kilo.json` or the same under `.kilo/`, whichever exists, else `.kilo/kilo.jsonc` | `~/.agents/skills` / `.agents/skills` |
| `droid` | `droid mcp add codefast 'BIN mcp'`, else `~/.factory/mcp.json` | `.factory/mcp.json` | `~/.factory/skills` / `.agents/skills` |
| `crush` | the line `mcp add codefast --command BIN --args mcp` in `~/.config/crush/crushrc` (`$CRUSH_GLOBAL_CONFIG`), and `crush.json` only where it is the only configuration | `.crushrc` (or an existing `crushrc`) | `~/.agents/skills` / `.agents/skills` |
| `zed` | `context_servers.codefast` in `~/.config/zed/settings.json` | `.zed/settings.json` | none: Zed runs other agents |
| `kiro` | `~/.kiro/settings/mcp.json` | `.kiro/settings/mcp.json` | none: its skill directory is not documented |
| `junie` | `~/.junie/mcp/mcp.json` | `.junie/mcp/mcp.json` | `~/.junie/skills` / `.junie/skills` |
| `deepagents` | `~/.deepagents/.mcp.json` | `.mcp.json` | `~/.agents/skills` / `.agents/skills` |
| `pi` | none: Pi has no MCP client | none | `~/.agents/skills` / `.agents/skills` |
| `devin` | printed for you to add (the schema of `mcp_config.json` is not published) | printed | `~/.agents/skills` / `.agents/skills` |
| `antigravity` | printed (`agy plugin install`): its servers come from plugins | printed | `~/.agents/skills` / `.agents/skills` |
| `dsh` | printed (a row for the Settings page or `cordis.patch.yml`) | printed | `~/.agents/skills` / `.agents/skills` |
| `aider` | no MCP: the rules block in the project's `AGENTS.md`, and `read: [AGENTS.md]` in `.aider.conf.yml` | the same | none |

Hooks, with `--hooks`: `claude-code` in `~/.claude/settings.json` or `.claude/settings.json`,
`codex` in `~/.codex/hooks.json` or `.codex/hooks.json`. `$XDG_CONFIG_HOME` replaces `~/.config`
where a harness honours it (Zed on macOS does not).

Only the first two rows were exercised with the real program (Claude Code 2.1.323, codex-cli
0.164.0). For the others the file is written in the format the harness documents and read back
by codefast's own tests, and the command lines were checked with stand-in programs.

What setup makes of a plugin that is already installed:

| Harness | The plugin brings | Setup still registers |
|---|---|---|
| Claude Code | skill, MCP server, hooks | nothing. Registered a second time, the server and the hooks would both run twice, which `codefast doctor` reports as `twice`. A plugin installed with `--scope project` counts in that project only. |
| Codex | skill | the MCP server, and the hooks with `--hooks`. The plugin's own server is started in the plugin's directory and needs `root` with every call. The entry in `config.toml` takes its place. |

## Paths to keep free

Several harnesses read the same default paths with different meanings. These must not appear at
the repository root, and `tests/packaging/manifests/validate.py` fails if they do.

| Path | Why not |
|---|---|
| `hooks/hooks.json` | Claude Code, Codex, Gemini CLI, Goose and Cursor-format plugins all load it by default. Gemini CLI uses other event names and millisecond timeouts, warns about every event it does not know ("Invalid hook event name") and mentions hooks in its install consent. So the Claude Code hooks have their own file name, and Gemini CLI gets none. |
| `.mcp.json` | It is project-scope MCP configuration for Claude Code and Deep Agents Code whenever someone opens this repository, where `${CLAUDE_PLUGIN_ROOT}` is not set. The Claude Code manifest declares the server inline instead. |
| `CLAUDE.md` | Claude Code does not load it from a plugin and `claude plugin validate` warns about it. |
| `commands/`, `agents/`, `output-styles/`, `workflows/`, `themes/`, `monitors/`, `settings.json`, `policies/` | Claude Code or Gemini CLI would load them as plugin content. |
| `.codex-plugin/` | `plugin.json` already carries `extensions.com.openai`, which replaces that overlay. |

## How the MCP server learns which repository to answer for

The answer differs by harness, because Agent Plugins starts the server in the plugin's own
directory:

| Started by | The repository comes from |
|---|---|
| Claude Code plugin | `CODEFAST_ROOT=${CLAUDE_PROJECT_DIR}` in the manifest (Claude Code also exports `CLAUDE_PROJECT_DIR`). |
| Gemini CLI extension | `--root ${workspacePath}`. |
| A config entry you wrote (`codefast mcp`) | The working directory the harness starts it in, when that is a git work tree. Or `--root`, where the harness has a workspace variable (`${workspaceFolder}` in Cursor and VS Code). |
| Agent Plugins clients (Codex, Cursor, Copilot, ...) | The working directory is the plugin directory, which the server refuses to treat as a project (the launcher tells it where the plugin is through `CODEFAST_PLUGIN_ROOT`). It then asks for the `root` argument on each call, and says so in the instructions it gives the model. It does not read the client's workspace roots. Verified with Codex 0.164.0, which keeps the plugin as the git checkout it cloned (so the plugin directory is a work tree, of the wrong repository) and declares no `roots` capability: a call without `root` is refused by name, a call with it is answered. |
| Codex, an entry in `config.toml` (`codex mcp add`, `codefast setup codex`) | The working directory, which is the project. Codex uses this entry in place of the plugin's server of the same name. |

The skill does not have this problem: the agent's shell is already in the repository.

## The binary

A plugin is a git checkout and cannot hold one executable per platform, so plugins ship the
launcher. Its cache is one directory for the user,
`${CODEFAST_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/codefast}/<version>/`, not the per-plugin
data directory a harness offers: Claude Code does not give `CLAUDE_PLUGIN_DATA` to commands run
by its Bash tool, so a per-plugin cache would be filled once by the MCP server and missed by
every command the agent types. One cache also means one download for all harnesses.

When no binary is there yet, `codefast hook session-start` starts the download in the background
and returns. The other hooks return at once. The background download lets go of the hook's
stdout and stderr first, because a harness waits until both are closed. `codefast mcp` and
ordinary commands download in the foreground and say so on stderr. A downloaded binary has to
answer `--version` before it enters the cache: the cache is consulted before `PATH`, so a binary
that cannot run would otherwise hide every working codefast.

The launcher's variables reach it only where the harness passes the environment on. Claude Code
does. Codex starts a plugin's MCP server with `HOME`, `PATH`, `TMPDIR`, `TERM`, `PLUGIN_ROOT` and
`PLUGIN_DATA`, and with nothing else that was set in the shell Codex was started from (recorded
with 0.164.0 by a stand-in `codefast` on `PATH`). Gemini CLI documents the same reduced
environment for extensions. There `CODEFAST_BIN`, `CODEFAST_NO_DOWNLOAD`, `CODEFAST_CACHE_DIR`
and proxy settings have no effect, and the binary has to be on `PATH` or already in the cache
under `HOME`.

Windows: the launcher is a shell script, so a plugin's MCP entry cannot start it there. Use the
npm package or a `codefast.exe` on `PATH` with a hand-written MCP entry. Not tested.

## Hooks

| Event | Matcher in Claude Code | Matcher in Codex | Command |
|---|---|---|---|
| `SessionStart` | none | none | `codefast hook session-start` |
| `PreToolUse` | `Read\|Bash` | `Bash` | `codefast hook pre-tool-use` |
| `PostToolUse`, and `PostToolUseFailure` in Claude Code | `Bash\|Grep\|Glob` | `Bash` | `codefast hook post-tool-use` |

This is the wiring `src/setup/hook.rs` documents. `Bash` is in every matcher because Claude Code
on macOS and Linux searches through its Bash tool, and Codex has no other tool that reads or
searches. Timeouts are 10 seconds for `SessionStart` and 5 for the others.

The same wiring exists in three places, and `tests/hooks_manifests.rs` fails when they drift
apart: `hooks/claude-code.json` (the plugin, which runs `"${CLAUDE_PLUGIN_ROOT}/bin/codefast"`),
what `codefast setup claude-code --hooks` writes into `settings.json` (it runs the binary by its
path), and `hooks/codex.json`, which is what `codefast setup codex --hooks --scope project`
writes. Every line that is not inside the plugin ends in `2>/dev/null || true`, so that a hooks
file shared through a repository stays silent on a machine without codefast.

Codex loads no hooks from this plugin: its loader skips hooks for every plugin whose manifest is
the Agent Plugins `plugin.json` (`codex-rs/core-plugins/src/loader.rs`, confirmed with 0.164.0,
where `plugin/read` reports none whether the hooks are declared in `extensions.com.openai`, in a
`.codex-plugin/plugin.json` overlay or as `hooks/hooks.json`). The same hooks do load from a
plugin that has only the older `.codex-plugin/plugin.json`, which this repository cannot use
without giving up the Agent Plugins manifest. OpenAI's plugin documentation describes
`extensions.com.openai.hooks` and says plugin hooks are supported for plugins installed by hand in
the Codex desktop app. That app was not tested. `hooks/codex.json` is therefore a
configuration-layer file. If a later Codex loads plugin hooks after all, do not add them to
`plugin.json` without removing the config-layer copy from the instructions, or each hook would
run twice.

## Version

`Cargo.toml` is the source. The same version is repeated in `plugin.json`,
`.claude-plugin/plugin.json`, `gemini-extension.json`, `server.json` (twice), `npm/codefast/package.json`
(the version and every optional dependency), each `npm/platforms/*/package.json`, and the
`version=` line of `bin/codefast`, which decides which release the launcher downloads.
`validate.py` fails when one of them differs.

## Tests

```sh
cargo test
python3 tests/packaging/manifests/validate.py --schemas DIR --fetch --binary target/release/codefast
sh tests/packaging/manifests/launcher_test.sh target/release/codefast
sh tests/packaging/manifests/npm_shim_test.sh target/release/codefast
sh tests/packaging/manifests/harness_sandbox_test.sh target/release/codefast
```

- `cargo test` runs the packaging tests with everything else: `tests/mcp_*.rs` (the protocol in
  both eras, malformed input, which repository a call addresses), `tests/hooks_*.rs` (the hook
  command against recorded payloads of each harness, and against hostile ones) and
  `tests/setup_*.rs` (setup and doctor against throwaway home directories). Two of the files hold
  the pieces against each other and read the shipped files to do so: `tests/mcp_manifests.rs`
  starts the MCP server through the launcher the way each manifest starts it (command, arguments,
  working directory and environment of that harness) and expects the same five tools and one
  version everywhere. `tests/hooks_manifests.rs` runs every command line of the shipped hooks
  files and compares them with what `codefast setup --hooks` writes. `tests/setup_native.rs`
  runs the real `claude` and `codex` against a sandbox when they are installed and is skipped
  otherwise.
- `validate.py`: every manifest against the rules of its format and against each other (names,
  versions, paths), plus `sh -n`, `dash -n` and
  shellcheck on the two scripts. `--schemas DIR --fetch` downloads the Agent Plugins and MCP
  Registry schemas into DIR and validates against them. Without it only the built-in rules run.
- `launcher_test.sh`: the launcher's five steps, its checksum failures, a download that cannot
  run, an unpublished copy that must not reach the network, its hook behaviour and `install.sh`,
  in a throwaway `HOME` with a local directory as the release server. `CODEFAST_TEST_SH=dash`
  chooses the shell.
- `npm_shim_test.sh`: the npm wrapper in a hand-made `node_modules` layout, including
  `CODEFAST_BIN` set to the wrapper itself. It is skipped when Node.js is not installed.
- `harness_sandbox_test.sh`: stages this repository as a git repository of its own, serves it on
  127.0.0.1, and installs it into Claude Code and Codex with throwaway configuration directories
  and an empty environment. It starts no session and calls no model. The two halves do not
  depend on each other, and a harness that is not installed is skipped.

Two things to know when a sandbox for such a test lies inside a checkout of this repository
(under `target/`, for example):

- codefast takes the nearest directory with `.git` or `.codefast` above the place it is run in
  for the repository. A sandbox project therefore needs a `.git` of its own, and a hook must be
  given a payload that names its directory. Otherwise the checkout itself is indexed and gets a
  `.codefast/` directory (ignored by git, harmless, but not what the test meant).
- Codex prefixes the name of every skill with the plugin whose `.claude-plugin/plugin.json` or
  `.codex-plugin/plugin.json` is found above the skill's file. Inside this checkout that is
  `codefast`, so every skill is listed as `codefast:NAME`, the built-in ones included. The
  plugin's own skill is `codefast:codefast` everywhere, because of the plugin's own root. A skill
  that setup copies to `~/.agents/skills` is plain `codefast` in an ordinary home directory.

## Where this differs from the format survey it was built from

The packaging follows a survey of harness plugin formats written on 2026-11-12 from each
harness's own documentation (`docs/research/harness-plugin-formats.md`, a working note that is
kept out of the repository by `.gitignore`). Building and testing it turned up eight corrections
to that survey. The section numbers are the survey's:

1. Section 4 puts the Claude Code hooks at `hooks/hooks.json`. Gemini CLI and Goose load that
   path too (see "Paths to keep free"), so the file is `hooks/claude-code.json`.
2. Section 4.1 declares Codex hooks in `extensions.com.openai.hooks`. Codex ignores them for
   Agent Plugins manifests (see "Hooks").
3. Section 4.2 caches the binary under `PLUGIN_DATA`. The launcher uses one user-level cache
   (see "The binary").
4. Section 7 lists the argument form of `codex plugin add` as unverified. It is
   `codex plugin add PLUGIN@MARKETPLACE`, or `codex plugin add PLUGIN --marketplace MARKETPLACE`.
5. Section 4.1 gives the Codex marketplace entry a `url` source. A `local` source with path `./`
   works for a marketplace added from a path and from git, and avoids a second clone.
6. Section 4.1 files the Codex listing under the category `Coding`. OpenAI's own marketplace
   (`openai/plugins`, read on 2026-11-13) has no such category: its developer plugins are under
   `Developer Tools`, which is what `plugin.json` and the marketplace entry use. The capability
   label stays `Read`, one of the three labels those manifests use (`Interactive`, `Read`,
   `Write`).
7. Section 4.3 puts "legacy `roots/list` when the client advertises roots" into the order in
   which the server finds the repository. The server does not ask for roots. It never sends a
   request of its own (the stateless revision 2026-07-28 has none, so this would be a path for
   handshake-era clients only), and Codex 0.164.0, the one Agent Plugins client that could be
   run here, declares no `roots` capability, so the request would not help where it is needed.
   The `root` argument and, for Codex, an entry in `config.toml` do the job instead (see "What
   `codefast setup` registers").
8. Section 5 gives the Codex plugin as the whole install. It brings the skill, but its MCP server
   is started in the plugin's directory and Codex loads no hooks from it, so the install
   instructions add `codefast setup codex --hooks --apply`.
