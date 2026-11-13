//! What is known about each coding harness: how to tell that it is installed, where it keeps
//! its MCP servers, skills and hooks at user and project scope, which of its own commands
//! register a server, and what it calls its plugin.
//!
//! Sources: `docs/research/harness-plugin-formats.md` (sections 1.3, 2 and 5),
//! `docs/research/harness-gaps-and-context-audit.md` (part A) and each harness's documentation
//! as re-read on 2026-11-13. `claude` and `codex` were run against a sandbox; every other entry
//! is the documented format, and none of those harnesses was run.

use super::formats::{Server, YamlStyle};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The name codefast registers under everywhere.
pub const NAME: &str = "codefast";
pub const DESCRIPTION: &str = "Code search, references and call graphs from a prebuilt index";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    pub fn word(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

/// The machine as setup and doctor see it. Everything comes from the environment, so a test
/// (or a cautious user) can point all of it at a sandbox.
pub struct Ctx {
    /// The project: where project-scope files go.
    pub root: PathBuf,
    pub home: Option<PathBuf>,
    pub path: Vec<PathBuf>,
    /// The running executable.
    pub exe: PathBuf,
}

/// An environment variable that names a path: set, not empty, and not an unexpanded placeholder.
pub fn env_path(key: &str) -> Option<PathBuf> {
    let v = std::env::var_os(key)?;
    let ok = {
        let s = v.to_string_lossy();
        !s.trim().is_empty() && !s.contains("${")
    };
    ok.then(|| PathBuf::from(v))
}

impl Ctx {
    pub fn new(root: &Path) -> Ctx {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let home = env_path("HOME").or_else(|| env_path("USERPROFILE"));
        let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).filter(|d| !d.as_os_str().is_empty()).collect()).unwrap_or_default();
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(NAME));
        Ctx { root, home, path, exe }
    }

    /// A path under the home directory; without a home directory, a path that cannot exist.
    pub fn home(&self, rel: &str) -> PathBuf {
        match &self.home {
            Some(h) => h.join(rel),
            None => PathBuf::from("/nonexistent-home").join(rel),
        }
    }

    /// `$XDG_CONFIG_HOME/REL`, else `~/.config/REL`.
    pub fn config(&self, rel: &str) -> PathBuf {
        match env_path("XDG_CONFIG_HOME") {
            Some(c) => c.join(rel),
            None => self.home(".config").join(rel),
        }
    }

    pub fn project(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// The first executable called `name` on `PATH`.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        let names: Vec<String> = if cfg!(windows) { vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_string()] } else { vec![name.to_string()] };
        self.path.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| is_executable(p))
    }

    /// The path to show a person: `~/...` under the home directory, relative inside the project.
    pub fn show(&self, path: &Path) -> String {
        if let Ok(rel) = path.strip_prefix(&self.root)
            && !rel.as_os_str().is_empty()
        {
            return rel.display().to_string();
        }
        if let Some(rel) = self.home.as_ref().and_then(|h| path.strip_prefix(h).ok()) {
            return format!("~/{}", rel.display());
        }
        path.display().to_string()
    }

    /// The command to write into a configuration: for the user's own files the absolute path of
    /// this program (through the `PATH` entry that leads to it, which survives an upgrade better
    /// than a versioned install path); for files a project shares, the bare name.
    pub fn command(&self, scope: Scope) -> String {
        if scope == Scope::Project {
            return NAME.to_string();
        }
        let real = self.exe.canonicalize().unwrap_or_else(|_| self.exe.clone());
        let on_path = self.which(NAME).filter(|p| p.canonicalize().is_ok_and(|c| c == real));
        on_path.unwrap_or_else(|| self.exe.clone()).display().to_string()
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// One of the harness's own commands, as an argument vector.
#[derive(Debug, Clone, PartialEq)]
pub struct Native {
    pub program: &'static str,
    pub add: Vec<String>,
    /// Run first when an entry of ours is being replaced.
    pub remove: Option<Vec<String>>,
}

/// Where and how a harness keeps an MCP server entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Mcp {
    /// A JSON (or JSONC) file with an object of servers at `path`.
    Json {
        file: PathBuf,
        path: Vec<String>,
        entry: Value,
        native: Option<Native>,
        /// False when the file is only read to see what is registered and changes must go
        /// through the harness's own command.
        writable: bool,
    },
    /// A `config.toml`: Codex's `[mcp_servers.NAME]` tables, or Mistral Vibe's `[[mcp_servers]]`.
    Toml { file: PathBuf, native: Option<Native>, style: TomlStyle },
    /// A `config.yaml` with a map of servers: Goose's or Hermes Agent's.
    Yaml { file: PathBuf, style: YamlStyle },
    /// Crush's `crushrc`.
    CrushRc { file: PathBuf },
    /// Nothing this program can write: the text says what to do by hand.
    Manual(String),
    /// The harness has no MCP client, or no configuration at this scope.
    Unavailable(String),
}

/// How a `config.toml` holds its servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TomlStyle {
    /// `[mcp_servers.NAME]` with `command` and `args` (Codex).
    Codex,
    /// `[[mcp_servers]]` with `name`, `transport`, `command` and `args` (Mistral Vibe).
    Vibe,
}

/// A hooks file in the Claude Code shape (`{"hooks": {EVENT: [{matcher, hooks: [...]}]}}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Hooks {
    pub file: PathBuf,
    /// Event name, matcher, hook subcommand, timeout in seconds.
    pub events: Vec<(&'static str, Option<&'static str>, &'static str, u32)>,
    pub note: Option<&'static str>,
}

pub struct Harness {
    pub id: &'static str,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Executables that mean the harness is installed.
    pub bins: &'static [&'static str],
}

