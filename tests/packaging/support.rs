//! Helpers shared by the integration tests: throwaway repositories, a scrubbed environment, and
//! an MCP session over the binary's stdio. Included with `#[path = "packaging/support.rs"]`.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub const BIN: &str = env!("CARGO_BIN_EXE_codefast");
pub const MODERN: &str = "2026-07-28";
pub const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const META_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
pub const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";
pub const META_SUB: &str = "io.modelcontextprotocol/subscriptionId";

/// How long a test waits for the server before it fails instead of hanging.
pub const WAIT: Duration = Duration::from_secs(20);

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh, empty directory that is removed when the value is dropped. It lives in the system's
/// temporary directory, not under the build directory: the build directory sits inside this
/// project's own git work tree, and a test directory must not resolve to that repository.
pub struct Dir(PathBuf);

impl std::ops::Deref for Dir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn scratch(tag: &str) -> Dir {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("codefast-test-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let mut up = dir.parent();
    while let Some(p) = up {
        assert!(
            !p.join(".git").exists() && !p.join(".codefast").exists(),
            "the temporary directory {} is inside a repository ({}); set TMPDIR elsewhere to run these tests",
            dir.display(),
            p.display()
        );
        up = p.parent();
    }
    Dir(dir)
}

pub fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

/// A small C project that looks like a git work tree. Every repository also defines a function
/// named `marker_<tag>`, so a test can tell which repository answered.
pub struct Repo {
    pub dir: Dir,
    pub marker: String,
}

impl Repo {
    pub fn new(tag: &str) -> Repo {
        let dir = scratch(tag);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let marker = format!("marker_{}", tag.replace('-', "_"));
        write(
            &dir.join("src/io.h"),
            "struct dev;\nstruct dev_ops {\n    int (*read)(struct dev *d, char *buf, int count);\n    int (*write)(struct dev *d, const char *buf, int count);\n};\nstruct dev { const struct dev_ops *ops; int fd; };\nint dev_read(struct dev *d, char *buf, int count);\nint dev_write(struct dev *d, const char *buf, int count);\nvoid log_msg(const char *fmt, ...);\n",
        );
        write(
            &dir.join("src/io.c"),
            "#include \"io.h\"\n\n/* Read up to count bytes from the device. */\nint dev_read(struct dev *d, char *buf, int count)\n{\n    if (!d)\n        return -1;\n    return d->ops->read(d, buf, count);\n}\n\nint dev_write(struct dev *d, const char *buf, int count)\n{\n    log_msg(\"dev_write: %d bytes\", count);\n    return d->ops->write(d, buf, count);\n}\n",
        );
        write(
            &dir.join("src/main.c"),
            "#include \"io.h\"\n\nstatic int copy_all(struct dev *in, struct dev *out)\n{\n    char buf[512];\n    int n;\n    while ((n = dev_read(in, buf, sizeof buf)) > 0)\n        dev_write(out, buf, n);\n    return n;\n}\n\nint main(void)\n{\n    struct dev a, b;\n    return copy_all(&a, &b);\n}\n",
        );
        write(&dir.join("lib/marker.c"), &format!("int {marker}(int x)\n{{\n    return x + 1;\n}}\n"));
        // A file long enough to be worth an outline: 450 small functions.
        let mut big = String::new();
        for i in 0..450 {
            big.push_str(&format!("int big_fn_{i}(int x)\n{{\n    return x + {i};\n}}\n\n"));
        }
        write(&dir.join("src/big.c"), &big);
        Repo { dir, marker }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    pub fn str(&self) -> &str {
        self.dir.to_str().unwrap()
    }
}

/// A command for the binary with every variable that steers codefast or a harness removed, and
/// `HOME` plus each harness's configuration directory pointed into `home`, so that no test can
/// read or write the developer's real configuration.
pub fn command(home: &Path) -> Command {
    let mut c = Command::new(BIN);
    for k in [
        "CODEFAST_ROOT",
        "CODEFAST_INDEX_DIR",
        "CODEFAST_HOOKS",
        "CODEFAST_NO_REFRESH",
        "CODEFAST_BIN",
        "CODEFAST_CACHE",
        "CODEFAST_PLUGIN_ROOT",
        "CLAUDE_PROJECT_DIR",
        "PLUGIN_ROOT",
        "PLUGIN_DATA",
        "CLAUDE_PLUGIN_ROOT",
        "CLAUDE_PLUGIN_DATA",
        "CURSOR_PLUGIN_ROOT",
        "COPILOT_PLUGIN_ROOT",
        "DROID_PLUGIN_ROOT",
        "CLAUDECODE",
        "CLAUDE_CODE_CHILD_SESSION",
    ] {
        c.env_remove(k);
    }
    c.env("HOME", home);
    c.env("XDG_CONFIG_HOME", home.join(".config"));
    c.env("XDG_CACHE_HOME", home.join(".cache"));
    c.env("CLAUDE_CONFIG_DIR", home.join(".claude"));
    c.env("CODEX_HOME", home.join(".codex"));
    c
}

/// Run the binary to completion; returns (exit code, stdout, stderr).
pub fn run(home: &Path, cwd: &Path, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> (i32, String, String) {
    let mut c = command(home);
    c.current_dir(cwd).args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in envs {
        c.env(k, v);
    }
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = c.spawn().expect("spawn codefast");
    if let Some(text) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        let _ = pipe.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().expect("wait for codefast");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// What a finished session left behind.
pub struct Finished {
    pub code: Option<i32>,
    /// Lines the server wrote after the last one the test consumed.
    pub rest: Vec<String>,
    pub stderr: String,
}

/// One `codefast mcp` process and the JSON-RPC conversation with it.
pub struct Session {
    _home: Dir,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: i64,
    /// Every stdout line seen so far, for "stdout carries only protocol messages" checks.
    pub seen: Vec<String>,
}

impl Session {
    pub fn start(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Session {
        let home = scratch("home");
        let mut c = command(&home);
        c.current_dir(cwd).arg("mcp").args(args);
        for (k, v) in envs {
            c.env(k, v);
        }
        Session::spawn(c, home)
    }

    /// A session with any command that serves MCP on its stdio (a manifest's start command, as
    /// a harness would run it). `home` is kept until the session ends.
    pub fn spawn(mut c: Command, home: Dir) -> Session {
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().expect("spawn the MCP server");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let stdin = child.stdin.take();
        Session { _home: home, child, stdin, lines: rx, next_id: 0, seen: Vec::new() }
    }

    /// A session started inside `repo`, the common case.
    pub fn in_repo(repo: &Repo) -> Session {
        Session::start(&repo.dir, &[], &[])
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) {
        let pipe = self.stdin.as_mut().expect("stdin already closed");
        pipe.write_all(bytes).unwrap();
        pipe.flush().unwrap();
    }

    pub fn send_raw(&mut self, line: &str) {
        self.send_bytes(format!("{line}\n").as_bytes());
    }

    pub fn send(&mut self, msg: &Value) {
        self.send_raw(&msg.to_string());
    }

    /// The next stdout line, or `None` when nothing arrives within `wait`.
    pub fn next_line(&mut self, wait: Duration) -> Option<String> {
        let line = self.lines.recv_timeout(wait).ok()?;
        self.seen.push(line.clone());
        Some(line)
    }

    /// The next message; the test fails when the server stays silent.
    pub fn recv(&mut self) -> Value {
        let line = self.next_line(WAIT).expect("the server sent nothing");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"))
    }

    fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    /// A handshake-era request (no per-request metadata); returns the whole response.
    pub fn legacy(&mut self, method: &str, params: Value) -> Value {
        let id = self.id();
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            msg["params"] = params;
        }
        self.send(&msg);
        let reply = self.recv();
        assert_eq!(reply["id"], json!(id), "response id for {method}: {reply}");
        reply
    }

    /// A stateless request under revision 2026-07-28; returns the whole response.
    pub fn modern(&mut self, method: &str, params: Value) -> Value {
        let id = self.id();
        let mut params = if params.is_null() { json!({}) } else { params };
        params["_meta"] = modern_meta();
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let reply = self.recv();
        assert_eq!(reply["id"], json!(id), "response id for {method}: {reply}");
        reply
    }

    /// Call a tool in the stateless era and return its result object.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.modern("tools/call", json!({"name": tool, "arguments": arguments}));
        assert!(reply.get("error").is_none(), "tools/call {tool} failed at the protocol level: {reply}");
        reply["result"].clone()
    }

    /// Close stdin and wait for the server to exit.
    pub fn finish(mut self) -> Finished {
        drop(self.stdin.take());
        let deadline = Instant::now() + WAIT;
        let code = loop {
            match self.child.try_wait().unwrap() {
                Some(status) => break status.code(),
                None if Instant::now() > deadline => {
                    let _ = self.child.kill();
                    panic!("the server did not exit after stdin was closed");
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let mut rest = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(2)) {
            self.seen.push(line.clone());
            rest.push(line);
        }
        let mut stderr = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        Finished { code, rest, stderr }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn modern_meta() -> Value {
    json!({META_VERSION: MODERN, "io.modelcontextprotocol/clientInfo": {"name": "codefast-tests", "version": "0"}, META_CAPS: {}})
}

/// The text of a tool result's first content block.
pub fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text content in {result}"))
}

pub fn error_code(reply: &Value) -> i64 {
    reply["error"]["code"].as_i64().unwrap_or_else(|| panic!("not an error response: {reply}"))
}
