//! Recorded hook payloads and what the hook tests share. Included with
//! `#[path = "packaging/payloads/mod.rs"] mod payloads;` next to `support`.
//!
//! The JSON files in this directory are hook inputs as each harness documents them, with
//! `{{REPO}}` and `{{HOME}}` standing for the test's directories:
//!
//! * `claude-*`: Claude Code hooks reference (code.claude.com/docs/en/hooks, read 2026-11-13,
//!   Claude Code 2.1.322): common input fields plus the per-event examples.
//! * `codex-*`: Codex hooks page (learn.chatgpt.com/docs/hooks, read 2026-11-13) and the
//!   generated input schemas in openai/codex `codex-rs/hooks/schema/generated`: every required
//!   field is present. Compared on 2026-11-14 with what codex-cli 0.164.0 itself hands its
//!   hooks (`bench/codex/wiring.py`): the same fields, the shell as `Bash` with the command as
//!   one string, and after a patch the tool's text with its exit code in front, as it now is
//!   in `codex-post-apply-patch`.
//! * `cursor-*`, `gemini-*`: built from the Cursor and Gemini CLI hook references; neither
//!   harness was available to record from.
//! * `hostile-*`: inputs no harness should send.
#![allow(dead_code)]

use crate::support::{self, BIN, Dir, Repo};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// A recorded payload with the test's directories filled in.
pub fn payload(name: &str, repo: &Path, home: &Path) -> String {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/packaging/payloads").join(format!("{name}.json"));
    let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    text.replace("{{REPO}}", repo.to_str().unwrap()).replace("{{HOME}}", home.to_str().unwrap())
}

/// The same payload as another session.
pub fn in_session(payload: &str, id: &str) -> String {
    let mut v: Value = serde_json::from_str(payload).unwrap();
    v["session_id"] = Value::String(id.to_string());
    v.to_string()
}

pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

/// Run `codefast hook EVENT` in `cwd` with `stdin`. `codefast` is on `PATH`, as it is for an
/// agent's shell, and the caller summary gets all the time it needs unless `envs` says otherwise.
pub fn hook(home: &Path, cwd: &Path, event: &str, stdin: &[u8], envs: &[(&str, &str)]) -> Ran {
    let mut c = support::command(home);
    let bin_dir = Path::new(BIN).parent().unwrap();
    c.env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display())).env("CODEFAST_HOOK_BUDGET_MS", "5000");
    for (k, v) in envs {
        c.env(k, v);
    }
    c.current_dir(cwd).args(["hook", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = c.spawn().expect("spawn codefast hook");
    {
        let mut pipe = child.stdin.take().unwrap();
        let _ = pipe.write_all(stdin);
    }
    let out = child.wait_with_output().expect("wait for codefast hook");
    Ran {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        elapsed: started.elapsed(),
    }
}

/// Run `n` copies of `codefast hook EVENT` at the same moment: every process is started and
/// left waiting for its input, then all of them are given the payload together. This is how a
/// harness fires the hooks of tool calls it made in parallel.
pub fn hooks_at_once(home: &Path, cwd: &Path, event: &str, stdin: &[u8], n: usize) -> Vec<Ran> {
    let bin_dir = Path::new(BIN).parent().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));
    let mut waiting = Vec::new();
    for _ in 0..n {
        let mut c = support::command(home);
        c.env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display())).env("CODEFAST_HOOK_BUDGET_MS", "5000");
        c.current_dir(cwd).args(["hook", event]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().expect("spawn codefast hook");
        let (barrier, input) = (barrier.clone(), stdin.to_vec());
        waiting.push(std::thread::spawn(move || {
            let mut pipe = child.stdin.take().unwrap();
            barrier.wait();
            let started = Instant::now();
            let _ = pipe.write_all(&input);
            drop(pipe);
            let out = child.wait_with_output().expect("wait for codefast hook");
            Ran {
                code: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                elapsed: started.elapsed(),
            }
        }));
    }
    waiting.into_iter().map(|t| t.join().expect("hook thread")).collect()
}

impl Ran {
    /// A hook that had nothing to add: status 0 and not a byte on either stream.
    pub fn assert_quiet(&self, what: &str) {
        assert_eq!((self.code, self.stdout.as_str(), self.stderr.as_str()), (0, "", ""), "{what}");
    }

    /// The context a Claude Code or Codex hook reply carries. The reply must be exactly what
    /// both accept: one JSON object on one line, `hookSpecificOutput` with the event's name and
    /// `additionalContext`, and nothing else. No decision, no rewritten input, no `continue`.
    pub fn context(&self, event_name: &str) -> String {
        assert_eq!((self.code, self.stderr.as_str()), (0, ""), "a hook exits 0 and keeps stderr empty");
        assert!(self.stdout.ends_with('\n') && self.stdout.trim_end().lines().count() == 1, "one line of output: {:?}", self.stdout);
        let v: Value = serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", self.stdout));
        let top = v.as_object().expect("a JSON object");
        assert_eq!(top.keys().collect::<Vec<_>>(), ["hookSpecificOutput"], "{v}");
        let specific = top["hookSpecificOutput"].as_object().expect("hookSpecificOutput is an object");
        let mut keys: Vec<&str> = specific.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["additionalContext", "hookEventName"], "{v}");
        assert_eq!(specific["hookEventName"], event_name, "{v}");
        let text = specific["additionalContext"].as_str().expect("additionalContext is a string");
        assert!(!text.is_empty());
        text.to_string()
    }
}

/// A fixture repository with its index built, and a throwaway home directory.
pub fn indexed(tag: &str) -> (Repo, Dir) {
    let repo = Repo::new(tag);
    let home = support::scratch("home");
    let (code, out, err) = support::run(&home, &repo.dir, &["index"], &[], None);
    assert_eq!(code, 0, "codefast index: {out}{err}");
    (repo, home)
}

pub fn index_dir(repo: &Repo) -> PathBuf {
    repo.path(".codefast")
}

/// Wait until no background refresh is running in the repository and its index exists.
pub fn settle(repo: &Repo) {
    let (lock, base) = (index_dir(repo).join("refresh.lock"), index_dir(repo).join("base.cfseg"));
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut calm = 0;
    while calm < 8 {
        assert!(Instant::now() < deadline, "the background refresh did not finish");
        std::thread::sleep(Duration::from_millis(50));
        calm = if !lock.exists() && base.exists() { calm + 1 } else { 0 };
    }
}

/// The state files of the sessions that were told something.
pub fn session_files(repo: &Repo) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(index_dir(repo).join("sessions")) {
        Ok(entries) => entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}