pub const HARNESSES: &[Harness] = &[
    Harness { id: "claude-code", name: "Claude Code", aliases: &["claude"], bins: &["claude"] },
    Harness { id: "codex", name: "Codex CLI", aliases: &["codex-cli"], bins: &["codex"] },
    Harness { id: "gemini-cli", name: "Gemini CLI", aliases: &["gemini"], bins: &["gemini"] },
    Harness { id: "cursor", name: "Cursor", aliases: &[], bins: &["cursor", "cursor-agent"] },
    Harness { id: "copilot", name: "GitHub Copilot in VS Code", aliases: &["vscode", "vs-code", "github-copilot"], bins: &["code", "code-insiders"] },
    Harness { id: "copilot-cli", name: "GitHub Copilot CLI", aliases: &[], bins: &["copilot"] },
    Harness { id: "opencode", name: "OpenCode", aliases: &[], bins: &["opencode"] },
    Harness { id: "qwen-code", name: "Qwen Code", aliases: &["qwen"], bins: &["qwen"] },
    Harness { id: "goose", name: "Goose", aliases: &[], bins: &["goose"] },
    Harness { id: "amp", name: "Amp", aliases: &[], bins: &["amp"] },
    Harness { id: "cline", name: "Cline", aliases: &[], bins: &["cline"] },
    Harness { id: "kilo-code", name: "Kilo Code", aliases: &["kilo", "kilocode"], bins: &["kilo", "kilocode"] },
    Harness { id: "droid", name: "Factory Droid", aliases: &["factory"], bins: &["droid"] },
    Harness { id: "crush", name: "Crush", aliases: &[], bins: &["crush"] },
    Harness { id: "zed", name: "Zed", aliases: &[], bins: &["zed", "zeditor"] },
    Harness { id: "kiro", name: "Kiro", aliases: &[], bins: &["kiro", "kiro-cli"] },
    Harness { id: "junie", name: "Junie", aliases: &[], bins: &["junie"] },
    Harness { id: "deepagents", name: "Deep Agents Code", aliases: &["dcode", "deep-agents"], bins: &["dcode", "deepagents"] },
    Harness { id: "pi", name: "Pi", aliases: &[], bins: &["pi"] },
    Harness { id: "devin", name: "Devin Desktop", aliases: &["windsurf"], bins: &["devin", "windsurf"] },
    Harness { id: "antigravity", name: "Antigravity", aliases: &["agy"], bins: &["agy", "antigravity"] },
    Harness { id: "dsh", name: "DeepSeek Harness", aliases: &["deepseek-harness"], bins: &["dsh"] },
    Harness { id: "aider", name: "Aider", aliases: &[], bins: &["aider"] },
    Harness { id: "hermes", name: "Hermes Agent", aliases: &["hermes-agent"], bins: &["hermes"] },
    Harness { id: "openclaw", name: "OpenClaw", aliases: &[], bins: &["openclaw"] },
    Harness { id: "mistral-vibe", name: "Mistral Vibe", aliases: &["vibe"], bins: &["vibe"] },
    Harness { id: "continue", name: "Continue", aliases: &["continue-dev", "cn"], bins: &["cn"] },
    Harness { id: "openhands", name: "OpenHands CLI", aliases: &["open-hands"], bins: &["openhands"] },
    Harness { id: "warp", name: "Warp", aliases: &[], bins: &["warp-terminal"] },
    Harness { id: "auggie", name: "Auggie (Augment CLI)", aliases: &["augment"], bins: &["auggie"] },
    Harness { id: "codebuddy", name: "CodeBuddy Code", aliases: &["codebuddy-code"], bins: &["codebuddy"] },
    Harness { id: "kimi-code", name: "Kimi Code CLI", aliases: &["kimi"], bins: &["kimi"] },
];

pub fn lookup(word: &str) -> Option<&'static Harness> {
    let w = word.trim().to_ascii_lowercase().replace('_', "-");
    HARNESSES.iter().find(|h| h.id == w || h.aliases.contains(&w.as_str()))
}

/// `owner/repo` of the project, from the manifest's repository URL.
pub fn repo_slug() -> &'static str {
    let url = env!("CARGO_PKG_REPOSITORY");
    url.strip_prefix("https://github.com/").unwrap_or(url).trim_end_matches('/')
}

pub fn repo_url() -> &'static str {
    env!("CARGO_PKG_REPOSITORY").trim_end_matches('/')
}

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// The first of `names` that exists in `dir`, else the last one (the name to create).
fn existing(dir: &Path, names: &[&str]) -> PathBuf {
    names.iter().map(|n| dir.join(n)).find(|p| p.exists()).unwrap_or_else(|| dir.join(names[names.len() - 1]))
}

impl Harness {
    /// Claude Code's configuration directory.
    fn claude_dir(ctx: &Ctx) -> PathBuf {
        env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| ctx.home(".claude"))
    }

    fn codex_dir(ctx: &Ctx) -> PathBuf {
        env_path("CODEX_HOME").unwrap_or_else(|| ctx.home(".codex"))
    }

    fn gemini_dir(ctx: &Ctx) -> PathBuf {
        env_path("GEMINI_CLI_HOME").map(|h| h.join(".gemini")).unwrap_or_else(|| ctx.home(".gemini"))
    }

    fn copilot_dir(ctx: &Ctx) -> PathBuf {
        env_path("COPILOT_HOME").unwrap_or_else(|| ctx.home(".copilot"))
    }

    fn hermes_dir(ctx: &Ctx) -> PathBuf {
        env_path("HERMES_HOME").unwrap_or_else(|| ctx.home(".hermes"))
    }

    fn vibe_dir(ctx: &Ctx) -> PathBuf {
        env_path("VIBE_HOME").unwrap_or_else(|| ctx.home(".vibe"))
    }

    fn kimi_dir(ctx: &Ctx) -> PathBuf {
        env_path("KIMI_CODE_HOME").unwrap_or_else(|| ctx.home(".kimi-code"))
    }

    /// Devin's user configuration directory: `~/.config/devin`, `%APPDATA%\devin` on Windows.
    fn devin_dir(ctx: &Ctx) -> PathBuf {
        if cfg!(windows) {
            env_path("APPDATA").map(|a| a.join("devin")).unwrap_or_else(|| ctx.home("AppData/Roaming/devin"))
        } else {
            ctx.home(".config/devin")
        }
    }

    /// Zed's configuration directory, as `config_dir()` in Zed's `crates/paths` decides it:
    /// `$XDG_CONFIG_HOME/zed` on Linux and FreeBSD, `%APPDATA%\Zed` on Windows, and `~/.config/zed`
    /// on macOS, where Zed does not look at `XDG_CONFIG_HOME` at all.
    fn zed_dir(ctx: &Ctx) -> PathBuf {
        if cfg!(target_os = "macos") {
            ctx.home(".config/zed")
        } else if cfg!(windows) {
            env_path("APPDATA").map(|a| a.join("Zed")).unwrap_or_else(|| ctx.home("AppData/Roaming/Zed"))
        } else {
            ctx.config("zed")
        }
    }

    /// VS Code's user data directory for the default profile.
    fn vscode_user(ctx: &Ctx) -> PathBuf {
        if cfg!(target_os = "macos") {
            ctx.home("Library/Application Support/Code/User")
        } else if cfg!(windows) {
            env_path("APPDATA").map(|a| a.join("Code/User")).unwrap_or_else(|| ctx.home("AppData/Roaming/Code/User"))
        } else {
            ctx.config("Code/User")
        }
    }

    /// Files or directories whose existence shows that the harness has been used here.
    fn traces(&self, ctx: &Ctx) -> Vec<PathBuf> {
        match self.id {
            "claude-code" => vec![Self::claude_dir(ctx)],
            "codex" => vec![Self::codex_dir(ctx)],
            "gemini-cli" => vec![Self::gemini_dir(ctx)],
            "cursor" => vec![ctx.home(".cursor")],
            "copilot" => vec![Self::vscode_user(ctx)],
            "copilot-cli" => vec![Self::copilot_dir(ctx)],
            "opencode" => vec![ctx.config("opencode")],
            "qwen-code" => vec![ctx.home(".qwen")],
            "goose" => vec![ctx.config("goose")],
            "amp" => vec![ctx.config("amp")],
            "cline" => vec![ctx.home(".cline")],
            "kilo-code" => vec![ctx.config("kilo"), ctx.home(".kilo")],
            "droid" => vec![ctx.home(".factory")],
            "crush" => vec![ctx.config("crush")],
            "zed" => vec![Self::zed_dir(ctx)],
            "kiro" => vec![ctx.home(".kiro")],
            "junie" => vec![ctx.home(".junie")],
            "deepagents" => vec![ctx.home(".deepagents")],
            "pi" => vec![ctx.home(".pi")],
            "devin" => vec![Self::devin_dir(ctx), ctx.home(".codeium/windsurf")],
            "antigravity" => vec![ctx.home(".antigravity")],
            "dsh" => vec![ctx.home(".dsh")],
            "aider" => vec![ctx.home(".aider.conf.yml"), ctx.project(".aider.conf.yml")],
            "hermes" => vec![Self::hermes_dir(ctx)],
            "openclaw" => vec![ctx.home(".openclaw")],
            "mistral-vibe" => vec![Self::vibe_dir(ctx)],
            "continue" => vec![ctx.home(".continue")],
            "openhands" => vec![ctx.home(".openhands")],
            "warp" => vec![ctx.home(".warp")],
            "auggie" => vec![ctx.home(".augment")],
            "codebuddy" => vec![ctx.home(".codebuddy"), ctx.home(".codebuddy.json")],
            "kimi-code" => vec![Self::kimi_dir(ctx)],
            _ => Vec::new(),
        }
    }

    /// Why the harness counts as installed on this machine, if it does: one of its programs is
    /// on `PATH`, or its configuration directory or its user-level server file exists.
    pub fn found(&self, ctx: &Ctx) -> Option<String> {
        if let Some(bin) = self.bins.iter().find_map(|b| ctx.which(b)) {
            return Some(format!("{} on PATH", ctx.show(&bin)));
        }
        let mut traces = self.traces(ctx);
        let probe = Server { command: NAME.to_string(), args: Vec::new() };
        if let Mcp::Json { file, writable: true, .. } | Mcp::Toml { file, .. } | Mcp::Yaml { file, .. } | Mcp::CrushRc { file } =
            self.mcp(ctx, Scope::User, &probe)
        {
            traces.push(file);
        }
        traces.into_iter().find(|p| p.exists()).map(|p| format!("{} exists", ctx.show(&p)))
    }

    /// The MCP server entry of this harness at `scope`, launching `server`.
    pub fn mcp(&self, ctx: &Ctx, scope: Scope, server: &Server) -> Mcp {
        let user = scope == Scope::User;
        let (command, args) = (server.command.as_str(), &server.args);
        let json = |file: PathBuf, path: &[&str], entry: Value, native: Option<Native>| Mcp::Json { file, path: strings(path), entry, native, writable: true };
        // The shapes several harnesses share.
        let plain = || json!({"command": command, "args": args});
        let stdio = || json!({"type": "stdio", "command": command, "args": args});
        let local = || {
            let mut argv = vec![command.to_string()];
            argv.extend(args.iter().cloned());
            json!({"type": "local", "command": argv, "enabled": true})
        };
        let servers = ["mcpServers", NAME];
        // `PROGRAM mcp add ... NAME -- COMMAND ARGS`, the form most command lines share. An empty
        // `remove` means the harness has no command to take an entry out again.
        let dashed = |program: &'static str, before: &[&str], remove: &[&str]| {
            let mut add = strings(before);
            add.extend([NAME.to_string(), "--".to_string(), command.to_string()]);
            add.extend(args.iter().cloned());
            Some(Native { program, add, remove: (!remove.is_empty()).then(|| strings(remove)) })
        };
        match self.id {
            "claude-code" => {
                let file = if user {
                    // User-scope servers live in `.claude.json`: inside the configuration directory
                    // when one is named, else next to `~/.claude`.
                    env_path("CLAUDE_CONFIG_DIR").map(|d| d.join(".claude.json")).unwrap_or_else(|| ctx.home(".claude.json"))
                } else {
                    ctx.project(".mcp.json")
                };
                let s = scope.word();
                let native = dashed("claude", &["mcp", "add", "--transport", "stdio", "--scope", s], &["mcp", "remove", NAME, "--scope", s]);
                json(file, &servers, json!({"type": "stdio", "command": command, "args": args, "env": {}}), native)
            }
            "codex" => {
                if user {
                    Mcp::Toml { file: Self::codex_dir(ctx).join("config.toml"), native: dashed("codex", &["mcp", "add"], &[]), style: TomlStyle::Codex }
                } else {
                    Mcp::Toml { file: ctx.project(".codex/config.toml"), native: None, style: TomlStyle::Codex }
                }
            }
            "gemini-cli" | "qwen-code" => {
                let (program, dir): (&'static str, PathBuf) =
                    if self.id == "gemini-cli" { ("gemini", Self::gemini_dir(ctx)) } else { ("qwen", ctx.home(".qwen")) };
                let file = if user {
                    dir.join("settings.json")
                } else {
                    ctx.project(if self.id == "gemini-cli" { ".gemini/settings.json" } else { ".qwen/settings.json" })
                };
                let s = scope.word();
                let mut add = strings(&["mcp", "add", "--scope", s, NAME, command]);
                add.extend(args.iter().cloned());
                let native = Native { program, add, remove: Some(strings(&["mcp", "remove", "--scope", s, NAME])) };
                json(file, &servers, plain(), Some(native))
            }
            "cursor" => json(if user { ctx.home(".cursor/mcp.json") } else { ctx.project(".cursor/mcp.json") }, &servers, stdio(), None),
            "copilot" => {
                if user {
                    // The user profile's file is VS Code's to write: `code --add-mcp` does it.
                    let mut spec = stdio();
                    spec["name"] = json!(NAME);
                    let native = Native { program: "code", add: vec!["--add-mcp".to_string(), spec.to_string()], remove: None };
                    Mcp::Json {
                        file: Self::vscode_user(ctx).join("mcp.json"),
                        path: strings(&["servers", NAME]),
                        entry: stdio(),
                        native: Some(native),
                        writable: false,
                    }
                } else {
                    json(ctx.project(".vscode/mcp.json"), &["servers", NAME], stdio(), None)
                }
            }
            "copilot-cli" => {
                if user {
                    let entry = json!({"type": "local", "command": command, "args": args, "tools": ["*"]});
                    json(Self::copilot_dir(ctx).join("mcp-config.json"), &servers, entry, dashed("copilot", &["mcp", "add"], &["mcp", "remove", NAME]))
                } else {
                    json(ctx.project(".mcp.json"), &servers, stdio(), None)
                }
            }
            "opencode" => {
                let file = if user {
                    existing(&ctx.config("opencode"), &["opencode.jsonc", "opencode.json"])
                } else {
                    [
                        ctx.project("opencode.jsonc"),
                        ctx.project("opencode.json"),
                        ctx.project(".opencode/opencode.jsonc"),
                        ctx.project(".opencode/opencode.json"),
                    ]
                    .into_iter()
                    .find(|p| p.exists())
                    .unwrap_or_else(|| ctx.project("opencode.json"))
                };
                // A file already in the V2 layout keeps it; the V1 layout is read by both.
                let v2 = std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|t| super::json_edit::get(&t, &["mcp", "servers"]).ok().flatten())
                    .is_some_and(|v| v.is_object());
                if v2 {
                    let mut argv = vec![command.to_string()];
                    argv.extend(args.iter().cloned());
                    json(file, &["mcp", "servers", NAME], json!({"type": "local", "command": argv}), None)
                } else {
                    json(file, &["mcp", NAME], local(), None)
                }
            }
            "kilo-code" => {
                let file = if user {
                    existing(&ctx.config("kilo"), &["kilo.json", "kilo.jsonc"])
                } else {
                    [ctx.project("kilo.jsonc"), ctx.project("kilo.json"), ctx.project(".kilo/kilo.jsonc"), ctx.project(".kilo/kilo.json")]
                        .into_iter()
                        .find(|p| p.exists())
                        .unwrap_or_else(|| ctx.project(".kilo/kilo.jsonc"))
                };
                json(file, &["mcp", NAME], local(), None)
            }
            "goose" => {
                if user {
                    Mcp::Yaml { file: ctx.config("goose/config.yaml"), style: YamlStyle::Goose }
                } else {
                    Mcp::Unavailable("Goose keeps extensions in the user's config.yaml only; use --scope user".to_string())
                }
            }
            "amp" => {
                let path = ["amp.mcpServers", NAME];
                if user {
                    json(ctx.config("amp/settings.json"), &path, plain(), dashed("amp", &["mcp", "add"], &["mcp", "remove", NAME]))
                } else {
                    json(ctx.project(".amp/settings.json"), &path, plain(), None)
                }
            }
            "cline" => {
                if user {
                    let file = env_path("CLINE_MCP_SETTINGS_PATH").unwrap_or_else(|| ctx.home(".cline/data/settings/cline_mcp_settings.json"));
                    let entry = json!({"command": command, "args": args, "disabled": false, "autoApprove": []});
                    json(file, &servers, entry, dashed("cline", &["mcp", "add", "--yes"], &["mcp", "remove", NAME]))
                } else {
                    Mcp::Unavailable("Cline keeps MCP servers in the user's cline_mcp_settings.json only; use --scope user".to_string())
                }
            }
            "droid" => {
                let entry = json!({"type": "stdio", "command": command, "args": args, "disabled": false});
                if user {
                    let line = std::iter::once(command).chain(args.iter().map(String::as_str)).map(super::quote).collect::<Vec<_>>().join(" ");
                    let native =
                        Native { program: "droid", add: vec!["mcp".into(), "add".into(), NAME.into(), line], remove: Some(strings(&["mcp", "remove", NAME])) };
                    json(ctx.home(".factory/mcp.json"), &servers, entry, Some(native))
                } else {
                    json(ctx.project(".factory/mcp.json"), &servers, entry, None)
                }
            }
            "crush" => {
                let (rc, legacy) = if user {
                    let rc = match env_path("CRUSH_GLOBAL_CONFIG") {
                        Some(p) if p.is_dir() => p.join("crushrc"),
                        Some(p) => p,
                        None => ctx.config("crush/crushrc"),
                    };
                    (rc, ctx.config("crush/crush.json"))
                } else {
                    (existing(&ctx.root, &[".crushrc", "crushrc", ".crushrc"]), ctx.project("crush.json"))
                };
                // The JSON file is the older format: used only where it is all there is.
                if !rc.exists() && legacy.exists() { json(legacy, &["mcp", NAME], stdio(), None) } else { Mcp::CrushRc { file: rc } }
            }
            "zed" => json(
                if user { Self::zed_dir(ctx).join("settings.json") } else { ctx.project(".zed/settings.json") },
                &["context_servers", NAME],
                json!({"command": command, "args": args, "env": {}}),
                None,
            ),
            "kiro" => json(
                if user { ctx.home(".kiro/settings/mcp.json") } else { ctx.project(".kiro/settings/mcp.json") },
                &servers,
                json!({"command": command, "args": args, "disabled": false}),
                None,
            ),
            "junie" => json(if user { ctx.home(".junie/mcp/mcp.json") } else { ctx.project(".junie/mcp/mcp.json") }, &servers, plain(), None),
            "deepagents" => {
                if user {
                    json(ctx.home(".deepagents/.mcp.json"), &servers, plain(), None)
                } else {
                    json(ctx.project(".mcp.json"), &servers, stdio(), None)
                }
            }
            "devin" => {
                if user {
                    json(Self::devin_dir(ctx).join("mcp_config.json"), &servers, plain(), dashed("devin", &["mcp", "add", "-s", "user"], &[]))
                } else {
                    json(ctx.project(".devin/mcp_config.json"), &servers, plain(), None)
                }
            }
            "antigravity" => {
                json(if user { ctx.home(".gemini/config/mcp_config.json") } else { ctx.project(".agents/mcp_config.json") }, &servers, plain(), None)
            }
            "hermes" => {
                if user {
                    Mcp::Yaml { file: Self::hermes_dir(ctx).join("config.yaml"), style: YamlStyle::Hermes }
                } else {
                    Mcp::Unavailable("Hermes Agent keeps MCP servers in the user's config.yaml only; use --scope user".to_string())
                }
            }
            "openclaw" => {
                if user {
                    let mut add = strings(&["mcp", "add", NAME, "--command", command]);
                    for arg in args {
                        add.extend(["--arg".to_string(), arg.clone()]);
                    }
                    json(ctx.home(".openclaw/openclaw.json"), &["mcp", "servers", NAME], plain(), Some(Native { program: "openclaw", add, remove: None }))
                } else {
                    Mcp::Unavailable("OpenClaw keeps MCP servers in the user's openclaw.json only; use --scope user".to_string())
                }
            }
            "mistral-vibe" => {
                if user {
                    let mut add = strings(&["mcp", "add", NAME, "--transport", "stdio", "--command", command]);
                    for arg in args {
                        add.extend(["--arg".to_string(), arg.clone()]);
                    }
                    let native = Native { program: "vibe", add, remove: Some(strings(&["mcp", "remove", NAME])) };
                    Mcp::Toml { file: Self::vibe_dir(ctx).join("config.toml"), native: Some(native), style: TomlStyle::Vibe }
                } else {
                    Mcp::Toml { file: ctx.project(".vibe/config.toml"), native: None, style: TomlStyle::Vibe }
                }
            }
            "continue" => {
                if user {
                    // `mcpServers` in config.yaml is a list, which setup does not edit.
                    Mcp::Manual(format!(
                        "add this item to the mcpServers list of ~/.continue/config.yaml: - {{name: {NAME}, command: {}, args: [{}]}}",
                        super::quote(command),
                        args.join(", ")
                    ))
                } else {
                    json(ctx.project(".continue/mcpServers/mcp.json"), &servers, plain(), None)
                }
            }
            "openhands" => {
                if user {
                    let mut add = strings(&["mcp", "add", NAME, "--transport", "stdio", command, "--"]);
                    add.extend(args.iter().cloned());
                    let native = Native { program: "openhands", add, remove: Some(strings(&["mcp", "remove", NAME])) };
                    json(ctx.home(".openhands/mcp.json"), &servers, plain(), Some(native))
                } else {
                    Mcp::Unavailable("the OpenHands CLI keeps MCP servers in the user's mcp.json only; use --scope user".to_string())
                }
            }
            "warp" => json(if user { ctx.home(".warp/.mcp.json") } else { ctx.project(".warp/.mcp.json") }, &servers, plain(), None),
            "auggie" => {
                if user {
                    let add = strings(&["mcp", "add", NAME, "--command", command, "--args", &args.join(" ")]);
                    let native = Native { program: "auggie", add, remove: Some(strings(&["mcp", "remove", NAME])) };
                    json(ctx.home(".augment/settings.json"), &servers, plain(), Some(native))
                } else {
                    json(ctx.project(".augment/settings.json"), &servers, plain(), None)
                }
            }
            "codebuddy" => {
                if user {
                    // The first of these that exists is the one CodeBuddy reads and writes.
                    let file = [ctx.home(".codebuddy/.mcp.json"), ctx.home(".codebuddy/mcp.json"), ctx.home(".codebuddy.json")]
                        .into_iter()
                        .find(|p| p.exists())
                        .unwrap_or_else(|| ctx.home(".codebuddy/.mcp.json"));
                    let native = dashed("codebuddy", &["mcp", "add", "--scope", "user"], &["mcp", "remove", NAME, "--scope", "user"]);
                    json(file, &servers, stdio(), native)
                } else {
                    json(ctx.project(".mcp.json"), &servers, stdio(), None)
                }
            }
            "kimi-code" => json(if user { Self::kimi_dir(ctx).join("mcp.json") } else { ctx.project(".kimi-code/mcp.json") }, &servers, plain(), None),
            "dsh" => Mcp::Manual(format!(
                "add an MCP row in Settings -> MCP, or to the profile's cordis.patch.yml: {{id: mcp-{NAME}, name: '@deepseek-ai/dsh-mcp-client', serverName: {NAME}, transport: stdio, command: {}, args: [{}]}}",
                super::quote(command),
                args.join(", ")
            )),
            "pi" => Mcp::Unavailable("Pi has no MCP client: the skill, which runs the codefast command, is the integration".to_string()),
            "aider" => Mcp::Unavailable("Aider has no MCP client: it reads the conventions file".to_string()),
            _ => Mcp::Unavailable("unknown harness".to_string()),
        }
    }

    /// The directory this harness loads skills from at `scope` (the skill goes in a `codefast`
    /// folder inside it). The shared `.agents/skills` is used wherever the harness reads it, so
    /// that several harnesses do not each get a copy.
    pub fn skills(&self, ctx: &Ctx, scope: Scope) -> Option<PathBuf> {
        let user = scope == Scope::User;
        let pick = |user_dir: PathBuf, project_rel: &str| if user { user_dir } else { ctx.project(project_rel) };
        match self.id {
            "claude-code" => Some(pick(Self::claude_dir(ctx).join("skills"), ".claude/skills")),
            "qwen-code" => Some(pick(ctx.home(".qwen/skills"), ".qwen/skills")),
            "junie" => Some(pick(ctx.home(".junie/skills"), ".junie/skills")),
            "droid" => Some(pick(ctx.home(".factory/skills"), ".agents/skills")),
            // Kiro's CLI reads only the project's directory; the IDE reads both.
            "kiro" => Some(pick(ctx.home(".kiro/skills"), ".kiro/skills")),
            "antigravity" => Some(pick(ctx.home(".gemini/config/skills"), ".agents/skills")),
            "codebuddy" => Some(pick(ctx.home(".codebuddy/skills"), ".codebuddy/skills")),
            // Hermes reads `~/.agents/skills` only when it is named in its configuration.
            "hermes" => Some(pick(Self::hermes_dir(ctx).join("skills"), ".agents/skills")),
            // OpenClaw's project directories belong to its own agent workspace, not to a repository.
            "openclaw" if !user => None,
            // No skills of their own (Zed runs other agents; Aider has none), or not documented
            // (Continue's documentation names no skills directory).
            "zed" | "aider" | "continue" => None,
            _ => Some(pick(ctx.home(".agents/skills"), ".agents/skills")),
        }
    }

    /// Where lifecycle hooks are registered, for the harnesses whose hook protocol
    /// `codefast hook` speaks first-hand.
    pub fn hooks(&self, ctx: &Ctx, scope: Scope) -> Option<Hooks> {
        let user = scope == Scope::User;
        match self.id {
            "claude-code" => Some(Hooks {
                file: if user { Self::claude_dir(ctx).join("settings.json") } else { ctx.project(".claude/settings.json") },
                events: vec![
                    ("SessionStart", None, "session-start", 10),
                    ("UserPromptSubmit", None, "user-prompt-submit", 10),
                    ("PreToolUse", Some("Read|Bash"), "pre-tool-use", 5),
                    ("PostToolUse", Some("Bash|Grep|Glob|Edit|Write"), "post-tool-use", 10),
                    ("PostToolUseFailure", Some("Bash|Grep|Glob"), "post-tool-use", 5),
                ],
                note: None,
            }),
            "codex" => Some(Hooks {
                file: if user { Self::codex_dir(ctx).join("hooks.json") } else { ctx.project(".codex/hooks.json") },
                events: vec![
                    ("SessionStart", None, "session-start", 10),
                    ("UserPromptSubmit", None, "user-prompt-submit", 10),
                    ("PreToolUse", Some("Bash"), "pre-tool-use", 5),
                    ("PostToolUse", Some("Bash|apply_patch"), "post-tool-use", 10),
                ],
                note: Some("Codex runs new hooks only after you review and trust them once: open /hooks in a session"),
            }),
            _ => None,
        }
    }

    /// Whether the harness has codefast installed as a plugin that a session in this project
    /// loads (which brings the skill, the MCP server and, in Claude Code, the hooks by itself).
    pub fn plugin(&self, ctx: &Ctx) -> Option<String> {
        let prefix = format!("{NAME}@");
        // The project as the harness sees it: the repository root, and the directory this
        // program was started in when that lies inside it (a session started below the top).
        let mut here = vec![ctx.root.clone()];
        if let Some(cwd) = std::env::current_dir().ok().and_then(|d| d.canonicalize().ok()).filter(|d| d.starts_with(&ctx.root) && *d != ctx.root) {
            here.insert(0, cwd);
        }
        match self.id {
            "claude-code" => {
                let dir = Self::claude_dir(ctx);
                let doc: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("plugins/installed_plugins.json")).ok()?).ok()?;
                // An install at project or local scope is recorded with the project it was made
                // for, and Claude Code loads it in that project only (`claude plugin list` shows
                // it as disabled everywhere else). An install at user scope is for every project.
                let for_this_project = |entry: &Value| match entry["scope"].as_str() {
                    Some("project" | "local") => entry["projectPath"].as_str().is_some_and(|p| {
                        let project = Path::new(p).canonicalize().unwrap_or_else(|_| PathBuf::from(p));
                        here.iter().any(|d| d.starts_with(&project))
                    }),
                    _ => true,
                };
                let installed = doc
                    .get("plugins")?
                    .as_object()?
                    .iter()
                    .find(|(k, v)| k.starts_with(&prefix) && v.as_array().is_none_or(|entries| entries.iter().any(for_this_project)))
                    .map(|(k, _)| k.clone())?;
                // Installed and switched off is not installed as far as a session is concerned.
                // The settings closest to the project decide: local, project, then the user's.
                let mut settings: Vec<PathBuf> = here.iter().flat_map(|d| [d.join(".claude/settings.local.json"), d.join(".claude/settings.json")]).collect();
                settings.push(dir.join("settings.json"));
                let switch = settings.iter().find_map(|file| {
                    let doc = super::json_edit::parse(&std::fs::read_to_string(file).ok()?).ok()?;
                    doc["enabledPlugins"][&installed].as_bool()
                });
                (switch != Some(false)).then_some(installed)
            }
            "codex" => {
                // `codex plugin add` records `[plugins."codefast@MARKETPLACE"]` with `enabled = true`
                // in the user's config.toml; a repository's own `.codex/config.toml` can switch a
                // plugin on or off for that repository.
                let recorded = |file: PathBuf| codex_plugin(&std::fs::read_to_string(file).ok()?, &prefix);
                let (name, on) = recorded(ctx.project(".codex/config.toml")).or_else(|| recorded(Self::codex_dir(ctx).join("config.toml")))?;
                on.then_some(name)
            }
            _ => None,
        }
    }

    /// The parts an installed plugin brings by itself, which setup therefore leaves alone.
    ///
    /// Claude Code takes the skill, the MCP server and the hooks from the plugin. Codex takes the
    /// skill from it and nothing else that is of use as it is: it loads no hooks from a plugin
    /// whose manifest is the Agent Plugins one (`docs/HARNESSES.md`), and it starts the plugin's
    /// MCP server in the plugin's own directory, where the server cannot know the project and
    /// has to be given the `root` with every call. A server of the same name in `config.toml`
    /// takes the place of the plugin's and is started in the project (codex-cli 0.164.0 then
    /// never starts the plugin's), so with the Codex plugin installed the server and the hooks
    /// are still setup's to register.
    pub fn plugin_parts(&self) -> &'static [&'static str] {
        match self.id {
            "claude-code" => &["mcp", "skill", "hooks"],
            "codex" => &["skill"],
            _ => &[],
        }
    }

    /// Whether this harness starts a plugin's MCP server where the server cannot tell which
    /// project it is for, so that every call has to name the `root` until a server of the
    /// harness's own configuration takes its place.
    pub fn plugin_server_needs_root(&self) -> bool {
        self.id == "codex"
    }

    /// One-line pointers a person may want next to the plan: the plugin route, an approval step.
    pub fn notes(&self, scope: Scope) -> Vec<String> {
        let (slug, url) = (repo_slug(), repo_url());
        let mut out = Vec::new();
        match self.id {
            "claude-code" => out.push(format!("as a plugin (skill, MCP server and hooks in one step): claude plugin install {NAME} --marketplace {slug}")),
            "codex" => out.push(format!("as a plugin: codex plugin marketplace add {slug} && codex plugin add {NAME}@{NAME}")),
            "gemini-cli" => out.push(format!("as an extension: gemini extensions install {url} --auto-update")),
            "qwen-code" => out.push(format!("as an extension: qwen extensions install {slug}")),
            "copilot-cli" => out.push(format!("as a plugin: copilot plugin install {slug}")),
            "copilot" => out.push("as a plugin: run \"Chat: Install Plugin from Source\" in VS Code and give it the repository URL".to_string()),
            "droid" => out.push(format!("as a plugin: droid plugin marketplace add {slug} && droid plugin install {NAME}@{NAME}")),
            "goose" => out.push(format!("skills and hooks as a plugin: goose plugin install {url}.git")),
            "kiro" => {
                out.push(format!("as a Power: add {url} in Kiro's Powers panel"));
                if scope == Scope::User {
                    out.push("Kiro CLI reads skills from a project's .kiro/skills only; ~/.kiro/skills is for the IDE".to_string());
                }
            }
            "antigravity" => out.push(format!("as a plugin: agy plugin install {url}")),
            "pi" => out.push(format!("as a package: pi install git:{}", url.trim_start_matches("https://"))),
            "amp" if scope == Scope::Project => out.push(format!("Amp asks for approval of workspace servers once: amp mcp approve {NAME}")),
            "deepagents" if scope == Scope::Project => {
                out.push("Deep Agents Code asks for approval of project servers once (or --trust-project-mcp)".to_string())
            }
            "hermes" if scope == Scope::Project => out.push("Hermes Agent loads a repository's skills only after `hermes skills trust` in it".to_string()),
            "hermes" => out.push("a running Hermes session picks the server up after /reload-mcp".to_string()),
            "mistral-vibe" if scope == Scope::Project => {
                out.push("Vibe reads .vibe/config.toml and .agents/skills only in a folder you have trusted".to_string())
            }
            "continue" => out.push("Continue uses MCP tools in agent mode only".to_string()),
            "warp" if scope == Scope::Project => {
                out.push("Warp starts a project's servers only when you switch each one on in its MCP servers page, once per session".to_string())
            }
            "kimi-code" if scope == Scope::Project => {
                out.push("Kimi Code reads .kimi-code/mcp.json only in a trusted folder (KIMI_CODE_TRUST_WORKSPACE=1 for headless runs)".to_string())
            }
            _ => {}
        }
        out
    }
}

/// The codefast plugin a Codex `config.toml` records and whether it is switched on: the first
/// `[plugins."codefast@MARKETPLACE"]` table that is enabled, else one that is switched off.
fn codex_plugin(text: &str, prefix: &str) -> Option<(String, bool)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut switched_off = None;
    for (i, line) in lines.iter().enumerate() {
        let Some(name) = line.strip_prefix("[plugins.").and_then(|t| t.strip_suffix(']')).map(|t| t.trim_matches(['"', '\''])) else { continue };
        if !name.starts_with(prefix) {
            continue;
        }
        let off = lines[i + 1..].iter().take_while(|l| !l.starts_with('[')).any(|l| l.split('#').next().unwrap_or("").replace(' ', "") == "enabled=false");
        if !off {
            return Some((name.to_string(), true));
        }
        switched_off.get_or_insert_with(|| name.to_string());
    }
    switched_off.map(|name| (name, false))
}

/// What kind of script a file named `codefast` on `PATH` is, when it is one of the two this
/// project ships in place of the binary: a plugin's launcher (`bin/codefast`) or the npm wrapper
/// (`npm/codefast/bin/codefast.js`). Both carry a mark on their second line and both end up
/// running the binary, so neither is "another program" that hides it.
pub fn wrapper_kind(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 96];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).ok()?;
    let head = String::from_utf8_lossy(&head[..n]);
    let mut lines = head.lines();
    if !lines.next()?.starts_with("#!") {
        return None;
    }
    let mark = lines.next()?;
    if mark.contains("codefast-launcher") {
        Some("launcher")
    } else if mark.contains("codefast-npm-wrapper") {
        Some("npm")
    } else {
        None
    }
}

/// The server an existing JSON entry launches, whichever of the common shapes it has.
pub fn server_of(entry: &Value) -> Option<Server> {
    let list = |v: &Value| v.as_array()?.iter().map(|x| x.as_str().map(str::to_string)).collect::<Option<Vec<String>>>();
    match entry.get("command").or_else(|| entry.get("cmd"))? {
        Value::String(command) => {
            let args = match entry.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(a) => list(a)?,
            };
            Some(Server { command: command.clone(), args })
        }
        argv @ Value::Array(_) => {
            let mut argv = list(argv)?;
            if argv.is_empty() {
                return None;
            }
            let command = argv.remove(0);
            Some(Server { command, args: argv })
        }
        _ => None,
    }
}

/// An existing JSON entry with the program it launches replaced by `command`, in whichever shape
/// the entry names it; everything else in the entry is as it was.
pub fn moved(entry: &Value, command: &str) -> Value {
    let mut out = entry.clone();
    let key = if entry.get("command").is_some() { "command" } else { "cmd" };
    match out.get_mut(key) {
        Some(Value::Array(argv)) if !argv.is_empty() => argv[0] = json!(command),
        Some(program @ Value::String(_)) => *program = json!(command),
        _ => {}
    }
    out
}

/// Whether a server entry runs this program's MCP server, wherever the binary lives.
pub fn is_codefast(server: &Server) -> bool {
    let file = Path::new(&server.command).file_name().and_then(|f| f.to_str()).unwrap_or("");
    matches!(file, "codefast" | "codefast.exe" | "codefast.cmd") && server.args.first().is_some_and(|a| a == "mcp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_has_a_distinct_name_and_a_complete_description() {
        let ctx = Ctx { root: PathBuf::from("/work/project"), home: Some(PathBuf::from("/home/u")), path: Vec::new(), exe: PathBuf::from("/opt/bin/codefast") };
        let server = Server { command: "/opt/bin/codefast".to_string(), args: vec!["mcp".to_string()] };
        let mut words: Vec<&str> = Vec::new();
        for h in HARNESSES {
            words.push(h.id);
            words.extend(h.aliases);
            assert!(lookup(h.id).is_some_and(|x| x.id == h.id));
            for scope in [Scope::User, Scope::Project] {
                match h.mcp(&ctx, scope, &server) {
                    Mcp::Json { file, path, entry, .. } => {
                        assert!(file.is_absolute() && path.last().map(String::as_str) == Some(NAME), "{}", h.id);
                        // What is written is what is read back.
                        assert_eq!(server_of(&entry).as_ref(), Some(&server), "{}", h.id);
                        assert_eq!(scope == Scope::Project, file.starts_with(&ctx.root), "{} {:?}: {}", h.id, scope, file.display());
                    }
                    Mcp::Toml { file, .. } | Mcp::Yaml { file, .. } | Mcp::CrushRc { file } => {
                        assert_eq!(scope == Scope::Project, file.starts_with(&ctx.root), "{}", h.id)
                    }
                    Mcp::Manual(text) | Mcp::Unavailable(text) => assert!(!text.is_empty()),
                }
                if let Some(dir) = h.skills(&ctx, scope) {
                    assert_eq!(scope == Scope::Project, dir.starts_with(&ctx.root), "{}", h.id);
                    assert!(dir.ends_with("skills"));
                }
                if let Some(hooks) = h.hooks(&ctx, scope) {
                    assert_eq!(scope == Scope::Project, hooks.file.starts_with(&ctx.root), "{}", h.id);
                }
            }
        }
        let total = words.len();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), total, "ids and aliases are unique");
        for required in [
            "hermes",
            "openclaw",
            "mistral-vibe",
            "continue",
            "openhands",
            "warp",
            "auggie",
            "codebuddy",
            "kimi-code",
            "claude-code",
            "codex",
            "gemini-cli",
            "cursor",
            "copilot",
            "opencode",
            "qwen-code",
            "goose",
            "amp",
            "cline",
            "kilo-code",
            "droid",
            "crush",
            "zed",
            "kiro",
            "aider",
            "dsh",
        ] {
            assert!(lookup(required).is_some(), "{required}");
        }
        assert_eq!(lookup("Claude").map(|h| h.id), Some("claude-code"));
        assert_eq!(lookup("kilo_code").map(|h| h.id), Some("kilo-code"));
        assert!(lookup("mcp-json").is_none() && lookup("").is_none());
    }

    #[test]
    fn entries_are_recognised_in_every_shape() {
        let s = |c: &str, a: &[&str]| Server { command: c.to_string(), args: a.iter().map(|x| x.to_string()).collect() };
        assert_eq!(server_of(&json!({"command": "codefast", "args": ["mcp"]})), Some(s("codefast", &["mcp"])));
        assert_eq!(server_of(&json!({"type": "local", "command": ["/x/codefast", "mcp"], "enabled": true})), Some(s("/x/codefast", &["mcp"])));
        assert_eq!(server_of(&json!({"command": "npx"})), Some(s("npx", &[])));
        for odd in [json!({}), json!({"url": "https://x"}), json!({"command": 3}), json!({"command": []}), json!({"command": "x", "args": [1]}), json!("x")] {
            assert_eq!(server_of(&odd), None, "{odd}");
        }
        assert!(is_codefast(&s("/opt/homebrew/bin/codefast", &["mcp"])));
        assert!(is_codefast(&s("codefast", &["mcp", "--root", "/r"])));
        assert!(is_codefast(&s("C:\\tools\\codefast.exe", &["mcp"])) || cfg!(not(windows)));
        for other in [s("npx", &["-y", "codefast", "mcp"]), s("codefast", &[]), s("codefast", &["serve"]), s("/bin/codefast-old", &["mcp"])] {
            assert!(!is_codefast(&other), "{other:?}");
        }
        assert_eq!(repo_slug(), "YashSharmaa/codefast");
    }
}
